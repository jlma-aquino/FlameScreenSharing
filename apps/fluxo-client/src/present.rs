//! Leva o frame decodificado (NV12 na GPU) para a tela.
//!
//! - **Swapchain flip model** (`FLIP_DISCARD`), 2 buffers, no máximo 1 frame
//!   na fila do driver: o mínimo de atraso que o Windows permite em janela.
//! - Sem vsync, apresenta com **tearing** quando o sistema suporta: o frame
//!   aparece na hora, mesmo no meio de uma varredura do monitor.
//! - NV12 → BGRA, escala e barras pretas pelo **Video Processor** do D3D11,
//!   direto no back buffer (sem shaders nossos).

use std::collections::HashMap;
use std::mem::ManuallyDrop;

use windows::core::{Interface, BOOL};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11Texture2D, ID3D11VideoContext1, ID3D11VideoDevice, ID3D11VideoProcessor,
    ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
    D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_VIDEO_COLOR, D3D11_VIDEO_COLOR_0, D3D11_VIDEO_COLOR_RGBA,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
    DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN,
    DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIDevice1, IDXGIFactory2, IDXGIFactory5, IDXGISwapChain1,
    DXGI_FEATURE_PRESENT_ALLOW_TEARING, DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT, DXGI_PRESENT_ALLOW_TEARING,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

use fluxo_decode::DecodedFrame;

type Resultado<T> = windows::core::Result<T>;

/// Video Processor configurado para um par (tamanho do vídeo, tamanho da janela).
struct Processor {
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    video: (u32, u32),
    output: (u32, u32),
    /// Views de entrada por (textura, fatia): o decoder reaproveita um pool
    /// pequeno de superfícies, então o cache fica pequeno.
    inputs: HashMap<(usize, u32), ID3D11VideoProcessorInputView>,
}

pub struct Presenter {
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext1,
    swapchain: IDXGISwapChain1,
    tearing: bool,
    vsync: bool,
    size: (u32, u32),
    output_view: Option<ID3D11VideoProcessorOutputView>,
    processor: Option<Processor>,
}

impl Presenter {
    pub fn new(device: &ID3D11Device, hwnd: HWND, size: (u32, u32), vsync: bool) -> Resultado<Self> {
        // SAFETY: criação de objetos DXGI/D3D11 com descritores válidos.
        unsafe {
            let dxgi: IDXGIDevice = device.cast()?;
            let factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;
            let tearing = factory.cast::<IDXGIFactory5>().is_ok_and(|f5| {
                let mut allow = BOOL(0);
                f5.CheckFeatureSupport(
                    DXGI_FEATURE_PRESENT_ALLOW_TEARING,
                    &mut allow as *mut BOOL as *mut _,
                    std::mem::size_of::<BOOL>() as u32,
                )
                .is_ok()
                    && allow.as_bool()
            });
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: size.0,
                Height: size.1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: swapchain_flags(tearing).0 as u32,
                ..Default::default()
            };
            let swapchain = factory.CreateSwapChainForHwnd(device, hwnd, &desc, None, None)?;
            // Alt+Enter é nosso (tela cheia sem bordas), não o modo exclusivo do DXGI.
            factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
            // No máximo 1 frame esperando na fila do driver.
            let _ = device.cast::<IDXGIDevice1>()?.SetMaximumFrameLatency(1);

            let mut me = Self {
                video_device: device.cast()?,
                video_context: device.GetImmediateContext()?.cast()?,
                swapchain,
                tearing,
                vsync,
                size,
                output_view: None,
                processor: None,
            };
            me.create_output_view()?;
            Ok(me)
        }
    }

    /// `true` se os frames aparecem sem esperar a varredura do monitor.
    pub fn tearing(&self) -> bool {
        self.tearing && !self.vsync
    }

    /// A janela mudou de tamanho.
    pub fn resize(&mut self, size: (u32, u32)) -> Resultado<()> {
        if size == self.size {
            return Ok(());
        }
        // Todas as referências ao back buffer precisam sumir antes do resize.
        self.output_view = None;
        self.processor = None;
        // SAFETY: nenhuma view do back buffer existe neste ponto.
        unsafe {
            self.swapchain.ResizeBuffers(
                0,
                size.0,
                size.1,
                DXGI_FORMAT_UNKNOWN,
                swapchain_flags(self.tearing),
            )?;
        }
        self.size = size;
        self.create_output_view()
    }

    fn create_output_view(&mut self) -> Resultado<()> {
        // O enumerador da view precisa existir antes; um provisório basta
        // (a view vale para qualquer processor do mesmo device).
        let enumerator = self.enumerator((self.size.0, self.size.1), self.size)?;
        // SAFETY: no flip model do D3D11, o buffer 0 sempre é o back buffer
        // atual; a view criada sobre ele acompanha a troca de buffers.
        unsafe {
            let back: ID3D11Texture2D = self.swapchain.GetBuffer(0)?;
            let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                },
            };
            let mut view = None;
            self.video_device
                .CreateVideoProcessorOutputView(&back, &enumerator, &desc, Some(&mut view))?;
            self.output_view = view;
        }
        Ok(())
    }

    fn enumerator(&self, video: (u32, u32), output: (u32, u32)) -> Resultado<ID3D11VideoProcessorEnumerator> {
        let rate = DXGI_RATIONAL {
            Numerator: 60,
            Denominator: 1,
        };
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: video.0,
            InputHeight: video.1,
            OutputFrameRate: rate,
            OutputWidth: output.0,
            OutputHeight: output.1,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        // SAFETY: descritor válido.
        unsafe { self.video_device.CreateVideoProcessorEnumerator(&content) }
    }

    /// (Re)cria o Video Processor se o vídeo ou a janela mudaram de tamanho.
    fn processor_for(&mut self, video: (u32, u32)) -> Resultado<&mut Processor> {
        let ok = matches!(&self.processor, Some(p) if p.video == video && p.output == self.size);
        if !ok {
            let enumerator = self.enumerator(video, self.size)?;
            // SAFETY: configuração de um processor recém-criado.
            let processor = unsafe {
                let processor = self.video_device.CreateVideoProcessor(&enumerator, 0)?;
                let vc = &self.video_context;
                vc.VideoProcessorSetStreamFrameFormat(&processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
                vc.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
                // Entrada: o que o host gera (BT.709 limitado); saída: RGB da tela.
                vc.VideoProcessorSetStreamColorSpace1(
                    &processor,
                    0,
                    DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
                );
                vc.VideoProcessorSetOutputColorSpace1(&processor, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);
                let preto = D3D11_VIDEO_COLOR {
                    Anonymous: D3D11_VIDEO_COLOR_0 {
                        RGBA: D3D11_VIDEO_COLOR_RGBA {
                            R: 0.0,
                            G: 0.0,
                            B: 0.0,
                            A: 1.0,
                        },
                    },
                };
                vc.VideoProcessorSetOutputBackgroundColor(&processor, false, &preto);
                let tela = RECT {
                    left: 0,
                    top: 0,
                    right: self.size.0 as i32,
                    bottom: self.size.1 as i32,
                };
                let destino = letterbox(video, self.size);
                vc.VideoProcessorSetOutputTargetRect(&processor, true, Some(&tela));
                vc.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&destino));
                processor
            };
            self.processor = Some(Processor {
                enumerator,
                processor,
                video,
                output: self.size,
                inputs: HashMap::new(),
            });
        }
        Ok(self.processor.as_mut().expect("criado acima"))
    }

    /// Desenha o frame e apresenta.
    pub fn present(&mut self, frame: &DecodedFrame) -> Resultado<()> {
        let video = (frame.width(), frame.height());
        let video_device = self.video_device.clone();
        let video_context = self.video_context.clone();
        let output = self
            .output_view
            .clone()
            .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))?;
        let p = self.processor_for(video)?;

        let chave = (frame.texture().as_raw() as usize, frame.subresource());
        if !p.inputs.contains_key(&chave) {
            let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV {
                        MipSlice: 0,
                        ArraySlice: frame.subresource(),
                    },
                },
            };
            let mut view = None;
            // SAFETY: textura NV12 do decoder (ou nossa) e enumerador válidos.
            unsafe {
                video_device.CreateVideoProcessorInputView(
                    frame.texture(),
                    &p.enumerator,
                    &desc,
                    Some(&mut view),
                )?;
            }
            if let Some(v) = view {
                p.inputs.insert(chave, v);
            }
        }
        let input = p.inputs.get(&chave).cloned();
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(input),
            ..Default::default()
        };
        // SAFETY: `stream` vive até o fim da chamada; a referência COM em
        // ManuallyDrop é solta logo depois.
        let blt = unsafe {
            let r = video_context.VideoProcessorBlt(&p.processor, &output, 0, std::slice::from_ref(&stream));
            ManuallyDrop::drop(&mut stream.pInputSurface);
            r
        };
        blt?;

        let (sync, flags) = match (self.vsync, self.tearing) {
            (true, _) => (1, DXGI_PRESENT(0)),
            (false, true) => (0, DXGI_PRESENT_ALLOW_TEARING),
            (false, false) => (0, DXGI_PRESENT(0)),
        };
        // SAFETY: swapchain válida.
        unsafe { self.swapchain.Present(sync, flags) }.ok()
    }
}

fn swapchain_flags(tearing: bool) -> DXGI_SWAP_CHAIN_FLAG {
    if tearing {
        DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING
    } else {
        DXGI_SWAP_CHAIN_FLAG(0)
    }
}

/// Maior retângulo com a proporção do vídeo que cabe na janela, centralizado.
fn letterbox(video: (u32, u32), janela: (u32, u32)) -> RECT {
    let (vw, vh) = (video.0.max(1) as f64, video.1.max(1) as f64);
    let (jw, jh) = (janela.0 as f64, janela.1 as f64);
    let escala = (jw / vw).min(jh / vh);
    let (w, h) = ((vw * escala).round() as i32, (vh * escala).round() as i32);
    let (x, y) = ((janela.0 as i32 - w) / 2, (janela.1 as i32 - h) / 2);
    RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_16_9_em_janela_4_3_ganha_barras_em_cima_e_embaixo() {
        let r = letterbox((1920, 1080), (1024, 768));
        assert_eq!((r.left, r.right), (0, 1024));
        assert_eq!(r.bottom - r.top, 576);
        assert_eq!(r.top, (768 - 576) / 2);
    }

    #[test]
    fn video_do_mesmo_tamanho_ocupa_tudo() {
        let r = letterbox((1920, 1080), (1920, 1080));
        assert_eq!((r.left, r.top, r.right, r.bottom), (0, 0, 1920, 1080));
    }

    #[test]
    fn janela_larga_ganha_barras_nas_laterais() {
        let r = letterbox((1920, 1080), (2560, 1080));
        assert_eq!((r.top, r.bottom), (0, 1080));
        assert_eq!(r.left, (2560 - 1920) / 2);
    }
}
