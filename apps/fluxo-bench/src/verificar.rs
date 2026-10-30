//! `encode --verificar`: decodifica o H.264 gerado com o decoder de
//! software do próprio Windows e compara com a imagem capturada.
//!
//! Prova três coisas sem depender de VLC/ffmpeg: o bitstream é H.264 válido,
//! nenhum frame se perde, e as cores estão certas (a conversão BGRA→NV12 e o
//! VUI BT.709 batem — uma matriz errada derruba o PSNR e muda os tons).

use std::mem::ManuallyDrop;

use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

type Erro = Box<dyn std::error::Error>;

/// Decoder H.264 síncrono (Microsoft H.264 Video Decoder MFT).
pub struct Decoder {
    mft: IMFTransform,
    width: u32,
    height: u32,
    /// Linhas reais do plano Y no buffer de saída (1080 vira 1088).
    plane_height: u32,
    stride: u32,
    out_size: u32,
}

impl Decoder {
    pub fn new(width: u32, height: u32) -> Result<Self, Erro> {
        // SAFETY: inicialização padrão de um MFT síncrono de software.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_LITE)?;
            let mft: IMFTransform = CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)?;
            // Sem isso o decoder segura frames esperando reordenação.
            let _ = mft.GetAttributes()?.SetUINT32(&MF_LOW_LATENCY, 1);

            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            t.SetUINT64(&MF_MT_FRAME_SIZE, (width as u64) << 32 | height as u64)?;
            mft.SetInputType(0, &t, 0)?;
            let mut me = Self {
                mft,
                width,
                height,
                plane_height: height,
                stride: width,
                out_size: 0,
            };
            me.choose_output()?;
            me.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            Ok(me)
        }
    }

    /// Escolhe saída NV12 e lê a geometria real do buffer.
    fn choose_output(&mut self) -> Result<(), Erro> {
        // SAFETY: enumeração e escolha de tipo de saída do MFT.
        unsafe {
            for i in 0.. {
                let t = self.mft.GetOutputAvailableType(0, i)?;
                if t.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                    self.mft.SetOutputType(0, &t, 0)?;
                    let size = t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
                    self.plane_height = ((size & 0xFFFF_FFFF) as u32).max(self.height);
                    self.stride = t.GetUINT32(&MF_MT_DEFAULT_STRIDE).unwrap_or(self.width);
                    self.out_size = self.mft.GetOutputStreamInfo(0)?.cbSize;
                    return Ok(());
                }
            }
        }
        Err("decoder sem saída NV12".into())
    }

    /// Entrega um frame H.264 e devolve os frames decodificados (BGRA).
    pub fn decode(&mut self, h264: &[u8]) -> Result<Vec<Vec<u8>>, Erro> {
        // SAFETY: amostra com cópia dos bytes; o MFT copia o que precisar.
        unsafe {
            let buffer = MFCreateMemoryBuffer(h264.len() as u32)?;
            let mut p = std::ptr::null_mut();
            buffer.Lock(&mut p, None, None)?;
            std::ptr::copy_nonoverlapping(h264.as_ptr(), p, h264.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(h264.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            self.mft.ProcessInput(0, &sample, 0)?;
        }
        self.drain_outputs()
    }

    /// Pede ao decoder os frames que ainda estão com ele.
    pub fn finish(&mut self) -> Result<Vec<Vec<u8>>, Erro> {
        // SAFETY: mensagem padrão de fim de fluxo.
        unsafe { self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)? };
        self.drain_outputs()
    }

    fn drain_outputs(&mut self) -> Result<Vec<Vec<u8>>, Erro> {
        let mut frames = Vec::new();
        loop {
            // SAFETY: amostra de saída alocada por nós (MFT de software);
            // tomamos de volta as referências COM após a chamada.
            let (resultado, sample) = unsafe {
                let sample = MFCreateSample()?;
                sample.AddBuffer(&MFCreateMemoryBuffer(self.out_size.max(1))?)?;
                let mut buf = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(Some(sample)),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0;
                let r = self.mft.ProcessOutput(0, &mut buf, &mut status);
                let s = ManuallyDrop::take(&mut buf[0].pSample);
                let _ = ManuallyDrop::take(&mut buf[0].pEvents);
                (r, s)
            };
            match resultado {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(frames),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    self.choose_output()?;
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
            let Some(sample) = sample else { continue };
            // SAFETY: Lock/Unlock pareados; lemos dentro do tamanho travado.
            let bgra = unsafe {
                let buffer = sample.ConvertToContiguousBuffer()?;
                let mut p = std::ptr::null_mut();
                let mut len = 0u32;
                buffer.Lock(&mut p, None, Some(&mut len))?;
                let nv12 = std::slice::from_raw_parts(p, len as usize);
                let img = nv12_to_bgra(
                    nv12,
                    self.width,
                    self.height,
                    self.stride as usize,
                    self.plane_height as usize,
                );
                buffer.Unlock()?;
                img
            };
            frames.push(bgra);
        }
    }
}

/// NV12 (BT.709, faixa limitada) → BGRA. Inverso exato do que o encoder assume.
fn nv12_to_bgra(nv12: &[u8], width: u32, height: u32, stride: usize, plane_height: usize) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0u8; w * h * 4];
    let uv_base = stride * plane_height;
    if nv12.len() < uv_base + stride * (h / 2) {
        return out; // buffer menor que o esperado: devolve preto (PSNR acusa)
    }
    for y in 0..h {
        for x in 0..w {
            let yy = (nv12[y * stride + x] as f32 - 16.0) * (255.0 / 219.0);
            let uv = uv_base + (y / 2) * stride + (x / 2) * 2;
            let cb = (nv12[uv] as f32 - 128.0) * (255.0 / 224.0);
            let cr = (nv12[uv + 1] as f32 - 128.0) * (255.0 / 224.0);
            let r = yy + 1.5748 * cr;
            let g = yy - 0.1873 * cb - 0.4681 * cr;
            let b = yy + 1.8556 * cb;
            let o = (y * w + x) * 4;
            out[o] = b.round().clamp(0.0, 255.0) as u8;
            out[o + 1] = g.round().clamp(0.0, 255.0) as u8;
            out[o + 2] = r.round().clamp(0.0, 255.0) as u8;
            out[o + 3] = 255;
        }
    }
    out
}

/// PSNR em dB entre duas imagens BGRA do mesmo tamanho (canais B, G, R).
/// Referência: > 40 dB excelente, 35–40 muito bom, < 30 algo errado.
pub fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mut soma = 0f64;
    let mut n = 0u64;
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        for c in 0..3 {
            let d = pa[c] as f64 - pb[c] as f64;
            soma += d * d;
        }
        n += 3;
    }
    if soma == 0.0 || n == 0 {
        return 99.0;
    }
    10.0 * (255.0f64 * 255.0 / (soma / n as f64)).log10()
}
