//! # fluxo-decode
//!
//! Decodifica o H.264 que chega da rede em **texturas D3D11 na GPU**, prontas
//! para a tela. Usa o decoder H.264 do próprio Windows (MFT da Microsoft) com
//! aceleração DXVA: funciona com NVIDIA, AMD e Intel sem dependências extras.
//! Por que esse caminho: ADR-007 em `docs/DECISOES.md`.
//!
//! ## Uso
//!
//! ```no_run
//! # #[cfg(windows)] {
//! use fluxo_decode::{create_device, VideoDecoder};
//!
//! let device = create_device()?;            // GPU padrão, com suporte a vídeo
//! let mut dec = VideoDecoder::new(&device)?;
//! println!("decoder: {}", dec.name());
//! # let bytes_h264: Vec<u8> = Vec::new();
//! if let Some(frame) = dec.decode(&bytes_h264)? {
//!     // frame.texture() + frame.subresource(): imagem NV12 na GPU
//!     println!("{}x{}", frame.width(), frame.height());
//! }
//! # }
//! # Ok::<(), fluxo_decode::DecodeError>(())
//! ```
//!
//! ## Organização
//!
//! - `lib.rs` (este arquivo): erro comum a todas as plataformas.
//! - `mf.rs`: decoder Media Foundation e criação do device (só Windows).

use std::fmt;

#[cfg(windows)]
mod mf;

#[cfg(windows)]
pub use mf::{create_device, DecodedFrame, VideoDecoder};

/// Erros do decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Plataforma sem suporte (qualquer coisa que não seja Windows).
    Unsupported,
    /// Erro do Windows (D3D11 / Media Foundation), com a chamada que falhou.
    Windows {
        context: &'static str,
        hresult: i32,
        message: String,
    },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => write!(f, "decoder de vídeo só é suportado no Windows"),
            Self::Windows {
                context,
                hresult,
                message,
            } => write!(
                f,
                "{context}: erro do Windows 0x{:08X}: {message}",
                *hresult as u32
            ),
        }
    }
}

impl std::error::Error for DecodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mensagem_traz_a_chamada_e_o_hresult() {
        let e = DecodeError::Windows {
            context: "ProcessInput",
            hresult: 0x8000_4005_u32 as i32,
            message: "falha".into(),
        };
        assert_eq!(e.to_string(), "ProcessInput: erro do Windows 0x80004005: falha");
    }
}
