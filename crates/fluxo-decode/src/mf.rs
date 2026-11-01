//! Decoder de vídeo do Media Foundation (MFT) com saída em textura D3D11.
//!
//! H.264: o decoder da Microsoft, que vem em todo Windows. HEVC e AV1: o MFT
//! que estiver instalado (extensões da Microsoft Store), achado por
//! `MFTEnumEx`. Todos são síncronos e usam DXVA quando recebem o device.
//!
//! ## Dois caminhos
//!
//! - **Hardware (normal):** o MFT recebe um `IMFDXGIDeviceManager` com o
//!   nosso device e decodifica via DXVA. Cada frame sai como uma fatia de um
//!   *array* de texturas NV12 do próprio decoder — por isso [`DecodedFrame`]
//!   guarda a amostra: enquanto ela existir, o decoder não reutiliza a fatia.
//! - **Software (reserva):** sem DXVA (VM, driver genérico), o mesmo MFT
//!   decodifica na CPU; copiamos o NV12 para uma textura nossa. Mais lento,
//!   mas a imagem aparece.
//!
//! `MF_LOW_LATENCY` faz o MFT devolver cada frame assim que ele chega, sem
//! esperar frames futuros (não usamos B-frames).

use std::mem::ManuallyDrop;

use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, RPC_E_CHANGED_MODE};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D,
    D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG,
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

use crate::{DecodeError, VideoCodec};

/// Subtipo de entrada do Media Foundation para o codec.
fn subtipo(codec: VideoCodec) -> windows::core::GUID {
    match codec {
        VideoCodec::H264 => MFVideoFormat_H264,
        VideoCodec::Hevc => MFVideoFormat_HEVC,
        VideoCodec::Av1 => MFVideoFormat_AV1,
    }
}

fn win_err(context: &'static str) -> impl Fn(windows::core::Error) -> DecodeError {
    move |e| DecodeError::Windows {
        context,
        hresult: e.code().0,
        message: e.message().to_string(),
    }
}

/// Cria um device D3D11 na GPU padrão, pronto para decode e apresentação:
/// suporte a vídeo, BGRA e proteção multithread (o MFT usa o device de
/// threads próprias).
pub fn create_device() -> Result<ID3D11Device, DecodeError> {
    let com_video = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
    let device = create_device_with_flags(com_video)
        .or_else(|_| create_device_with_flags(D3D11_CREATE_DEVICE_BGRA_SUPPORT))?;
    // SAFETY: cast e chamada simples sobre um device recém-criado.
    unsafe {
        let mt: ID3D11Multithread = device.cast().map_err(win_err("ID3D11Multithread"))?;
        let _ = mt.SetMultithreadProtected(true);
    }
    Ok(device)
}

fn create_device_with_flags(flags: D3D11_CREATE_DEVICE_FLAG) -> Result<ID3D11Device, DecodeError> {
    let levels: [D3D_FEATURE_LEVEL; 2] = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    let mut device = None;
    // SAFETY: parâmetros válidos; o device é devolvido em `device`.
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            flags,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
        .map_err(win_err("D3D11CreateDevice"))?;
    }
    device.ok_or_else(|| missing("D3D11CreateDevice"))
}

/// Um frame decodificado, na GPU, em NV12.
///
/// Solte-o depois de desenhar: enquanto existir, a superfície correspondente
/// do decoder fica reservada.
pub struct DecodedFrame {
    _sample: Option<IMFSample>,
    texture: ID3D11Texture2D,
    subresource: u32,
    width: u32,
    height: u32,
}

impl DecodedFrame {
    /// Textura NV12 (pode ser um *array*: use [`Self::subresource`]).
    pub fn texture(&self) -> &ID3D11Texture2D {
        &self.texture
    }

    /// Índice da fatia do array onde está este frame.
    pub fn subresource(&self) -> u32 {
        self.subresource
    }

    /// Largura visível em pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Altura visível em pixels (1080, mesmo que a textura tenha 1088 linhas).
    pub fn height(&self) -> u32 {
        self.height
    }
}

/// Decoder (H.264/HEVC em Annex B, AV1 em OBUs) → texturas NV12.
///
/// Não é `Send`: use na thread que vai desenhar.
pub struct VideoDecoder {
    mft: IMFTransform,
    codec: VideoCodec,
    name: String,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    /// Mantido vivo enquanto o MFT usa o device.
    _manager: Option<IMFDXGIDeviceManager>,
    hardware: bool,
    provides_samples: bool,
    out_size: u32,
    width: u32,
    height: u32,
    /// Linhas reais do plano Y no buffer de saída por software (1088).
    plane_height: u32,
    stride: u32,
    /// Caminho por software: textura NV12 nossa, recriada se o tamanho mudar.
    upload: Option<(ID3D11Texture2D, u32, u32)>,
}

impl VideoDecoder {
    /// Decoder H.264 sobre `device` (de preferência criado por
    /// [`create_device`]). O tamanho do vídeo é descoberto no primeiro keyframe.
    pub fn new(device: &ID3D11Device) -> Result<Self, DecodeError> {
        Self::with_codec(device, VideoCodec::H264)
    }

    /// Decoder de `codec`. HEVC e AV1 falham se o Windows não tiver o
    /// decoder instalado (ver [`decodable_codecs`]).
    pub fn with_codec(device: &ID3D11Device, codec: VideoCodec) -> Result<Self, DecodeError> {
        // SAFETY: sequência padrão de configuração de um MFT síncrono.
        unsafe {
            // COM pode já estar iniciado na thread com outro modelo: tudo bem.
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            if hr.is_err() && hr != RPC_E_CHANGED_MODE {
                return Err(win_err("CoInitializeEx")(hr.into()));
            }
            MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(win_err("MFStartup"))?;

            let (mft, nome): (IMFTransform, String) = if codec == VideoCodec::H264 {
                let mft = CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)
                    .map_err(win_err("criar o decoder H.264 da Microsoft"))?;
                (mft, "H.264 Microsoft".into())
            } else {
                let activate = decoders_sincronos(&subtipo(codec))
                    .into_iter()
                    .next()
                    .ok_or_else(|| DecodeError::Windows {
                        context: "procurar o decoder",
                        hresult: MF_E_TOPO_CODEC_NOT_FOUND.0,
                        message: format!(
                            "nenhum decoder {} instalado (extensão da Microsoft Store)",
                            codec.name()
                        ),
                    })?;
                let nome = nome_do_mft(&activate);
                let mft = activate
                    .ActivateObject()
                    .map_err(win_err("IMFActivate::ActivateObject"))?;
                (mft, format!("{} {nome}", codec.name()))
            };
            let attrs = mft
                .GetAttributes()
                .map_err(win_err("IMFTransform::GetAttributes"))?;
            let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);

            // Tenta o caminho de hardware; se o driver recusar, segue por software.
            let manager = Self::attach_device(&mft, device).ok();
            let hardware = manager.is_some();

            let t = MFCreateMediaType().map_err(win_err("MFCreateMediaType"))?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(win_err("MF_MT_MAJOR_TYPE"))?;
            t.SetGUID(&MF_MT_SUBTYPE, &subtipo(codec))
                .map_err(win_err("MF_MT_SUBTYPE"))?;
            mft.SetInputType(0, &t, 0).map_err(win_err("SetInputType"))?;

            let mut me = Self {
                mft,
                codec,
                name: if hardware {
                    format!("{nome}, acelerado pela GPU (DXVA)")
                } else {
                    format!("{nome} por software (sem DXVA neste driver)")
                },
                device: device.clone(),
                context: device
                    .GetImmediateContext()
                    .map_err(win_err("GetImmediateContext"))?,
                _manager: manager,
                hardware,
                provides_samples: false,
                out_size: 0,
                width: 0,
                height: 0,
                plane_height: 0,
                stride: 0,
                upload: None,
            };
            // Sem o tamanho, o tipo de saída pode só existir depois do
            // primeiro keyframe (vem como STREAM_CHANGE). Não é erro.
            let _ = me.choose_output();
            me.mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(win_err("NOTIFY_BEGIN_STREAMING"))?;
            Ok(me)
        }
    }

    /// Liga o MFT ao device para decodificar na GPU.
    unsafe fn attach_device(
        mft: &IMFTransform,
        device: &ID3D11Device,
    ) -> windows::core::Result<IMFDXGIDeviceManager> {
        let mut token = 0u32;
        let mut manager = None;
        MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
        let manager =
            manager.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))?;
        manager.ResetDevice(device, token)?;
        mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)?;
        Ok(manager)
    }

    /// `true` se a decodificação roda na GPU (DXVA).
    pub fn is_hardware(&self) -> bool {
        self.hardware
    }

    /// Descrição para o usuário.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// O codec que este decoder entende.
    pub fn codec(&self) -> VideoCodec {
        self.codec
    }

    /// Entrega um frame completo (como chega do `fluxo-net`) e devolve o
    /// frame decodificado mais recente, se houver. `Ok(None)` no começo do
    /// vídeo, antes do primeiro keyframe.
    pub fn decode(&mut self, bitstream: &[u8]) -> Result<Option<DecodedFrame>, DecodeError> {
        let sample = memory_sample(bitstream)?;
        // SAFETY: entrega da amostra ao MFT, que copia o que precisar.
        let mut entregue = unsafe { self.mft.ProcessInput(0, &sample, 0) };
        let mut newest = None;
        if matches!(&entregue, Err(e) if e.code() == MF_E_NOTACCEPTING) {
            // O MFT quer esvaziar a saída antes de aceitar mais.
            newest = self.drain()?;
            // SAFETY: mesma entrega, agora que há espaço.
            entregue = unsafe { self.mft.ProcessInput(0, &sample, 0) };
        }
        entregue.map_err(win_err("IMFTransform::ProcessInput"))?;
        Ok(self.drain()?.or(newest))
    }

    /// Retira tudo o que o MFT tem pronto; devolve o mais novo.
    fn drain(&mut self) -> Result<Option<DecodedFrame>, DecodeError> {
        let mut newest = None;
        loop {
            let allocated = if self.provides_samples {
                None
            } else {
                Some(memory_sample_empty(self.out_size.max(1))?)
            };
            let mut buf = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(allocated),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            // SAFETY: buffer montado acima; depois tomamos posse das
            // referências COM deixadas nele.
            let (resultado, sample) = unsafe {
                let r = self.mft.ProcessOutput(0, &mut buf, &mut status);
                let s = ManuallyDrop::take(&mut buf[0].pSample);
                let _ = ManuallyDrop::take(&mut buf[0].pEvents);
                (r, s)
            };
            match resultado {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(newest),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    self.choose_output()?;
                    continue;
                }
                Err(e) => return Err(win_err("IMFTransform::ProcessOutput")(e)),
            }
            if let Some(s) = sample {
                newest = Some(self.frame_from_sample(s)?);
            }
        }
    }

    /// Escolhe a saída NV12 e lê a geometria do vídeo.
    fn choose_output(&mut self) -> Result<(), DecodeError> {
        // SAFETY: enumeração e escolha de tipo de saída do MFT.
        unsafe {
            for i in 0.. {
                let t = self
                    .mft
                    .GetOutputAvailableType(0, i)
                    .map_err(win_err("GetOutputAvailableType"))?;
                if t.GetGUID(&MF_MT_SUBTYPE).ok() != Some(MFVideoFormat_NV12) {
                    continue;
                }
                self.mft
                    .SetOutputType(0, &t, 0)
                    .map_err(win_err("SetOutputType (NV12)"))?;
                let size = t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
                let (fw, fh) = ((size >> 32) as u32, size as u32);
                // Área visível (1080 linhas dentro de um quadro de 1088).
                let mut area = [0u8; 16];
                let (vw, vh) = match t.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut area, None) {
                    Ok(()) => (
                        i32::from_ne_bytes([area[8], area[9], area[10], area[11]]) as u32,
                        i32::from_ne_bytes([area[12], area[13], area[14], area[15]]) as u32,
                    ),
                    Err(_) => (fw, fh),
                };
                (self.width, self.height) = (vw, vh);
                self.plane_height = fh.max(vh);
                self.stride = t.GetUINT32(&MF_MT_DEFAULT_STRIDE).unwrap_or(fw);
                let info = self
                    .mft
                    .GetOutputStreamInfo(0)
                    .map_err(win_err("GetOutputStreamInfo"))?;
                let provides = MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32;
                self.provides_samples = info.dwFlags & provides != 0;
                self.out_size = info.cbSize;
                return Ok(());
            }
        }
        unreachable!("o laço só termina por return ou erro")
    }

    /// Converte uma amostra de saída em [`DecodedFrame`].
    fn frame_from_sample(&mut self, sample: IMFSample) -> Result<DecodedFrame, DecodeError> {
        // SAFETY: leitura do buffer da amostra; no caminho de hardware a
        // textura vem com referência própria (GetResource soma uma).
        unsafe {
            let buffer = sample.GetBufferByIndex(0).map_err(win_err("GetBufferByIndex"))?;
            if let Ok(dxgi) = buffer.cast::<IMFDXGIBuffer>() {
                let mut raw = std::ptr::null_mut();
                dxgi.GetResource(&ID3D11Texture2D::IID, &mut raw)
                    .map_err(win_err("IMFDXGIBuffer::GetResource"))?;
                let texture = ID3D11Texture2D::from_raw(raw);
                let subresource = dxgi
                    .GetSubresourceIndex()
                    .map_err(win_err("GetSubresourceIndex"))?;
                return Ok(DecodedFrame {
                    _sample: Some(sample),
                    texture,
                    subresource,
                    width: self.width,
                    height: self.height,
                });
            }
            // Caminho por software: NV12 na RAM → nossa textura.
            let texture = self.upload_texture()?;
            let buffer = sample
                .ConvertToContiguousBuffer()
                .map_err(win_err("ConvertToContiguousBuffer"))?;
            let mut p = std::ptr::null_mut();
            let mut len = 0u32;
            buffer
                .Lock(&mut p, None, Some(&mut len))
                .map_err(win_err("IMFMediaBuffer::Lock"))?;
            let nv12 = std::slice::from_raw_parts(p, len as usize);
            let (stride, h, ph) = (
                self.stride as usize,
                self.height as usize,
                self.plane_height as usize,
            );
            let resultado = if nv12.len() < stride * (ph + h / 2) {
                Err(missing("buffer NV12 menor que o esperado"))
            } else {
                // A textura espera o plano UV logo após `h` linhas de Y; o
                // decoder o põe após `ph` (alinhado a 16). Reagrupamos.
                let mut packed = Vec::with_capacity(stride * (h + h / 2));
                packed.extend_from_slice(&nv12[..stride * h]);
                packed.extend_from_slice(&nv12[stride * ph..stride * (ph + h / 2)]);
                self.context
                    .UpdateSubresource(&texture, 0, None, packed.as_ptr().cast(), stride as u32, 0);
                Ok(())
            };
            let _ = buffer.Unlock();
            resultado?;
            Ok(DecodedFrame {
                _sample: None,
                texture,
                subresource: 0,
                width: self.width,
                height: self.height,
            })
        }
    }

    fn upload_texture(&mut self) -> Result<ID3D11Texture2D, DecodeError> {
        if let Some((t, w, h)) = &self.upload {
            if (*w, *h) == (self.width, self.height) {
                return Ok(t.clone());
            }
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: self.width,
            Height: self.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut tex = None;
        // SAFETY: descritor válido, sem dados iniciais.
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex)) }
            .map_err(win_err("CreateTexture2D (NV12)"))?;
        let tex = tex.ok_or_else(|| missing("CreateTexture2D"))?;
        self.upload = Some((tex.clone(), self.width, self.height));
        Ok(tex)
    }
}

/// Amostra MF com uma cópia de `data`.
fn memory_sample(data: &[u8]) -> Result<IMFSample, DecodeError> {
    let sample = memory_sample_empty(data.len().max(1) as u32)?;
    // SAFETY: o buffer tem pelo menos `data.len()` bytes; Lock/Unlock pareados.
    unsafe {
        let buffer = sample.GetBufferByIndex(0).map_err(win_err("GetBufferByIndex"))?;
        let mut p = std::ptr::null_mut();
        buffer
            .Lock(&mut p, None, None)
            .map_err(win_err("IMFMediaBuffer::Lock"))?;
        std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        buffer.Unlock().map_err(win_err("IMFMediaBuffer::Unlock"))?;
        buffer
            .SetCurrentLength(data.len() as u32)
            .map_err(win_err("SetCurrentLength"))?;
    }
    Ok(sample)
}

fn memory_sample_empty(size: u32) -> Result<IMFSample, DecodeError> {
    // SAFETY: criação de objetos MF novos.
    unsafe {
        let sample = MFCreateSample().map_err(win_err("MFCreateSample"))?;
        let buffer = MFCreateMemoryBuffer(size).map_err(win_err("MFCreateMemoryBuffer"))?;
        sample
            .AddBuffer(&buffer)
            .map_err(win_err("IMFSample::AddBuffer"))?;
        Ok(sample)
    }
}

fn missing(context: &'static str) -> DecodeError {
    DecodeError::Windows {
        context,
        hresult: 0,
        message: "a chamada não devolveu o objeto".into(),
    }
}

/// Decoders de vídeo registrados no Media Foundation para cada codec
/// (`"H.264"`, `"HEVC"`, `"AV1"`): nome e se é de hardware. HEVC e AV1 só
/// aparecem com as extensões da Microsoft Store ("HEVC Video Extensions",
/// "AV1 Video Extension").
pub fn available_decoders() -> Vec<(&'static str, Vec<(String, bool)>)> {
    iniciar_mf();
    VideoCodec::PREFERENCE
        .into_iter()
        .rev()
        .map(|codec| {
            let lista = [true, false]
                .into_iter()
                .flat_map(|hw| {
                    enumerar(&subtipo(codec), hw)
                        .iter()
                        .map(move |a| (nome_do_mft(a), hw))
                        .collect::<Vec<_>>()
                })
                .collect();
            (codec.name(), lista)
        })
        .collect()
}

/// Codecs que [`VideoDecoder::with_codec`] consegue abrir nesta máquina,
/// como máscara de [`VideoCodec::bit`] (o que o cliente anuncia ao host).
/// H.264 sempre; HEVC e AV1 se houver um decoder síncrono instalado (os de
/// hardware assíncronos não servem para o nosso laço de decode).
pub fn decodable_codecs() -> u8 {
    iniciar_mf();
    VideoCodec::PREFERENCE
        .into_iter()
        .filter(|&c| c == VideoCodec::H264 || !decoders_sincronos(&subtipo(c)).is_empty())
        .fold(0, |m, c| m | c.bit())
}

fn iniciar_mf() {
    // SAFETY: inicialização idempotente do COM e do MF nesta thread.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);
    }
}

/// Decoders síncronos de `subtipo`, o melhor primeiro.
fn decoders_sincronos(subtipo: &windows::core::GUID) -> Vec<IMFActivate> {
    enumerar(subtipo, false)
}

/// Decoders de `subtipo` (de hardware, assíncronos, ou os síncronos).
fn enumerar(subtipo: &windows::core::GUID, hardware: bool) -> Vec<IMFActivate> {
    let entrada = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: *subtipo,
    };
    let flags = match hardware {
        true => MFT_ENUM_FLAG_HARDWARE,
        false => MFT_ENUM_FLAG_SYNCMFT,
    } | MFT_ENUM_FLAG_LOCALMFT
        | MFT_ENUM_FLAG_SORTANDFILTER;
    let mut array: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: o MF aloca o array com CoTaskMemAlloc; cada elemento é tomado
    // e o array liberado.
    unsafe {
        if MFTEnumEx(
            MFT_CATEGORY_VIDEO_DECODER,
            flags,
            Some(&entrada),
            None,
            &mut array,
            &mut count,
        )
        .is_err()
        {
            return Vec::new();
        }
        let lista = (0..count as usize)
            .filter_map(|i| (*array.add(i)).take())
            .collect();
        if !array.is_null() {
            windows::Win32::System::Com::CoTaskMemFree(Some(array as *const _));
        }
        lista
    }
}

/// Nome amigável do MFT.
fn nome_do_mft(a: &IMFActivate) -> String {
    let mut p = windows::core::PWSTR::null();
    let mut n = 0u32;
    // SAFETY: GetAllocatedString aloca com CoTaskMemAlloc; liberamos após copiar.
    unsafe {
        match a.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut p, &mut n) {
            Ok(()) => {
                let s = p.to_string().unwrap_or_default();
                windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
                s
            }
            Err(_) => "(sem nome)".to_string(),
        }
    }
}
