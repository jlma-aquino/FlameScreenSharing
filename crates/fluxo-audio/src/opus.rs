//! Opus seguro: [`Encoder`] e [`Decoder`] sobre a libopus (crate
//! `unsafe-libopus`, a libopus de referência traduzida para Rust).
//!
//! Sempre 48 kHz estéreo, quadros de 10 ms ([`FRAME_SAMPLES`] por canal),
//! amostras `f32` intercaladas (E, D, E, D...). Todo o `unsafe` do crate fica
//! aqui: ponteiros criados pela própria libopus, usados só por este objeto e
//! destruídos no `Drop`.

use unsafe_libopus as unsafe_opus;

use crate::{AudioError, CHANNELS, FRAME_SAMPLES, SAMPLE_RATE};

/// Maior pacote Opus possível (o padrão recomenda reservar isso).
const MAX_PACKET: usize = 1275;

fn erro(codigo: i32) -> AudioError {
    AudioError::Opus(codigo)
}

/// Codifica quadros de 10 ms.
pub struct Encoder {
    st: *mut unsafe_opus::OpusEncoder,
}

// SAFETY: o estado da libopus não tem nada ligado à thread que o criou; só
// não pode ser usado por duas threads ao mesmo tempo (`&mut self` garante).
unsafe impl Send for Encoder {}

impl Encoder {
    /// Encoder de baixa latência para música e jogos, com `bitrate` em bit/s
    /// e FEC dentro do pacote (um quadro perdido pode ser refeito pelo
    /// seguinte), ajustado para ~`perda_pct`% de perda.
    pub fn new(bitrate: i32, perda_pct: i32) -> Result<Self, AudioError> {
        let mut e = 0;
        // SAFETY: parâmetros válidos; o ponteiro devolvido é nosso até o Drop.
        let st = unsafe {
            unsafe_opus::opus_encoder_create(
                SAMPLE_RATE as i32,
                CHANNELS as i32,
                unsafe_opus::OPUS_APPLICATION_RESTRICTED_LOWDELAY,
                &mut e,
            )
        };
        if st.is_null() || e != unsafe_opus::OPUS_OK {
            return Err(erro(e));
        }
        let me = Self { st };
        me.ctl(unsafe_opus::OPUS_SET_BITRATE_REQUEST, bitrate)?;
        me.ctl(unsafe_opus::OPUS_SET_INBAND_FEC_REQUEST, (perda_pct > 0) as i32)?;
        me.ctl(
            unsafe_opus::OPUS_SET_PACKET_LOSS_PERC_REQUEST,
            perda_pct.clamp(0, 100),
        )?;
        me.ctl(
            unsafe_opus::OPUS_SET_SIGNAL_REQUEST,
            unsafe_opus::OPUS_SIGNAL_MUSIC,
        )?;
        Ok(me)
    }

    fn ctl(&self, pedido: i32, valor: i32) -> Result<(), AudioError> {
        // SAFETY: estado válido; pedidos SET com um i32, como a libopus espera.
        let r = unsafe { unsafe_opus::opus_encoder_ctl_impl(self.st, pedido, unsafe_opus::varargs!(valor)) };
        match r {
            unsafe_opus::OPUS_OK => Ok(()),
            e => Err(erro(e)),
        }
    }

    /// Codifica um quadro (`FRAME_SAMPLES × CHANNELS` amostras).
    pub fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>, AudioError> {
        if pcm.len() != FRAME_SAMPLES * CHANNELS {
            return Err(AudioError::FrameSize(pcm.len()));
        }
        let mut saida = vec![0u8; MAX_PACKET];
        // SAFETY: `pcm` tem um quadro inteiro; `saida`, MAX_PACKET bytes.
        let n = unsafe {
            unsafe_opus::opus_encode_float(
                self.st,
                pcm.as_ptr(),
                FRAME_SAMPLES as i32,
                saida.as_mut_ptr(),
                MAX_PACKET as i32,
            )
        };
        if n < 0 {
            return Err(erro(n));
        }
        saida.truncate(n as usize);
        Ok(saida)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: criado por opus_encoder_create e não usado depois disto.
        unsafe { unsafe_opus::opus_encoder_destroy(self.st) };
    }
}

/// Decodifica quadros de 10 ms, inventando os que faltam.
pub struct Decoder {
    st: *mut unsafe_opus::OpusDecoder,
}

// SAFETY: como o Encoder.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new() -> Result<Self, AudioError> {
        let mut e = 0;
        // SAFETY: parâmetros válidos; o ponteiro é nosso até o Drop.
        let st = unsafe { unsafe_opus::opus_decoder_create(SAMPLE_RATE as i32, CHANNELS as i32, &mut e) };
        if st.is_null() || e != unsafe_opus::OPUS_OK {
            return Err(erro(e));
        }
        Ok(Self { st })
    }

    /// Um quadro: o pacote dele, ou (`None`) um quadro perdido, que a libopus
    /// inventa a partir dos anteriores (sem estalo, some aos poucos).
    pub fn decode(&mut self, pacote: Option<&[u8]>) -> Result<Vec<f32>, AudioError> {
        self.decodificar(pacote, false)
    }

    /// Refaz o quadro **anterior** a `pacote` a partir do FEC que vem dentro
    /// dele: melhor que inventar, quando o quadro seguinte já chegou.
    pub fn decode_fec(&mut self, pacote: &[u8]) -> Result<Vec<f32>, AudioError> {
        self.decodificar(Some(pacote), true)
    }

    fn decodificar(&mut self, pacote: Option<&[u8]>, fec: bool) -> Result<Vec<f32>, AudioError> {
        let mut pcm = vec![0f32; FRAME_SAMPLES * CHANNELS];
        let (ptr, len) = match pacote {
            Some(p) => (p.as_ptr(), p.len() as i32),
            None => (std::ptr::null(), 0),
        };
        // SAFETY: `pcm` cabe um quadro inteiro; pacote nulo = quadro perdido.
        let n = unsafe {
            unsafe_opus::opus_decode_float(
                self.st,
                ptr,
                len,
                pcm.as_mut_ptr(),
                FRAME_SAMPLES as i32,
                fec as i32,
            )
        };
        if n < 0 {
            return Err(erro(n));
        }
        pcm.truncate(n as usize * CHANNELS);
        Ok(pcm)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: criado por opus_decoder_create e não usado depois disto.
        unsafe { unsafe_opus::opus_decoder_destroy(self.st) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seno de 440 Hz estéreo, `quadros` quadros.
    fn seno(quadros: usize) -> Vec<f32> {
        (0..quadros * FRAME_SAMPLES)
            .flat_map(|i| {
                let v = (i as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin() * 0.5;
                [v, v]
            })
            .collect()
    }

    fn energia(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn ida_e_volta_mantem_o_som() {
        let mut enc = Encoder::new(128_000, 5).unwrap();
        let mut dec = Decoder::new().unwrap();
        let entrada = seno(50);
        let mut saida = Vec::new();
        let mut bytes = 0;
        for q in entrada.chunks(FRAME_SAMPLES * CHANNELS) {
            let p = enc.encode(q).unwrap();
            bytes += p.len();
            saida.extend(dec.decode(Some(&p)).unwrap());
        }
        assert_eq!(saida.len(), entrada.len());
        // 0,5 s a 128 kbps ≈ 8 KB (VBR: com folga).
        assert!((3000..12000).contains(&bytes), "{bytes} bytes");
        // O codec atrasa alguns ms: compara a energia, que não depende disso.
        let (e_in, e_out) = (energia(&entrada[9600..]), energia(&saida[9600..]));
        assert!((e_out / e_in - 1.0).abs() < 0.1, "{e_in} vs {e_out}");
    }

    #[test]
    fn quadro_perdido_e_inventado_ou_refeito_pelo_fec() {
        let mut enc = Encoder::new(128_000, 10).unwrap();
        let mut dec = Decoder::new().unwrap();
        let pacotes: Vec<Vec<u8>> = seno(20)
            .chunks(FRAME_SAMPLES * CHANNELS)
            .map(|q| enc.encode(q).unwrap())
            .collect();
        for p in &pacotes[..10] {
            dec.decode(Some(p)).unwrap();
        }
        // O 11º se perdeu: com o 12º em mãos, o FEC refaz; sem, inventa.
        let refeito = dec.decode_fec(&pacotes[11]).unwrap();
        assert_eq!(refeito.len(), FRAME_SAMPLES * CHANNELS);
        assert!(energia(&refeito) > 0.1, "o FEC trouxe som");
        let inventado = dec.decode(None).unwrap();
        assert_eq!(inventado.len(), FRAME_SAMPLES * CHANNELS);
    }

    #[test]
    fn quadro_do_tamanho_errado_e_recusado() {
        let mut enc = Encoder::new(64_000, 0).unwrap();
        assert_eq!(enc.encode(&[0.0; 100]), Err(AudioError::FrameSize(100)));
    }
}
