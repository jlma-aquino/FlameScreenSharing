//! Código de pareamento: 8 caracteres que uma pessoa lê e digita.

use std::fmt;

use sha2::{Digest, Sha256};

use crate::CryptoError;

/// Letras e números sem os que se confundem (0/O, 1/I/L): 31 símbolos.
/// 8 deles dão ~39,6 bits — com o PAKE, cada tentativa de quem não sabe o
/// código vale um palpite só, online (8,5 × 10¹¹ possibilidades).
const ALFABETO: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
const TAMANHO: usize = 8;

/// Código de pareamento normalizado (maiúsculas, sem separadores).
/// Mostrado como `XXXX-XXXX`.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode([u8; TAMANHO]);

impl PairingCode {
    /// Sorteia um código novo com o gerador aleatório do sistema.
    pub fn generate() -> Result<Self, CryptoError> {
        let mut codigo = [0u8; TAMANHO];
        let mut i = 0;
        while i < TAMANHO {
            let mut b = [0u8; 16];
            getrandom::getrandom(&mut b).map_err(|e| CryptoError::Random(e.to_string()))?;
            // Rejeição: só aceita bytes < 248 (8 × 31) para não favorecer
            // nenhum símbolo.
            for x in b {
                if i < TAMANHO && (x as usize) < ALFABETO.len() * 8 {
                    codigo[i] = ALFABETO[x as usize % ALFABETO.len()];
                    i += 1;
                }
            }
        }
        Ok(Self(codigo))
    }

    /// Lê um código digitado: ignora espaços e hífens e aceita minúsculas.
    pub fn parse(texto: &str) -> Result<Self, CryptoError> {
        let limpo: Vec<u8> = texto
            .bytes()
            .filter(|b| !matches!(b, b'-' | b' '))
            .map(|b| b.to_ascii_uppercase())
            .collect();
        if limpo.len() != TAMANHO || !limpo.iter().all(|b| ALFABETO.contains(b)) {
            return Err(CryptoError::InvalidCode);
        }
        let mut codigo = [0u8; TAMANHO];
        codigo.copy_from_slice(&limpo);
        Ok(Self(codigo))
    }

    /// Bytes normalizados (a "senha" do PAKE).
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Número da sala no servidor de sinalização: [`BITS_DA_SALA`] bits do
    /// SHA-256 do código. O servidor vê a sala, não o código: sobram ~20
    /// bits que ele não tem como deduzir, e o PAKE só deixa testar um
    /// palpite por pareamento (quem controla o servidor acerta 1 vez em
    /// ~800 mil). Duas pessoas na mesma sala ao mesmo tempo é raro (1 em
    /// ~1 milhão por par); o servidor recusa a segunda.
    pub fn room(&self) -> u32 {
        let h = Sha256::new()
            .chain_update(b"fluxo sala v1")
            .chain_update(self.0)
            .finalize();
        u32::from_be_bytes([h[0], h[1], h[2], h[3]]) >> (32 - BITS_DA_SALA)
    }
}

/// Bits do código que o servidor de sinalização vê (ver [`PairingCode::room`]).
pub const BITS_DA_SALA: u32 = 20;

impl fmt::Display for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = std::str::from_utf8(&self.0).map_err(|_| fmt::Error)?;
        write!(f, "{}-{}", &s[..4], &s[4..])
    }
}

/// Não mostra o código em logs de depuração por acidente.
impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairingCode(****-****)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gerado_tem_8_simbolos_do_alfabeto_e_varia() {
        let a = PairingCode::generate().unwrap();
        let b = PairingCode::generate().unwrap();
        assert!(a.as_bytes().iter().all(|c| ALFABETO.contains(c)));
        assert_ne!(a, b, "dois sorteios iguais: improvável");
        let mostrado = a.to_string();
        assert_eq!(mostrado.len(), 9);
        assert_eq!(&mostrado[4..5], "-");
    }

    #[test]
    fn digitado_e_normalizado() {
        let c = PairingCode::parse("k7m4-qx9p").unwrap();
        assert_eq!(c.to_string(), "K7M4-QX9P");
        assert_eq!(PairingCode::parse(" K7M4 QX9P ").unwrap(), c);
    }

    #[test]
    fn simbolos_ambiguos_ou_tamanho_errado_sao_recusados() {
        for errado in ["K7M4-QX9", "K7M4-QX9PP", "K7M0-QX9P", "K7M4-QXIP", "K7M4-QX9Ç"] {
            assert_eq!(
                PairingCode::parse(errado),
                Err(CryptoError::InvalidCode),
                "{errado}"
            );
        }
    }

    #[test]
    fn debug_nao_mostra_o_codigo() {
        let c = PairingCode::parse("K7M4-QX9P").unwrap();
        assert!(!format!("{c:?}").contains("K7M4"));
    }

    #[test]
    fn sala_e_fixa_por_codigo_e_cabe_em_20_bits() {
        let c = PairingCode::parse("K7M4-QX9P").unwrap();
        assert_eq!(c.room(), PairingCode::parse("k7m4qx9p").unwrap().room());
        assert!(c.room() < 1 << BITS_DA_SALA);
        let outro = PairingCode::parse("K7M4-QX9Q").unwrap();
        assert_ne!(c.room(), outro.room());
    }
}
