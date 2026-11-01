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
//! 6. **FEC:** pacotes de paridade ([`crate::fec`]) ficam guardados por
//!    grupo. Quando um grupo tem exatamente um fragmento faltando e a sua
//!    paridade chegou, o fragmento é reconstruído na hora. Paridade de um
//!    frame que já saiu completo é simplesmente ignorada (não é "atraso").

use std::collections::HashMap;

use crate::fec;
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
    /// Primeiro frame de uma onda de intra-refresh ([`crate::FLAG_REFRESH_START`]).
    pub refresh_start: bool,
    /// Último frame de uma onda de intra-refresh ([`crate::FLAG_REFRESH_END`]).
    pub refresh_end: bool,
    /// Codec do frame (bits 4–5 das flags); `None`: valor desconhecido.
    pub codec: Option<crate::VideoCodec>,
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
    /// Fragmentos perdidos que o FEC reconstruiu.
    pub fec_recovered: u64,
    /// Grupos de FEC que **não** se recuperaram, nos frames abandonados, por
    /// quantos pacotes do grupo (dados + paridade) faltaram: 2, ou 3 ou mais.
    /// O XOR só salva 1 perda por grupo; um código com 2 paridades por grupo
    /// (Reed-Solomon) salvaria os de 2. Mede se as perdas vêm em rajada.
    pub fec_groups_failed_2: u64,
    pub fec_groups_failed_3plus: u64,
    /// Frames abandonados sem nenhuma paridade recebida (grupo desconhecido:
    /// o frame inteiro, ou quase, sumiu — rajada longa, ou FEC desligado).
    pub frames_abandoned_without_parity: u64,
}

/// Frame ainda incompleto.
#[derive(Debug)]
struct PartialFrame {
    fragments: Vec<Option<Vec<u8>>>,
    /// Paridades recebidas, por grupo. Vazio enquanto nenhuma chegou.
    parity: Vec<Option<Vec<u8>>>,
    /// Fragmentos por grupo de FEC (0 = nenhuma paridade vista ainda).
    fec_group: u8,
    received: u16,
    total_bytes: usize,
    flags: u8,
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
    /// Pacotes de vídeo e de paridade são processados; os demais tipos são
    /// ignorados (devolve `None`).
    pub fn push(&mut self, header: &PacketHeader, payload: &[u8]) -> Option<CompletedFrame> {
        let paridade = match header.kind {
            PacketKind::Video => false,
            PacketKind::Parity => true,
            _ => return None,
        };
        let id = header.frame_id;

        // Regra 3: frame já entregue ou pulado.
        if let Some(last) = self.last_delivered {
            if !is_newer(id, last) {
                // A paridade do último grupo costuma chegar depois de o frame
                // já ter saído completo: sobra normal, não atraso da rede.
                if !paridade {
                    self.stats.late_packets += 1;
                }
                return None;
            }
        }

        let grupos = fec::group_count(header.frag_count, header.fec_group);
        let invalido = header.frag_count == 0
            || header.frag_count > self.config.max_fragments_per_frame
            || match paridade {
                false => header.frag_index >= header.frag_count,
                true => header.fec_group == 0 || header.frag_index >= grupos,
            };
        if invalido {
            self.stats.invalid_packets += 1;
            return None;
        }

        let partial = self.pending.entry(id).or_insert_with(|| PartialFrame {
            fragments: vec![None; header.frag_count as usize],
            parity: Vec::new(),
            fec_group: 0,
            received: 0,
            total_bytes: 0,
            flags: header.flags,
            timestamp_us: header.timestamp_us,
        });

        if partial.fragments.len() != header.frag_count as usize {
            self.stats.invalid_packets += 1;
            return None;
        }

        // Grupo de FEC afetado por este pacote, para tentar reconstruir.
        let grupo = if paridade {
            if partial.fec_group == 0 {
                partial.fec_group = header.fec_group;
                partial.parity = vec![None; grupos as usize];
            } else if partial.fec_group != header.fec_group {
                self.stats.invalid_packets += 1;
                return None;
            }
            let slot = &mut partial.parity[header.frag_index as usize];
            if slot.is_some() {
                self.stats.duplicate_packets += 1;
                return None;
            }
            *slot = Some(payload.to_vec());
            Some(header.frag_index)
        } else {
            let slot = &mut partial.fragments[header.frag_index as usize];
            if slot.is_some() {
                self.stats.duplicate_packets += 1;
                return None;
            }
            *slot = Some(payload.to_vec());
            partial.received += 1;
            partial.total_bytes += payload.len();
            (partial.fec_group > 0).then(|| header.frag_index / partial.fec_group as u16)
        };
        self.stats.packets_received += 1;

        if let Some(g) = grupo {
            if Self::try_recover(partial, g) {
                self.stats.fec_recovered += 1;
            }
        }

        if partial.received as usize == partial.fragments.len() {
            return Some(self.complete(id));
        }

        // Regra 5: limita a memória.
        if self.pending.len() > self.config.max_pending_frames {
            self.evict_oldest();
        }
        None
    }

    /// Se o grupo `g` tem exatamente um fragmento faltando e a paridade dele
    /// chegou, reconstrói o fragmento. Devolve `true` se reconstruiu.
    fn try_recover(partial: &mut PartialFrame, g: u16) -> bool {
        let Some(Some(par)) = partial.parity.get(g as usize) else {
            return false;
        };
        let faixa = fec::group_range(g, partial.fec_group, partial.fragments.len() as u16);
        let mut faltando = faixa.clone().filter(|&i| partial.fragments[i as usize].is_none());
        let (Some(falta), None) = (faltando.next(), faltando.next()) else {
            return false; // nada faltando, ou 2+ (o XOR não resolve)
        };
        let outros: Vec<&[u8]> = faixa
            .filter_map(|i| partial.fragments[i as usize].as_deref())
            .collect();
        let Some(recuperado) = fec::recover(par, &outros) else {
            return false; // paridade incoerente: ignora
        };
        partial.total_bytes += recuperado.len();
        partial.received += 1;
        partial.fragments[falta as usize] = Some(recuperado);
        true
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
        let abandonados: Vec<u32> = self
            .pending
            .keys()
            .copied()
            .filter(|&o| !is_newer(o, id))
            .collect();
        for o in abandonados {
            if let Some(p) = self.pending.remove(&o) {
                self.contar_abandono(&p);
            }
        }

        self.last_delivered = Some(id);
        self.stats.frames_completed += 1;

        CompletedFrame {
            frame_id: id,
            keyframe: partial.flags & crate::FLAG_KEYFRAME != 0,
            timestamp_us: partial.timestamp_us,
            data,
            preceded_by_loss: gap > 0,
            refresh_start: partial.flags & crate::FLAG_REFRESH_START != 0,
            refresh_end: partial.flags & crate::FLAG_REFRESH_END != 0,
            codec: crate::VideoCodec::from_flags(partial.flags),
        }
    }

    fn evict_oldest(&mut self) {
        let oldest = self
            .pending
            .keys()
            .copied()
            .reduce(|a, b| if is_newer(a, b) { b } else { a });
        if let Some(id) = oldest {
            if let Some(p) = self.pending.remove(&id) {
                self.contar_abandono(&p);
            }
            self.stats.frames_evicted += 1;
        }
    }

    /// Conta, num frame que não vai mais se completar, os grupos de FEC que
    /// falharam e quantos pacotes faltaram em cada um.
    fn contar_abandono(&mut self, p: &PartialFrame) {
        if p.fec_group == 0 {
            self.stats.frames_abandoned_without_parity += 1;
            return;
        }
        let total = p.fragments.len() as u16;
        for g in 0..crate::fec::group_count(total, p.fec_group) {
            let dados = crate::fec::group_range(g, p.fec_group, total)
                .filter(|&i| p.fragments[i as usize].is_none())
                .count();
            if dados == 0 {
                continue;
            }
            let paridade_faltou = !matches!(p.parity.get(g as usize), Some(Some(_)));
            match dados + paridade_faltou as usize {
                0 | 1 => {}
                2 => self.stats.fec_groups_failed_2 += 1,
                _ => self.stats.fec_groups_failed_3plus += 1,
            }
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
        assert!(!out[0].refresh_start && !out[0].refresh_end);
        assert_eq!(r.pending_frames(), 0);
    }

    #[test]
    fn flags_de_intra_refresh_chegam_ao_frame() {
        let mut f = Fragmenter::new(10);
        let mut r = Reassembler::default();
        let (_, a) = f
            .fragment_with_flags(&[1u8; 25], crate::FLAG_REFRESH_START, 0)
            .unwrap();
        let (_, b) = f
            .fragment_with_flags(&[2u8; 25], crate::FLAG_REFRESH_END, 0)
            .unwrap();
        let out = feed(&mut r, &decode_all(&[a, b].concat()));
        assert_eq!(out.len(), 2);
        assert!(out[0].refresh_start && !out[0].refresh_end && !out[0].keyframe);
        assert!(!out[1].refresh_start && out[1].refresh_end);
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

    // ---------- FEC ----------

    /// Frame de `n` fragmentos de 100 bytes (o último menor), com FEC de grupo `k`.
    fn frame_com_fec(n: usize, k: u8) -> (Vec<u8>, Vec<Vec<u8>>) {
        let dados: Vec<u8> = (0..n * 100 - 37).map(|i| (i * 7 + 3) as u8).collect();
        let mut f = Fragmenter::new(100).with_fec_group(k);
        let (_, pacotes) = f.fragment(&dados, false, 0).unwrap();
        (dados, pacotes)
    }

    fn entregar(pacotes: impl IntoIterator<Item = Vec<u8>>) -> (Option<CompletedFrame>, ReassemblerStats) {
        let mut r = Reassembler::default();
        let mut saiu = None;
        for p in pacotes {
            let (h, pl) = PacketHeader::decode(&p).unwrap();
            if let Some(f) = r.push(&h, pl) {
                saiu = Some(f);
            }
        }
        (saiu, r.stats())
    }

    fn eh_paridade(p: &[u8]) -> bool {
        PacketHeader::decode(p).unwrap().0.kind == PacketKind::Parity
    }

    #[test]
    fn fec_reconstroi_qualquer_fragmento_perdido() {
        let (dados, pacotes) = frame_com_fec(25, 10);
        let indices_dados: Vec<usize> = (0..pacotes.len())
            .filter(|&i| !eh_paridade(&pacotes[i]))
            .collect();
        assert_eq!(indices_dados.len(), 25);
        for &perdido in &indices_dados {
            let restantes = pacotes
                .iter()
                .enumerate()
                .filter(|&(i, _)| i != perdido)
                .map(|(_, p)| p.clone());
            let (frame, s) = entregar(restantes);
            assert_eq!(
                frame.expect("frame reconstruído").data,
                dados,
                "perdeu o pacote {perdido}"
            );
            assert_eq!(s.fec_recovered, 1);
        }
    }

    #[test]
    fn fec_nao_resolve_duas_perdas_no_mesmo_grupo() {
        let (_, pacotes) = frame_com_fec(25, 10);
        // Pacotes 0 e 1 são dados do grupo 0.
        let (frame, s) = entregar(pacotes.into_iter().skip(2));
        assert!(frame.is_none());
        assert_eq!(s.fec_recovered, 0);
    }

    #[test]
    fn fec_resolve_uma_perda_em_cada_grupo() {
        let (dados, pacotes) = frame_com_fec(25, 10);
        // Ordem de envio: d0..d9 P0 d10..d19 P1 d20..d24 P2 → posições 3, 14 e 24 são dados.
        let (frame, s) = entregar(
            pacotes
                .into_iter()
                .enumerate()
                .filter(|&(i, _)| ![3, 14, 24].contains(&i))
                .map(|(_, p)| p),
        );
        assert_eq!(frame.unwrap().data, dados);
        assert_eq!(s.fec_recovered, 3);
    }

    #[test]
    fn paridade_que_chega_antes_dos_dados_tambem_serve() {
        let (dados, mut pacotes) = frame_com_fec(10, 10);
        pacotes.reverse(); // paridade primeiro
        pacotes.remove(5); // perde um dado
        let (frame, s) = entregar(pacotes);
        assert_eq!(frame.unwrap().data, dados);
        assert_eq!(s.fec_recovered, 1);
    }

    #[test]
    fn paridade_que_sobra_nao_conta_como_atraso() {
        let (dados, pacotes) = frame_com_fec(25, 10);
        let (frame, s) = entregar(pacotes);
        assert_eq!(frame.unwrap().data, dados);
        assert_eq!((s.fec_recovered, s.late_packets, s.invalid_packets), (0, 0, 0));
    }

    #[test]
    fn grupos_que_falharam_sao_contados_por_tamanho_da_rajada() {
        let mut f = Fragmenter::new(100).with_fec_group(10);
        let mut r = Reassembler::default();
        let mut entregar = |pacotes: Vec<Vec<u8>>| {
            for p in pacotes {
                let (h, pl) = PacketHeader::decode(&p).unwrap();
                r.push(&h, pl);
            }
        };
        // Ordem de envio com 25 fragmentos: d0..d9 P0 | d10..d19 P1 | d20..d24 P2
        // (posições 0–10, 11–21, 22–27). Perde: grupo 0 com 2 dados; grupo 1
        // com 3 dados; grupo 2 com 1 dado e a própria paridade.
        let (_, a) = f.fragment(&[1u8; 2_500], false, 0).unwrap();
        let perdidos = [0, 1, 11, 12, 13, 22, 27];
        entregar(
            a.into_iter()
                .enumerate()
                .filter(|(i, _)| !perdidos.contains(i))
                .map(|(_, p)| p)
                .collect(),
        );
        // Outro frame sem paridade nenhuma (e com um dado a menos).
        let (_, b) = f.fragment(&[2u8; 2_500], false, 0).unwrap();
        entregar(b.into_iter().skip(1).filter(|p| !eh_paridade(p)).collect());
        // Um frame inteiro depois: os dois anteriores são abandonados.
        let (_, c) = f.fragment(&[3u8; 300], false, 0).unwrap();
        entregar(c);

        let s = r.stats();
        assert_eq!((s.fec_groups_failed_2, s.fec_groups_failed_3plus), (2, 1));
        assert_eq!(s.frames_abandoned_without_parity, 1);
        assert_eq!(s.frames_lost, 2);
    }

    #[test]
    fn perder_so_a_paridade_nao_atrapalha() {
        let (dados, pacotes) = frame_com_fec(25, 10);
        let (frame, _) = entregar(pacotes.into_iter().filter(|p| !eh_paridade(p)));
        assert_eq!(frame.unwrap().data, dados);
    }
}
