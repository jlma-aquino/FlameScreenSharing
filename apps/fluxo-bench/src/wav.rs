//! Gravação de WAV simples (PCM 16 bits), para conferir o áudio recebido sem
//! precisar tocá-lo.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};

/// Arquivo WAV aberto; o cabeçalho ganha os tamanhos certos no `finish`.
pub struct WavWriter {
    arquivo: BufWriter<File>,
    amostras: u32,
    canais: u16,
}

impl WavWriter {
    pub fn create(caminho: &str, taxa: u32, canais: u16) -> std::io::Result<Self> {
        let mut arquivo = BufWriter::new(File::create(caminho)?);
        let bloco = canais * 2;
        arquivo.write_all(b"RIFF\0\0\0\0WAVEfmt ")?;
        arquivo.write_all(&16u32.to_le_bytes())?;
        arquivo.write_all(&1u16.to_le_bytes())?; // PCM
        arquivo.write_all(&canais.to_le_bytes())?;
        arquivo.write_all(&taxa.to_le_bytes())?;
        arquivo.write_all(&(taxa * bloco as u32).to_le_bytes())?;
        arquivo.write_all(&bloco.to_le_bytes())?;
        arquivo.write_all(&16u16.to_le_bytes())?;
        arquivo.write_all(b"data\0\0\0\0")?;
        Ok(Self {
            arquivo,
            amostras: 0,
            canais,
        })
    }

    /// Amostras `f32` intercaladas (−1 a 1).
    pub fn write(&mut self, pcm: &[f32]) -> std::io::Result<()> {
        for v in pcm {
            self.arquivo
                .write_all(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
        }
        self.amostras += pcm.len() as u32;
        Ok(())
    }

    /// Segundos gravados.
    pub fn seconds(&self, taxa: u32) -> f64 {
        self.amostras as f64 / self.canais as f64 / taxa as f64
    }

    pub fn finish(mut self) -> std::io::Result<()> {
        let dados = self.amostras * 2;
        self.arquivo.seek(SeekFrom::Start(4))?;
        self.arquivo.write_all(&(36 + dados).to_le_bytes())?;
        self.arquivo.seek(SeekFrom::Start(40))?;
        self.arquivo.write_all(&dados.to_le_bytes())?;
        self.arquivo.flush()
    }
}
