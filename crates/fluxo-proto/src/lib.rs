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
//! | [`packet`] | Cabeçalho binário fixo de 20 bytes (codificar/decodificar). |
//! | [`fragment`] | Quebrar um frame de vídeo codificado em pacotes que cabem no MTU. |
//! | [`reassembly`] | Juntar os pacotes recebidos de volta em frames completos. |
//! | [`seq`] | Comparação de números de sequência que "dão a volta" (wrap-around). |
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

pub use feedback::{Feedback, FEEDBACK_LEN};
pub use fragment::Fragmenter;
pub use packet::{
    stamp_send_time, PacketHeader, PacketKind, ProtoError, FLAG_KEYFRAME, FLAG_REFRESH_END,
    FLAG_REFRESH_START, HEADER_LEN, MAGIC, PONG_LEN, PROTOCOL_VERSION,
};
pub use reassembly::{CompletedFrame, Reassembler, ReassemblerConfig, ReassemblerStats};

/// Tamanho máximo padrão do *payload* de cada pacote, em bytes.
///
/// 1200 bytes de payload + 20 de cabeçalho Fluxo + 8 de UDP + 20/40 de IP
/// fica abaixo de 1280 (o MTU mínimo garantido pelo IPv6) e bem abaixo dos
/// 1500 típicos de Ethernet. Assim evitamos fragmentação de IP, que na
/// internet aumenta muito a chance de perda. Ver ADR-005 em `docs/DECISOES.md`.
pub const DEFAULT_MAX_PAYLOAD: usize = 1200;
