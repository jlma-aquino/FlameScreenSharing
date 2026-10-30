//! Histórico de pacotes enviados, para a conta de perda do bitrate adaptativo.
//!
//! O cliente relata quantos pacotes **recebeu** até o momento em que mandou o
//! relatório. Os pacotes que nós mandamos logo antes disso ainda estavam no
//! caminho (rede, fila do Wi-Fi) e não podem contar como perdidos. Por isso a
//! comparação é com quantos pacotes tinham saído **um RTT antes** de o
//! relatório chegar — e para isso guardamos o total enviado ao longo do tempo.
//!
//! Medido no teste Windows → notebook no Wi-Fi: sem o desconto, pacotes na
//! fila (até 80 ms a 45 Mbps) e pacotes de um frame ainda no pacing apareciam
//! como 3–39% de "perda" com 0 pacotes realmente perdidos.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Quanto histórico guardar: muito mais que qualquer RTT útil.
const JANELA: Duration = Duration::from_secs(2);
/// Resolução: no máximo uma amostra por milissegundo.
const RESOLUCAO: Duration = Duration::from_millis(1);

#[derive(Debug, Default)]
pub(crate) struct SentHistory {
    /// (instante, total enviado até ele), em ordem crescente de instante.
    amostras: VecDeque<(Instant, u64)>,
    total: u64,
    /// Total da última amostra descartada por velhice: o que já tinha saído
    /// antes de todas as amostras guardadas (0 enquanto nada foi descartado).
    antes_de_tudo: u64,
}

impl SentHistory {
    /// Registra `n` pacotes saindo agora.
    pub fn record(&mut self, n: u64, now: Instant) {
        self.total += n;
        match self.amostras.back_mut() {
            Some((t, total)) if now.saturating_duration_since(*t) < RESOLUCAO => *total = self.total,
            _ => self.amostras.push_back((now, self.total)),
        }
        while self
            .amostras
            .front()
            .is_some_and(|&(t, _)| now.saturating_duration_since(t) > JANELA)
        {
            if let Some((_, total)) = self.amostras.pop_front() {
                self.antes_de_tudo = total;
            }
        }
    }

    /// Total enviado até `quando`: o da amostra mais recente que não passa
    /// dele.
    pub fn at(&self, quando: Instant) -> u64 {
        match self.amostras.partition_point(|&(t, _)| t <= quando) {
            0 => self.antes_de_tudo,
            i => self.amostras[i - 1].1,
        }
    }

    /// Total enviado até agora.
    pub fn total(&self) -> u64 {
        self.total
    }
}

/// Contadores de um relatório do cliente, junto com quantos pacotes tinham
/// saído do host até um RTT antes de ele chegar.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LossSample {
    pub quando: Instant,
    pub recebidos: u32,
    pub tentados: u64,
    pub frames_perdidos: u32,
    pub fec_reconstruidos: u32,
}

/// Perda medida sobre ~1 s de relatórios.
///
/// Duas proteções contra "perda fantasma" no Wi-Fi, que entrega pacotes em
/// lotes e segura alguns por milissegundos a mais:
///
/// - **Janela de 1 s** (não 250 ms): poucos pacotes atrasados viram ruído.
/// - **Confirmação pelo cliente:** perda de verdade sempre aparece do lado de
///   lá — como fragmento reconstruído pelo FEC ou como frame perdido. Se o
///   cliente não viu nenhum dos dois na janela, a diferença é só pacote
///   atrasado, e a perda conta como 0. (Congestionamento sem perda ainda é
///   pego pelo atraso de fila.)
///
/// Medido no teste Windows → notebook no Wi-Fi: sem isso, 3–6% de "perda" a
/// cada poucos segundos derrubavam o bitrate de 20 para 2,5 Mbps com zero
/// pacotes realmente perdidos.
#[derive(Debug, Default)]
pub(crate) struct LossWindow {
    amostras: VecDeque<LossSample>,
}

/// Duração da janela de perda.
const JANELA_PERDA: Duration = Duration::from_secs(1);
/// Abaixo disso, a conta não vale (tela parada).
const MIN_PACOTES: u64 = 100;

impl LossWindow {
    /// Registra um relatório e devolve a perda da última ~1 s, se houve
    /// pacotes suficientes para a conta valer.
    pub fn add(&mut self, a: LossSample) -> Option<f64> {
        self.amostras.push_back(a);
        // A base é a amostra mais recente com pelo menos 1 s de idade.
        while self.amostras.len() >= 2
            && a.quando.saturating_duration_since(self.amostras[1].quando) >= JANELA_PERDA
        {
            self.amostras.pop_front();
        }
        let base = *self.amostras.front()?;
        let tentados = a.tentados.saturating_sub(base.tentados);
        if tentados < MIN_PACOTES {
            return None;
        }
        let confirmada =
            a.frames_perdidos != base.frames_perdidos || a.fec_reconstruidos != base.fec_reconstruidos;
        if !confirmada {
            return Some(0.0);
        }
        let recebidos = a.recebidos.wrapping_sub(base.recebidos) as u64;
        Some((1.0 - recebidos as f64 / tentados as f64).clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn devolve_o_total_de_um_instante_passado() {
        let t0 = Instant::now();
        let mut h = SentHistory::default();
        for i in 0..100 {
            h.record(10, t0 + i * 2 * MS); // 10 pacotes a cada 2 ms
        }
        assert_eq!(h.total(), 1000);
        // 50 ms atrás do último registro (198 ms): até 148 ms tinham saído 750.
        assert_eq!(h.at(t0 + 148 * MS), 750);
        assert_eq!(h.at(t0 + 149 * MS), 750);
        assert_eq!(h.at(t0 + 500 * MS), 1000);
    }

    #[test]
    fn varios_registros_no_mesmo_milissegundo_viram_uma_amostra() {
        let t0 = Instant::now();
        let mut h = SentHistory::default();
        for _ in 0..30 {
            h.record(1, t0);
        }
        assert_eq!(h.amostras.len(), 1);
        assert_eq!(h.at(t0), 30);
    }

    #[test]
    fn antes_de_tudo_e_zero() {
        let t0 = Instant::now();
        let mut h = SentHistory::default();
        assert_eq!(h.at(t0), 0);
        h.record(5, t0 + 10 * MS);
        assert_eq!(h.at(t0), 0);
    }

    #[test]
    fn historico_antigo_e_descartado() {
        let t0 = Instant::now();
        let mut h = SentHistory::default();
        for i in 0..5_000u32 {
            h.record(1, t0 + i * MS);
        }
        assert!(h.amostras.len() <= 2_002, "{}", h.amostras.len());
        assert_eq!(h.total(), 5_000);
    }

    fn amostra(ms: u64, recebidos: u32, tentados: u64, perdidos: u32, fec: u32) -> LossSample {
        LossSample {
            quando: Instant::now() + Duration::from_millis(ms),
            recebidos,
            tentados,
            frames_perdidos: perdidos,
            fec_reconstruidos: fec,
        }
    }

    #[test]
    fn pacotes_atrasados_sem_perda_confirmada_nao_contam() {
        let mut w = LossWindow::default();
        let t0 = Instant::now();
        let mut s = |ms: u64, rec: u32, tent: u64| {
            w.add(LossSample {
                quando: t0 + Duration::from_millis(ms),
                recebidos: rec,
                tentados: tent,
                frames_perdidos: 0,
                fec_reconstruidos: 0,
            })
        };
        s(0, 0, 0);
        // O Wi-Fi segura 30 pacotes: parece 6% de perda, mas o cliente não
        // perdeu nada (nem FEC, nem frame).
        assert_eq!(s(1_000, 470, 500), Some(0.0));
    }

    #[test]
    fn perda_confirmada_pelo_cliente_conta() {
        let mut w = LossWindow::default();
        w.add(amostra(0, 0, 0, 0, 0));
        let p = w.add(amostra(1_000, 900, 1_000, 3, 40)).unwrap();
        assert!((p - 0.10).abs() < 1e-9, "{p}");
    }

    #[test]
    fn poucos_pacotes_nao_dao_conta() {
        let mut w = LossWindow::default();
        w.add(amostra(0, 0, 0, 0, 0));
        assert_eq!(w.add(amostra(1_000, 40, 50, 1, 0)), None);
    }

    #[test]
    fn janela_e_de_cerca_de_um_segundo() {
        let mut w = LossWindow::default();
        // 10 relatórios a cada 250 ms; perda real só no começo.
        w.add(amostra(0, 0, 0, 0, 0));
        w.add(amostra(250, 400, 500, 2, 30));
        for i in 2..10u64 {
            let p = w.add(amostra(i * 250, (i * 500 - 100) as u32, i * 500, 2, 30));
            if i >= 6 {
                // A perda do começo já saiu da janela.
                assert_eq!(p, Some(0.0), "relatório {i}");
            }
        }
    }
}
