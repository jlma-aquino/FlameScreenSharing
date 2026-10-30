//! Fragmentação: quebra um frame codificado em vários pacotes.
//!
//! Um frame H.264/HEVC 1080p a ~20 Mbps e 60 fps tem em média ~40 KB
//! (keyframes podem passar de 200 KB). Um datagrama UDP seguro na internet
//! carrega ~1200 bytes, então cada frame vira dezenas de pacotes. Todos levam
//! o mesmo `frame_id` e um `frag_index` diferente.

use crate::packet::{PacketHeader, PacketKind, ProtoError, FLAG_KEYFRAME};
use crate::DEFAULT_MAX_PAYLOAD;

/// Quebra frames em pacotes prontos para enviar e numera os frames.
///
/// ```
/// use fluxo_proto::{Fragmenter, PacketHeader};
///
/// let mut frag = Fragmenter::new(1200);
/// let frame = vec![7u8; 3000];          // um frame "codificado" de 3000 bytes
/// let (id, pacotes) = frag.fragment(&frame, true, 0).unwrap();
///
/// assert_eq!(id, 0);
/// assert_eq!(pacotes.len(), 3);         // 1200 + 1200 + 600
/// let (h, _) = PacketHeader::decode(&pacotes[2]).unwrap();
/// assert_eq!((h.frag_index, h.frag_count), (2, 3));
/// ```
#[derive(Debug, Clone)]
pub struct Fragmenter {
    max_payload: usize,
    next_frame_id: u32,
}

impl Fragmenter {
    /// Cria um fragmentador com o tamanho máximo de payload por pacote.
    ///
    /// # Panics
    /// Se `max_payload` for 0 ou maior que 65535.
    pub fn new(max_payload: usize) -> Self {
        assert!(
            (1..=u16::MAX as usize).contains(&max_payload),
            "max_payload precisa estar entre 1 e 65535"
        );
        Self {
            max_payload,
            next_frame_id: 0,
        }
    }

    /// Começa a numeração a partir de `id` (útil para IDs aleatórios por sessão).
    pub fn with_first_frame_id(mut self, id: u32) -> Self {
        self.next_frame_id = id;
        self
    }

    /// Tamanho máximo de payload configurado.
    pub fn max_payload(&self) -> usize {
        self.max_payload
    }

    /// Fragmenta um frame, atribuindo o próximo `frame_id`.
    ///
    /// Devolve o `frame_id` usado e a lista de datagramas (cabeçalho +
    /// payload) na ordem de envio. Um frame vazio gera 1 pacote vazio, para o
    /// cliente ainda "ver" que o frame existiu.
    ///
    /// Erro se o frame precisar de mais de 65535 fragmentos
    /// (≈ 78 MB com payload de 1200 — não acontece com vídeo real).
    pub fn fragment(
        &mut self,
        frame: &[u8],
        keyframe: bool,
        timestamp_us: u32,
    ) -> Result<(u32, Vec<Vec<u8>>), ProtoError> {
        let frame_id = self.next_frame_id;

        let count = frame.len().div_ceil(self.max_payload).max(1);
        let frag_count = u16::try_from(count).map_err(|_| ProtoError::PayloadTooLarge(frame.len()))?;

        let mut packets = Vec::with_capacity(count);
        let mut chunks = frame.chunks(self.max_payload);
        for frag_index in 0..frag_count {
            let payload = chunks.next().unwrap_or(&[]);
            let header = PacketHeader {
                kind: PacketKind::Video,
                frame_id,
                frag_index,
                frag_count,
                flags: if keyframe { FLAG_KEYFRAME } else { 0 },
                payload_len: 0, // preenchido por encode_with_payload
                timestamp_us,
            };
            packets.push(header.encode_with_payload(payload)?);
        }

        // Só avança o ID se tudo deu certo.
        self.next_frame_id = frame_id.wrapping_add(1);
        Ok((frame_id, packets))
    }
}

impl Default for Fragmenter {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_PAYLOAD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::HEADER_LEN;

    #[test]
    fn divide_exato() {
        let mut f = Fragmenter::new(100);
        let (_, p) = f.fragment(&[1u8; 300], false, 0).unwrap();
        assert_eq!(p.len(), 3);
        assert!(p.iter().all(|x| x.len() == HEADER_LEN + 100));
    }

    #[test]
    fn frame_vazio_gera_um_pacote() {
        let mut f = Fragmenter::new(100);
        let (_, p) = f.fragment(&[], false, 0).unwrap();
        assert_eq!(p.len(), 1);
        let (h, payload) = PacketHeader::decode(&p[0]).unwrap();
        assert_eq!(h.frag_count, 1);
        assert!(payload.is_empty());
    }

    #[test]
    fn ids_crescem_e_dao_a_volta() {
        let mut f = Fragmenter::new(100).with_first_frame_id(u32::MAX);
        assert_eq!(f.fragment(b"a", false, 0).unwrap().0, u32::MAX);
        assert_eq!(f.fragment(b"b", false, 0).unwrap().0, 0);
        assert_eq!(f.fragment(b"c", false, 0).unwrap().0, 1);
    }

    #[test]
    fn flag_keyframe() {
        let mut f = Fragmenter::default();
        let (_, p) = f.fragment(b"x", true, 0).unwrap();
        assert!(PacketHeader::decode(&p[0]).unwrap().0.is_keyframe());
    }
}
