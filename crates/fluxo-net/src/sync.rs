//! Sincronização de relógio entre host e cliente, pelos Ping/Pong (como o NTP).
//!
//! ```text
//! nós                          outro lado
//!  t0 ── Ping (t0) ────────────►
//!                               t1 (relógio dele, na hora de responder)
//!  t3 ◄──────────── Pong (t0, t1)
//! ```
//!
//! `RTT = t3 − t0`. Supondo ida e volta iguais, o outro lado respondeu no
//! nosso instante `t0 + RTT/2`; então a **diferença entre os relógios** é
//! `t1 − (t0 + RTT/2)` (positiva: o relógio dele está adiantado).
//!
//! Medições com RTT alto são as mais tortas (um sentido demorou mais que o
//! outro numa fila). Como o NTP, guardamos as últimas e usamos a de **menor
//! RTT** — a que a rede menos atrapalhou. O erro máximo da estimativa é
//! metade do RTT dessa medição (~0,5 ms numa rede local).

use std::collections::VecDeque;

use crate::clock_diff_us;

/// Quantas medições recentes considerar (~8 s com pings a cada 500 ms).
const AMOSTRAS: usize = 16;
/// RTT acima disso é descartado (pong velho ou relógio que pulou).
const MAX_RTT_US: i32 = 2_000_000;

#[derive(Debug, Default)]
pub(crate) struct ClockSync {
    /// (RTT, diferença), ambos em µs.
    amostras: VecDeque<(i32, i32)>,
}

impl ClockSync {
    /// Registra uma troca Ping/Pong: `t0` = nosso relógio ao mandar o ping,
    /// `t1` = relógio do outro lado ao responder, `t3` = nosso relógio ao
    /// receber o pong (todos de [`crate::clock_us`]).
    pub fn add(&mut self, t0: u32, t1: u32, t3: u32) {
        let rtt = clock_diff_us(t3, t0);
        if !(0..=MAX_RTT_US).contains(&rtt) {
            return;
        }
        let diferenca = clock_diff_us(t1, t0).wrapping_sub(rtt / 2);
        if self.amostras.len() == AMOSTRAS {
            self.amostras.pop_front();
        }
        self.amostras.push_back((rtt, diferenca));
    }

    /// Relógio do outro lado menos o nosso, em µs (`None` sem medições).
    pub fn offset_us(&self) -> Option<i32> {
        self.amostras.iter().min_by_key(|&&(rtt, _)| rtt).map(|&(_, d)| d)
    }

    /// RTT da medição usada: o erro máximo da diferença é metade disso.
    pub fn best_rtt_us(&self) -> Option<i32> {
        self.amostras.iter().map(|&(rtt, _)| rtt).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mede_a_diferenca_com_ida_e_volta_iguais() {
        let mut s = ClockSync::default();
        // Relógio do outro lado 700 ms atrasado; 1 ms para cada lado.
        let t0 = 10_000_000u32;
        let t1 = t0 + 1_000 - 700_000;
        let t3 = t0 + 2_000;
        s.add(t0, t1, t3);
        assert_eq!(s.offset_us(), Some(-700_000));
        assert_eq!(s.best_rtt_us(), Some(2_000));
    }

    #[test]
    fn usa_a_medicao_de_menor_rtt() {
        let mut s = ClockSync::default();
        let base = 50_000_000u32;
        // Ida de 30 ms (fila), volta de 1 ms: medição torta em 14,5 ms.
        s.add(base, base + 30_000 + 5_000, base + 31_000);
        // Ida e volta de 1 ms: medição boa.
        s.add(base + 500_000, base + 501_000 + 5_000, base + 502_000);
        assert_eq!(s.offset_us(), Some(5_000));
    }

    #[test]
    fn funciona_quando_o_relogio_de_32_bits_da_a_volta() {
        let mut s = ClockSync::default();
        let t0 = u32::MAX - 500; // pouco antes da volta
        let t1 = t0.wrapping_add(1_000).wrapping_add(250_000);
        let t3 = t0.wrapping_add(2_000);
        s.add(t0, t1, t3);
        assert_eq!(s.offset_us(), Some(250_000));
    }

    #[test]
    fn descarta_rtt_absurdo_e_guarda_so_as_recentes() {
        let mut s = ClockSync::default();
        s.add(100, 0, 50); // RTT negativo
        s.add(0, 0, 3_000_000); // RTT de 3 s
        assert_eq!(s.offset_us(), None);
        for i in 0..40u32 {
            s.add(i * 1_000_000, i * 1_000_000 + 1_000 + i, i * 1_000_000 + 2_000);
        }
        assert_eq!(s.amostras.len(), AMOSTRAS);
    }
}
