//! # fluxo-audio
//!
//! O som do PC que compartilha, levado ao PC que assiste.
//!
//! | Peça | Faz | Plataforma |
//! |---|---|---|
//! | [`Encoder`] / [`Decoder`] | Opus 48 kHz estéreo, quadros de 10 ms, FEC e ocultação de perda | qualquer |
//! | [`JitterBuffer`] | Segura alguns quadros e entrega em ritmo fixo; o alvo acompanha a rede | qualquer |
//! | [`Converter`] | Taxa e canais da placa ↔ 48 kHz estéreo | qualquer |
//! | `LoopbackCapture` / `AudioOutput` | WASAPI: captura do que o PC toca; reprodução | Windows |
//! | `AudioPlayer` | Buffer de jitter + decoder + saída: toca o que chega da rede | Windows |
//!
//! ```text
//! HOST                                              CLIENTE
//! WASAPI loopback → Converter → Encoder ─ rede ─► JitterBuffer → Decoder → Converter → WASAPI
//!  (o que toca)      48k estéreo  Opus 10 ms       (~30 ms)      (FEC/PLC)   taxa da placa
//! ```

mod jitter;
mod opus;
#[cfg(windows)]
mod player;
mod resample;
#[cfg(windows)]
mod wasapi;

use std::fmt;

pub use jitter::{JitterBuffer, JitterStats, Next, MAX_TARGET, MIN_TARGET, START_TARGET};
pub use opus::{Decoder, Encoder};
#[cfg(windows)]
pub use player::{AudioPlayer, PlayerStats, DEVICE_QUEUE_MS};
pub use resample::Converter;
#[cfg(windows)]
pub use wasapi::{output_devices, AudioOutput, LoopbackCapture, OutputDevice};

/// Taxa do Opus (e de tudo entre os dois Converters).
pub const SAMPLE_RATE: u32 = 48_000;
/// Estéreo.
pub const CHANNELS: usize = 2;
/// Amostras por canal num quadro de 10 ms.
pub const FRAME_SAMPLES: usize = SAMPLE_RATE as usize / 100;
/// Bitrate padrão do Opus (bit/s): música estéreo transparente.
pub const DEFAULT_BITRATE: i32 = 128_000;

/// Erros de áudio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioError {
    /// Código de erro da libopus.
    Opus(i32),
    /// Quadro com o número errado de amostras.
    FrameSize(usize),
    /// Erro do Windows (WASAPI), com o que estava sendo feito.
    Windows { context: &'static str, message: String },
    /// Formato da placa que não sabemos ler.
    Format(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Opus(c) => write!(f, "Opus: erro {c}"),
            Self::FrameSize(n) => write!(
                f,
                "quadro de áudio com {n} amostras (esperado {})",
                FRAME_SAMPLES * CHANNELS
            ),
            Self::Windows { context, message } => write!(f, "{context}: {message}"),
            Self::Format(m) => write!(f, "formato de áudio não suportado: {m}"),
        }
    }
}

impl std::error::Error for AudioError {}
