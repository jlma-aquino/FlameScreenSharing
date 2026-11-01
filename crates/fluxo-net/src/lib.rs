//! # fluxo-net
//!
//! Transporte UDP do Fluxo: é aqui que os pacotes do `fluxo-proto` viram
//! datagramas de verdade.
//!
//! | Tipo | Lado | Faz |
//! |---|---|---|
//! | [`VideoSender`] | host | Fragmenta frames, envia com *pacing*, mede RTT |
//! | [`VideoReceiver`] | cliente | Recebe, remonta frames, responde/mede pings |
//! | [`SignalServer`] | servidor | Apresenta host e cliente em redes diferentes (sinalização) |
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
mod conexao;
mod delay;
mod history;
mod internet;
mod observar;
mod pacer;
mod portas;
mod receiver;
mod recovery;
mod rtt;
mod sender;
mod signal;
mod sim;
mod stun;
mod sync;
mod upnp;

use std::fmt;

pub use bitrate::{BitrateChange, BitrateController, NetworkReport, DEFAULT_QUEUE_LIMIT_MS, START_KBPS};
pub use clock::{clock_diff_us, clock_us};
pub use conexao::{CodeError, ConnectionCode, ReplyCode};
pub use internet::Internet;
pub use observar::{PairingEvent, PairingObserver};
pub use receiver::{
    AudioPacket, AudioStream, CursorView, Puncher, ReceivedFrame, ReceiverConfig, ReceiverStats,
    VideoReceiver,
};
pub use recovery::{FrameAction, Recovery, RecoveryRequest};
pub use rtt::RttStats;
pub use sender::{SenderConfig, SenderStats, VideoSender, PUNCH_TTL};
pub use signal::{SignalEvent, SignalServer, ROOM_TTL};
pub use stun::{discover_public_addr, probe_nat, PublicAddr, StunAnswer, STUN_SERVERS};
pub use upnp::{find_gateway, is_private_v4, open_port, Gateway, PortMapping, UpnpOutcome, UPNP_LEASE};
// Flags que acompanham cada frame (`send_frame_with_flags`, `CompletedFrame`).
pub use fluxo_proto::{VideoCodec, FLAG_KEYFRAME, FLAG_REFRESH_END, FLAG_REFRESH_START};
// Código de pareamento (`SenderConfig`/`ReceiverConfig::pairing_code`).
pub use fluxo_crypto::PairingCode;
// Imagem do cursor (`VideoSender::set_cursor_shape`, `CursorView::shape`).
pub use fluxo_proto::CursorShape;

/// Erros da camada de rede.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetError {
    /// Erro de socket, com a operação que falhou.
    Io { context: &'static str, message: String },
    /// O frame não pôde ser fragmentado (grande demais).
    Proto(fluxo_proto::ProtoError),
    /// A thread de rede terminou (o objeto está sendo destruído ou falhou).
    Closed,
    /// O pareamento com o cliente falhou (código errado, ou ele não respondeu).
    Pairing(String),
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
            Self::Pairing(m) => write!(f, "pareamento: {m}"),
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

/// Pong para um Ping recebido: ecoa `frame_id` e `timestamp_us` e leva o
/// nosso relógio no payload (sincronização, ver `sync.rs`).
pub(crate) fn pong_for(ping: &fluxo_proto::PacketHeader) -> Option<Vec<u8>> {
    let agora = clock_us();
    fluxo_proto::PacketHeader {
        kind: fluxo_proto::PacketKind::Pong,
        send_us: agora,
        ..*ping
    }
    .encode_with_payload(&agora.to_be_bytes())
    .ok()
}

/// Relógio de quem respondeu, lido do payload de um Pong.
pub(crate) fn pong_clock(payload: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(
        payload.get(..fluxo_proto::PONG_LEN)?.try_into().ok()?,
    ))
}

/// IP deste PC na rede local: o da interface que leva à internet. Descoberto
/// sem mandar nada (só pergunta ao sistema a rota até um IP público).
pub fn local_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:53").ok()?;
    Some(s.local_addr().ok()?.ip()).filter(|ip| !ip.is_unspecified())
}

/// Endereços locais para informar ao servidor de sinalização: o IP da rede
/// local com a porta do socket (o outro lado tenta direto, se estiver na
/// mesma rede).
pub(crate) fn local_candidates(socket: Option<&std::net::UdpSocket>) -> Vec<std::net::SocketAddr> {
    let porta = socket.and_then(|s| s.local_addr().ok()).map_or(0, |a| a.port());
    local_ip()
        .filter(|_| porta != 0)
        .map(|ip| std::net::SocketAddr::new(ip, porta))
        .into_iter()
        .collect()
}

/// Porta padrão do servidor de sinalização (`fluxo-sinal`).
pub const SIGNAL_PORT: u16 = 7000;

/// Lê o endereço de um servidor de sinalização como digitado: `host:porta`,
/// ou só `host` (porta [`SIGNAL_PORT`]); nomes são resolvidos (DNS),
/// preferindo IPv4.
pub fn resolve_signal_server(texto: &str) -> Result<std::net::SocketAddr, NetError> {
    use std::net::ToSocketAddrs;
    let erro = |message: String| NetError::Io {
        context: "servidor de sinalização",
        message,
    };
    let com_porta = match texto.parse::<std::net::SocketAddr>() {
        Ok(a) => return Ok(a),
        // Sem ":porta" (IPv6 puro tem vários ":"; aí a porta é obrigatória).
        Err(_) if !texto.contains(':') => format!("{texto}:{SIGNAL_PORT}"),
        Err(_) => texto.to_string(),
    };
    let enderecos: Vec<_> = com_porta
        .to_socket_addrs()
        .map_err(|e| erro(format!("{texto}: {e}")))?
        .collect();
    enderecos
        .iter()
        .find(|a| a.is_ipv4())
        .or(enderecos.first())
        .copied()
        .ok_or_else(|| erro(format!("{texto}: nenhum endereço")))
}
