//! # fluxo-capture
//!
//! Captura frames da tela usando a **DXGI Desktop Duplication API** do Windows.
//!
//! ## Por que Desktop Duplication
//!
//! - O frame chega como **textura na GPU** (`ID3D11Texture2D`). No pipeline
//!   final ele vai direto para o encoder de hardware sem nunca passar pela
//!   RAM — isso é o que torna 1080p/4K a 60+ fps barato.
//! - O Windows só entrega um frame novo quando a tela **muda**. Tela parada =
//!   zero trabalho. (Consequência: num benchmark com a área de trabalho
//!   parada você verá poucos FPS; abra um vídeo ou jogo para medir.)
//! - Comparação com Windows.Graphics.Capture está no ADR-002 em
//!   `docs/DECISOES.md`.
//!
//! ## Uso
//!
//! ```no_run
//! use fluxo_capture::{list_monitors, DesktopCapturer};
//!
//! let monitores = list_monitors()?;
//! let mut cap = DesktopCapturer::new(0)?; // monitor 0
//! loop {
//!     // Espera até 100 ms por um frame novo.
//!     if let Some(mut frame) = cap.next_frame(100)? {
//!         println!("frame {}x{}", frame.width(), frame.height());
//!         // frame.texture()  -> textura na GPU (para o encoder)
//!         // frame.copy_to_cpu(&mut buf) -> pixels BGRA na RAM (debug/screenshot)
//!     } // <- o frame é devolvido ao Windows aqui (Drop)
//! }
//! # Ok::<(), fluxo_capture::CaptureError>(())
//! ```
//!
//! ## Organização
//!
//! - `lib.rs` (este arquivo): tipos comuns a todas as plataformas.
//! - `dxgi.rs`: implementação real, só compilada no Windows.
//! - `stub.rs`: implementação vazia para outros sistemas (sempre retorna
//!   [`CaptureError::Unsupported`]).

use std::fmt;

#[cfg(windows)]
mod dxgi;
#[cfg(windows)]
pub use dxgi::{enable_dpi_awareness, list_monitors, DesktopCapturer, Frame};

#[cfg(not(windows))]
mod stub;
#[cfg(not(windows))]
pub use stub::{enable_dpi_awareness, list_monitors, DesktopCapturer, Frame};

/// Um monitor que pode ser capturado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Índice global usado em [`DesktopCapturer::new`] (0, 1, 2...).
    pub index: usize,
    /// Nome do dispositivo no Windows, ex.: `\\.\DISPLAY1`.
    pub device_name: String,
    /// Nome da placa de vídeo à qual o monitor está ligado.
    pub adapter_name: String,
    /// Posição do canto superior esquerdo na área de trabalho virtual.
    pub x: i32,
    /// Posição do canto superior esquerdo na área de trabalho virtual.
    pub y: i32,
    /// Largura em pixels físicos.
    pub width: u32,
    /// Altura em pixels físicos.
    pub height: u32,
}

/// Metadados de um frame capturado.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameInfo {
    /// Momento em que o Windows apresentou o frame, em unidades do
    /// `QueryPerformanceCounter`. 0 se só o cursor mudou.
    pub present_time_qpc: i64,
    /// Quantos frames o Windows acumulou desde a última captura. Valor > 1
    /// significa que não fomos rápidos o bastante e perdemos frames.
    pub accumulated_frames: u32,
    /// `true` se a imagem da tela mudou; `false` se foi só o cursor.
    pub desktop_updated: bool,
}

/// Pixels de um frame copiados para a RAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuImageInfo {
    /// Largura em pixels.
    pub width: u32,
    /// Altura em pixels.
    pub height: u32,
    /// Bytes por linha no buffer de saída (sempre `width * 4`, sem padding).
    pub stride: usize,
}

/// Erros da captura.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    /// Plataforma sem suporte (qualquer coisa que não seja Windows).
    Unsupported,
    /// Não existe monitor com esse índice.
    NoSuchMonitor(usize),
    /// O Windows revogou a captura (troca de resolução, tela de UAC/bloqueio,
    /// jogo entrando em tela cheia exclusiva...). O capturador tenta se
    /// recriar sozinho na próxima chamada de `next_frame`.
    AccessLost,
    /// Outra aplicação já está usando Desktop Duplication neste monitor
    /// além do limite do sistema, ou o recurso não está disponível.
    NotAvailable(String),
    /// Erro genérico do Windows, com o código HRESULT e a mensagem.
    Windows { hresult: i32, message: String },
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => write!(f, "captura de tela só é suportada no Windows"),
            Self::NoSuchMonitor(i) => write!(f, "monitor {i} não existe"),
            Self::AccessLost => write!(f, "acesso à captura perdido (será recriado)"),
            Self::NotAvailable(m) => write!(f, "Desktop Duplication indisponível: {m}"),
            Self::Windows { hresult, message } => {
                write!(f, "erro do Windows 0x{:08X}: {message}", *hresult as u32)
            }
        }
    }
}

impl std::error::Error for CaptureError {}
