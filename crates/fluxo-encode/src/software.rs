//! Backend por software: OpenH264 (Cisco) na CPU, para máquinas sem encoder
//! de hardware (VMs, GPUs antigas ou sem driver de vídeo completo).
//!
//! ## Fluxo de um frame
//!
//! ```text
//! textura BGRA ─► Nv12Converter (GPU) ─► NV12 ─► cópia para textura de
//! leitura ─► Map (CPU) ─► I420 ─► OpenH264 ─► bytes H.264 ─► EncodedFrame
//! ```
//!
//! A conversão de cor (e o redimensionamento) continuam na GPU, iguais aos
//! dos outros backends; só a compressão vai para a CPU. Mesmo um adaptador
//! sem encoder de vídeo costuma ter o Video Processor do D3D11 (o WARP, o
//! adaptador por software do Windows, também tem).
//!
//! ## Configuração
//!
//! - Uso "câmera em tempo real", controle de taxa por bitrate, sem pular
//!   frames. O modo "conteúdo de tela" do OpenH264 faz uma busca de
//!   movimento a mais que, sem as rotinas em assembly (precisam do NASM, que
//!   não usamos), deixa o frame 1080p 2,4× mais lento (36 contra 15 ms numa
//!   thread), com o mesmo PSNR (~44 dB).
//! - Frame dividido em até 8 fatias codificadas em paralelo: mais 2×
//!   (15 → 7,5 ms em 1080p numa CPU de 16 núcleos).
//! - Só H.264 (perfil Baseline, o que o OpenH264 gera), até 3840x2160.
//! - Keyframes só quando pedidos; cada um traz SPS/PPS.
//! - Sem intra-refresh: a recuperação de perda é sempre por keyframe.

use openh264::encoder::{
    BitRate, Complexity, Encoder as OpenH264, EncoderConfig as OpenH264Config, FrameRate, FrameType,
    IntraFramePeriod, RateControlMode, UsageType, VuiConfig,
};
use openh264::formats::YUVSource;
use openh264::OpenH264API;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, ID3D11VideoProcessorOutputView,
    D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};

use crate::convert::Nv12Converter;
use crate::platform::{check_size, win_err, Encoder};
use crate::{Backend, EncodeError, EncodedFrame, EncoderConfig, VideoCodec};

/// Maior vídeo que o OpenH264 aceita (nível 5.2).
const MAXIMO: (u32, u32) = (3840, 2160);

/// Fatias por frame (e threads), no máximo. Mais que isso pouco ajuda e cada
/// fatia custa um pouco de compressão.
const FATIAS: usize = 8;

pub(crate) struct SoftwareEncoder {
    encoder: OpenH264,
    converter: Nv12Converter,
    context: ID3D11DeviceContext,
    nv12_view: ID3D11VideoProcessorOutputView,
    nv12: ID3D11Texture2D,
    /// Cópia do NV12 que a CPU consegue ler.
    leitura: ID3D11Texture2D,
    /// O frame em I420 (planos separados), o formato que o OpenH264 lê.
    yuv: I420,
    input: (u32, u32),
    fps: u32,
    bitrate_kbps: u32,
    next_index: u64,
    /// Núcleos da CPU.
    threads: usize,
    name: String,
}

impl SoftwareEncoder {
    pub fn new(device: &ID3D11Device, cfg: &EncoderConfig) -> Result<Self, EncodeError> {
        let indisponivel = |reason: String| EncodeError::BackendUnavailable {
            backend: Backend::Software,
            reason,
        };
        if cfg.codec != VideoCodec::H264 {
            return Err(indisponivel(format!(
                "o encoder por software só faz H.264 (pedido: {})",
                cfg.codec.name()
            )));
        }
        let (w, h) = (cfg.width, cfg.height);
        if w.max(h) > MAXIMO.0 || w.min(h) > MAXIMO.1 {
            return Err(indisponivel(format!(
                "{w}x{h} acima do máximo do OpenH264 (3840x2160)"
            )));
        }

        let encoder = criar(cfg.bitrate_bps(), cfg.fps)?;

        let converter = Nv12Converter::new(device, cfg.input(), (w, h), cfg.fps)?;
        let (nv12, nv12_view) = converter.create_output(device, w, h)?;
        let leitura = textura_de_leitura(device, w, h)?;
        // SAFETY: GetImmediateContext não tem pré-condições.
        let context = unsafe { device.GetImmediateContext() }.map_err(win_err("GetImmediateContext"))?;
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        Ok(Self {
            encoder,
            converter,
            context,
            nv12_view,
            nv12,
            leitura,
            yuv: I420::new(w as usize, h as usize),
            input: cfg.input(),
            fps: cfg.fps,
            bitrate_kbps: cfg.bitrate_kbps,
            next_index: 0,
            threads,
            name: nome(threads.min(FATIAS)),
        })
    }

    /// Divide cada frame em [`FATIAS`] fatias horizontais fixas, codificadas
    /// em paralelo (uma thread por fatia). O wrapper do OpenH264 sempre
    /// configura uma fatia só, então mudamos os parâmetros pela API crua,
    /// que reinicializa o encoder por dentro. Falhou: segue com uma thread.
    fn fatiar(&mut self) {
        let n = self.threads.min(FATIAS);
        if n < 2 {
            return;
        }
        let mut p = openh264_sys2::SEncParamExt::default();
        // SAFETY: GetOption copia os parâmetros atuais para a struct do mesmo
        // tipo; SetOption lê dela. As duas vivem durante as chamadas.
        let ok = unsafe {
            let api = self.encoder.raw_api();
            api.get_option(
                openh264_sys2::ENCODER_OPTION_SVC_ENCODE_PARAM_EXT,
                (&raw mut p).cast(),
            ) == 0
                && {
                    p.iMultipleThreadIdc = n as u16;
                    let fatia = &mut p.sSpatialLayers[0].sSliceArgument;
                    fatia.uiSliceMode = openh264_sys2::SM_FIXEDSLCNUM_SLICE;
                    fatia.uiSliceNum = n as u32;
                    api.set_option(
                        openh264_sys2::ENCODER_OPTION_SVC_ENCODE_PARAM_EXT,
                        (&raw mut p).cast(),
                    ) == 0
                }
        };
        if ok {
            return;
        }
        self.name = nome(1);
    }

    /// Lê o NV12 da GPU para `self.yuv` (I420).
    fn baixar(&mut self) -> Result<(), EncodeError> {
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: as duas texturas têm o mesmo tamanho e formato; o Map
        // espera a GPU terminar a cópia e a conversão enfileiradas antes. A
        // leitura fica dentro de RowPitch × (altura × 3/2), o tamanho de um
        // NV12 mapeado, e o Unmap vem logo depois.
        unsafe {
            self.context.CopyResource(&self.leitura, &self.nv12);
            self.context
                .Map(&self.leitura, 0, D3D11_MAP_READ, 0, Some(&mut m))
                .map_err(win_err("Map (NV12 para a CPU)"))?;
            let pitch = m.RowPitch as usize;
            let dados = std::slice::from_raw_parts(m.pData as *const u8, pitch * self.yuv.h * 3 / 2);
            self.yuv.de_nv12(dados, pitch);
            self.context.Unmap(&self.leitura, 0);
        }
        Ok(())
    }
}

/// Nome para o usuário, com quantas fatias (threads) cada frame usa.
fn nome(fatias: usize) -> String {
    match fatias {
        1 => "OpenH264 por software (CPU, 1 thread)".into(),
        n => format!("OpenH264 por software (CPU, {n} fatias em paralelo)"),
    }
}

/// Um OpenH264 configurado para tela em tempo real.
fn criar(bitrate_bps: u32, fps: u32) -> Result<OpenH264, EncodeError> {
    let config = OpenH264Config::new()
        .usage_type(UsageType::CameraVideoRealTime)
        .rate_control_mode(RateControlMode::Bitrate)
        .bitrate(BitRate::from_bps(bitrate_bps))
        .max_frame_rate(FrameRate::from_hz(fps as f32))
        // Pular frames deixa o controle de taxa segurar o bitrate (sem isso o
        // OpenH264 avisa que não consegue); um frame pulado sai como nenhum.
        .skip_frames(true)
        .complexity(Complexity::Low)
        .intra_frame_period(IntraFramePeriod::from_num_frames(0))
        // Mesma cor da conversão em `convert.rs`: BT.709, faixa limitada.
        .vui(VuiConfig::bt709());
    OpenH264::with_api_config(OpenH264API::from_source(), config).map_err(|e| {
        EncodeError::BackendUnavailable {
            backend: Backend::Software,
            reason: format!("OpenH264: {e}"),
        }
    })
}

fn textura_de_leitura(device: &ID3D11Device, w: u32, h: u32) -> Result<ID3D11Texture2D, EncodeError> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut t = None;
    // SAFETY: descritor válido, sem dados iniciais.
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut t)) }
        .map_err(win_err("CreateTexture2D (leitura)"))?;
    t.ok_or_else(|| EncodeError::Windows {
        context: "CreateTexture2D (leitura)",
        hresult: 0,
        message: "a chamada não devolveu o objeto".into(),
    })
}

impl Encoder for SoftwareEncoder {
    fn backend(&self) -> Backend {
        Backend::Software
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn encode(
        &mut self,
        texture: &ID3D11Texture2D,
        force_keyframe: bool,
    ) -> Result<Vec<EncodedFrame>, EncodeError> {
        check_size(texture, self.input.0, self.input.1)?;
        self.converter.convert(texture, &self.nv12_view)?;
        self.baixar()?;

        let index = self.next_index;
        self.next_index += 1;
        if force_keyframe {
            self.encoder.force_intra_frame();
        }
        let saida = self
            .encoder
            .encode(&self.yuv)
            .map_err(|e| EncodeError::BackendUnavailable {
                backend: Backend::Software,
                reason: format!("OpenH264 encode: {e}"),
            })?;
        let tipo = saida.frame_type();
        let quadro = (tipo != FrameType::Skip).then(|| EncodedFrame {
            data: saida.to_vec(),
            keyframe: matches!(tipo, FrameType::IDR | FrameType::I),
            index,
            refresh_start: false,
            refresh_end: false,
        });
        // O OpenH264 só se inicializa no primeiro frame; depois dele dá para
        // ligar as fatias em várias threads (o próximo frame sai keyframe).
        if index == 0 {
            self.fatiar();
        }
        Ok(quadro.into_iter().collect())
    }

    fn set_bitrate(&mut self, kbps: u32) -> Result<(), EncodeError> {
        // O OpenH264 só se inicializa no primeiro frame: antes dele, um
        // encoder novo já nasce com o bitrate certo.
        if self.next_index == 0 {
            self.encoder = criar(kbps.saturating_mul(1000), self.fps)?;
            self.bitrate_kbps = kbps;
            return Ok(());
        }
        let mut info = openh264_sys2::SBitrateInfo {
            iLayer: openh264_sys2::SPATIAL_LAYER_ALL,
            iBitrate: kbps.saturating_mul(1000) as i32,
        };
        // SAFETY: opção documentada do OpenH264 com a struct que ela espera,
        // viva durante a chamada; o encoder já foi inicializado (houve encode).
        let st = unsafe {
            self.encoder
                .raw_api()
                .set_option(openh264_sys2::ENCODER_OPTION_BITRATE, (&raw mut info).cast())
        };
        if st != 0 {
            return Err(EncodeError::BackendUnavailable {
                backend: Backend::Software,
                reason: format!("OpenH264 não aceitou o bitrate novo (código {st})"),
            });
        }
        self.bitrate_kbps = kbps;
        Ok(())
    }

    fn bitrate_kbps(&self) -> u32 {
        self.bitrate_kbps
    }
}

/// Um frame em I420: plano Y inteiro e U, V em meia resolução, separados.
struct I420 {
    w: usize,
    h: usize,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

impl I420 {
    fn new(w: usize, h: usize) -> Self {
        let c = (w / 2) * (h / 2);
        Self {
            w,
            h,
            y: vec![0; w * h],
            u: vec![0; c],
            v: vec![0; c],
        }
    }

    /// Copia de um NV12 com `pitch` bytes por linha: o plano Y igual, e o
    /// UV intercalado (U V U V…) separado em dois planos.
    fn de_nv12(&mut self, nv12: &[u8], pitch: usize) {
        let (w, h) = (self.w, self.h);
        for (linha, destino) in self.y.chunks_exact_mut(w).enumerate() {
            destino.copy_from_slice(&nv12[linha * pitch..linha * pitch + w]);
        }
        let uv = &nv12[pitch * h..];
        let cw = w / 2;
        for linha in 0..h / 2 {
            let origem = &uv[linha * pitch..linha * pitch + w];
            let (u, v) = (
                &mut self.u[linha * cw..(linha + 1) * cw],
                &mut self.v[linha * cw..(linha + 1) * cw],
            );
            for (i, par) in origem.chunks_exact(2).enumerate() {
                u[i] = par[0];
                v[i] = par[1];
            }
        }
    }
}

impl YUVSource for I420 {
    fn dimensions(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    fn strides(&self) -> (usize, usize, usize) {
        (self.w, self.w / 2, self.w / 2)
    }

    fn y(&self) -> &[u8] {
        &self.y
    }

    fn u(&self) -> &[u8] {
        &self.u
    }

    fn v(&self) -> &[u8] {
        &self.v
    }
}

#[cfg(test)]
mod tests {
    use super::I420;

    #[test]
    fn nv12_vira_i420_respeitando_o_pitch() {
        // 4x2 com pitch 6: Y em 2 linhas, depois 1 linha de UV intercalado.
        #[rustfmt::skip]
        let nv12 = [
            1, 2, 3, 4, 0, 0,
            5, 6, 7, 8, 0, 0,
            10, 20, 11, 21, 0, 0,
        ];
        let mut f = I420::new(4, 2);
        f.de_nv12(&nv12, 6);
        assert_eq!(f.y, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(f.u, [10, 11]);
        assert_eq!(f.v, [20, 21]);
    }
}
