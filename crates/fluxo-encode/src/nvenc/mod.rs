//! Backend NVENC: encoder de hardware das placas NVIDIA, direto pela API do
//! driver (versão 12.1, driver 531+).
//!
//! ## Fluxo de um frame
//!
//! ```text
//! textura BGRA ─► Nv12Converter ─► textura NV12 (registrada no NVENC)
//!                                     │ nvEncMapInputResource
//!                                     ▼
//!                               nvEncEncodePicture (síncrono)
//!                                     │ nvEncLockBitstream
//!                                     ▼
//!                               bytes H.264 ─► EncodedFrame
//! ```
//!
//! ## Configuração (otimizada para streaming de baixa latência)
//!
//! - Preset P4 com *tuning* "ultra low latency" (equilíbrio qualidade/tempo).
//! - Perfil High, sem B-frames (`frameIntervalP = 1`).
//! - GOP infinito: keyframes só quando pedidos (`force_keyframe`); cada
//!   keyframe repete SPS/PPS.
//! - CBR com VBV de **um frame** (`bitrate / fps`): nenhum frame fica muito
//!   maior que a média, então nenhum demora a mais para atravessar a rede.
//! - VUI sinaliza BT.709 faixa limitada, igual à conversão em `convert.rs`.

mod api;
// Gerado por nvenc/gerar_layout.ps1; fica como o gerador escreveu.
#[rustfmt::skip]
mod layout;

use std::ffi::c_void;
use std::ptr::null_mut;

use windows::core::{Interface, GUID};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D, ID3D11VideoProcessorOutputView};

use self::api::{Api, Blob};
use self::layout::*;
use crate::convert::Nv12Converter;
use crate::platform::{adapter_info, check_size, Encoder};
use crate::{Backend, EncodeError, EncodedFrame, EncoderConfig};

/// Preset de qualidade: P1 (mais rápido) … P7 (melhor qualidade).
const PRESET: GUID = PRESET_P4_GUID;
const PRESET_NAME: &str = "P4";

pub(crate) struct NvencEncoder {
    api: Api,
    /// Sessão do encoder. Nulo só durante a construção.
    enc: *mut c_void,
    registered: *mut c_void,
    bitstream: *mut c_void,
    converter: Nv12Converter,
    nv12: ID3D11Texture2D,
    nv12_view: ID3D11VideoProcessorOutputView,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_kbps: u32,
    next_index: u64,
    name: String,
    /// Configuração e parâmetros de inicialização, guardados para mudar o
    /// bitrate depois (`nvEncReconfigureEncoder` recebe os dois de novo).
    /// Em Box: `init` aponta para `config`, que não pode mudar de endereço.
    config: Box<Blob<CONFIG_SIZE>>,
    init: Box<Blob<INITIALIZE_PARAMS_SIZE>>,
}

impl NvencEncoder {
    pub fn new(device: &ID3D11Device, cfg: &EncoderConfig) -> Result<Self, EncodeError> {
        let api = Api::new()?;
        let adapter = adapter_info(device)?;
        let converter = Nv12Converter::new(device, cfg.width, cfg.height, cfg.fps)?;
        let (nv12, nv12_view) = converter.create_output(device, cfg.width, cfg.height)?;

        // Montamos a struct já com os handles nulos: se qualquer passo de
        // `init` falhar, o Drop libera o que chegou a ser criado.
        let mut me = Self {
            api,
            enc: null_mut(),
            registered: null_mut(),
            bitstream: null_mut(),
            converter,
            nv12,
            nv12_view,
            width: cfg.width,
            height: cfg.height,
            fps: cfg.fps,
            bitrate_kbps: cfg.bitrate_kbps,
            next_index: 0,
            name: format!(
                "NVENC ({}), preset {PRESET_NAME}, ultra baixa latência",
                adapter.name
            ),
            config: Blob::new(),
            init: Blob::new(),
        };
        me.init(device, cfg)?;
        Ok(me)
    }

    fn init(&mut self, device: &ID3D11Device, cfg: &EncoderConfig) -> Result<(), EncodeError> {
        let api = &self.api;

        // 1. Sessão no device da captura (as texturas ficam na mesma GPU).
        let mut open = Blob::<OPEN_SESSION_SIZE>::new();
        open.set_u32(OS_VERSION, OPEN_SESSION_VER);
        open.set_u32(OS_DEVICE_TYPE, DEVICE_TYPE_DIRECTX);
        open.set_ptr(OS_DEVICE, device.as_raw());
        open.set_u32(OS_API_VERSION, NVENCAPI_VERSION);
        let mut enc = null_mut();
        // SAFETY: struct com versão e device válidos; `enc` recebe a sessão.
        let st = unsafe { api.open_session(open.as_mut_ptr(), &mut enc) };
        if st != SUCCESS {
            // A documentação pede para destruir a sessão mesmo se a abertura falhar.
            if !enc.is_null() {
                // SAFETY: handle devolvido pelo próprio driver.
                unsafe { api.destroy_encoder(enc) };
            }
            return api.check(null_mut(), "nvEncOpenEncodeSessionEx", st);
        }
        self.enc = enc;

        // 2. A GPU codifica H.264? (Todas as NVENC até hoje sim, mas é barato conferir.)
        // SAFETY: sessão aberta.
        if !unsafe { api.encode_guids(enc)? }.contains(&CODEC_H264_GUID) {
            return Err(EncodeError::BackendUnavailable {
                backend: Backend::Nvenc,
                reason: "esta GPU não codifica H.264".into(),
            });
        }

        // 3. Parte da configuração recomendada pelo driver para o preset.
        let mut preset = Blob::<PRESET_CONFIG_SIZE>::new();
        preset.set_u32(PC_VERSION, PRESET_CONFIG_VER);
        preset.set_u32(PC_PRESET_CFG + CFG_VERSION, CONFIG_VER);
        // SAFETY: sessão aberta, struct com as duas versões preenchidas.
        let st = unsafe {
            api.preset_config(
                enc,
                CODEC_H264_GUID,
                PRESET,
                TUNING_ULTRA_LOW_LATENCY,
                preset.as_mut_ptr(),
            )
        };
        api.check(enc, "nvEncGetEncodePresetConfigEx", st)?;

        let config = &mut self.config;
        config
            .bytes_mut()
            .copy_from_slice(&preset.bytes()[PC_PRESET_CFG..PC_PRESET_CFG + CONFIG_SIZE]);
        config.set_u32(CFG_VERSION, CONFIG_VER);
        config.set_guid(CFG_PROFILE_GUID, &H264_PROFILE_HIGH_GUID);
        config.set_u32(CFG_GOP_LENGTH, INFINITE_GOPLENGTH);
        config.set_u32(CFG_FRAME_INTERVAL_P, 1);
        config.set_u32(CFG_RC_MODE, RC_CBR);
        set_rate(config, cfg.bitrate_bps(), cfg.fps);
        config.set_u32(CFG_H264_IDR_PERIOD, INFINITE_GOPLENGTH);
        config.set_bit(CFG_H264_REPEAT_SPSPPS, true);
        config.set_u32(CFG_H264_VUI_SIGNAL_TYPE_PRESENT, 1);
        config.set_u32(CFG_H264_VUI_VIDEO_FORMAT, VUI_VIDEO_FORMAT_UNSPECIFIED);
        config.set_u32(CFG_H264_VUI_FULL_RANGE, 0);
        config.set_u32(CFG_H264_VUI_COLOUR_DESC_PRESENT, 1);
        config.set_u32(CFG_H264_VUI_COLOUR_PRIMARIES, VUI_COLOR_PRIMARIES_BT709);
        config.set_u32(CFG_H264_VUI_TRANSFER, VUI_TRANSFER_CHARACTERISTIC_BT709);
        config.set_u32(CFG_H264_VUI_COLOUR_MATRIX, VUI_MATRIX_COEFFS_BT709);

        // 4. Inicializa.
        let init = &mut self.init;
        init.set_u32(IP_VERSION, INITIALIZE_PARAMS_VER);
        init.set_guid(IP_ENCODE_GUID, &CODEC_H264_GUID);
        init.set_guid(IP_PRESET_GUID, &PRESET);
        init.set_u32(IP_ENCODE_WIDTH, cfg.width);
        init.set_u32(IP_ENCODE_HEIGHT, cfg.height);
        init.set_u32(IP_DAR_WIDTH, cfg.width);
        init.set_u32(IP_DAR_HEIGHT, cfg.height);
        init.set_u32(IP_FRAME_RATE_NUM, cfg.fps);
        init.set_u32(IP_FRAME_RATE_DEN, 1);
        init.set_u32(IP_ENABLE_ENCODE_ASYNC, 0);
        init.set_u32(IP_ENABLE_PTD, 1); // o driver decide o tipo de cada frame
        init.set_ptr(IP_ENCODE_CONFIG, config.as_mut_ptr());
        init.set_u32(IP_MAX_ENCODE_WIDTH, cfg.width);
        init.set_u32(IP_MAX_ENCODE_HEIGHT, cfg.height);
        init.set_u32(IP_TUNING_INFO, TUNING_ULTRA_LOW_LATENCY);
        // SAFETY: `config` fica guardado no encoder (Box), vivo enquanto ele existir.
        let st = unsafe { api.call(FL_INITIALIZE_ENCODER, enc, init.as_mut_ptr()) };
        api.check(enc, "nvEncInitializeEncoder", st)?;

        // 5. Registra a textura NV12 de entrada (uma só: o encode é síncrono).
        let mut reg = Blob::<REGISTER_RESOURCE_SIZE>::new();
        reg.set_u32(RR_VERSION, REGISTER_RESOURCE_VER);
        reg.set_u32(RR_RESOURCE_TYPE, INPUT_RESOURCE_TYPE_DIRECTX);
        reg.set_u32(RR_WIDTH, cfg.width);
        reg.set_u32(RR_HEIGHT, cfg.height);
        reg.set_u32(RR_PITCH, 0);
        reg.set_u32(RR_SUBRESOURCE_INDEX, 0);
        reg.set_ptr(RR_RESOURCE_TO_REGISTER, self.nv12.as_raw());
        reg.set_u32(RR_BUFFER_FORMAT, BUFFER_FORMAT_NV12);
        reg.set_u32(RR_BUFFER_USAGE, INPUT_IMAGE);
        // SAFETY: a textura é mantida viva em `self.nv12` enquanto registrada.
        let st = unsafe { api.call(FL_REGISTER_RESOURCE, enc, reg.as_mut_ptr()) };
        api.check(enc, "nvEncRegisterResource", st)?;
        self.registered = reg.get_ptr(RR_REGISTERED_RESOURCE);

        // 6. Buffer de saída do bitstream.
        let mut bs = Blob::<CREATE_BITSTREAM_BUFFER_SIZE>::new();
        bs.set_u32(CB_VERSION, CREATE_BITSTREAM_BUFFER_VER);
        // SAFETY: sessão inicializada, struct com versão.
        let st = unsafe { api.call(FL_CREATE_BITSTREAM_BUFFER, enc, bs.as_mut_ptr()) };
        api.check(enc, "nvEncCreateBitstreamBuffer", st)?;
        self.bitstream = bs.get_ptr(CB_BITSTREAM_BUFFER);
        Ok(())
    }

    /// Codifica a textura NV12 já mapeada.
    fn encode_mapped(
        &mut self,
        input: *mut c_void,
        format: u32,
        force_keyframe: bool,
    ) -> Result<Vec<EncodedFrame>, EncodeError> {
        let index = self.next_index;
        self.next_index += 1;
        let (api, enc) = (&self.api, self.enc);

        let mut pic = Blob::<PIC_PARAMS_SIZE>::new();
        pic.set_u32(PP_VERSION, PIC_PARAMS_VER);
        pic.set_u32(PP_INPUT_WIDTH, self.width);
        pic.set_u32(PP_INPUT_HEIGHT, self.height);
        pic.set_u32(PP_INPUT_PITCH, self.width);
        let flags = if force_keyframe {
            PIC_FLAG_FORCEIDR | PIC_FLAG_OUTPUT_SPSPPS
        } else {
            0
        };
        pic.set_u32(PP_ENCODE_PIC_FLAGS, flags);
        pic.set_u32(PP_FRAME_IDX, index as u32);
        pic.set_u64(PP_INPUT_TIMESTAMP, index);
        pic.set_ptr(PP_INPUT_BUFFER, input);
        pic.set_ptr(PP_OUTPUT_BITSTREAM, self.bitstream);
        pic.set_u32(PP_BUFFER_FMT, format);
        pic.set_u32(PP_PICTURE_STRUCT, PIC_STRUCT_FRAME);
        // SAFETY: entrada mapeada e bitstream pertencem a esta sessão.
        let st = unsafe { api.call(FL_ENCODE_PICTURE, enc, pic.as_mut_ptr()) };
        if st == ERR_NEED_MORE_INPUT {
            return Ok(Vec::new()); // só com B-frames/lookahead, que não usamos
        }
        api.check(enc, "nvEncEncodePicture", st)?;

        let mut lock = Blob::<LOCK_BITSTREAM_SIZE>::new();
        lock.set_u32(LB_VERSION, LOCK_BITSTREAM_VER);
        lock.set_bit(LB_DO_NOT_WAIT, false); // bloqueia até o frame ficar pronto
        lock.set_ptr(LB_OUTPUT_BITSTREAM, self.bitstream);
        // SAFETY: bitstream desta sessão, com um encode pendente.
        let st = unsafe { api.call(FL_LOCK_BITSTREAM, enc, lock.as_mut_ptr()) };
        api.check(enc, "nvEncLockBitstream", st)?;

        let size = lock.get_u32(LB_BITSTREAM_SIZE) as usize;
        let ptr = lock.get_ptr(LB_BITSTREAM_PTR) as *const u8;
        let data = if ptr.is_null() {
            Vec::new()
        } else {
            // SAFETY: enquanto travado, o driver garante `size` bytes em `ptr`.
            unsafe { std::slice::from_raw_parts(ptr, size) }.to_vec()
        };
        let pic_type = lock.get_u32(LB_PICTURE_TYPE);

        // SAFETY: destrava o mesmo bitstream travado acima.
        let st = unsafe { api.call(FL_UNLOCK_BITSTREAM, enc, self.bitstream) };
        api.check(enc, "nvEncUnlockBitstream", st)?;

        Ok(vec![EncodedFrame {
            data,
            keyframe: pic_type == PIC_TYPE_IDR || pic_type == PIC_TYPE_I,
            index,
        }])
    }
}

/// Controle de taxa CBR com VBV de **um frame** (`bitrate / fps`): nenhum
/// frame sai muito maior que a média, então nenhum demora a mais na rede.
fn set_rate(config: &mut Blob<CONFIG_SIZE>, bitrate_bps: u32, fps: u32) {
    let um_frame = bitrate_bps / fps.max(1);
    config.set_u32(CFG_RC_AVERAGE_BITRATE, bitrate_bps);
    config.set_u32(CFG_RC_MAX_BITRATE, bitrate_bps);
    config.set_u32(CFG_RC_VBV_BUFFER_SIZE, um_frame);
    config.set_u32(CFG_RC_VBV_INITIAL_DELAY, um_frame);
}

impl Encoder for NvencEncoder {
    fn backend(&self) -> Backend {
        Backend::Nvenc
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn set_bitrate(&mut self, kbps: u32) -> Result<(), EncodeError> {
        set_rate(&mut self.config, kbps.saturating_mul(1000), self.fps);
        // Reconfiguração sem reset nem IDR: só o controle de taxa muda.
        let mut rp = Blob::<RECONFIGURE_PARAMS_SIZE>::new();
        rp.set_u32(RP_VERSION, RECONFIGURE_PARAMS_VER);
        rp.bytes_mut()[RP_REINIT_PARAMS..RP_REINIT_PARAMS + INITIALIZE_PARAMS_SIZE]
            .copy_from_slice(self.init.bytes());
        // SAFETY: sessão aberta; os parâmetros copiados apontam para
        // `self.config`, que vive (em Box) enquanto o encoder existir.
        let st = unsafe { self.api.call(FL_RECONFIGURE_ENCODER, self.enc, rp.as_mut_ptr()) };
        self.api.check(self.enc, "nvEncReconfigureEncoder", st)?;
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
        check_size(texture, self.width, self.height)?;
        self.converter.convert(texture, &self.nv12_view)?;

        let enc = self.enc;
        let mut map = Blob::<MAP_INPUT_RESOURCE_SIZE>::new();
        map.set_u32(MI_VERSION, MAP_INPUT_RESOURCE_VER);
        map.set_ptr(MI_REGISTERED_RESOURCE, self.registered);
        // SAFETY: recurso registrado nesta sessão. O mapeamento também
        // sincroniza com a conversão enfileirada acima no mesmo device.
        let st = unsafe { self.api.call(FL_MAP_INPUT_RESOURCE, enc, map.as_mut_ptr()) };
        self.api.check(enc, "nvEncMapInputResource", st)?;
        let mapped = map.get_ptr(MI_MAPPED_RESOURCE);

        let resultado = self.encode_mapped(mapped, map.get_u32(MI_MAPPED_BUFFER_FMT), force_keyframe);

        // Desmapeia sempre, mesmo se o encode falhou.
        // SAFETY: `mapped` veio do nvEncMapInputResource acima.
        let st = unsafe { self.api.call(FL_UNMAP_INPUT_RESOURCE, enc, mapped) };
        let frames = resultado?;
        self.api.check(enc, "nvEncUnmapInputResource", st)?;
        Ok(frames)
    }
}

impl Drop for NvencEncoder {
    fn drop(&mut self) {
        if self.enc.is_null() {
            return;
        }
        // SAFETY: handles criados por esta sessão, liberados na ordem inversa
        // e uma única vez. Erros aqui não têm o que fazer além de ignorar.
        unsafe {
            if !self.bitstream.is_null() {
                self.api
                    .call(FL_DESTROY_BITSTREAM_BUFFER, self.enc, self.bitstream);
            }
            if !self.registered.is_null() {
                self.api.call(FL_UNREGISTER_RESOURCE, self.enc, self.registered);
            }
            self.api.destroy_encoder(self.enc);
        }
        self.enc = null_mut();
    }
}
