//! Estimativa de RTT (tempo de ida e volta) a partir dos Ping/Pong.

/// Resumo das medições de RTT.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RttStats {
    /// Última medição, em ms.
    pub last_ms: f64,
    /// Média suavizada (peso 1/8 para cada amostra nova, como no TCP —
    /// RFC 6298), em ms. É o valor a usar em decisões.
    pub smoothed_ms: f64,
    /// Menor RTT já visto, em ms: aproxima o atraso da rede sem filas.
    pub min_ms: f64,
    /// Quantas medições válidas houve. 0 = ainda nenhuma (os outros campos são 0).
    pub samples: u64,
}

/// Acumula medições de RTT.
#[derive(Debug, Default)]
pub(crate) struct RttEstimator {
    stats: RttStats,
}

/// Acima disso a amostra é tratada como lixo (pong de sessão antiga, relógio
/// que pulou).
const MAX_PLAUSIBLE_US: i32 = 10_000_000;

impl RttEstimator {
    /// Registra uma amostra em µs. Ignora valores negativos ou absurdos.
    pub fn add(&mut self, rtt_us: i32) {
        if !(0..=MAX_PLAUSIBLE_US).contains(&rtt_us) {
            return;
        }
        let ms = rtt_us as f64 / 1000.0;
        let s = &mut self.stats;
        if s.samples == 0 {
            s.smoothed_ms = ms;
            s.min_ms = ms;
        } else {
            s.smoothed_ms += (ms - s.smoothed_ms) / 8.0;
            s.min_ms = s.min_ms.min(ms);
        }
        s.last_ms = ms;
        s.samples += 1;
    }

    pub fn stats(&self) -> RttStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primeira_amostra_define_tudo() {
        let mut r = RttEstimator::default();
        r.add(2_000);
        let s = r.stats();
        assert_eq!(
            (s.last_ms, s.smoothed_ms, s.min_ms, s.samples),
            (2.0, 2.0, 2.0, 1)
        );
    }

    #[test]
    fn media_suavizada_anda_um_oitavo() {
        let mut r = RttEstimator::default();
        r.add(8_000);
        r.add(16_000);
        let s = r.stats();
        assert_eq!(s.smoothed_ms, 9.0);
        assert_eq!(s.min_ms, 8.0);
        assert_eq!(s.last_ms, 16.0);
    }

    #[test]
    fn amostras_absurdas_sao_ignoradas() {
        let mut r = RttEstimator::default();
        r.add(-5);
        r.add(MAX_PLAUSIBLE_US + 1);
        assert_eq!(r.stats().samples, 0);
    }
}
