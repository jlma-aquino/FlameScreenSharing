//! # fluxo-proto
//!
//! Protocolo de rede do Fluxo. Este crate **não faz I/O**: ele só transforma
//! bytes. Quem abre sockets e envia pacotes é a camada de transporte (o
//! crate `fluxo-net`). Isso deixa a lógica 100% testável sem rede.
//!
//! ## Peças
//!
//! | Módulo | Responsabilidade |
//! |---|---|
//! | [`packet`] | Cabeçalho binário fixo de 28 bytes (codificar/decodificar). |
//! | [`fragment`] | Quebrar um frame de vídeo codificado em pacotes que cabem no MTU. |
//! | [`reassembly`] | Juntar os pacotes recebidos de volta em frames completos. |
//! | [`seq`] | Comparação de números de sequência que "dão a volta" (wrap-around). |
//! | [`signal`] | Mensagens do servidor de sinalização (achar o outro lado pela internet). |
//! | [`stun`] | STUN mínimo: perguntar a um servidor público o nosso endereço visto da internet. |
//!
//! ## Fluxo de dados
//!
//! ```text
//! HOST                                         CLIENTE
//! frame codificado ──► Fragmenter ──► UDP ──► Reassembler ──► frame completo
//!   (ex: 40 KB)         (~35 pacotes          (descarta frames      (vai pro
//!                        de 1220 bytes)        incompletos antigos)  decoder)
//! ```
//!
//! A especificação byte a byte está em `docs/PROTOCOLO.md`.

pub mod fec;
pub mod feedback;
pub mod fragment;
pub mod packet;
pub mod reassembly;
pub mod seq;
pub mod signal;
pub mod stun;

pub use feedback::{Feedback, FEEDBACK_LEN};
pub use fragment::Fragmenter;
pub use packet::{
    prepare_seal, stamp_send_time, PacketHeader, PacketKind, ProtoError, FLAG_ENCRYPTED, FLAG_KEYFRAME,
    FLAG_REFRESH_END, FLAG_REFRESH_START, HEADER_LEN, MAGIC, PONG_LEN, PROTOCOL_VERSION,
};
pub use reassembly::{CompletedFrame, Reassembler, ReassemblerConfig, ReassemblerStats};
pub use signal::SignalMessage;

/// Tamanho máximo padrão do *payload* de cada pacote, em bytes.
///
/// 1180 bytes de payload + 28 de cabeçalho Fluxo + 16 da tag da cifra + 8 de
/// UDP + 20/40 de IP = 1252/1272 bytes: abaixo de 1280 (o MTU mínimo
/// garantido pelo IPv6) e bem abaixo dos 1500 típicos de Ethernet. Assim
/// evitamos fragmentação de IP, que na internet aumenta muito a chance de
/// perda. Ver ADR-005 em `docs/DECISOES.md`.
pub const DEFAULT_MAX_PAYLOAD: usize = 1180;
