//! Simulações de rede ruim, **só para testes**: atraso e jitter.
//!
//! A perda e a banda limitada ficam em `sender.rs`, onde os pacotes saem; o
//! atraso precisa de uma thread própria ([`DelayLine`]), que segura cada
//! pacote até a hora de "chegar".

use std::net::UdpSocket;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::sender::Shared;

/// De quanto em quanto tempo o jitter sorteia um atraso extra novo. Entre
/// um sorteio e outro o atraso fica constante, como num link real: quando
/// ele cai, os pacotes segurados saem juntos (rajada, como o Wi-Fi faz);
/// quando sobe, abre um buraco.
const JITTER_PERIOD: Duration = Duration::from_millis(20);

/// Gerador pseudoaleatório mínimo (xorshift) para as simulações.
pub(crate) struct Rng(u64);

impl Rng {
    pub fn new() -> Self {
        let semente = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self(semente | 1)
    }

    /// Número em [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Decide quando cada pacote "chega": `base` + um extra sorteado em
/// `[0, jitter]` a cada [`JITTER_PERIOD`]. Os pacotes nunca se
/// ultrapassam (a fila é única, como num link de verdade).
pub(crate) struct DelayModel {
    base: Duration,
    jitter: Duration,
    extra: Duration,
    proximo_sorteio: Option<Instant>,
    ultima_chegada: Option<Instant>,
    rng: Rng,
}

impl DelayModel {
    pub fn new(base: Duration, jitter: Duration) -> Self {
        Self {
            base,
            jitter,
            extra: Duration::ZERO,
            proximo_sorteio: None,
            ultima_chegada: None,
            rng: Rng::new(),
        }
    }

    /// Quando um pacote que sai em `agora` deve ser entregue.
    pub fn due(&mut self, agora: Instant) -> Instant {
        if !self.jitter.is_zero() && self.proximo_sorteio.map_or(true, |t| agora >= t) {
            self.extra = self.jitter.mul_f64(self.rng.next_f64());
            self.proximo_sorteio = Some(agora + JITTER_PERIOD);
        }
        let chegada = agora + self.base + self.extra;
        let chegada = self.ultima_chegada.map_or(chegada, |u| u.max(chegada));
        self.ultima_chegada = Some(chegada);
        chegada
    }
}

/// Linha de atraso na saída: os pacotes entram com [`send`](Self::send) e
/// uma thread os manda pelo socket na hora sorteada pelo [`DelayModel`].
/// Erros de envio aqui são ignorados (só existe em testes).
pub(crate) struct DelayLine {
    fila: Mutex<mpsc::Sender<(Instant, Vec<u8>)>>,
    modelo: Mutex<DelayModel>,
}

impl DelayLine {
    /// Cria a linha e a thread que entrega os pacotes. A thread termina
    /// quando `shared.stop` liga e a fila esvazia.
    pub fn spawn(
        socket: UdpSocket,
        modelo: DelayModel,
        shared: Arc<Shared>,
    ) -> std::io::Result<(Arc<Self>, JoinHandle<()>)> {
        let (tx, rx) = mpsc::channel::<(Instant, Vec<u8>)>();
        let thread = thread::Builder::new()
            .name("fluxo-net-atraso".into())
            .spawn(move || loop {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok((quando, pacote)) => {
                        let espera = quando.saturating_duration_since(Instant::now());
                        if !espera.is_zero() {
                            thread::sleep(espera);
                        }
                        let _ = socket.send(&pacote);
                    }
                    Err(RecvTimeoutError::Timeout) if shared.stop.load(Ordering::Relaxed) => return,
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            })?;
        let linha = Self {
            fila: Mutex::new(tx),
            modelo: Mutex::new(modelo),
        };
        Ok((Arc::new(linha), thread))
    }

    /// Agenda o envio de `pacote`. Sempre "dá certo" (o envio de verdade é
    /// depois).
    pub fn send(&self, pacote: &[u8]) -> usize {
        let quando = self
            .modelo
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .due(Instant::now());
        let _ = self
            .fila
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send((quando, pacote.to_vec()));
        pacote.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn sem_jitter_o_atraso_e_fixo() {
        let mut m = DelayModel::new(30 * MS, Duration::ZERO);
        let t0 = Instant::now();
        for i in 0..50 {
            let agora = t0 + i * MS;
            assert_eq!(m.due(agora), agora + 30 * MS);
        }
    }

    #[test]
    fn jitter_fica_na_faixa_e_varia() {
        let mut m = DelayModel::new(10 * MS, 40 * MS);
        let t0 = Instant::now();
        let mut extras = Vec::new();
        // Um pacote a cada 25 ms: cada um cai num sorteio novo. A fila única
        // pode segurar um pacote atrás do anterior, mas nunca além do máximo.
        for i in 0..200u32 {
            let agora = t0 + i * 25 * MS;
            let atraso = m.due(agora) - agora;
            extras.push(atraso);
        }
        let menor = *extras.iter().min().unwrap();
        let maior = *extras.iter().max().unwrap();
        assert!(menor >= 10 * MS && maior <= 50 * MS, "{menor:?}..{maior:?}");
        assert!(
            maior - menor > 20 * MS,
            "o atraso deveria variar: {menor:?}..{maior:?}"
        );
    }

    #[test]
    fn pacotes_nunca_se_ultrapassam() {
        let mut m = DelayModel::new(5 * MS, 60 * MS);
        let t0 = Instant::now();
        let mut anterior = t0;
        for i in 0..2_000u32 {
            let chegada = m.due(t0 + Duration::from_micros(i as u64 * 300));
            assert!(chegada >= anterior);
            anterior = chegada;
        }
    }
}
