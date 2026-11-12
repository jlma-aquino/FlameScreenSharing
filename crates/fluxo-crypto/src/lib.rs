//! # fluxo-crypto
//!
//! Segurança do Fluxo, sem I/O:
//!
//! | Peça | Faz |
//! |---|---|
//! | [`PairingCode`] | Código de 8 caracteres que uma pessoa lê e digita (`K7M4-QX9P`) |
//! | [`HostHandshake`] / [`respond`] | Handshake SPAKE2: o código vira uma chave de sessão |
//! | [`Sealer`] / [`Opener`] | Cifra ChaCha20-Poly1305 de cada pacote, com anti-replay |
//! | [`ServerKey`] | Chave de acesso ao servidor de sinalização (`FSS1-…`): endereço + segredo; assina os pedidos |
//! | [`UpdateManifest`] | Manifesto assinado (Ed25519) de cada versão publicada, conferido pelo atualizador |
//!
//! Só usa bibliotecas prontas e auditadas (RustCrypto, e o ed25519-dalek para as atualizações); nenhum
//! algoritmo é escrito aqui. O `fluxo-net` liga estas peças ao socket; o
//! formato dos pacotes de pareamento está em `docs/PROTOCOLO.md`.

mod chave;
mod cifra;
mod codigo;
mod handshake;
mod manifesto;

use std::fmt;

pub use chave::{ServerKey, KEY_TAG_LEN};
pub use cifra::{Opener, Sealer, TAG_LEN};
pub use codigo::{PairingCode, ALFABETO, BITS_DA_SALA};
pub use handshake::{respond, HostHandshake, Session, INIT_LEN, REPLY_LEN};
pub use manifesto::{de_hex, hex, update_public_key, UpdateManifest, UPDATE_PUBLIC_KEY_LEN};

/// Erros de criptografia. Nenhum leva detalhes que ajudem um atacante.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// O código digitado não tem 8 caracteres do alfabeto.
    InvalidCode,
    /// A chave do servidor não confere (copiada errada, ou não é uma chave).
    InvalidKey,
    /// O gerador aleatório do sistema falhou.
    Random(String),
    /// Mensagem de pareamento malformada.
    Handshake,
    /// O outro lado usou outro código de pareamento.
    WrongCode,
    /// Pacote alterado, forjado ou de outra sessão.
    Auth,
    /// Pacote repetido ou velho demais (anti-replay).
    Replay,
    /// A cifra recusou cifrar (não acontece com tamanhos de UDP).
    Seal,
    /// Manifesto de atualização malformado.
    Manifest,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCode => write!(
                f,
                "código de pareamento inválido (8 letras/números, ex.: K7M4-QX9P)"
            ),
            Self::InvalidKey => write!(
                f,
                "chave do servidor inválida (confira se copiou inteira: FSS1-…)"
            ),
            Self::Random(e) => write!(f, "gerador aleatório do sistema falhou: {e}"),
            Self::Handshake => write!(f, "mensagem de pareamento inválida"),
            Self::WrongCode => write!(f, "o código de pareamento não confere"),
            Self::Auth => write!(f, "pacote não autenticado"),
            Self::Replay => write!(f, "pacote repetido"),
            Self::Seal => write!(f, "falha ao cifrar"),
            Self::Manifest => write!(f, "manifesto de atualização malformado"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// Bytes aleatórios do gerador do sistema (ex.: identificadores de pedidos
/// STUN, que não podem ser adivinhados por quem forja respostas).
pub fn random_bytes(buf: &mut [u8]) -> Result<(), CryptoError> {
    getrandom::getrandom(buf).map_err(|e| CryptoError::Random(e.to_string()))
}
