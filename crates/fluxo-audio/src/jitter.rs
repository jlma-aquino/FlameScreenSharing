//! Buffer de jitter: [`JitterBuffer`].
//!
//! Os pacotes de áudio chegam com espaçamento irregular (e às vezes fora de
//! ordem ou faltando); a placa de som consome um quadro a cada 10 ms em
//! ritmo fixo. O buffer segura alguns quadros (o **alvo**) antes de começar
//! e entrega um por vez, em ordem. Quadro faltando com o seguinte já aqui:
//! [`Next::Lost`] com o seguinte, para o FEC do Opus refazer o perdido.
//! Buffer vazio: [`Next::Empty`] (a placa toca silêncio) e o alvo cresce um
//! quadro; um bom tempo sem faltar, ele diminui — a latência acompanha a rede.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Quadros que o buffer tenta manter (10 ms cada): começa em 3 (30 ms).
pub const START_TARGET: usize = 3;
pub const MIN_TARGET: usize = 2;
pub const MAX_TARGET: usize = 12;
/// Sem faltar por este tempo, o alvo cai um quadro.
const ESTAVEL: Duration = Duration::from_secs(10);
/// Passou do alvo por tanto, descarta os mais velhos (o atraso não acumula).
const FOLGA: usize = 6;

/// O que tocar agora.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// O quadro esperado.
    Packet(Vec<u8>),
    /// O esperado não veio; `following` é o seguinte, se já chegou (FEC).
    Lost { following: Option<Vec<u8>> },
    /// Nada para tocar (início, ou a rede atrasou): silêncio.
    Empty,
}

/// Contadores do buffer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JitterStats {
    pub played: u64,
    pub lost: u64,
    /// Vezes que o buffer esvaziou (a placa tocou silêncio no meio).
    pub underruns: u64,
    /// Pacotes que chegaram tarde demais (o quadro já tinha passado).
    pub late: u64,
    /// Quadros descartados para não acumular atraso.
    pub dropped: u64,
    /// Alvo atual, em quadros.
    pub target: usize,
}

#[derive(Debug)]
pub struct JitterBuffer {
    pacotes: BTreeMap<u32, Vec<u8>>,
    /// Próximo número a tocar (`None`: ainda enchendo).
    proximo: Option<u32>,
    alvo: usize,
    ultimo_vazio: Instant,
    stats: JitterStats,
}

impl Default for JitterBuffer {
    fn default() -> Self {
        Self {
            pacotes: BTreeMap::new(),
            proximo: None,
            alvo: START_TARGET,
            ultimo_vazio: Instant::now(),
            stats: JitterStats {
                target: START_TARGET,
                ..Default::default()
            },
        }
    }
}

impl JitterBuffer {
    /// Guarda o pacote de número `seq` (quadros contados pelo host).
    pub fn push(&mut self, seq: u32, pacote: Vec<u8>) {
        if self.proximo.is_some_and(|p| seq < p) {
            self.stats.late += 1;
            return;
        }
        self.pacotes.insert(seq, pacote);
        // Rede acelerou depois de um atraso: descarta o excesso pelo começo.
        while self.pacotes.len() > self.alvo + FOLGA {
            if let Some((&velho, _)) = self.pacotes.iter().next() {
                self.pacotes.remove(&velho);
                self.proximo = Some(velho + 1);
                self.stats.dropped += 1;
            }
        }
    }

    /// O próximo quadro a tocar (chame a cada 10 ms, no ritmo da placa).
    pub fn next(&mut self, agora: Instant) -> Next {
        let Some(p) = self.proximo else {
            // Enchendo: começa quando tiver o alvo.
            if self.pacotes.len() < self.alvo {
                return Next::Empty;
            }
            self.proximo = self.pacotes.keys().next().copied();
            return self.next(agora);
        };
        if let Some(pacote) = self.pacotes.remove(&p) {
            self.proximo = Some(p + 1);
            self.stats.played += 1;
            self.diminuir_se_estavel(agora);
            return Next::Packet(pacote);
        }
        if self.pacotes.is_empty() {
            // Esvaziou: volta a encher, com um alvo maior.
            self.proximo = None;
            self.stats.underruns += 1;
            self.alvo = (self.alvo + 1).min(MAX_TARGET);
            self.stats.target = self.alvo;
            self.ultimo_vazio = agora;
            return Next::Empty;
        }
        // Faltou só este: os seguintes já chegaram.
        self.proximo = Some(p + 1);
        self.stats.lost += 1;
        Next::Lost {
            following: self.pacotes.get(&(p + 1)).cloned(),
        }
    }

    fn diminuir_se_estavel(&mut self, agora: Instant) {
        if agora.duration_since(self.ultimo_vazio) >= ESTAVEL && self.alvo > MIN_TARGET {
            self.alvo -= 1;
            self.stats.target = self.alvo;
            self.ultimo_vazio = agora;
        }
    }

    /// Quadros esperando agora.
    pub fn len(&self) -> usize {
        self.pacotes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pacotes.is_empty()
    }

    pub fn stats(&self) -> JitterStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(n: u8) -> Vec<u8> {
        vec![n]
    }

    #[test]
    fn enche_ate_o_alvo_e_toca_em_ordem() {
        let mut j = JitterBuffer::default();
        let t = Instant::now();
        j.push(10, p(10));
        j.push(12, p(12)); // fora de ordem
        assert_eq!(j.next(t), Next::Empty, "ainda enchendo");
        j.push(11, p(11));
        assert_eq!(j.next(t), Next::Packet(p(10)));
        assert_eq!(j.next(t), Next::Packet(p(11)));
        assert_eq!(j.next(t), Next::Packet(p(12)));
    }

    #[test]
    fn perdido_vem_com_o_seguinte_para_o_fec() {
        let mut j = JitterBuffer::default();
        let t = Instant::now();
        for s in [1, 3, 4] {
            j.push(s, p(s as u8));
        }
        assert_eq!(j.next(t), Next::Packet(p(1)));
        assert_eq!(
            j.next(t),
            Next::Lost {
                following: Some(p(3))
            }
        );
        assert_eq!(j.next(t), Next::Packet(p(3)));
        // O 2 chegou atrasado: descartado.
        j.push(2, p(2));
        assert_eq!(j.stats().late, 1);
    }

    #[test]
    fn esvaziar_aumenta_o_alvo_e_estabilidade_diminui() {
        let mut j = JitterBuffer::default();
        let t = Instant::now();
        for s in 0..3 {
            j.push(s, p(s as u8));
        }
        for _ in 0..3 {
            assert!(matches!(j.next(t), Next::Packet(_)));
        }
        assert_eq!(j.next(t), Next::Empty);
        assert_eq!(j.stats().underruns, 1);
        assert_eq!(j.stats().target, START_TARGET + 1);
        // Volta a encher até o alvo novo (4) antes de tocar.
        for s in 3..6 {
            j.push(s, p(s as u8));
        }
        assert_eq!(j.next(t), Next::Empty);
        j.push(6, p(6));
        assert_eq!(j.next(t), Next::Packet(p(3)));
        // Muito tempo sem esvaziar: o alvo cai.
        assert_eq!(j.next(t + ESTAVEL), Next::Packet(p(4)));
        assert_eq!(j.stats().target, START_TARGET);
    }

    #[test]
    fn excesso_e_descartado_pelo_comeco() {
        let mut j = JitterBuffer::default();
        for s in 0..20 {
            j.push(s, p(s as u8));
        }
        assert_eq!(j.len(), START_TARGET + FOLGA);
        assert_eq!(j.stats().dropped, 20 - (START_TARGET + FOLGA) as u64);
        let primeiro = 20 - (START_TARGET + FOLGA) as u8;
        assert_eq!(j.next(Instant::now()), Next::Packet(p(primeiro)));
    }
}
