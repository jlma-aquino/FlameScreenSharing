//! Cabeçalho binário dos pacotes do Fluxo.
//!
//! Todo pacote UDP do Fluxo começa com este cabeçalho fixo de
//! [`HEADER_LEN`] = 20 bytes, em ordem de bytes *big-endian* (padrão de rede),
//! seguido de `payload_len` bytes de dados.
//!
//! ```text
//!  0       2   3   4               8       10      12  13  14      16              20
//!  +-------+---+---+---------------+-------+-------+---+---+-------+---------------+
//!  | magic |ver|knd|   frame_id    | frag  | frag  |flg|rsv|payload|  timestamp_us |
//!  | "FL"  |   |   |               | index | count |   |   |  len  |               |
//!  +-------+---+---+---------------+-------+-------+---+---+-------+---------------+
//! ```
//!
//! Detalhes de cada campo: `docs/PROTOCOLO.md`.

use std::fmt;

/// Tamanho fixo do cabeçalho em bytes.
pub const HEADER_LEN: usize = 20;

/// Assinatura "FL" (0x46 0x4C) no início de todo pacote.
/// Serve para descartar rapidamente lixo que chegue na porta.
pub const MAGIC: u16 = 0x464C;

/// Versão atual do protocolo. Incrementar sempre que o formato mudar de
/// forma incompatível (e registrar em `docs/PROTOCOLO.md` e no CHANGELOG).
///
/// - 1: Video, Ping, Pong.
/// - 2: + KeyframeRequest.
pub const PROTOCOL_VERSION: u8 = 2;

/// Bit 0 de `flags`: o frame é um *keyframe* (IDR), decodificável sozinho.
pub const FLAG_KEYFRAME: u8 = 0b0000_0001;

/// Tipo do pacote (campo `kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketKind {
    /// Um fragmento de um frame de vídeo codificado.
    Video = 1,
    /// Pedido de medição de RTT. `frame_id` = número do ping,
    /// `timestamp_us` = relógio de quem enviou.
    Ping = 2,
    /// Resposta a um [`PacketKind::Ping`], ecoando `frame_id` e `timestamp_us`.
    Pong = 3,
    /// Cliente → host: "perdi a referência, mande um keyframe". `frame_id`
    /// = número do pedido, `timestamp_us` = relógio de quem pediu.
    KeyframeRequest = 4,
}

impl PacketKind {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Video),
            2 => Some(Self::Ping),
            3 => Some(Self::Pong),
            4 => Some(Self::KeyframeRequest),
            _ => None,
        }
    }

    /// Cabeçalho de um pacote de controle (sem payload nem fragmentos).
    pub fn control(self, id: u32, timestamp_us: u32) -> PacketHeader {
        PacketHeader {
            kind: self,
            frame_id: id,
            frag_index: 0,
            frag_count: 0,
            flags: 0,
            payload_len: 0,
            timestamp_us,
        }
    }
}

/// Erros ao decodificar um pacote recebido.
///
/// Pacotes inválidos devem ser **descartados silenciosamente** pela camada de
/// rede (no máximo contados em estatística): qualquer um pode mandar lixo
/// para uma porta UDP aberta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtoError {
    /// O datagrama é menor que o cabeçalho.
    TooShort { len: usize },
    /// Os dois primeiros bytes não são "FL".
    BadMagic(u16),
    /// Versão de protocolo diferente da nossa.
    UnsupportedVersion(u8),
    /// Valor de `kind` desconhecido.
    UnknownKind(u8),
    /// `payload_len` não bate com o tamanho real do datagrama.
    LengthMismatch { declared: usize, actual: usize },
    /// `frag_index >= frag_count`, ou `frag_count == 0`.
    BadFragment { index: u16, count: u16 },
    /// Tentativa de codificar um payload maior que 65535 bytes.
    PayloadTooLarge(usize),
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { len } => write!(f, "pacote curto demais ({len} bytes)"),
            Self::BadMagic(m) => write!(f, "assinatura inválida 0x{m:04X}"),
            Self::UnsupportedVersion(v) => write!(f, "versão de protocolo {v} não suportada"),
            Self::UnknownKind(k) => write!(f, "tipo de pacote desconhecido {k}"),
            Self::LengthMismatch { declared, actual } => {
                write!(f, "payload declarado {declared} bytes, recebido {actual}")
            }
            Self::BadFragment { index, count } => {
                write!(f, "fragmento inválido {index}/{count}")
            }
            Self::PayloadTooLarge(n) => write!(f, "payload de {n} bytes excede 65535"),
        }
    }
}

impl std::error::Error for ProtoError {}

/// Cabeçalho decodificado de um pacote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
    /// Tipo do pacote.
    pub kind: PacketKind,
    /// Número do frame (Video) ou do ping (Ping/Pong). Cresce 1 a 1 e pode
    /// dar a volta — compare sempre com [`crate::seq::is_newer`].
    pub frame_id: u32,
    /// Posição deste fragmento dentro do frame, de 0 a `frag_count - 1`.
    pub frag_index: u16,
    /// Quantos fragmentos o frame tem no total (≥ 1).
    pub frag_count: u16,
    /// Bits de flags. Ver [`FLAG_KEYFRAME`].
    pub flags: u8,
    /// Tamanho do payload que vem logo após o cabeçalho.
    pub payload_len: u16,
    /// Momento da captura do frame em microssegundos (relógio do host,
    /// truncado em 32 bits — dá a volta a cada ~71 minutos). Usado para
    /// medir latência e ordenar a apresentação.
    pub timestamp_us: u32,
}

impl PacketHeader {
    /// `true` se o frame deste pacote é um keyframe.
    pub fn is_keyframe(&self) -> bool {
        self.flags & FLAG_KEYFRAME != 0
    }

    /// Escreve o cabeçalho nos primeiros [`HEADER_LEN`] bytes de `out`.
    ///
    /// # Panics
    /// Se `out.len() < HEADER_LEN` (erro de programação, não de rede).
    pub fn write_to(&self, out: &mut [u8]) {
        let b = &mut out[..HEADER_LEN];
        b[0..2].copy_from_slice(&MAGIC.to_be_bytes());
        b[2] = PROTOCOL_VERSION;
        b[3] = self.kind as u8;
        b[4..8].copy_from_slice(&self.frame_id.to_be_bytes());
        b[8..10].copy_from_slice(&self.frag_index.to_be_bytes());
        b[10..12].copy_from_slice(&self.frag_count.to_be_bytes());
        b[12] = self.flags;
        b[13] = 0; // reservado
        b[14..16].copy_from_slice(&self.payload_len.to_be_bytes());
        b[16..20].copy_from_slice(&self.timestamp_us.to_be_bytes());
    }

    /// Monta um pacote completo (cabeçalho + payload) num `Vec` novo.
    /// O campo `payload_len` é preenchido automaticamente.
    pub fn encode_with_payload(mut self, payload: &[u8]) -> Result<Vec<u8>, ProtoError> {
        self.payload_len =
            u16::try_from(payload.len()).map_err(|_| ProtoError::PayloadTooLarge(payload.len()))?;
        let mut buf = vec![0u8; HEADER_LEN + payload.len()];
        self.write_to(&mut buf);
        buf[HEADER_LEN..].copy_from_slice(payload);
        Ok(buf)
    }

    /// Decodifica um datagrama recebido. Em caso de sucesso devolve o
    /// cabeçalho e uma fatia apontando para o payload (sem cópia).
    pub fn decode(datagram: &[u8]) -> Result<(PacketHeader, &[u8]), ProtoError> {
        if datagram.len() < HEADER_LEN {
            return Err(ProtoError::TooShort { len: datagram.len() });
        }
        let be16 = |i: usize| u16::from_be_bytes([datagram[i], datagram[i + 1]]);
        let be32 =
            |i: usize| u32::from_be_bytes([datagram[i], datagram[i + 1], datagram[i + 2], datagram[i + 3]]);

        let magic = be16(0);
        if magic != MAGIC {
            return Err(ProtoError::BadMagic(magic));
        }
        if datagram[2] != PROTOCOL_VERSION {
            return Err(ProtoError::UnsupportedVersion(datagram[2]));
        }
        let kind = PacketKind::from_u8(datagram[3]).ok_or(ProtoError::UnknownKind(datagram[3]))?;

        let header = PacketHeader {
            kind,
            frame_id: be32(4),
            frag_index: be16(8),
            frag_count: be16(10),
            flags: datagram[12],
            payload_len: be16(14),
            timestamp_us: be32(16),
        };

        let actual = datagram.len() - HEADER_LEN;
        if header.payload_len as usize != actual {
            return Err(ProtoError::LengthMismatch {
                declared: header.payload_len as usize,
                actual,
            });
        }
        if kind == PacketKind::Video && (header.frag_count == 0 || header.frag_index >= header.frag_count) {
            return Err(ProtoError::BadFragment {
                index: header.frag_index,
                count: header.frag_count,
            });
        }

        Ok((header, &datagram[HEADER_LEN..]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exemplo() -> PacketHeader {
        PacketHeader {
            kind: PacketKind::Video,
            frame_id: 0xDEAD_BEEF,
            frag_index: 3,
            frag_count: 10,
            flags: FLAG_KEYFRAME,
            payload_len: 0,
            timestamp_us: 123_456,
        }
    }

    #[test]
    fn ida_e_volta() {
        let pkt = exemplo().encode_with_payload(b"ola").unwrap();
        assert_eq!(pkt.len(), HEADER_LEN + 3);
        assert_eq!(&pkt[0..2], b"FL");
        let (h, payload) = PacketHeader::decode(&pkt).unwrap();
        assert_eq!(payload, b"ola");
        assert_eq!(h.frame_id, 0xDEAD_BEEF);
        assert_eq!(h.frag_index, 3);
        assert_eq!(h.frag_count, 10);
        assert!(h.is_keyframe());
        assert_eq!(h.payload_len, 3);
        assert_eq!(h.timestamp_us, 123_456);
    }

    #[test]
    fn rejeita_curto() {
        assert_eq!(
            PacketHeader::decode(&[0u8; 5]),
            Err(ProtoError::TooShort { len: 5 })
        );
    }

    #[test]
    fn rejeita_magic_errado() {
        let mut pkt = exemplo().encode_with_payload(b"").unwrap();
        pkt[0] = b'X';
        assert!(matches!(PacketHeader::decode(&pkt), Err(ProtoError::BadMagic(_))));
    }

    #[test]
    fn rejeita_versao() {
        let mut pkt = exemplo().encode_with_payload(b"").unwrap();
        pkt[2] = 99;
        assert_eq!(
            PacketHeader::decode(&pkt),
            Err(ProtoError::UnsupportedVersion(99))
        );
    }

    #[test]
    fn rejeita_tamanho_inconsistente() {
        let mut pkt = exemplo().encode_with_payload(b"abc").unwrap();
        pkt.pop(); // perdeu 1 byte no caminho
        assert!(matches!(
            PacketHeader::decode(&pkt),
            Err(ProtoError::LengthMismatch {
                declared: 3,
                actual: 2
            })
        ));
    }

    #[test]
    fn rejeita_fragmento_invalido() {
        let mut h = exemplo();
        h.frag_index = 10; // == frag_count
        let pkt = h.encode_with_payload(b"").unwrap();
        assert!(matches!(
            PacketHeader::decode(&pkt),
            Err(ProtoError::BadFragment { .. })
        ));
    }

    #[test]
    fn rejeita_payload_gigante() {
        let grande = vec![0u8; 70_000];
        assert_eq!(
            exemplo().encode_with_payload(&grande),
            Err(ProtoError::PayloadTooLarge(70_000))
        );
    }

    #[test]
    fn pedido_de_keyframe_vai_e_volta() {
        let h = PacketKind::KeyframeRequest.control(7, 123_456);
        let pkt = h.encode_with_payload(&[]).unwrap();
        assert_eq!(pkt.len(), HEADER_LEN);
        let (d, payload) = PacketHeader::decode(&pkt).unwrap();
        assert_eq!(d.kind, PacketKind::KeyframeRequest);
        assert_eq!((d.frame_id, d.timestamp_us), (7, 123_456));
        assert!(payload.is_empty());
    }

    #[test]
    fn rejeita_versao_anterior() {
        let mut pkt = PacketKind::Ping.control(1, 2).encode_with_payload(&[]).unwrap();
        pkt[2] = 1; // pacote de um Fluxo antigo
        assert_eq!(PacketHeader::decode(&pkt), Err(ProtoError::UnsupportedVersion(1)));
    }
}
