//! Cabeçalho binário dos pacotes do Fluxo.
//!
//! Todo pacote UDP do Fluxo começa com este cabeçalho fixo de
//! [`HEADER_LEN`] = 28 bytes, em ordem de bytes *big-endian* (padrão de rede),
//! seguido de `payload_len` bytes de dados.
//!
//! ```text
//!  0       2   3   4               8       10      12  13  14      16              20              24              28
//!  +-------+---+---+---------------+-------+-------+---+---+-------+---------------+---------------+---------------+
//!  | magic |ver|knd|   frame_id    | frag  | frag  |flg|fec|payload|  timestamp_us |    send_us    |      seq      |
//!  | "FL"  |   |   |               | index | count |   |grp|  len  |   (captura)   |    (envio)    | (nonce/cifra) |
//!  +-------+---+---+---------------+-------+-------+---+---+-------+---------------+---------------+---------------+
//! ```
//!
//! Com [`FLAG_ENCRYPTED`], o payload vem cifrado e com a tag de 16 bytes no
//! fim; o cabeçalho continua visível, mas autenticado. Detalhes de cada
//! campo: `docs/PROTOCOLO.md`.

use std::fmt;

/// Tamanho fixo do cabeçalho em bytes.
pub const HEADER_LEN: usize = 28;

/// Posição de `send_us` no cabeçalho: o emissor o preenche com
/// [`stamp_send_time`] na hora exata de mandar cada pacote.
const SEND_US_AT: usize = 20;
/// Posição de `seq`: número do pacote no sentido dele, nonce da cifra.
const SEQ_AT: usize = 24;

/// Assinatura "FL" (0x46 0x4C) no início de todo pacote.
/// Serve para descartar rapidamente lixo que chegue na porta.
pub const MAGIC: u16 = 0x464C;

/// Versão atual do protocolo. Incrementar sempre que o formato mudar de
/// forma incompatível (e registrar em `docs/PROTOCOLO.md` e no CHANGELOG).
///
/// - 1: Video, Ping, Pong.
/// - 2: + KeyframeRequest.
/// - 3: + Parity (FEC) e o byte 13 do cabeçalho vira `fec_group`.
/// - 4: + Feedback (relatório do cliente para o bitrate adaptativo).
/// - 5: Pong leva 4 bytes com o relógio de quem responde (sincronização).
/// - 6: cabeçalho de 24 bytes: + `send_us`, a hora em que o pacote saiu.
/// - 7: flags de intra-refresh no vídeo; o KeyframeRequest diz se aceita uma
///   onda de intra-refresh em vez de keyframe.
/// - 8: cabeçalho de 28 bytes (+ `seq`), pacotes de pareamento (PairInit,
///   PairReply) e payload cifrado ([`FLAG_ENCRYPTED`]).
/// - 9: sinalização pela internet (Signal) e furo de NAT (Punch).
/// - 10: cursor do mouse à parte (Cursor).
/// - 11: áudio (Audio): um quadro Opus de 10 ms por pacote.
pub const PROTOCOL_VERSION: u8 = 11;

/// Bit 0 de `flags`: o frame é um *keyframe* (IDR), decodificável sozinho.
/// Num [`PacketKind::KeyframeRequest`]: o cliente **precisa** de um keyframe
/// (não tem imagem nenhuma, ou o decoder falhou); sem o bit, uma onda de
/// intra-refresh também serve.
pub const FLAG_KEYFRAME: u8 = 0b0000_0001;
/// Bit 1 de `flags`: primeiro frame de uma onda de intra-refresh.
pub const FLAG_REFRESH_START: u8 = 0b0000_0010;
/// Bit 2 de `flags`: último frame de uma onda de intra-refresh. Um decoder
/// que perdeu algo **antes** do início da onda está limpo de novo depois
/// deste frame.
pub const FLAG_REFRESH_END: u8 = 0b0000_0100;
/// Bit 3 de `flags`: payload cifrado (ChaCha20-Poly1305, tag no fim),
/// cabeçalho autenticado. Vale para qualquer tipo menos os de pareamento.
pub const FLAG_ENCRYPTED: u8 = 0b0000_1000;

/// Tamanho do payload de um [`PacketKind::Pong`]: o relógio (u32, big-endian)
/// de quem respondeu.
pub const PONG_LEN: usize = 4;

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
    /// Payload: [`PONG_LEN`] bytes com o relógio de quem respondeu, na hora de
    /// responder — para sincronizar os relógios (estilo NTP).
    Pong = 3,
    /// Cliente → host: "perdi a referência, mande um keyframe". `frame_id`
    /// = número do pedido, `timestamp_us` = relógio de quem pediu.
    KeyframeRequest = 4,
    /// Paridade XOR de um grupo de fragmentos de vídeo (FEC, ver [`crate::fec`]).
    /// `frame_id`, `frag_count`, `flags` e `timestamp_us` iguais aos do
    /// frame; `frag_index` = número do grupo; `fec_group` = fragmentos por grupo.
    Parity = 5,
    /// Cliente → host: relatório periódico (pacotes recebidos, atraso de fila,
    /// perdas) para o bitrate adaptativo. Payload: [`crate::Feedback`].
    Feedback = 6,
    /// Host → cliente: início do pareamento (mensagem SPAKE2). Em claro.
    /// `frame_id` = número da tentativa (o mesmo nos reenvios).
    PairInit = 7,
    /// Cliente → host: resposta do pareamento (mensagem SPAKE2 + confirmação
    /// da chave). Em claro; `frame_id` igual ao do PairInit.
    PairReply = 8,
    /// Com o servidor de sinalização, nos dois sentidos: payload
    /// [`crate::SignalMessage`]. Em claro (o servidor não tem chave);
    /// `frame_id` = número do pedido, ecoado na resposta.
    Signal = 9,
    /// Cliente → host, sem payload: abre o caminho no NAT/firewall do
    /// cliente para os pacotes do host. Quem recebe ignora. Em claro.
    Punch = 10,
    /// Cursor do mouse: posição e forma (host → cliente) e pedido de forma
    /// (cliente → host). Payload: [`crate::cursor::CursorMessage`]. Cifrado.
    Cursor = 11,
    // 12 reservado: Input (controle remoto; ver docs/PROTOCOLO.md).
    /// Host → cliente: um quadro de áudio Opus (10 ms, 48 kHz estéreo).
    /// `frame_id` = número do quadro (sobe de 1 em 1), `timestamp_us` =
    /// captura da primeira amostra. Cifrado.
    Audio = 13,
}

impl PacketKind {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Video),
            2 => Some(Self::Ping),
            3 => Some(Self::Pong),
            4 => Some(Self::KeyframeRequest),
            5 => Some(Self::Parity),
            6 => Some(Self::Feedback),
            7 => Some(Self::PairInit),
            8 => Some(Self::PairReply),
            9 => Some(Self::Signal),
            10 => Some(Self::Punch),
            11 => Some(Self::Cursor),
            13 => Some(Self::Audio),
            _ => None,
        }
    }

    /// `true` nos pacotes de pareamento, que nunca vão cifrados.
    pub fn is_pairing(self) -> bool {
        matches!(self, Self::PairInit | Self::PairReply)
    }

    /// `true` nos tipos que nunca vão cifrados: pareamento, sinalização e furo.
    pub fn is_always_clear(self) -> bool {
        matches!(
            self,
            Self::PairInit | Self::PairReply | Self::Signal | Self::Punch
        )
    }

    /// Cabeçalho de um pacote de controle (sem payload nem fragmentos).
    pub fn control(self, id: u32, timestamp_us: u32) -> PacketHeader {
        PacketHeader {
            kind: self,
            frame_id: id,
            frag_index: 0,
            frag_count: 0,
            flags: 0,
            fec_group: 0,
            payload_len: 0,
            timestamp_us,
            send_us: timestamp_us,
            seq: 0,
        }
    }
}

/// Prepara um pacote montado para a cifra: grava `seq`, liga
/// [`FLAG_ENCRYPTED`] e soma `tag_len` ao `payload_len`. Depois disso o
/// cabeçalho (os [`HEADER_LEN`] primeiros bytes) não muda mais: é o dado
/// autenticado da cifra, que cifra o resto e acrescenta a tag.
///
/// # Panics
/// Se `datagram.len() < HEADER_LEN` (erro de programação).
pub fn prepare_seal(datagram: &mut [u8], seq: u32, tag_len: usize) {
    datagram[12] |= FLAG_ENCRYPTED;
    let len = u16::from_be_bytes([datagram[14], datagram[15]]).saturating_add(tag_len as u16);
    datagram[14..16].copy_from_slice(&len.to_be_bytes());
    datagram[SEQ_AT..HEADER_LEN].copy_from_slice(&seq.to_be_bytes());
}

/// Grava `send_us` (relógio de quem envia, µs em 32 bits) num pacote já
/// montado. O emissor chama logo antes de mandar: os pacotes de um
/// frame são montados juntos, mas saem espalhados pelo pacing.
///
/// # Panics
/// Se `datagram.len() < HEADER_LEN` (erro de programação).
pub fn stamp_send_time(datagram: &mut [u8], send_us: u32) {
    datagram[SEND_US_AT..SEQ_AT].copy_from_slice(&send_us.to_be_bytes());
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
    /// Mensagem de sinalização malformada ou curta demais.
    BadSignal,
    /// Mensagem de cursor malformada.
    BadCursor,
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
            Self::BadSignal => write!(f, "mensagem de sinalização inválida"),
            Self::BadCursor => write!(f, "mensagem de cursor inválida"),
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
    /// Só em [`PacketKind::Parity`]: quantos fragmentos cada grupo de FEC
    /// cobre. 0 nos outros tipos.
    pub fec_group: u8,
    /// Tamanho do payload que vem logo após o cabeçalho.
    pub payload_len: u16,
    /// Momento da captura do frame em microssegundos (relógio do host,
    /// truncado em 32 bits — dá a volta a cada ~71 minutos). Usado para
    /// medir latência e ordenar a apresentação.
    pub timestamp_us: u32,
    /// Momento em que **este pacote** saiu, no relógio de quem enviou (mesmo
    /// formato de `timestamp_us`). `chegada − send_us` varia só com a rede:
    /// não inclui encode, espera do limite de fps nem pacing. Base do atraso
    /// de fila do bitrate adaptativo.
    pub send_us: u32,
    /// Número do pacote no seu sentido (host → cliente ou o contrário),
    /// nonce da cifra. 0 em pacotes em claro.
    pub seq: u32,
}

impl PacketHeader {
    /// `true` se o frame deste pacote é um keyframe.
    pub fn is_keyframe(&self) -> bool {
        self.flags & FLAG_KEYFRAME != 0
    }

    /// `true` se o payload vem cifrado ([`FLAG_ENCRYPTED`]).
    pub fn is_encrypted(&self) -> bool {
        self.flags & FLAG_ENCRYPTED != 0
    }

    /// Cabeçalho do pacote depois de decifrado: sem [`FLAG_ENCRYPTED`] e com
    /// o tamanho do payload em claro. Confira o payload com
    /// [`check_payload`](Self::check_payload) antes de usar.
    pub fn decrypted(&self, payload_len: usize) -> PacketHeader {
        PacketHeader {
            flags: self.flags & !FLAG_ENCRYPTED,
            payload_len: payload_len as u16,
            ..*self
        }
    }

    /// Confere o payload contra o tipo do pacote (tamanhos fixos, paridade
    /// coerente). [`decode`](Self::decode) faz isso sozinho nos pacotes em
    /// claro; nos cifrados, chame depois de decifrar.
    pub fn check_payload(&self, payload: &[u8]) -> Result<(), ProtoError> {
        let actual = payload.len();
        let esperado = match self.kind {
            PacketKind::Pong => Some(PONG_LEN),
            PacketKind::Feedback => Some(crate::feedback::FEEDBACK_LEN),
            _ => None,
        };
        if let Some(declared) = esperado.filter(|&e| e != actual) {
            return Err(ProtoError::LengthMismatch { declared, actual });
        }
        if self.kind == PacketKind::Parity
            && (self.frag_count == 0
                || self.fec_group == 0
                || self.frag_index >= crate::fec::group_count(self.frag_count, self.fec_group)
                || actual < crate::fec::PARITY_HEADER_LEN)
        {
            return Err(ProtoError::BadFragment {
                index: self.frag_index,
                count: self.frag_count,
            });
        }
        Ok(())
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
        b[13] = self.fec_group;
        b[14..16].copy_from_slice(&self.payload_len.to_be_bytes());
        b[16..20].copy_from_slice(&self.timestamp_us.to_be_bytes());
        b[SEND_US_AT..SEQ_AT].copy_from_slice(&self.send_us.to_be_bytes());
        b[SEQ_AT..HEADER_LEN].copy_from_slice(&self.seq.to_be_bytes());
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
            fec_group: datagram[13],
            payload_len: be16(14),
            timestamp_us: be32(16),
            send_us: be32(SEND_US_AT),
            seq: be32(SEQ_AT),
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
        let payload = &datagram[HEADER_LEN..];
        // Cifrado: o payload ainda tem a tag e não dá para conferir; quem
        // decifra chama `check_payload` depois.
        if !header.is_encrypted() {
            header.check_payload(payload)?;
        }
        Ok((header, payload))
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
            fec_group: 0,
            payload_len: 0,
            timestamp_us: 123_456,
            send_us: 130_000,
            seq: 77,
        }
    }

    #[test]
    fn pacote_cifrado_so_e_conferido_depois_de_decifrar() {
        let mut pkt = PacketKind::Pong
            .control(1, 2)
            .encode_with_payload(&[0; 4])
            .unwrap();
        prepare_seal(&mut pkt, 42, 16);
        pkt.extend_from_slice(&[0; 16]); // o lugar da tag
                                         // Pong com 20 bytes de payload: em claro seria recusado, cifrado passa.
        let (h, payload) = PacketHeader::decode(&pkt).unwrap();
        assert!(h.is_encrypted());
        assert_eq!((h.seq, payload.len()), (42, 20));
        let claro = h.decrypted(4);
        assert!(!claro.is_encrypted());
        assert!(claro.check_payload(&[0; 4]).is_ok());
        assert!(claro.check_payload(&[0; 3]).is_err());
    }

    #[test]
    fn hora_de_envio_gravada_no_pacote_pronto() {
        let mut pkt = exemplo().encode_with_payload(b"x").unwrap();
        stamp_send_time(&mut pkt, 0xFFFF_FFF0);
        let (h, payload) = PacketHeader::decode(&pkt).unwrap();
        assert_eq!((h.timestamp_us, h.send_us), (123_456, 0xFFFF_FFF0));
        assert_eq!(payload, b"x");
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
        assert_eq!(h.send_us, 130_000);
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
