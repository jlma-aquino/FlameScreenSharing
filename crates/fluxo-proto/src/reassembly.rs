//! Remontagem: junta os fragmentos recebidos de volta em frames.
//!
//! ## Regras (ver também `docs/PROTOCOLO.md`)
//!
//! 1. Fragmentos podem chegar fora de ordem e duplicados — ambos são tratados.
//! 2. Um frame é entregue assim que **todos** os seus fragmentos chegam.
//! 3. Frames são entregues sempre em ordem crescente de `frame_id`. Depois
//!    que o frame N é entregue, qualquer pacote de frame ≤ N é "atrasado" e
//!    descartado: em streaming ao vivo, mostrar um frame velho é pior que
//!    pulá-lo.
//! 4. Quando o frame N é entregue, frames incompletos mais antigos que N são
//!    abandonados. Se houve buraco (N não é o sucessor do último entregue),
//!    o frame sai marcado com [`CompletedFrame::preceded_by_loss`] — o
//!    decodificador provavelmente vai precisar de um keyframe novo.
//! 5. No máximo `max_pending_frames` frames incompletos ficam em memória;
//!    passando disso, o mais antigo é descartado. Isso limita memória mesmo
//!    sob ataque ou perda pesada.

use std::collections::HashMap;

use crate::packet::{PacketHeader, PacketKind};
use crate::seq::is_newer;

/// Parâmetros do [`Reassembler`].
#[derive(Debug, Clone, Copy)]
pub struct ReassemblerConfig {
    /// Quantos frames incompletos manter ao mesmo tempo.
    /// A 60 fps, 8 frames = ~133 ms de tolerância a atraso/reordenação.
    pub max_pending_frames: usize,
    /// Máximo de fragmentos aceitos por frame (proteção de memória).
    /// 4096 × 1200 B ≈ 4,9 MB, muito acima de qualquer keyframe 1080p/4K.
    pub max_fragments_per_frame: u16,
}

impl Default for ReassemblerConfig {
    fn default() -> Self {
        Self {
            max_pending_frames: 8,
            max_fragments_per_frame: 4096,
        }
    }
}

/// Um frame completo, pronto para ir ao decodificador.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedFrame {
    /// ID do frame.
    pub frame_id: u32,
    /// `true` se é keyframe (decodificável sozinho).
    pub keyframe: bool,
    /// Timestamp de captura no host (µs, 32 bits).
    pub timestamp_us: u32,
    /// Bytes do frame codificado, já concatenados na ordem certa.
    pub data: Vec<u8>,
    /// `true` se algum frame entre o último entregue e este se perdeu.
    pub preceded_by_loss: bool,
}

/// Contadores para diagnóstico (mostrar na tela de estatísticas, logs etc.).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReassemblerStats {
    /// Pacotes de vídeo aceitos para processamento.
    pub packets_received: u64,
    /// Fragmentos que já tínhamos recebido (rede duplicou).
    pub duplicate_packets: u64,
    /// Pacotes de frames que já foram entregues ou pulados.
    pub late_packets: u64,
    /// Pacotes incoerentes (frag_count mudou, frame grande demais...).
    pub invalid_packets: u64,
    /// Frames entregues completos.
    pub frames_completed: u64,
    /// Frames que nunca foram entregues (buracos na sequência).
    pub frames_lost: u64,
    /// Frames incompletos expulsos por falta de espaço na fila.
    pub frames_evicted: u64,
}

/// Frame ainda incompleto.
#[derive(Debug)]
struct PartialFrame {
    fragments: Vec<Option<Vec<u8>>>,
    received: u16,
    total_bytes: usize,
    keyframe: bool,
    timestamp_us: u32,
}

/// Remonta frames a partir dos pacotes recebidos.
///
/// ```
/// use fluxo_proto::{Fragmenter, PacketHeader, Reassembler};
///
/// let mut frag = Fragmenter::new(4);
/// let mut reasm = Reassembler::default();
///
/// let (_, mut pacotes) = frag.fragment(b"abcdefghij", false, 0).unwrap();
/// pacotes.reverse(); // chegam fora de ordem
///
/// let mut entregue = None;
/// for p in &pacotes {
///     let (h, payload) = PacketHeader::decode(p).unwrap();
///     if let Some(f) = reasm.push(&h, payload) {
///         entregue = Some(f);
///     }
/// }
/// assert_eq!(entregue.unwrap().data, b"abcdefghij");
/// ```
#[derive(Debug, Default)]
pub struct Reassembler {
    config: ReassemblerConfig,
    pending: HashMap<u32, PartialFrame>,
    last_delivered: Option<u32>,
    stats: ReassemblerStats,
}

impl Reassembler {
    /// Cria um remontador com a configuração dada.
    pub fn new(config: ReassemblerConfig) -> Self {
        Self {
            config,
            ..Default::default()
        }
    }

    /// Estatísticas acumuladas desde a criação.
    pub fn stats(&self) -> ReassemblerStats {
        self.stats
    }

    /// ID do último frame entregue, se houver.
    pub fn last_delivered(&self) -> Option<u32> {
        self.last_delivered
    }

    /// Quantos frames incompletos estão em memória agora.
    pub fn pending_frames(&self) -> usize {
        self.pending.len()
    }

    /// Processa um pacote já decodificado. Devolve `Some(frame)` quando este
    /// pacote completou um frame.
    ///
    /// Pacotes que não são de vídeo são ignorados (devolve `None`).
    pub fn push(&mut self, header: &PacketHeader, payload: &[u8]) -> Option<CompletedFrame> {
        if header.kind != PacketKind::Video {
            return None;
        }
        let id = header.frame_id;

        // Regra 3: frame já entregue ou pulado.
        if let Some(last) = self.last_delivered {
            if !is_newer(id, last) {
                self.stats.late_packets += 1;
                return None;
            }
        }

        if header.frag_count == 0
            || header.frag_index >= header.frag_count
            || header.frag_count > self.config.max_fragments_per_frame
        {
            self.stats.invalid_packets += 1;
            return None;
        }

        let partial = self.pending.entry(id).or_insert_with(|| PartialFrame {
            fragments: vec![None; header.frag_count as usize],
            received: 0,
            total_bytes: 0,
            keyframe: header.is_keyframe(),
            timestamp_us: header.timestamp_us,
        });

        if partial.fragments.len() != header.frag_count as usize {
            self.stats.invalid_packets += 1;
            return None;
        }

        let slot = &mut partial.fragments[header.frag_index as usize];
        if slot.is_some() {
            self.stats.duplicate_packets += 1;
            return None;
        }
        *slot = Some(payload.to_vec());
        partial.received += 1;
        partial.total_bytes += payload.len();
        self.stats.packets_received += 1;

        if partial.received as usize == partial.fragments.len() {
            return Some(self.complete(id));
        }

        // Regra 5: limita a memória.
        if self.pending.len() > self.config.max_pending_frames {
            self.evict_oldest();
        }
        None
    }

    /// Monta o frame `id` (que deve estar completo) e limpa os mais antigos.
    fn complete(&mut self, id: u32) -> CompletedFrame {
        let partial = self.pending.remove(&id).expect("frame completo deve existir");
        let mut data = Vec::with_capacity(partial.total_bytes);
        for frag in partial.fragments {
            data.extend_from_slice(&frag.expect("todos os fragmentos presentes"));
        }

        // Regra 4: buracos na sequência. Antes da primeira entrega, a
        // referência é o frame pendente mais antigo que já vimos.
        let gap = match self.last_delivered {
            Some(last) => id.wrapping_sub(last).wrapping_sub(1),
            None => self
                .pending
                .keys()
                .filter(|&&other| is_newer(id, other))
                .map(|&other| id.wrapping_sub(other))
                .max()
                .unwrap_or(0),
        };
        self.stats.frames_lost += gap as u64;
        self.pending.retain(|&other, _| is_newer(other, id));

        self.last_delivered = Some(id);
        self.stats.frames_completed += 1;

        CompletedFrame {
            frame_id: id,
            keyframe: partial.keyframe,
            timestamp_us: partial.timestamp_us,
            data,
            preceded_by_loss: gap > 0,
        }
    }

    fn evict_oldest(&mut self) {
        let oldest = self
            .pending
            .keys()
            .copied()
            .reduce(|a, b| if is_newer(a, b) { b } else { a });
        if let Some(id) = oldest {
            self.pending.remove(&id);
            self.stats.frames_evicted += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Fragmenter;

    fn decode_all(pkts: &[Vec<u8>]) -> Vec<(PacketHeader, Vec<u8>)> {
        pkts.iter()
            .map(|p| {
                let (h, pl) = PacketHeader::decode(p).unwrap();
                (h, pl.to_vec())
            })
            .collect()
    }

    fn feed(r: &mut Reassembler, pkts: &[(PacketHeader, Vec<u8>)]) -> Vec<CompletedFrame> {
        pkts.iter().filter_map(|(h, p)| r.push(h, p)).collect()
    }

    #[test]
    fn frame_em_ordem() {
        let mut f = Fragmenter::new(10);
        let mut r = Reassembler::default();
        let data: Vec<u8> = (0..95).collect();
        let (_, p) = f.fragment(&data, true, 42).unwrap();
        let out = feed(&mut r, &decode_all(&p));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].data, data);
        assert!(out[0].keyframe);
        assert_eq!(out[0].timestamp_us, 42);
        assert!(!out[0].preceded_by_loss);
        assert_eq!(r.pending_frames(), 0);
    }

    #[test]
    fn duplicados_sao_ignorados() {
        let mut f = Fragmenter::new(10);
        let mut r = Reassembler::default();
        let (_, p) = f.fragment(&[1u8; 30], false, 0).unwrap();
        let d = decode_all(&p);
        r.push(&d[0].0, &d[0].1);
        r.push(&d[0].0, &d[0].1);
        r.push(&d[1].0, &d[1].1);
        assert!(r.push(&d[2].0, &d[2].1).is_some());
        assert_eq!(r.stats().duplicate_packets, 1);
    }

    #[test]
    fn frames_intercalados() {
        let mut f = Fragmenter::new(5);
        let mut r = Reassembler::default();
        let a = decode_all(&f.fragment(b"AAAAAAAAAA", false, 0).unwrap().1);
        let b = decode_all(&f.fragment(b"BBBBBBBBBB", false, 0).unwrap().1);
        // a0, b0, a1, b1
        assert!(r.push(&a[0].0, &a[0].1).is_none());
        assert!(r.push(&b[0].0, &b[0].1).is_none());
        let fa = r.push(&a[1].0, &a[1].1).unwrap();
        let fb = r.push(&b[1].0, &b[1].1).unwrap();
        assert_eq!(fa.data, b"AAAAAAAAAA");
        assert_eq!(fb.data, b"BBBBBBBBBB");
        assert!(!fb.preceded_by_loss);
    }

    #[test]
    fn frame_incompleto_e_abandonado_quando_o_proximo_completa() {
        let mut f = Fragmenter::new(5);
        let mut r = Reassembler::default();
        let a = decode_all(&f.fragment(b"AAAAAAAAAA", false, 0).unwrap().1);
        let b = decode_all(&f.fragment(b"BBBBB", false, 0).unwrap().1);

        r.push(&a[0].0, &a[0].1); // a1 se perdeu
        let fb = r.push(&b[0].0, &b[0].1).unwrap();
        assert!(fb.preceded_by_loss);
        assert_eq!(r.stats().frames_lost, 1);
        assert_eq!(r.pending_frames(), 0);

        // a1 chega atrasado: descartado.
        assert!(r.push(&a[1].0, &a[1].1).is_none());
        assert_eq!(r.stats().late_packets, 1);
    }

    #[test]
    fn limite_de_frames_pendentes() {
        let mut f = Fragmenter::new(5);
        let mut r = Reassembler::new(ReassemblerConfig {
            max_pending_frames: 3,
            ..Default::default()
        });
        // 5 frames de 2 fragmentos, só o primeiro fragmento de cada chega.
        for _ in 0..5 {
            let p = decode_all(&f.fragment(b"0123456789", false, 0).unwrap().1);
            r.push(&p[0].0, &p[0].1);
        }
        assert_eq!(r.pending_frames(), 3);
        assert_eq!(r.stats().frames_evicted, 2);
    }

    #[test]
    fn rejeita_frame_com_fragmentos_demais() {
        let mut r = Reassembler::new(ReassemblerConfig {
            max_fragments_per_frame: 2,
            ..Default::default()
        });
        let mut f = Fragmenter::new(1);
        let p = decode_all(&f.fragment(b"abc", false, 0).unwrap().1);
        assert!(r.push(&p[0].0, &p[0].1).is_none());
        assert_eq!(r.stats().invalid_packets, 1);
    }

    #[test]
    fn funciona_atravessando_o_wrap_around() {
        let mut f = Fragmenter::new(4).with_first_frame_id(u32::MAX - 1);
        let mut r = Reassembler::default();
        let mut ids = vec![];
        for _ in 0..4 {
            let p = decode_all(&f.fragment(b"12345678", false, 0).unwrap().1);
            ids.extend(feed(&mut r, &p).into_iter().map(|c| c.frame_id));
        }
        assert_eq!(ids, vec![u32::MAX - 1, u32::MAX, 0, 1]);
        assert_eq!(r.stats().frames_lost, 0);
    }
}
