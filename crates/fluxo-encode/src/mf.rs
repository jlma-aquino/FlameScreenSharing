//! Backend Media Foundation: o encoder (H.264, HEVC ou AV1) de hardware que o fabricante da
//! GPU instala no Windows (AMD AMF, Intel QuickSync, NVIDIA NVENC), acessado
//! como um *MFT* (Media Foundation Transform).
//!
//! ## Particularidades
//!
//! - MFTs de hardware são **assíncronos**: avisam por eventos quando aceitam
//!   entrada (`METransformNeedInput`) e quando há saída
//!   (`METransformHaveOutput`). [`MfEncoder::encode`] traduz isso para a
//!   chamada síncrona da trait: entrega um frame e espera até sair o
//!   resultado ou o encoder pedir o próximo.
//! - Com `CODECAPI_AVLowLatencyMode` o encoder devolve 1 frame para cada
//!   frame de entrada, sem fila.
//! - Na mesma placa pode haver mais de um MFT (ex.: notebook com duas GPUs);
//!   escolhemos o da placa do device pelo LUID do adaptador.
//! - Propriedades de `ICodecAPI` variam por fabricante: as que o MFT recusar
//!   são ignoradas (o encoder segue com o padrão dele).

use std::mem::ManuallyDrop;
use std::sync::OnceLock;

use windows::core::{Interface, GUID};
use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, VARIANT_TRUE};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11Multithread, ID3D11Texture2D, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_UI4};

use crate::convert::Nv12Converter;
use crate::platform::{adapter_info, check_size, win_err, Encoder};
use crate::{Backend, EncodeError, EncodedFrame, EncoderConfig, VideoCodec};

/// Subtipo de saída do MFT e perfil (`MF_MT_MPEG2_PROFILE`) para o codec.
fn subtipo(codec: VideoCodec) -> (GUID, u32) {
    match codec {
        VideoCodec::H264 => (MFVideoFormat_H264, eAVEncH264VProfile_High.0 as u32),
        VideoCodec::Hevc => (MFVideoFormat_HEVC, eAVEncH265VProfile_Main_420_8.0 as u32),
        VideoCodec::Av1 => (MFVideoFormat_AV1, eAVEncAV1VProfile_Main_420_8.0 as u32),
    }
}

/// Quantas texturas NV12 alternamos. O MFT pode segurar a amostra de
/// entrada um pouco depois de pedir a próxima; 4 dá folga de sobra.
const POOL: usize = 4;

/// Unidades de tempo do Media Foundation: 100 ns.
const MF_SECOND: u64 = 10_000_000;

pub(crate) struct MfEncoder {
    activate: IMFActivate,
    transform: IMFTransform,
    events: IMFMediaEventGenerator,
    codec: Option<ICodecAPI>,
    /// Mantido vivo enquanto o MFT usa o device.
    _manager: IMFDXGIDeviceManager,
    converter: Nv12Converter,
    pool: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    input_id: u32,
    output_id: u32,
    /// O MFT aloca as amostras de saída (comum em hardware).
    provides_samples: bool,
    output_size: u32,
    /// Pedidos de entrada (`NeedInput`) recebidos e ainda não atendidos.
    need_input: u32,
    /// Frames entregues ao MFT que ainda não saíram.
    in_flight: u32,
    /// Tamanho das texturas de entrada (da captura).
    input: (u32, u32),
    fps: u32,
    bitrate_kbps: u32,
    next_input: u64,
    next_output: u64,
    name: String,
}

impl MfEncoder {
    pub fn new(device: &ID3D11Device, cfg: &EncoderConfig) -> Result<Self, EncodeError> {
        start_media_foundation()?;
        let adapter = adapter_info(device)?;

        // SAFETY: sequência padrão de configuração de um MFT de hardware;
        // todos os objetos são criados aqui e guardados em `Self`.
        unsafe {
            // O MFT usa o device em threads próprias: o device precisa de
            // proteção multithread (afeta só este device, o da captura).
            let mt: ID3D11Multithread = device.cast().map_err(win_err("ID3D11Multithread"))?;
            let _ = mt.SetMultithreadProtected(true);

            let (activate, mft_name) = find_hardware_encoder(adapter.luid, cfg.codec)?;
            let transform: IMFTransform = activate
                .ActivateObject()
                .map_err(win_err("IMFActivate::ActivateObject"))?;

            let attrs = transform
                .GetAttributes()
                .map_err(win_err("IMFTransform::GetAttributes"))?;
            if attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) == 0 {
                return Err(unavailable(format!(
                    "{mft_name} não é assíncrono (não suportado)"
                )));
            }
            attrs
                .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                .map_err(win_err("MF_TRANSFORM_ASYNC_UNLOCK"))?;
            if attrs.GetUINT32(&MF_SA_D3D11_AWARE).unwrap_or(0) == 0 {
                return Err(unavailable(format!("{mft_name} não aceita texturas D3D11")));
            }

            // Entrega o device da captura ao MFT: as texturas não saem da GPU.
            let mut token = 0u32;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)
                .map_err(win_err("MFCreateDXGIDeviceManager"))?;
            let manager = manager.ok_or_else(|| unavailable("MFCreateDXGIDeviceManager vazio".into()))?;
            manager
                .ResetDevice(device, token)
                .map_err(win_err("IMFDXGIDeviceManager::ResetDevice"))?;
            transform
                .ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)
                .map_err(win_err("MFT_MESSAGE_SET_D3D_MANAGER"))?;

            // Muitos MFTs têm IDs fixos e respondem E_NOTIMPL: aí são 0 e 0.
            let (mut ins, mut outs) = ([0u32], [0u32]);
            let _ = transform.GetStreamIDs(&mut ins, &mut outs);
            let (input_id, output_id) = (ins[0], outs[0]);

            // Propriedades de baixa latência — antes dos tipos de mídia, como
            // alguns fabricantes exigem.
            let codec: Option<ICodecAPI> = transform.cast().ok();
            if let Some(c) = &codec {
                let _ = c.SetValue(&CODECAPI_AVLowLatencyMode, &variant_bool(true));
                let _ = c.SetValue(
                    &CODECAPI_AVEncCommonRateControlMode,
                    &variant_u32(eAVEncCommonRateControlMode_CBR.0 as u32),
                );
                let _ = c.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &variant_u32(cfg.bitrate_bps()));
                // VBV de um frame, como no NVENC: limita o tamanho dos keyframes.
                let um_frame = cfg.bitrate_bps() / cfg.fps;
                let _ = c.SetValue(&CODECAPI_AVEncCommonMaxBitRate, &variant_u32(cfg.bitrate_bps()));
                let _ = c.SetValue(&CODECAPI_AVEncCommonBufferSize, &variant_u32(um_frame));
                let _ = c.SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &variant_u32(0));
                // Sem GOP infinito padronizado no MF: um keyframe por minuto,
                // os demais só quando pedidos.
                let _ = c.SetValue(
                    &CODECAPI_AVEncMPVGOPSize,
                    &variant_u32(cfg.fps.saturating_mul(60)),
                );
            }

            // Tipo de saída primeiro (regra dos encoders de hardware), depois o de entrada.
            let (formato, perfil) = subtipo(cfg.codec);
            let out_type = media_type(&formato, cfg)?;
            out_type
                .SetUINT32(&MF_MT_AVG_BITRATE, cfg.bitrate_bps())
                .map_err(win_err("MF_MT_AVG_BITRATE"))?;
            out_type
                .SetUINT32(&MF_MT_MPEG2_PROFILE, perfil)
                .map_err(win_err("MF_MT_MPEG2_PROFILE"))?;
            transform
                .SetOutputType(output_id, &out_type, 0)
                .map_err(win_err("IMFTransform::SetOutputType"))?;
            let in_type = media_type(&MFVideoFormat_NV12, cfg)?;
            transform
                .SetInputType(input_id, &in_type, 0)
                .map_err(win_err("IMFTransform::SetInputType (NV12)"))?;

            let info = transform
                .GetOutputStreamInfo(output_id)
                .map_err(win_err("GetOutputStreamInfo"))?;
            let provides =
                MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32;

            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(win_err("MFT_MESSAGE_NOTIFY_BEGIN_STREAMING"))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(win_err("MFT_MESSAGE_NOTIFY_START_OF_STREAM"))?;
            let events: IMFMediaEventGenerator =
                transform.cast().map_err(win_err("IMFMediaEventGenerator"))?;

            let converter = Nv12Converter::new(device, cfg.input(), (cfg.width, cfg.height), cfg.fps)?;
            let pool = (0..POOL)
                .map(|_| converter.create_output(device, cfg.width, cfg.height))
                .collect::<Result<Vec<_>, _>>()?;

            Ok(Self {
                activate,
                transform,
                events,
                codec,
                _manager: manager,
                converter,
                pool,
                input_id,
                output_id,
                provides_samples: info.dwFlags & provides != 0,
                output_size: info.cbSize.max(1024 * 1024),
                need_input: 0,
                in_flight: 0,
                input: cfg.input(),
                fps: cfg.fps,
                bitrate_kbps: cfg.bitrate_kbps,
                next_input: 0,
                next_output: 0,
                name: format!("Media Foundation: {mft_name} ({})", adapter.name),
            })
        }
    }

    /// Espera o próximo evento do MFT e reage a ele. Devolve o tipo do evento.
    fn wait_event(&mut self, out: &mut Vec<EncodedFrame>) -> Result<i32, EncodeError> {
        // SAFETY: GetEvent bloqueante sobre o gerador deste MFT.
        let event = unsafe { self.events.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0)) }
            .map_err(win_err("IMFMediaEventGenerator::GetEvent"))?;
        // SAFETY: leitura de propriedades do evento recebido.
        let (kind, status) = unsafe {
            (
                event.GetType().map_err(win_err("IMFMediaEvent::GetType"))? as i32,
                event.GetStatus().map_err(win_err("IMFMediaEvent::GetStatus"))?,
            )
        };
        status.ok().map_err(win_err("evento do MFT com erro"))?;
        if kind == METransformNeedInput.0 {
            self.need_input += 1;
        } else if kind == METransformHaveOutput.0 {
            self.pull_output(out)?;
        }
        Ok(kind)
    }

    /// Retira um frame pronto do MFT.
    fn pull_output(&mut self, out: &mut Vec<EncodedFrame>) -> Result<(), EncodeError> {
        let sample = if self.provides_samples {
            None
        } else {
            Some(new_sample(self.output_size)?)
        };
        let mut buf = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: self.output_id,
            pSample: ManuallyDrop::new(sample),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        }];
        let mut status = 0u32;
        // SAFETY: buffer de saída montado acima; depois da chamada tomamos
        // posse das referências COM que o MFT deixou nele.
        let resultado = unsafe { self.transform.ProcessOutput(0, &mut buf, &mut status) };
        let (sample, _events) = unsafe {
            (
                ManuallyDrop::take(&mut buf[0].pSample),
                ManuallyDrop::take(&mut buf[0].pEvents),
            )
        };

        match resultado {
            Ok(()) => {}
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                // O encoder ajustou o formato de saída: aceitamos o que ele
                // propõe. O frame virá num próximo HaveOutput.
                // SAFETY: renegociação padrão do tipo de saída.
                unsafe {
                    let t = self
                        .transform
                        .GetOutputAvailableType(self.output_id, 0)
                        .map_err(win_err("GetOutputAvailableType"))?;
                    self.transform
                        .SetOutputType(self.output_id, &t, 0)
                        .map_err(win_err("SetOutputType (renegociação)"))?;
                }
                return Ok(());
            }
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
            Err(e) => return Err(win_err("IMFTransform::ProcessOutput")(e)),
        }

        let sample = sample.ok_or_else(|| unavailable("ProcessOutput sem amostra".into()))?;
        // SAFETY: Lock/Unlock pareados; copiamos `len` bytes válidos.
        let data = unsafe {
            let buffer = sample
                .ConvertToContiguousBuffer()
                .map_err(win_err("ConvertToContiguousBuffer"))?;
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            buffer
                .Lock(&mut ptr, None, Some(&mut len))
                .map_err(win_err("IMFMediaBuffer::Lock"))?;
            let data = std::slice::from_raw_parts(ptr, len as usize).to_vec();
            let _ = buffer.Unlock();
            data
        };
        // SAFETY: leitura de atributo da amostra.
        let keyframe = unsafe { sample.GetUINT32(&MFSampleExtension_CleanPoint) }.unwrap_or(0) != 0;

        self.in_flight = self.in_flight.saturating_sub(1);
        out.push(EncodedFrame {
            data,
            keyframe,
            index: self.next_output,
            refresh_start: false,
            refresh_end: false,
        });
        self.next_output += 1;
        Ok(())
    }
}

impl Encoder for MfEncoder {
    fn backend(&self) -> Backend {
        Backend::MediaFoundation
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn set_bitrate(&mut self, kbps: u32) -> Result<(), EncodeError> {
        let Some(c) = &self.codec else {
            return Err(unavailable(
                "este MFT não aceita mudar o bitrate (sem ICodecAPI)".into(),
            ));
        };
        let bps = kbps.saturating_mul(1000);
        // SAFETY: propriedades dinâmicas do ICodecAPI; valem do próximo frame em diante.
        unsafe {
            c.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &variant_u32(bps))
                .map_err(win_err("CODECAPI_AVEncCommonMeanBitRate"))?;
            // Opcionais: alguns fabricantes recusam mudar estes em tempo real.
            let _ = c.SetValue(&CODECAPI_AVEncCommonMaxBitRate, &variant_u32(bps));
            let _ = c.SetValue(
                &CODECAPI_AVEncCommonBufferSize,
                &variant_u32(bps / self.fps.max(1)),
            );
        }
        self.bitrate_kbps = kbps;
        Ok(())
    }

    fn bitrate_kbps(&self) -> u32 {
        self.bitrate_kbps
    }

    fn encode(
        &mut self,
        texture: &ID3D11Texture2D,
        force_keyframe: bool,
    ) -> Result<Vec<EncodedFrame>, EncodeError> {
        check_size(texture, self.input.0, self.input.1)?;
        let mut out = Vec::new();

        // 1. Só podemos entregar entrada depois que o MFT pedir.
        while self.need_input == 0 {
            self.wait_event(&mut out)?;
        }

        // 2. Converte para uma textura livre do pool e entrega.
        let index = self.next_input;
        let (tex, view) = &self.pool[(index % POOL as u64) as usize];
        self.converter.convert(texture, view)?;
        let duration = MF_SECOND / self.fps as u64;
        // SAFETY: criação da amostra que embrulha a textura NV12 (sem cópia)
        // e entrega ao MFT, que a segura até terminar de ler.
        unsafe {
            let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, tex, 0, false)
                .map_err(win_err("MFCreateDXGISurfaceBuffer"))?;
            if let Ok(b2) = buffer.cast::<IMF2DBuffer>() {
                if let Ok(len) = b2.GetContiguousLength() {
                    let _ = buffer.SetCurrentLength(len);
                }
            }
            let sample = MFCreateSample().map_err(win_err("MFCreateSample"))?;
            sample
                .AddBuffer(&buffer)
                .map_err(win_err("IMFSample::AddBuffer"))?;
            let _ = sample.SetSampleTime((index * duration) as i64);
            let _ = sample.SetSampleDuration(duration as i64);

            if force_keyframe {
                if let Some(c) = &self.codec {
                    let _ = c.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &variant_u32(1));
                }
            }
            self.transform
                .ProcessInput(self.input_id, &sample, 0)
                .map_err(win_err("IMFTransform::ProcessInput"))?;
        }
        self.need_input -= 1;
        self.in_flight += 1;
        self.next_input += 1;

        // 3. Espera a saída deste frame — ou o MFT pedir mais entrada antes
        //    (aí o frame sai numa chamada seguinte, sem travar aqui).
        while self.in_flight > 0 {
            if self.wait_event(&mut out)? == METransformNeedInput.0 {
                break;
            }
        }
        Ok(out)
    }
}

impl Drop for MfEncoder {
    fn drop(&mut self) {
        // SAFETY: encerramento padrão; erros não têm tratamento útil aqui.
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            // MFTs assíncronos só liberam suas threads com ShutdownObject.
            let _ = self.activate.ShutdownObject();
        }
    }
}

/// Inicializa COM nesta thread e o Media Foundation (uma vez por processo).
fn start_media_foundation() -> Result<(), EncodeError> {
    // SAFETY: CoInitializeEx é por thread e pode ser chamado de novo; se a
    // thread já usa outro modelo (RPC_E_CHANGED_MODE), COM já está pronto.
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_err() && hr != RPC_E_CHANGED_MODE {
        return Err(win_err("CoInitializeEx")(hr.into()));
    }
    static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
    // SAFETY: MFStartup é seguro de chamar uma vez; nunca chamamos
    // MFShutdown (o processo encerra tudo ao sair).
    STARTED
        .get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) }.map_err(|e| e.message().to_string()))
        .clone()
        .map_err(|m| unavailable(format!("MFStartup falhou: {m}")))
}

/// Procura o encoder de hardware deste codec na placa com este LUID.
fn find_hardware_encoder(
    luid: windows::Win32::Foundation::LUID,
    codec: VideoCodec,
) -> Result<(IMFActivate, String), EncodeError> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: subtipo(codec).0,
    };
    let nome_codec = codec.name();
    let mut array: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: o MF aloca o array com CoTaskMemAlloc; tomamos posse de cada
    // elemento e liberamos o array em seguida.
    let all: Vec<IMFActivate> = unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            Some(&output),
            &mut array,
            &mut count,
        )
        .map_err(win_err("MFTEnumEx"))?;
        let v = (0..count as usize)
            .filter_map(|i| (*array.add(i)).take())
            .collect();
        if !array.is_null() {
            CoTaskMemFree(Some(array as *const _));
        }
        v
    };
    if all.is_empty() {
        return Err(unavailable(format!(
            "nenhum encoder {nome_codec} de hardware instalado (driver de vídeo? GPU antiga?)"
        )));
    }

    let mut want = [0u8; 8];
    want[..4].copy_from_slice(&luid.LowPart.to_ne_bytes());
    want[4..].copy_from_slice(&luid.HighPart.to_ne_bytes());
    // Preferimos o MFT da mesma placa; um sem LUID declarado serve de reserva.
    let mut reserva = None;
    for activate in all {
        let mut blob = [0u8; 8];
        // SAFETY: GetBlob escreve no máximo 8 bytes (tamanho de um LUID).
        match unsafe { activate.GetBlob(&MFT_ENUM_ADAPTER_LUID, &mut blob, None) } {
            Ok(()) if blob == want => return Ok(named(activate)),
            Ok(()) => {}
            Err(_) if reserva.is_none() => reserva = Some(activate),
            Err(_) => {}
        }
    }
    reserva.map(named).ok_or_else(|| {
        unavailable(format!(
            "a placa do monitor não tem encoder {nome_codec} de hardware"
        ))
    })
}

/// Junta o activate com o nome amigável do MFT.
fn named(activate: IMFActivate) -> (IMFActivate, String) {
    let mut pwstr = windows::core::PWSTR::null();
    let mut len = 0u32;
    // SAFETY: GetAllocatedString aloca com CoTaskMemAlloc; liberamos após copiar.
    let nome = unsafe {
        match activate.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut pwstr, &mut len) {
            Ok(()) => {
                let s = pwstr.to_string().unwrap_or_default();
                CoTaskMemFree(Some(pwstr.0 as *const _));
                s
            }
            Err(_) => "encoder sem nome".into(),
        }
    };
    (activate, nome)
}

/// Tipo de mídia de vídeo com tamanho, taxa e cor (BT.709 limitado).
fn media_type(subtype: &GUID, cfg: &EncoderConfig) -> Result<IMFMediaType, EncodeError> {
    // SAFETY: criação e preenchimento de atributos de um objeto novo.
    unsafe {
        let t = MFCreateMediaType().map_err(win_err("MFCreateMediaType"))?;
        let set = |k: &GUID, v: u32| t.SetUINT32(k, v).map_err(win_err("IMFMediaType::SetUINT32"));
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(win_err("MF_MT_MAJOR_TYPE"))?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype)
            .map_err(win_err("MF_MT_SUBTYPE"))?;
        // Pares (a, b) vão empacotados num u64 como (a << 32) | b.
        t.SetUINT64(&MF_MT_FRAME_SIZE, (cfg.width as u64) << 32 | cfg.height as u64)
            .map_err(win_err("MF_MT_FRAME_SIZE"))?;
        t.SetUINT64(&MF_MT_FRAME_RATE, (cfg.fps as u64) << 32 | 1)
            .map_err(win_err("MF_MT_FRAME_RATE"))?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, 1u64 << 32 | 1)
            .map_err(win_err("MF_MT_PIXEL_ASPECT_RATIO"))?;
        set(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        set(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
        set(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
        set(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
        set(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        Ok(t)
    }
}

/// Amostra com um buffer de memória, para MFTs que não alocam a saída.
fn new_sample(size: u32) -> Result<IMFSample, EncodeError> {
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

fn variant_u32(v: u32) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: preenche a variante VT_UI4 de uma VARIANT zerada.
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_UI4;
        (*var.Anonymous.Anonymous).Anonymous.ulVal = v;
    }
    var
}

fn variant_bool(v: bool) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: preenche a variante VT_BOOL de uma VARIANT zerada.
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_BOOL;
        (*var.Anonymous.Anonymous).Anonymous.boolVal = if v { VARIANT_TRUE } else { Default::default() };
    }
    var
}

fn unavailable(reason: String) -> EncodeError {
    EncodeError::BackendUnavailable {
        backend: Backend::MediaFoundation,
        reason,
    }
}
