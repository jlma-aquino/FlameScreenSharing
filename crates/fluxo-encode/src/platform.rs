//! Parte Windows comum aos backends: a trait [`Encoder`], a abertura com
//! fallback ([`open_encoder`]) e a identificação da GPU.

use windows::core::Interface;
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;

use crate::{
    backend_order, mf, nvenc, Backend, BackendChoice, EncodeError, EncodedFrame, EncoderConfig, GpuVendor,
};

/// Um encoder H.264 aberto.
///
/// Não é `Send`: use na mesma thread em que foi criado, a mesma da captura
/// (o encoder usa o device D3D11 do capturador).
pub trait Encoder {
    /// Qual backend está rodando.
    fn backend(&self) -> Backend;

    /// Descrição para mostrar ao usuário, ex.: `NVENC (NVIDIA GeForce RTX 5050), preset P4`.
    fn name(&self) -> &str;

    /// Comprime um frame.
    ///
    /// `texture` é a textura BGRA do capturador (mesmo device); pode ser
    /// solta logo depois desta chamada, pois o encoder copia o que precisa.
    /// `force_keyframe` pede um IDR (ex.: cliente novo ou perda de pacotes).
    ///
    /// Normalmente devolve exatamente um frame. Pode devolver zero se o
    /// encoder ainda está processando (o frame sai numa chamada seguinte).
    fn encode(
        &mut self,
        texture: &ID3D11Texture2D,
        force_keyframe: bool,
    ) -> Result<Vec<EncodedFrame>, EncodeError>;

    /// Muda o bitrate alvo (kbit/s) a partir do próximo frame, sem keyframe
    /// e sem recriar o encoder — para o bitrate adaptativo.
    fn set_bitrate(&mut self, kbps: u32) -> Result<(), EncodeError>;

    /// Bitrate alvo atual, em kbit/s.
    fn bitrate_kbps(&self) -> u32;

    /// Começa uma onda de intra-refresh no próximo [`encode`](Self::encode):
    /// ao longo de [`EncoderConfig::intra_refresh_frames`] frames, cada faixa
    /// da imagem é redesenhada sem depender de frames anteriores. Os frames
    /// da onda saem com `refresh_start` / `refresh_end` marcados.
    ///
    /// Devolve `false` se o backend não tem intra-refresh (ou está
    /// desligado): aí, para recuperar o decoder, use `force_keyframe`.
    fn start_intra_refresh(&mut self) -> bool {
        false
    }
}

/// Resultado de [`open_encoder`].
pub struct OpenedEncoder {
    /// O encoder pronto para uso.
    pub encoder: Box<dyn Encoder>,
    /// Backends tentados antes e que falharam, com o motivo. Vazio quando o
    /// primeiro da lista funcionou. Mostre ao usuário: explica, por exemplo,
    /// por que uma placa NVIDIA acabou usando Media Foundation.
    pub skipped: Vec<(Backend, String)>,
}

/// Abre um encoder para o `device` do capturador, seguindo `cfg.backend`.
///
/// Em [`BackendChoice::Auto`], tenta os backends na ordem de
/// [`backend_order`](crate::backend_order) e devolve o primeiro que funcionar.
pub fn open_encoder(device: &ID3D11Device, cfg: &EncoderConfig) -> Result<OpenedEncoder, EncodeError> {
    cfg.validate()?;
    let vendor = gpu_vendor(device)?;
    let mut skipped = Vec::new();
    for backend in backend_order(cfg.backend, vendor) {
        match probe_backend(device, cfg, backend) {
            Ok(encoder) => return Ok(OpenedEncoder { encoder, skipped }),
            Err(e) => skipped.push((backend, e.to_string())),
        }
    }
    match cfg.backend {
        // Pedido explícito: devolve o erro daquele backend, sem embrulho.
        BackendChoice::Only(backend) => Err(EncodeError::BackendUnavailable {
            backend,
            reason: skipped.pop().map(|(_, m)| m).unwrap_or_default(),
        }),
        BackendChoice::Auto => Err(EncodeError::NoBackend(skipped)),
    }
}

/// Tenta abrir exatamente este backend, ignorando `cfg.backend`.
///
/// Útil para diagnóstico (`fluxo-bench encoders`) e comparações.
pub fn probe_backend(
    device: &ID3D11Device,
    cfg: &EncoderConfig,
    backend: Backend,
) -> Result<Box<dyn Encoder>, EncodeError> {
    cfg.validate()?;
    Ok(match backend {
        Backend::Nvenc => Box::new(nvenc::NvencEncoder::new(device, cfg)?),
        Backend::MediaFoundation => Box::new(mf::MfEncoder::new(device, cfg)?),
    })
}

/// Fabricante da placa de vídeo dona do `device`.
pub fn gpu_vendor(device: &ID3D11Device) -> Result<GpuVendor, EncodeError> {
    Ok(GpuVendor::from_pci_id(adapter_info(device)?.vendor_id))
}

/// Dados da placa de vídeo de um device.
pub(crate) struct AdapterInfo {
    pub name: String,
    pub vendor_id: u32,
    pub luid: LUID,
}

pub(crate) fn adapter_info(device: &ID3D11Device) -> Result<AdapterInfo, EncodeError> {
    // SAFETY: chamadas COM simples sobre um device válido; GetDesc só
    // preenche a struct devolvida.
    let desc = unsafe {
        let dxgi: IDXGIDevice = device.cast().map_err(win_err("IDXGIDevice"))?;
        let adapter = dxgi.GetAdapter().map_err(win_err("GetAdapter"))?;
        adapter.GetDesc().map_err(win_err("IDXGIAdapter::GetDesc"))?
    };
    let end = desc
        .Description
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(desc.Description.len());
    Ok(AdapterInfo {
        name: String::from_utf16_lossy(&desc.Description[..end]),
        vendor_id: desc.VendorId,
        luid: desc.AdapterLuid,
    })
}

/// Converte um erro do crate `windows` em [`EncodeError::Windows`],
/// anotando qual chamada falhou.
pub(crate) fn win_err(context: &'static str) -> impl Fn(windows::core::Error) -> EncodeError {
    move |e| EncodeError::Windows {
        context,
        hresult: e.code().0,
        message: e.message().to_string(),
    }
}

/// Confere se a textura tem o tamanho com que o encoder foi criado.
pub(crate) fn check_size(texture: &ID3D11Texture2D, width: u32, height: u32) -> Result<(), EncodeError> {
    let mut desc = Default::default();
    // SAFETY: GetDesc só escreve na struct passada.
    unsafe { texture.GetDesc(&mut desc) };
    if (desc.Width, desc.Height) != (width, height) {
        return Err(EncodeError::SizeMismatch {
            expected: (width, height),
            got: (desc.Width, desc.Height),
        });
    }
    Ok(())
}
