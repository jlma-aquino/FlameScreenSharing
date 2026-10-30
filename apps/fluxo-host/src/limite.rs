//! Limite de fps do host, sem perder a última imagem.
//!
//! Em monitores de 144/165 Hz, a captura entrega mais frames do que o
//! `--fps` pedido; codificar todos multiplica o bitrate. Descartar os
//! excedentes também não serve: se a animação parar logo depois de um
//! descarte, a imagem final nunca seria enviada e o cliente ficaria com a
//! tela velha.
//!
//! A regra:
//!
//! - Os envios seguem uma **grade** de horários a cada `1/fps` segundos.
//! - Frame que chega no horário (ou até 10% de intervalo antes) sai na hora.
//! - Frame que chega cedo vira **pendente** (o host guarda uma cópia na GPU).
//!   Um frame mais novo substitui o pendente; no horário seguinte, o
//!   pendente sai se nada mais novo saiu antes.
//!
//! A grade avança um intervalo por envio (não a partir do instante do
//! envio), então a média fica exatamente em `fps` mesmo com uma fonte mais
//! rápida e irregular. Se a tela ficar parada, a grade recomeça no próximo
//! frame, sem "rajada" para compensar o tempo parado.
//!
//! A lógica recebe o instante atual como parâmetro, para ser testada sem
//! relógio de verdade.

use std::time::{Duration, Instant};

pub struct FpsLimiter {
    interval: Duration,
    /// Tolerância: frames até este tanto antes do horário saem na hora
    /// (absorve a variação natural de uma fonte de 60 Hz com limite de 60).
    slack: Duration,
    /// Próximo horário da grade; `None` antes do primeiro envio.
    next: Option<Instant>,
    pending: bool,
}

impl FpsLimiter {
    /// `fps` = 0 desliga o limite (todo frame sai na hora).
    pub fn new(fps: u32) -> Self {
        let interval = match fps {
            0 => Duration::ZERO,
            f => Duration::from_secs_f64(1.0 / f as f64),
        };
        Self {
            interval,
            slack: interval / 10,
            next: None,
            pending: false,
        }
    }

    /// Chegou uma imagem nova. `true`: codifique já. `false`: guarde-a como
    /// pendente (substituindo a anterior) e espere [`Self::deadline`].
    pub fn on_frame(&mut self, now: Instant) -> bool {
        let pode = match self.next {
            None => true,
            Some(next) => now + self.slack >= next,
        };
        if pode {
            self.sent(now);
        } else {
            self.pending = true;
        }
        pode
    }

    /// Quando o pendente deve sair (`None` se não há pendente). O laço usa
    /// isto como prazo máximo de espera por um frame novo.
    pub fn deadline(&self) -> Option<Instant> {
        self.next.filter(|_| self.pending)
    }

    /// Chame depois de esperar. `true`: codifique o pendente agora.
    pub fn on_tick(&mut self, now: Instant) -> bool {
        match self.deadline() {
            Some(d) if now >= d => {
                self.sent(now);
                true
            }
            _ => false,
        }
    }

    fn sent(&mut self, now: Instant) {
        self.pending = false;
        self.next = Some(match self.next {
            // Grade atrasada mais de um intervalo (tela parada): recomeça daqui.
            Some(next) if now < next + self.interval => next + self.interval,
            _ => now + self.interval,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    /// Simula uma fonte com frames a cada `periodo` durante `total` e devolve
    /// os instantes (em ms desde t0) em que algo foi codificado.
    fn simular(fps: u32, periodo: Duration, total: Duration) -> Vec<f64> {
        let t0 = Instant::now();
        let mut l = FpsLimiter::new(fps);
        let mut enviados = Vec::new();
        let mut proximo_frame = t0;
        loop {
            // Avança até o que vier primeiro: frame novo ou prazo do pendente.
            let agora = l.deadline().unwrap_or(proximo_frame).min(proximo_frame);
            if agora >= t0 + total {
                return enviados;
            }
            if agora == proximo_frame {
                if l.on_frame(agora) {
                    enviados.push((agora - t0).as_secs_f64() * 1000.0);
                }
                proximo_frame += periodo;
            }
            if l.on_tick(agora) {
                enviados.push((agora - t0).as_secs_f64() * 1000.0);
            }
        }
    }

    #[test]
    fn fonte_de_165_hz_sai_a_60_fps() {
        let periodo = Duration::from_secs_f64(1.0 / 165.0);
        let enviados = simular(60, periodo, Duration::from_secs(2));
        assert!(
            (119..=121).contains(&enviados.len()),
            "{} envios em 2 s",
            enviados.len()
        );
    }

    #[test]
    fn fonte_de_60_hz_passa_inteira_sem_atraso() {
        let periodo = Duration::from_secs_f64(1.0 / 60.0);
        let enviados = simular(60, periodo, Duration::from_secs(1));
        assert_eq!(enviados.len(), 60);
        // Cada frame sai no instante em que chegou (múltiplos de 16,67 ms).
        for (i, t) in enviados.iter().enumerate() {
            assert!((t - i as f64 * 1000.0 / 60.0).abs() < 0.01, "frame {i} em {t} ms");
        }
    }

    #[test]
    fn ultimo_frame_de_uma_rajada_nao_se_perde() {
        let t0 = Instant::now();
        let mut l = FpsLimiter::new(60);
        assert!(l.on_frame(t0)); // sai na hora
        assert!(!l.on_frame(t0 + 3 * MS)); // cedo: pendente
        assert!(!l.on_frame(t0 + 6 * MS)); // mais novo substitui o pendente
                                           // A tela para. No horário (16,67 ms), o pendente sai.
        let prazo = l.deadline().expect("há pendente");
        assert!(prazo > t0 + 16 * MS && prazo < t0 + 17 * MS);
        assert!(!l.on_tick(t0 + 10 * MS));
        assert!(l.on_tick(prazo));
        assert_eq!(l.deadline(), None);
    }

    #[test]
    fn tela_parada_nao_gera_rajada_depois() {
        let t0 = Instant::now();
        let mut l = FpsLimiter::new(60);
        assert!(l.on_frame(t0));
        // 1 s sem nada; depois dois frames colados: só o primeiro sai na hora.
        let t1 = t0 + Duration::from_secs(1);
        assert!(l.on_frame(t1));
        assert!(!l.on_frame(t1 + MS));
    }

    #[test]
    fn fps_zero_desliga_o_limite() {
        let t0 = Instant::now();
        let mut l = FpsLimiter::new(0);
        for i in 0..100 {
            assert!(l.on_frame(t0 + i * Duration::from_micros(100)));
        }
        assert_eq!(l.deadline(), None);
    }
}
