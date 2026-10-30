//! Conversão BGRA → NV12 na GPU, com o Video Processor do D3D11.
//!
//! A captura entrega BGRA (4 bytes/pixel, RGB). Encoders H.264 trabalham em
//! YUV 4:2:0; o formato padrão de hardware para isso é o NV12 (plano Y
//! inteiro + plano UV intercalado em meia resolução). Fazemos a conversão
//! nós mesmos, igual para os dois backends, para que a cor seja a mesma
//! independente do encoder: **BT.709, faixa limitada (16–235)** — o que
//! decoders e players assumem por padrão em HD.
//!
//! Custo típico em 1080p: bem abaixo de 1 ms de GPU.

use std::mem::ManuallyDrop;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, ID3D11VideoContext1, ID3D11VideoDevice,
    ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
    ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_TEX2D_VPIV,
    D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
    D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709, DXGI_FORMAT,
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};

use crate::platform::win_err;
use crate::EncodeError;

/// Converte texturas BGRA de um tamanho fixo para NV12.
pub(crate) struct Nv12Converter {
    context: ID3D11DeviceContext,
    video_context: ID3D11VideoContext1,
    processor: ID3D11VideoProcessor,
    enumerator: ID3D11VideoProcessorEnumerator,
    video_device: ID3D11VideoDevice,
    /// Cópia nossa do frame BGRA: a textura da captura muda a cada frame e
    /// só vale enquanto o frame existir, então a view de entrada aponta
    /// para esta, criada uma vez.
    input: ID3D11Texture2D,
    input_view: ID3D11VideoProcessorInputView,
}

impl Nv12Converter {
    pub fn new(device: &ID3D11Device, width: u32, height: u32, fps: u32) -> Result<Self, EncodeError> {
        // SAFETY: todas as chamadas abaixo são criação de objetos D3D11 com
        // descritores válidos montados aqui; nenhum ponteiro sobrevive além
        // da chamada.
        unsafe {
            let video_device: ID3D11VideoDevice = device.cast().map_err(win_err(
                "ID3D11VideoDevice (o device precisa de D3D11_CREATE_DEVICE_VIDEO_SUPPORT)",
            ))?;
            let context = device
                .GetImmediateContext()
                .map_err(win_err("GetImmediateContext"))?;
            let video_context: ID3D11VideoContext1 = context
                .cast()
                .map_err(win_err("ID3D11VideoContext1 (requer Windows 10)"))?;

            let rate = DXGI_RATIONAL {
                Numerator: fps,
                Denominator: 1,
            };
            let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: width,
                InputHeight: height,
                OutputFrameRate: rate,
                OutputWidth: width,
                OutputHeight: height,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
            };
            let enumerator = video_device
                .CreateVideoProcessorEnumerator(&content)
                .map_err(win_err("CreateVideoProcessorEnumerator"))?;
            let processor = video_device
                .CreateVideoProcessor(&enumerator, 0)
                .map_err(win_err("CreateVideoProcessor"))?;

            // Sem "melhorias" automáticas do driver (nitidez, redução de ruído...).
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            video_context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            // Entrada: RGB da área de trabalho (0–255, sRGB ≈ gama 2.2, primárias 709).
            // Saída: YCbCr BT.709 faixa limitada — o que vai sinalizado no VUI do H.264.
            video_context.VideoProcessorSetStreamColorSpace1(
                &processor,
                0,
                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
            );
            video_context
                .VideoProcessorSetOutputColorSpace1(&processor, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);

            let input = create_texture(
                device,
                width,
                height,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                D3D11_BIND_RENDER_TARGET.0 as u32 | D3D11_BIND_SHADER_RESOURCE.0 as u32,
            )?;
            let view_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV {
                        MipSlice: 0,
                        ArraySlice: 0,
                    },
                },
            };
            let mut input_view = None;
            video_device
                .CreateVideoProcessorInputView(&input, &enumerator, &view_desc, Some(&mut input_view))
                .map_err(win_err("CreateVideoProcessorInputView"))?;
            let input_view = input_view.ok_or_else(|| missing("CreateVideoProcessorInputView"))?;

            Ok(Self {
                context,
                video_context,
                processor,
                enumerator,
                video_device,
                input,
                input_view,
            })
        }
    }

    /// Cria uma textura NV12 que pode receber a saída da conversão, e a
    /// view de saída correspondente.
    pub fn create_output(
        &self,
        device: &ID3D11Device,
        width: u32,
        height: u32,
    ) -> Result<(ID3D11Texture2D, ID3D11VideoProcessorOutputView), EncodeError> {
        let texture = create_texture(
            device,
            width,
            height,
            DXGI_FORMAT_NV12,
            D3D11_BIND_RENDER_TARGET.0 as u32,
        )?;
        let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut view = None;
        // SAFETY: textura e enumerador válidos; `view` recebe a view criada.
        unsafe {
            self.video_device
                .CreateVideoProcessorOutputView(&texture, &self.enumerator, &desc, Some(&mut view))
                .map_err(win_err("CreateVideoProcessorOutputView"))?;
        }
        Ok((
            texture,
            view.ok_or_else(|| missing("CreateVideoProcessorOutputView"))?,
        ))
    }

    /// Converte `bgra` (textura da captura) para a textura NV12 de `output`.
    /// O trabalho é enfileirado na GPU; quem consome a saída (encoder) usa
    /// o mesmo device, então a ordem é garantida pelo driver.
    pub fn convert(
        &self,
        bgra: &ID3D11Texture2D,
        output: &ID3D11VideoProcessorOutputView,
    ) -> Result<(), EncodeError> {
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(Some(self.input_view.clone())),
            ..Default::default()
        };
        // SAFETY: as texturas têm o mesmo tamanho e formato (verificado por
        // check_size antes), e `stream` vive até o fim da chamada.
        let resultado = unsafe {
            self.context.CopyResource(&self.input, bgra);
            self.video_context
                .VideoProcessorBlt(&self.processor, output, 0, std::slice::from_ref(&stream))
        };
        // A struct guarda a referência COM em ManuallyDrop: soltamos à mão.
        // SAFETY: o campo foi inicializado acima e não é usado depois.
        unsafe { ManuallyDrop::drop(&mut stream.pInputSurface) };
        resultado.map_err(win_err("VideoProcessorBlt"))
    }
}

fn create_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    bind_flags: u32,
) -> Result<ID3D11Texture2D, EncodeError> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind_flags,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut tex = None;
    // SAFETY: descritor válido, sem dados iniciais.
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut tex)) }.map_err(win_err("CreateTexture2D"))?;
    tex.ok_or_else(|| missing("CreateTexture2D"))
}

fn missing(context: &'static str) -> EncodeError {
    EncodeError::Windows {
        context,
        hresult: 0,
        message: "a chamada não devolveu o objeto".into(),
    }
}
