//! Pacing por *token bucket*: limita a taxa média de envio e o tamanho das
//! rajadas.
//!
//! O balde ganha `rate` bytes por segundo, até no máximo `burst` bytes. Um
//! pacote pode sair quando o saldo não está negativo; ao sair, desconta seu
//! tamanho (o saldo pode ficar negativo — a "dívida" atrasa o próximo).
//! Assim pacotes maiores que o balde nunca travam, e a taxa média é exata.
//!
//! A lógica recebe o instante atual como parâmetro, para ser testada sem
//! relógio de verdade.

use std::time::{Duration, Instant};

pub(crate) struct Pacer {
    /// Bytes por segundo; 0 = sem limite.
    rate: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl Pacer {
    /// Começa com o balde cheio: o primeiro `burst` sai imediatamente.
    pub fn new(rate_bps: u64, burst_bytes: usize, now: Instant) -> Self {
        Self {
            rate: rate_bps as f64 / 8.0,
            burst: burst_bytes as f64,
            tokens: burst_bytes as f64,
            last: now,
        }
    }

    /// Muda a taxa (ex.: controle de bitrate). Vale a partir de agora.
    pub fn set_rate(&mut self, rate_bps: u64, now: Instant) {
        self.refill(now);
        self.rate = rate_bps as f64 / 8.0;
    }

    /// Quanto esperar antes de poder enviar o próximo pacote.
    pub fn delay(&mut self, now: Instant) -> Duration {
        self.refill(now);
        if self.rate <= 0.0 || self.tokens >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-self.tokens / self.rate)
        }
    }

    /// Registra o envio de `bytes`.
    pub fn consume(&mut self, bytes: usize, now: Instant) {
        self.refill(now);
        if self.rate > 0.0 {
            self.tokens -= bytes as f64;
        }
    }

    fn refill(&mut self, now: Instant) {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * self.rate).min(self.burst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn rajada_inicial_sai_sem_espera() {
        let t0 = Instant::now();
        let mut p = Pacer::new(8_000_000, 10_000, t0); // 1 MB/s
        for _ in 0..10 {
            assert_eq!(p.delay(t0), Duration::ZERO);
            p.consume(1_000, t0);
        }
        // Balde zerado: ainda pode (saldo não negativo)…
        assert_eq!(p.delay(t0), Duration::ZERO);
        p.consume(1_000, t0);
        // …mas agora deve 1000 bytes = 1 ms a 1 MB/s.
        let d = p.delay(t0);
        assert!(
            d >= MS - Duration::from_micros(1) && d <= MS + Duration::from_micros(1),
            "{d:?}"
        );
    }

    #[test]
    fn taxa_media_respeitada() {
        // Simula 1 s mandando pacotes de 1200 bytes o mais rápido permitido.
        let t0 = Instant::now();
        let mut p = Pacer::new(8_000_000, 12_000, t0);
        let mut now = t0;
        let mut enviados = 0usize;
        while now < t0 + Duration::from_secs(1) {
            now += p.delay(now);
            p.consume(1_200, now);
            enviados += 1_200;
        }
        // 1 MB/s por 1 s + o balde inicial (12 000) + um pacote que sai com
        // saldo zero + o último, que pode começar logo antes de 1 s.
        assert!((1_000_000..=1_014_400).contains(&enviados), "{enviados}");
    }

    #[test]
    fn balde_nao_acumula_alem_do_burst() {
        let t0 = Instant::now();
        let mut p = Pacer::new(8_000_000, 5_000, t0);
        p.consume(5_000, t0);
        // Depois de 1 s parado, o saldo volta só até 5000.
        let t1 = t0 + Duration::from_secs(1);
        p.consume(5_000, t1);
        p.consume(1_000, t1);
        assert!(p.delay(t1) >= MS - Duration::from_micros(1));
    }

    #[test]
    fn taxa_zero_desliga_o_pacing() {
        let t0 = Instant::now();
        let mut p = Pacer::new(0, 0, t0);
        for _ in 0..100 {
            p.consume(1_500, t0);
            assert_eq!(p.delay(t0), Duration::ZERO);
        }
    }

    #[test]
    fn pacote_maior_que_o_balde_nao_trava() {
        let t0 = Instant::now();
        let mut p = Pacer::new(8_000_000, 100, t0);
        assert_eq!(p.delay(t0), Duration::ZERO);
        p.consume(60_000, t0);
        let d = p.delay(t0);
        assert!(
            d > Duration::from_millis(59) && d < Duration::from_millis(61),
            "{d:?}"
        );
    }
}
