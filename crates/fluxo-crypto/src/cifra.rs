//! Cifra dos pacotes: ChaCha20-Poly1305 com o cabeçalho como dado
//! autenticado.
//!
//! Cada sentido (host → cliente, cliente → host) tem a sua chave, então o
//! nonce pode ser só o número de sequência do pacote (32 bits, no
//! cabeçalho): nunca se repete com a mesma chave. Depois de 2³² pacotes
//! (~10 dias a 5 000 pacotes/s) a sessão precisa de um handshake novo.
//!
//! O receptor recusa pacotes repetidos ou velhos demais com uma janela de
//! 64 números de sequência (como no IPsec e no DTLS): um atacante que grave
//! pacotes e os reenvie não consegue nem repetir um pedido de keyframe.

use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, Tag};

use crate::CryptoError;

/// Bytes que a cifra acrescenta a cada pacote (a tag de autenticação).
pub const TAG_LEN: usize = 16;

fn nonce(seq: u32) -> Nonce {
    let mut n = [0u8; 12];
    n[8..].copy_from_slice(&seq.to_be_bytes());
    Nonce::from(n)
}

/// Lado que cifra: um por sentido.
pub struct Sealer {
    cifra: ChaCha20Poly1305,
    proximo: Option<u32>,
}

impl Sealer {
    pub(crate) fn new(chave: &[u8; 32]) -> Self {
        Self {
            cifra: ChaCha20Poly1305::new(Key::from_slice(chave)),
            proximo: Some(0),
        }
    }

    /// Reserva o próximo número de sequência. `None` quando os 2³² se
    /// esgotaram: a sessão precisa de um handshake novo.
    pub fn next_seq(&mut self) -> Option<u32> {
        let seq = self.proximo?;
        self.proximo = seq.checked_add(1);
        Some(seq)
    }

    /// Cifra `datagrama[cabecalho..]` no lugar e acrescenta a tag. Os
    /// primeiros `cabecalho` bytes ficam visíveis, mas autenticados: precisam
    /// já conter `seq` (e o tamanho do payload com a tag).
    pub fn seal(&self, seq: u32, datagrama: &mut Vec<u8>, cabecalho: usize) -> Result<(), CryptoError> {
        let (aad, corpo) = datagrama.split_at_mut(cabecalho);
        let tag = self
            .cifra
            .encrypt_in_place_detached(&nonce(seq), aad, corpo)
            .map_err(|_| CryptoError::Seal)?;
        datagrama.extend_from_slice(&tag);
        Ok(())
    }
}

/// Lado que decifra: um por sentido, com a janela anti-replay.
pub struct Opener {
    cifra: ChaCha20Poly1305,
    janela: ReplayWindow,
}

impl Opener {
    pub(crate) fn new(chave: &[u8; 32]) -> Self {
        Self {
            cifra: ChaCha20Poly1305::new(Key::from_slice(chave)),
            janela: ReplayWindow::default(),
        }
    }

    /// Confere e decifra um pacote recebido; devolve o payload em claro.
    /// Erro se a tag não bate (chave errada, pacote alterado) ou se `seq` já
    /// foi visto ou é velho demais.
    pub fn open(&mut self, seq: u32, datagrama: &[u8], cabecalho: usize) -> Result<Vec<u8>, CryptoError> {
        if !self.janela.aceita(seq) {
            return Err(CryptoError::Replay);
        }
        let corpo = datagrama.get(cabecalho..).ok_or(CryptoError::Auth)?;
        if corpo.len() < TAG_LEN {
            return Err(CryptoError::Auth);
        }
        let (cifrado, tag) = corpo.split_at(corpo.len() - TAG_LEN);
        let mut claro = cifrado.to_vec();
        self.cifra
            .decrypt_in_place_detached(
                &nonce(seq),
                &datagrama[..cabecalho],
                &mut claro,
                Tag::from_slice(tag),
            )
            .map_err(|_| CryptoError::Auth)?;
        // Só marca depois de autenticado: pacote forjado não mexe na janela.
        self.janela.marca(seq);
        Ok(claro)
    }
}

/// Números de sequência já vistos: o maior e um mapa de bits dos 64 abaixo.
#[derive(Debug, Default)]
struct ReplayWindow {
    maior: Option<u32>,
    /// Bit `i` = `maior - i` já foi visto.
    vistos: u64,
}

impl ReplayWindow {
    fn aceita(&self, seq: u32) -> bool {
        let Some(maior) = self.maior else {
            return true;
        };
        if seq > maior {
            return true;
        }
        let atras = maior - seq;
        atras < 64 && self.vistos & (1 << atras) == 0
    }

    fn marca(&mut self, seq: u32) {
        match self.maior {
            Some(maior) if seq <= maior => self.vistos |= 1 << (maior - seq),
            Some(maior) => {
                let avanco = seq - maior;
                self.vistos = if avanco >= 64 {
                    1
                } else {
                    (self.vistos << avanco) | 1
                };
                self.maior = Some(seq);
            }
            None => {
                self.maior = Some(seq);
                self.vistos = 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn par() -> (Sealer, Opener) {
        let chave = [7u8; 32];
        (Sealer::new(&chave), Opener::new(&chave))
    }

    fn pacote(s: &mut Sealer, cabecalho: &[u8], dados: &[u8]) -> (u32, Vec<u8>) {
        let seq = s.next_seq().unwrap();
        let mut d = cabecalho.to_vec();
        d.extend_from_slice(dados);
        s.seal(seq, &mut d, cabecalho.len()).unwrap();
        (seq, d)
    }

    #[test]
    fn ida_e_volta_e_cabecalho_visivel() {
        let (mut s, mut o) = par();
        let (seq, d) = pacote(&mut s, b"CAB", b"segredo");
        assert_eq!(&d[..3], b"CAB");
        assert_eq!(d.len(), 3 + 7 + TAG_LEN);
        assert!(
            !d.windows(7).any(|w| w == b"segredo"),
            "payload deve sair cifrado"
        );
        assert_eq!(o.open(seq, &d, 3).unwrap(), b"segredo");
    }

    #[test]
    fn alterar_cabecalho_ou_payload_e_detectado() {
        let (mut s, mut o) = par();
        let (seq, d) = pacote(&mut s, b"CAB", b"segredo");
        for i in [0, 5, d.len() - 1] {
            let mut x = d.clone();
            x[i] ^= 1;
            assert_eq!(o.open(seq, &x, 3), Err(CryptoError::Auth), "byte {i}");
        }
        // O original continua valendo (as falsificações não mexeram na janela).
        assert!(o.open(seq, &d, 3).is_ok());
    }

    #[test]
    fn chave_errada_nao_abre() {
        let mut s = Sealer::new(&[1; 32]);
        let mut o = Opener::new(&[2; 32]);
        let (seq, d) = pacote(&mut s, b"CAB", b"x");
        assert_eq!(o.open(seq, &d, 3), Err(CryptoError::Auth));
    }

    #[test]
    fn repetido_e_recusado_mas_fora_de_ordem_passa() {
        let (mut s, mut o) = par();
        let pacotes: Vec<_> = (0..5).map(|i| pacote(&mut s, b"C", &[i])).collect();
        // Chegam 0, 2, 1, 4, 3: tudo aceito.
        for i in [0, 2, 1, 4, 3] {
            let (seq, d) = &pacotes[i];
            assert!(o.open(*seq, d, 1).is_ok(), "pacote {i}");
        }
        // Qualquer um de novo: recusado.
        let (seq, d) = &pacotes[2];
        assert_eq!(o.open(*seq, d, 1), Err(CryptoError::Replay));
    }

    #[test]
    fn velho_demais_e_recusado() {
        let (mut s, mut o) = par();
        let primeiro = pacote(&mut s, b"C", b"0");
        for _ in 0..70 {
            let (seq, d) = pacote(&mut s, b"C", b"x");
            o.open(seq, &d, 1).unwrap();
        }
        assert_eq!(o.open(primeiro.0, &primeiro.1, 1), Err(CryptoError::Replay));
    }

    #[test]
    fn sequencia_esgota_em_2_elevado_a_32() {
        let mut s = Sealer::new(&[0; 32]);
        s.proximo = Some(u32::MAX);
        assert_eq!(s.next_seq(), Some(u32::MAX));
        assert_eq!(s.next_seq(), None);
    }
}
