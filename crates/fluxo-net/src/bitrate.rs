//! Controle adaptativo de bitrate no host.
//!
//! A cada relatório do cliente ([`NetworkReport`]), decide se o bitrate do
//! vídeo deve cair, subir ou ficar. A regra segue a ideia de AIMD (como o
//! TCP): **cai rápido** quando a rede reclama e **sobe devagar** quando está
//! folgada, para não oscilar.
//!
//! | Situação | Ação |
//! |---|---|
//! | perda ≥ 10% ou fila ≥ 2,4 × limite (36 ms) | × 0,7 |
//! | perda ≥ 3% ou fila ≥ limite (15 ms) | × 0,85 |
//! | perda < 1%, fila < 0,4 × limite (6 ms), envio ≥ 50% do alvo, 2 s sem quedas | + 8% (no máximo 1× por segundo) |
//! | entre uma coisa e outra | mantém |
//!
//! O limite de fila (padrão [`DEFAULT_QUEUE_LIMIT_MS`]) troca qualidade por
//! latência: mais baixo, o bitrate para de subir mais cedo e a imagem chega
//! antes; mais alto, mais bitrate e mais atraso.
//!
//! Depois de uma queda, outra só vem 500 ms depois: a fila leva um tempo
//! para esvaziar, e cair de novo por causa dela seria punir duas vezes.
//! Perdas até ~2% ficam abaixo do limiar de propósito: o FEC dá conta delas.
//!
//! A lógica recebe o instante atual como parâmetro, para ser testada sem
//! relógio de verdade.

use std::time::{Duration, Instant};

/// O que o host sabe da rede depois de um relatório do cliente.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NetworkReport {
    /// Fração dos pacotes enviados desde o relatório anterior que não
    /// chegaram (0.0–1.0). `None` se o intervalo teve pacotes de menos para
    /// a conta valer (tela parada).
    pub loss: Option<f64>,
    /// Atraso de fila estimado pelo cliente, em ms.
    pub queue_delay_ms: f64,
    /// Quanto o host de fato mandou desde o relatório anterior, em kbit/s
    /// (`None` = desconhecido). Com a tela parada o vídeo gasta bem menos que
    /// o alvo, e a rede "folgada" não prova que o alvo cabe: o bitrate só
    /// sobe se o envio usar pelo menos metade dele.
    pub sent_kbps: Option<u32>,
}

/// Por que o bitrate mudou (para o relatório do host).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitrateChange {
    /// Caiu por perda de pacotes.
    DownLoss,
    /// Caiu por atraso de fila (congestionamento sem perda ainda).
    DownDelay,
    /// Subiu: rede folgada.
    Up,
}

/// Limite de fila padrão, em ms (ver [`BitrateController::with_queue_limit`]).
pub const DEFAULT_QUEUE_LIMIT_MS: f64 = 15.0;

/// Bitrate inicial máximo, em kbit/s (20 Mbps: folgado em 1080p60 e seguro
/// em quase todo Wi-Fi).
pub const START_KBPS: u32 = 20_000;

/// Fração do alvo que o envio precisa usar para o bitrate poder subir.
const USO_PARA_SUBIR: f64 = 0.5;

const QUEDA_FORTE: f64 = 0.7;
const QUEDA: f64 = 0.85;
const SUBIDA: f64 = 1.08;
const ENTRE_QUEDAS: Duration = Duration::from_millis(500);
const CALMA_PARA_SUBIR: Duration = Duration::from_secs(2);
const ENTRE_SUBIDAS: Duration = Duration::from_secs(1);

/// Controlador de bitrate entre um mínimo e um máximo.
#[derive(Debug, Clone)]
pub struct BitrateController {
    min_kbps: u32,
    max_kbps: u32,
    current_kbps: u32,
    queue_limit_ms: f64,
    last_down: Option<Instant>,
    last_up: Option<Instant>,
    started: Instant,
}

impl BitrateController {
    /// Começa em [`START_KBPS`] (ou no máximo, se for menor) e sobe aos poucos
    /// se a rede deixar: começar direto num máximo alto satura o Wi-Fi e a
    /// largada fica ~2 s com 100–200 ms de atraso.
    pub fn new(min_kbps: u32, max_kbps: u32, now: Instant) -> Self {
        let min_kbps = min_kbps.min(max_kbps);
        Self {
            min_kbps,
            max_kbps,
            current_kbps: max_kbps.min(START_KBPS).max(min_kbps),
            queue_limit_ms: DEFAULT_QUEUE_LIMIT_MS,
            last_down: None,
            last_up: None,
            started: now,
        }
    }

    /// Troca o limite de fila (ms): o bitrate cai com a fila acima dele, cai
    /// forte acima de 2,4× e só sobe abaixo de 0,4×.
    pub fn with_queue_limit(mut self, ms: f64) -> Self {
        self.queue_limit_ms = ms.max(1.0);
        self
    }

    /// Limite de fila em uso, em ms.
    pub fn queue_limit_ms(&self) -> f64 {
        self.queue_limit_ms
    }

    /// Bitrate atual em kbit/s.
    pub fn current_kbps(&self) -> u32 {
        self.current_kbps
    }

    /// Processa um relatório. Devolve o novo bitrate e o motivo, se mudou.
    pub fn on_report(&mut self, r: &NetworkReport, now: Instant) -> Option<(u32, BitrateChange)> {
        let perda = r.loss.unwrap_or(0.0);
        let pode_cair = self
            .last_down
            .is_none_or(|t| now.duration_since(t) >= ENTRE_QUEDAS);

        let limite = self.queue_limit_ms;
        let queda = if perda >= 0.10 || r.queue_delay_ms >= limite * 2.4 {
            Some(QUEDA_FORTE)
        } else if perda >= 0.03 || r.queue_delay_ms >= limite {
            Some(QUEDA)
        } else {
            None
        };
        if let Some(fator) = queda {
            if !pode_cair {
                return None;
            }
            let motivo = if perda >= 0.03 {
                BitrateChange::DownLoss
            } else {
                BitrateChange::DownDelay
            };
            self.last_down = Some(now);
            return self.set((self.current_kbps as f64 * fator) as u32, motivo);
        }

        let folgada = perda < 0.01 && r.queue_delay_ms < limite * 0.4;
        let usando = r
            .sent_kbps
            .is_none_or(|s| s as f64 >= self.current_kbps as f64 * USO_PARA_SUBIR);
        let calma = now.duration_since(self.last_down.unwrap_or(self.started)) >= CALMA_PARA_SUBIR
            || self.last_down.is_none();
        let pode_subir = self
            .last_up
            .is_none_or(|t| now.duration_since(t) >= ENTRE_SUBIDAS);
        if folgada && usando && calma && pode_subir && self.current_kbps < self.max_kbps {
            self.last_up = Some(now);
            let alvo = (self.current_kbps as f64 * SUBIDA).max(self.current_kbps as f64 + 300.0);
            return self.set(alvo as u32, BitrateChange::Up);
        }
        None
    }

    fn set(&mut self, kbps: u32, motivo: BitrateChange) -> Option<(u32, BitrateChange)> {
        let novo = kbps.clamp(self.min_kbps, self.max_kbps);
        if novo == self.current_kbps {
            return None;
        }
        self.current_kbps = novo;
        Some((novo, motivo))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn rel(loss: f64, fila_ms: f64) -> NetworkReport {
        NetworkReport {
            loss: Some(loss),
            queue_delay_ms: fila_ms,
            sent_kbps: None,
        }
    }

    #[test]
    fn comeca_no_maximo_e_fica_se_a_rede_esta_boa() {
        let t0 = Instant::now();
        let mut c = BitrateController::new(2_500, 20_000, t0);
        for i in 1..40 {
            assert_eq!(c.on_report(&rel(0.0, 2.0), t0 + i * 250 * MS), None);
        }
        assert_eq!(c.current_kbps(), 20_000);
    }

    #[test]
    fn perda_alta_derruba_forte() {
        let t0 = Instant::now();
        let mut c = BitrateController::new(2_500, 20_000, t0);
        assert_eq!(
            c.on_report(&rel(0.15, 0.0), t0),
            Some((14_000, BitrateChange::DownLoss))
        );
    }

    #[test]
    fn fila_derruba_mesmo_sem_perda() {
        let t0 = Instant::now();
        let mut c = BitrateController::new(2_500, 20_000, t0);
        assert_eq!(
            c.on_report(&rel(0.0, 30.0), t0),
            Some((17_000, BitrateChange::DownDelay))
        );
    }

    #[test]
    fn perda_pequena_fica_para_o_fec() {
        let t0 = Instant::now();
        let mut c = BitrateController::new(2_500, 20_000, t0);
        assert_eq!(c.on_report(&rel(0.02, 5.0), t0), None);
    }

    #[test]
    fn quedas_seguidas_respeitam_intervalo() {
        let t0 = Instant::now();
        let mut c = BitrateController::new(2_500, 20_000, t0);
        assert!(c.on_report(&rel(0.0, 30.0), t0).is_some());
        // A fila ainda não esvaziou 250 ms depois: não cai de novo.
        assert_eq!(c.on_report(&rel(0.0, 30.0), t0 + 250 * MS), None);
        assert!(c.on_report(&rel(0.0, 30.0), t0 + 500 * MS).is_some());
    }

    #[test]
    fn nunca_passa_do_minimo() {
        let t0 = Instant::now();
        let mut c = BitrateController::new(2_500, 20_000, t0);
        for i in 0..20 {
            c.on_report(&rel(0.5, 100.0), t0 + i * 600 * MS);
        }
        assert_eq!(c.current_kbps(), 2_500);
    }

    #[test]
    fn sobe_devagar_depois_da_calma() {
        let t0 = Instant::now();
        let mut c = BitrateController::new(2_500, 20_000, t0);
        c.on_report(&rel(0.15, 0.0), t0); // 14 000
                                          // Logo depois da queda, mesmo com a rede boa, espera 2 s.
        assert_eq!(c.on_report(&rel(0.0, 1.0), t0 + 1_000 * MS), None);
        let subiu = c.on_report(&rel(0.0, 1.0), t0 + 2_000 * MS);
        assert_eq!(subiu, Some((15_120, BitrateChange::Up)));
        // No máximo uma subida por segundo.
        assert_eq!(c.on_report(&rel(0.0, 1.0), t0 + 2_250 * MS), None);
        assert!(c.on_report(&rel(0.0, 1.0), t0 + 3_000 * MS).is_some());
    }

    #[test]
    fn converge_para_a_banda_disponivel() {
        // Rede simulada que aguenta 8 Mbps: acima disso, fila e perda
        // proporcionais ao excesso. O bitrate deve oscilar em volta de 8.
        let t0 = Instant::now();
        let mut c = BitrateController::new(1_000, 20_000, t0);
        let mut ultimos = Vec::new();
        for i in 0..240 {
            let excesso = (c.current_kbps() as f64 / 8_000.0 - 1.0).max(0.0);
            let r = rel((excesso * 0.5).min(0.5), excesso * 200.0);
            c.on_report(&r, t0 + i * 250 * MS);
            if i >= 160 {
                ultimos.push(c.current_kbps());
            }
        }
        let max = *ultimos.iter().max().unwrap();
        let min = *ultimos.iter().min().unwrap();
        assert!(max <= 9_000 && min >= 5_000, "oscilou entre {min} e {max} kbps");
    }

    #[test]
    fn tela_parada_sem_conta_de_perda_nao_atrapalha() {
        let t0 = Instant::now();
        let mut c = BitrateController::new(2_500, 20_000, t0);
        let r = NetworkReport {
            loss: None,
            queue_delay_ms: 1.0,
            sent_kbps: None,
        };
        assert_eq!(c.on_report(&r, t0), None);
    }

    #[test]
    fn limite_de_fila_menor_reage_antes() {
        let t0 = Instant::now();
        let mut padrao = BitrateController::new(2_500, 20_000, t0).with_queue_limit(25.0);
        let mut baixo = BitrateController::new(2_500, 20_000, t0).with_queue_limit(12.0);
        // 15 ms de fila: com limite 25 mantém; com 12 ms cai.
        assert_eq!(padrao.on_report(&rel(0.0, 15.0), t0), None);
        assert_eq!(
            baixo.on_report(&rel(0.0, 15.0), t0),
            Some((17_000, BitrateChange::DownDelay))
        );
        // 30 ms (≥ 2,4 × 12): queda forte.
        let t1 = t0 + 600 * MS;
        assert_eq!(
            baixo.on_report(&rel(0.0, 30.0), t1),
            Some((11_900, BitrateChange::DownDelay))
        );
        // Só sobe com fila abaixo de 0,4 × 12 = 4,8 ms.
        let t2 = t1 + 3_000 * MS;
        assert_eq!(baixo.on_report(&rel(0.0, 6.0), t2), None);
        assert!(baixo.on_report(&rel(0.0, 4.0), t2).is_some());
    }

    #[test]
    fn tela_parada_nao_sobe_o_alvo() {
        let t0 = Instant::now();
        let mut c = BitrateController::new(2_500, 60_000, t0);
        assert_eq!(c.current_kbps(), START_KBPS);
        let parada = NetworkReport {
            sent_kbps: Some(3_000),
            ..rel(0.0, 1.0)
        };
        let video = NetworkReport {
            sent_kbps: Some(15_000),
            ..rel(0.0, 1.0)
        };
        // Rede folgada, mas o envio usa 15% do alvo: fica.
        assert_eq!(c.on_report(&parada, t0 + 3_000 * MS), None);
        // Usando 75%: sobe.
        assert!(c.on_report(&video, t0 + 3_100 * MS).is_some());
    }
}
