//! # fluxo-net
//!
//! Transporte UDP do Fluxo: é aqui que os pacotes do `fluxo-proto` viram
//! datagramas de verdade.
//!
//! | Tipo | Lado | Faz |
//! |---|---|---|
//! | [`VideoSender`] | host | Fragmenta frames, envia com *pacing*, mede RTT |
//! | [`VideoReceiver`] | cliente | Recebe, remonta frames, responde/mede pings |
//!
//! Os dois rodam em threads próprias: enviar um frame com pacing leva alguns
//! milissegundos, e a captura/encode não pode esperar por isso; do lado do
//! cliente, a leitura do socket não pode parar enquanto o decoder trabalha
//! (senão o buffer do sistema transborda e pacotes se perdem).
//!
//! ## Pacing
//!
//! Um frame de 40 KB vira ~35 pacotes. Mandá-los de uma vez cria uma rajada
//! que roteadores e Wi-Fi descartam quando o buffer enche. O [`VideoSender`]
//! espalha os pacotes a uma taxa limitada (por padrão 2,5× o bitrate do
//! vídeo), permitindo rajadas curtas de 10 pacotes. Custo: um frame de 40 KB
//! leva ~6 ms para sair inteiro a 50 Mbps.
//!
//! ## Relógio
//!
//! `timestamp_us` dos pacotes usa [`clock_us`]: o relógio de parede em µs,
//! truncado para 32 bits. Na mesma máquina (ou com relógios sincronizados
//! por NTP), o receptor calcula a latência de ida
//! ([`ReceivedFrame::transit_us`]). O RTT dos pings não depende disso.
//!
//! ## Exemplo
//!
//! ```no_run
//! use std::time::Duration;
//! use fluxo_net::{clock_us, ReceiverConfig, SenderConfig, VideoReceiver, VideoSender};
//!
//! // Cliente
//! let rx = VideoReceiver::bind("0.0.0.0:9000", ReceiverConfig::default())?;
//! // Host
//! let tx = VideoSender::connect("0.0.0.0:0", "192.168.0.10:9000", SenderConfig::for_bitrate(20_000_000))?;
//!
//! tx.send_frame(vec![0u8; 40_000], true, clock_us())?;
//! if let Some(f) = rx.recv_frame(Duration::from_millis(100))? {
//!     println!("frame {} com {} bytes", f.frame.frame_id, f.frame.data.len());
//! }
//! # Ok::<(), fluxo_net::NetError>(())
//! ```

mod bitrate;
mod clock;
mod delay;
mod history;
mod pacer;
mod receiver;
mod rtt;
mod sender;

use std::fmt;

pub use bitrate::{BitrateChange, BitrateController, NetworkReport};
pub use clock::{clock_diff_us, clock_us};
pub use receiver::{ReceivedFrame, ReceiverConfig, ReceiverStats, VideoReceiver};
pub use rtt::RttStats;
pub use sender::{SenderConfig, SenderStats, VideoSender};

/// Erros da camada de rede.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetError {
    /// Erro de socket, com a operação que falhou.
    Io { context: &'static str, message: String },
    /// O frame não pôde ser fragmentado (grande demais).
    Proto(fluxo_proto::ProtoError),
    /// A thread de rede terminou (o objeto está sendo destruído ou falhou).
    Closed,
}

impl NetError {
    pub(crate) fn io(context: &'static str) -> impl Fn(std::io::Error) -> NetError {
        move |e| NetError::Io {
            context,
            message: e.to_string(),
        }
    }
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, message } => write!(f, "{context}: {message}"),
            Self::Proto(e) => write!(f, "protocolo: {e}"),
            Self::Closed => write!(f, "a thread de rede foi encerrada"),
        }
    }
}

impl std::error::Error for NetError {}

/// Erros de leitura que não significam falha: tempo esgotado, ou o Windows
/// avisando que um datagrama nosso anterior não teve destino (porta do outro
/// lado ainda fechada — chega como "connection reset" mesmo em UDP).
pub(crate) fn is_transient(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    matches!(
        e.kind(),
        WouldBlock | TimedOut | Interrupted | ConnectionReset | ConnectionRefused
    )
}
