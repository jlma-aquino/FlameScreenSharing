//! Estimativa do atraso de fila a partir do trânsito dos pacotes.
//!
//! `trânsito = chegada (relógio do cliente) − captura (relógio do host)`. Com
//! relógios diferentes o valor absoluto não significa nada, mas a diferença
//! entre os relógios é constante: o **menor** trânsito visto recentemente
//! corresponde à rede vazia, e quanto um frame passa disso é tempo parado em
//! alguma fila (roteador, Wi-Fi, placa de rede) — o primeiro sinal de
//! congestionamento, antes mesmo de haver perda.
//!
//! O receptor usa `chegada − send_us` (a hora em que o pacote saiu do host),
//! não a hora da captura: medido desde a captura, o tempo de encode, a
//! espera do limite de fps e o pacing espalhando o frame (que crescem com o
//! tamanho dele) pareciam fila — no Wi-Fi, 15 ms a 7 Mbps num link de 30+,
//! e o bitrate caía sem motivo.
//!
//! O mínimo é de uma janela deslizante (10 s), para acompanhar mudanças de
//! rota ou de relógio; o atraso de fila sai suavizado (média móvel
//! exponencial), para um frame grande isolado não disparar alarme.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Janela do mínimo.
const JANELA: Duration = Duration::from_secs(10);
/// Granularidade da janela: um mínimo por segundo.
const BALDE: Duration = Duration::from_secs(1);
/// Peso de cada amostra nova na média (≈ 10 frames de memória).
const PESO: f64 = 0.1;

#[derive(Debug, Default)]
pub(crate) struct QueueDelay {
    /// Mínimo de cada segundo recente: (início do segundo, mínimo em µs).
    baldes: VecDeque<(Instant, i64)>,
    suavizado_us: f64,
}

impl QueueDelay {
    /// Registra o trânsito de um frame (µs, pode ser negativo entre PCs).
    pub fn add(&mut self, transit_us: i32, now: Instant) {
        let t = transit_us as i64;
        match self.baldes.back_mut() {
            Some((inicio, min)) if now.duration_since(*inicio) < BALDE => *min = (*min).min(t),
            _ => self.baldes.push_back((now, t)),
        }
        while self
            .baldes
            .front()
            .is_some_and(|(inicio, _)| now.duration_since(*inicio) > JANELA)
        {
            self.baldes.pop_front();
        }
        let base = self.baldes.iter().map(|&(_, m)| m).min().unwrap_or(t);
        let fila = (t - base).max(0) as f64;
        self.suavizado_us += (fila - self.suavizado_us) * PESO;
    }

    /// Atraso de fila atual (suavizado), em µs.
    pub fn current_us(&self) -> u32 {
        self.suavizado_us.round().clamp(0.0, u32::MAX as f64) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn rede_estavel_da_atraso_zero_mesmo_com_relogio_defasado() {
        let t0 = Instant::now();
        let mut d = QueueDelay::default();
        // Relógios a −700 ms um do outro, trânsito real constante.
        for i in 0..120 {
            d.add(-700_000, t0 + i * 16 * MS);
        }
        assert_eq!(d.current_us(), 0);
    }

    #[test]
    fn fila_crescendo_aparece_no_atraso() {
        let t0 = Instant::now();
        let mut d = QueueDelay::default();
        for i in 0..60 {
            d.add(5_000, t0 + i * 16 * MS);
        }
        // Fila de 30 ms se forma e se mantém por meio segundo.
        for i in 60..90 {
            d.add(35_000, t0 + i * 16 * MS);
        }
        let atraso = d.current_us();
        assert!((25_000..=30_000).contains(&atraso), "atraso {atraso} µs");
    }

    #[test]
    fn um_frame_atrasado_isolado_quase_nao_mexe() {
        let t0 = Instant::now();
        let mut d = QueueDelay::default();
        for i in 0..60 {
            d.add(5_000, t0 + i * 16 * MS);
        }
        d.add(45_000, t0 + 60 * 16 * MS);
        assert!(d.current_us() <= 4_000, "{}", d.current_us());
    }

    #[test]
    fn minimo_antigo_sai_da_janela() {
        let t0 = Instant::now();
        let mut d = QueueDelay::default();
        d.add(0, t0);
        // A rota mudou: o trânsito mínimo agora é 20 ms. Depois de 10 s,
        // isso vira a nova base e o atraso de fila volta a zero.
        for i in 0..800 {
            d.add(20_000, t0 + Duration::from_millis(100) + i * 16 * MS);
        }
        assert!(d.current_us() < 1_000, "{}", d.current_us());
    }
}
