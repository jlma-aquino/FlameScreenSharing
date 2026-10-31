//! Handshake de pareamento: o código vira uma chave de sessão (SPAKE2).
//!
//! ```text
//! host                                       cliente
//!  HostHandshake::start(código) ─ PairInit ──► respond(código, init)
//!                                               → Session (já pode decifrar)
//!  finish(reply) ◄──────────────── PairReply ─ [msg SPAKE2 | confirmação]
//!  → Session (ou "código não confere")
//! ```
//!
//! SPAKE2 é um PAKE: os dois lados chegam à mesma chave só se usaram o
//! mesmo código. Quem não sabe o código e se mete na conversa ganha **um**
//! palpite por tentativa — não dá para gravar a troca e testar códigos
//! depois, offline. A confirmação do cliente (um bloco vazio cifrado com uma
//! chave derivada só para isso) deixa o host saber na hora se o código bate.
//! O cliente confere o host implicitamente: só quem tem a chave produz
//! pacotes que ele consegue decifrar.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

use crate::cifra::{Opener, Sealer, TAG_LEN};
use crate::{CryptoError, PairingCode};

const ID_HOST: &[u8] = b"fluxo-host";
const ID_CLIENTE: &[u8] = b"fluxo-cliente";
/// Dado autenticado da confirmação do cliente.
const CONFIRMA: &[u8] = b"fluxo: o cliente confirma a chave";
/// "Sal" da derivação de chaves: muda se o formato do protocolo mudar.
const SAL: &[u8] = b"fluxo protocolo v8";

/// Tamanho da mensagem do SPAKE2 (Ed25519: 1 byte de lado + 32).
pub const INIT_LEN: usize = 33;
/// Resposta do cliente: mensagem SPAKE2 + confirmação.
pub const REPLY_LEN: usize = INIT_LEN + TAG_LEN;

/// Sessão estabelecida: uma chave para cifrar o que sai, outra para abrir o
/// que chega.
pub struct Session {
    pub sealer: Sealer,
    pub opener: Opener,
}

struct Chaves {
    host_cliente: [u8; 32],
    cliente_host: [u8; 32],
    confirmacao: [u8; 32],
}

fn derivar(segredo: &[u8]) -> Chaves {
    let hk = Hkdf::<Sha256>::new(Some(SAL), segredo);
    let mut c = Chaves {
        host_cliente: [0; 32],
        cliente_host: [0; 32],
        confirmacao: [0; 32],
    };
    // `expand` só falha para saídas maiores que 255 × 32 bytes.
    for (info, saida) in [
        (&b"host -> cliente"[..], &mut c.host_cliente),
        (b"cliente -> host", &mut c.cliente_host),
        (b"confirmacao", &mut c.confirmacao),
    ] {
        hk.expand(info, saida).expect("32 bytes cabem no HKDF");
    }
    c
}

fn confirmacao(chave: &[u8; 32]) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(Key::from_slice(chave))
}

/// Lado do host: começa o handshake e confere a resposta.
pub struct HostHandshake {
    spake: Spake2<Ed25519Group>,
    msg: Vec<u8>,
}

impl HostHandshake {
    pub fn start(codigo: &PairingCode) -> Self {
        let (spake, msg) = Spake2::<Ed25519Group>::start_a(
            &Password::new(codigo.as_bytes()),
            &Identity::new(ID_HOST),
            &Identity::new(ID_CLIENTE),
        );
        Self { spake, msg }
    }

    /// Mensagem do `PairInit` (a mesma em cada reenvio).
    pub fn message(&self) -> &[u8] {
        &self.msg
    }

    /// Confere a resposta do cliente. [`CryptoError::WrongCode`] se o
    /// cliente usou outro código.
    pub fn finish(self, resposta: &[u8]) -> Result<Session, CryptoError> {
        if resposta.len() != REPLY_LEN {
            return Err(CryptoError::Handshake);
        }
        let (msg, tag) = resposta.split_at(INIT_LEN);
        let segredo = self.spake.finish(msg).map_err(|_| CryptoError::Handshake)?;
        let c = derivar(&segredo);
        confirmacao(&c.confirmacao)
            .decrypt(
                &Nonce::default(),
                Payload {
                    msg: tag,
                    aad: CONFIRMA,
                },
            )
            .map_err(|_| CryptoError::WrongCode)?;
        Ok(Session {
            sealer: Sealer::new(&c.host_cliente),
            opener: Opener::new(&c.cliente_host),
        })
    }
}

/// Lado do cliente: responde a um `PairInit`. Devolve a sessão (pronta para
/// decifrar o que o host mandar) e a resposta a enviar.
pub fn respond(codigo: &PairingCode, init: &[u8]) -> Result<(Session, Vec<u8>), CryptoError> {
    if init.len() != INIT_LEN {
        return Err(CryptoError::Handshake);
    }
    let (spake, msg) = Spake2::<Ed25519Group>::start_b(
        &Password::new(codigo.as_bytes()),
        &Identity::new(ID_HOST),
        &Identity::new(ID_CLIENTE),
    );
    let segredo = spake.finish(init).map_err(|_| CryptoError::Handshake)?;
    let c = derivar(&segredo);
    let tag = confirmacao(&c.confirmacao)
        .encrypt(
            &Nonce::default(),
            Payload {
                msg: &[],
                aad: CONFIRMA,
            },
        )
        .map_err(|_| CryptoError::Seal)?;
    let mut resposta = msg;
    resposta.extend_from_slice(&tag);
    Ok((
        Session {
            sealer: Sealer::new(&c.cliente_host),
            opener: Opener::new(&c.host_cliente),
        },
        resposta,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codigo(s: &str) -> PairingCode {
        PairingCode::parse(s).unwrap()
    }

    fn trocar(s: &mut Session, o: &mut Session, dados: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let seq = s.sealer.next_seq().unwrap();
        let mut d = b"CAB".to_vec();
        d.extend_from_slice(dados);
        s.sealer.seal(seq, &mut d, 3)?;
        o.opener.open(seq, &d, 3)
    }

    #[test]
    fn mesmo_codigo_gera_sessao_nos_dois_sentidos() {
        let c = codigo("K7M4-QX9P");
        let host = HostHandshake::start(&c);
        assert_eq!(host.message().len(), INIT_LEN);
        let (mut cliente, resposta) = respond(&c, host.message()).unwrap();
        assert_eq!(resposta.len(), REPLY_LEN);
        let mut host = host.finish(&resposta).unwrap();
        assert_eq!(trocar(&mut host, &mut cliente, b"video").unwrap(), b"video");
        assert_eq!(trocar(&mut cliente, &mut host, b"ping").unwrap(), b"ping");
    }

    #[test]
    fn codigo_errado_e_detectado_pelo_host() {
        let host = HostHandshake::start(&codigo("K7M4-QX9P"));
        let (mut cliente, resposta) = respond(&codigo("K7M4-QX9Q"), host.message()).unwrap();
        // O host percebe na hora; o cliente, porque nada do host decifra.
        assert!(matches!(host.finish(&resposta), Err(CryptoError::WrongCode)));
        // E um host com o código certo, mas de outro handshake, também não
        // fala com essa sessão.
        let outro = HostHandshake::start(&codigo("K7M4-QX9Q"));
        let (_, r2) = respond(&codigo("K7M4-QX9Q"), outro.message()).unwrap();
        let mut outro = outro.finish(&r2).unwrap();
        assert_eq!(trocar(&mut outro, &mut cliente, b"x"), Err(CryptoError::Auth));
    }

    #[test]
    fn cada_handshake_gera_chaves_novas() {
        let c = codigo("K7M4-QX9P");
        let h1 = HostHandshake::start(&c);
        let (mut cli1, r1) = respond(&c, h1.message()).unwrap();
        let mut h1 = h1.finish(&r1).unwrap();
        let h2 = HostHandshake::start(&c);
        let (_, r2) = respond(&c, h2.message()).unwrap();
        let mut h2 = h2.finish(&r2).unwrap();
        assert!(trocar(&mut h1, &mut cli1, b"ok").is_ok());
        // O 1º pacote da sessão intrusa (seq 0) já cai na janela anti-replay;
        // o 2º passa por ela e é recusado pela autenticação.
        assert_eq!(trocar(&mut h2, &mut cli1, b"x"), Err(CryptoError::Replay));
        assert_eq!(trocar(&mut h2, &mut cli1, b"y"), Err(CryptoError::Auth));
    }

    #[test]
    fn mensagens_malformadas_sao_recusadas() {
        let c = codigo("K7M4-QX9P");
        assert!(matches!(respond(&c, &[0; 10]), Err(CryptoError::Handshake)));
        let host = HostHandshake::start(&c);
        assert!(matches!(host.finish(&[0; 5]), Err(CryptoError::Handshake)));
    }
}
