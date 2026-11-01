//! Tocar o áudio que chega da rede: [`AudioPlayer`] (Windows).
//!
//! Junta o [`JitterBuffer`], o [`Decoder`] e a [`AudioOutput`]: os pacotes
//! entram em qualquer ritmo ([`push`](AudioPlayer::push)); a placa pede som
//! no ritmo dela ([`pump`](AudioPlayer::pump) enche a fila da placa até
//! [`DEVICE_QUEUE_MS`]). Quadro perdido com o seguinte em mãos: o FEC do
//! Opus refaz; sem o seguinte, o Opus inventa. Buffer vazio: a placa toca
//! silêncio sozinha, e o buffer passa a segurar um quadro a mais.

use std::time::Instant;

use crate::{AudioError, AudioOutput, Decoder, JitterBuffer, JitterStats, Next};

/// Som mantido na fila da placa (além do buffer de jitter), em ms.
pub const DEVICE_QUEUE_MS: f64 = 20.0;

/// Contadores do [`AudioPlayer`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PlayerStats {
    pub jitter: JitterStats,
    /// Quadros refeitos pelo FEC (o resto dos perdidos foi inventado).
    pub fec: u64,
    /// Som na fila da placa agora, em ms.
    pub device_ms: f64,
}

pub struct AudioPlayer {
    saida: AudioOutput,
    jitter: JitterBuffer,
    dec: Decoder,
    fec: u64,
}

impl AudioPlayer {
    /// Abre a saída de som padrão com `volume` (0.0 a 1.0, só do Fluxo).
    pub fn new(volume: f32) -> Result<Self, AudioError> {
        Ok(Self {
            saida: AudioOutput::new(volume)?,
            jitter: JitterBuffer::default(),
            dec: Decoder::new()?,
            fec: 0,
        })
    }

    /// Um pacote Opus que chegou (número do quadro no host).
    pub fn push(&mut self, seq: u32, opus: Vec<u8>) {
        self.jitter.push(seq, opus);
    }

    /// Enche a fila da placa até [`DEVICE_QUEUE_MS`] (chame a cada poucos ms).
    pub fn pump(&mut self, agora: Instant) -> Result<(), AudioError> {
        while self.saida.queued_ms() < DEVICE_QUEUE_MS {
            let pcm = match self.jitter.next(agora) {
                Next::Packet(p) => self.dec.decode(Some(&p))?,
                Next::Lost { following: Some(p) } => {
                    self.fec += 1;
                    self.dec.decode_fec(&p)?
                }
                Next::Lost { following: None } => self.dec.decode(None)?,
                // Nada para tocar: a placa toca silêncio até chegar mais.
                Next::Empty => return Ok(()),
            };
            self.saida.write(&pcm)?;
        }
        Ok(())
    }

    /// Muda o volume (0.0 a 1.0).
    pub fn set_volume(&self, v: f32) {
        self.saida.set_volume(v);
    }

    pub fn stats(&self) -> PlayerStats {
        PlayerStats {
            jitter: self.jitter.stats(),
            fec: self.fec,
            device_ms: self.saida.queued_ms(),
        }
    }
}
