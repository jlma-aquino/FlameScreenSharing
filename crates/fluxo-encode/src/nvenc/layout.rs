// GERADO por crates/fluxo-encode/nvenc/gen_layout.c a partir de
// nvEncodeAPI.h 12.1. N├âO EDITE ├Ç M├âO: rode nvenc/gerar_layout.ps1.
#![allow(dead_code)]

use windows::core::GUID;

/// Posi├º├úo de um bitfield de 1 bit dentro de uma struct.
#[derive(Debug, Clone, Copy)]
pub struct Bit {
    pub byte: usize,
    pub bit: u8,
}

// --- Vers├╡es ---
pub const API_MAJOR: u32 = 12;
pub const API_MINOR: u32 = 1;
pub const NVENCAPI_VERSION: u32 = 0x0100000C;
pub const FUNCTION_LIST_VER: u32 = 0x7102000C;
pub const OPEN_SESSION_VER: u32 = 0x7101000C;
pub const PRESET_CONFIG_VER: u32 = 0xF104000C;
pub const CONFIG_VER: u32 = 0xF108000C;
pub const INITIALIZE_PARAMS_VER: u32 = 0xF106000C;
pub const REGISTER_RESOURCE_VER: u32 = 0x7104000C;
pub const CREATE_BITSTREAM_BUFFER_VER: u32 = 0x7101000C;
pub const MAP_INPUT_RESOURCE_VER: u32 = 0x7104000C;
pub const PIC_PARAMS_VER: u32 = 0xF106000C;
pub const LOCK_BITSTREAM_VER: u32 = 0xF101000C;
pub const RECONFIGURE_PARAMS_VER: u32 = 0xF101000C;

// --- Constantes ---
pub const INFINITE_GOPLENGTH: u32 = 0xFFFFFFFF;
pub const DEVICE_TYPE_DIRECTX: u32 = 0x00000000;
pub const INPUT_RESOURCE_TYPE_DIRECTX: u32 = 0x00000000;
pub const INPUT_IMAGE: u32 = 0x00000000;
pub const BUFFER_FORMAT_NV12: u32 = 0x00000001;
pub const RC_CBR: u32 = 0x00000002;
pub const TUNING_ULTRA_LOW_LATENCY: u32 = 0x00000003;
pub const PIC_FLAG_FORCEIDR: u32 = 0x00000002;
pub const PIC_FLAG_OUTPUT_SPSPPS: u32 = 0x00000004;
pub const PIC_STRUCT_FRAME: u32 = 0x00000001;
pub const PIC_TYPE_I: u32 = 0x00000002;
pub const PIC_TYPE_IDR: u32 = 0x00000003;
pub const VUI_VIDEO_FORMAT_UNSPECIFIED: u32 = 0x00000005;
pub const VUI_COLOR_PRIMARIES_BT709: u32 = 0x00000001;
pub const VUI_TRANSFER_CHARACTERISTIC_BT709: u32 = 0x00000001;
pub const VUI_MATRIX_COEFFS_BT709: u32 = 0x00000001;
pub const SUCCESS: i32 = 0;
pub const ERR_NEED_MORE_INPUT: i32 = 17;

// --- GUIDs ---
pub const CODEC_H264_GUID: GUID = GUID::from_values(0x6BC82762, 0x4E63, 0x4CA4, [0xAA, 0x85, 0x1E, 0x50, 0xF3, 0x21, 0xF6, 0xBF]);
pub const H264_PROFILE_HIGH_GUID: GUID = GUID::from_values(0xE7CBC309, 0x4F7A, 0x4B89, [0xAF, 0x2A, 0xD5, 0x37, 0xC9, 0x2B, 0xE3, 0x10]);
pub const PRESET_P1_GUID: GUID = GUID::from_values(0xFC0A8D3E, 0x45F8, 0x4CF8, [0x80, 0xC7, 0x29, 0x88, 0x71, 0x59, 0x0E, 0xBF]);
pub const PRESET_P2_GUID: GUID = GUID::from_values(0xF581CFB8, 0x88D6, 0x4381, [0x93, 0xF0, 0xDF, 0x13, 0xF9, 0xC2, 0x7D, 0xAB]);
pub const PRESET_P3_GUID: GUID = GUID::from_values(0x36850110, 0x3A07, 0x441F, [0x94, 0xD5, 0x36, 0x70, 0x63, 0x1F, 0x91, 0xF6]);
pub const PRESET_P4_GUID: GUID = GUID::from_values(0x90A7B826, 0xDF06, 0x4862, [0xB9, 0xD2, 0xCD, 0x6D, 0x73, 0xA0, 0x86, 0x81]);
pub const PRESET_P5_GUID: GUID = GUID::from_values(0x21C6E6B4, 0x297A, 0x4CBA, [0x99, 0x8F, 0xB6, 0xCB, 0xDE, 0x72, 0xAD, 0xE3]);
pub const PRESET_P6_GUID: GUID = GUID::from_values(0x8E75C279, 0x6299, 0x4AB6, [0x83, 0x02, 0x0B, 0x21, 0x5A, 0x33, 0x5C, 0xF5]);
pub const PRESET_P7_GUID: GUID = GUID::from_values(0x84848C12, 0x6F71, 0x4C13, [0x93, 0x1B, 0x53, 0xE2, 0x83, 0xF5, 0x79, 0x74]);

// --- NV_ENCODE_API_FUNCTION_LIST ---
pub const FUNCTION_LIST_SIZE: usize = 2552;
pub const FL_VERSION: usize = 0;
pub const FL_GET_ENCODE_GUID_COUNT: usize = 16;
pub const FL_GET_ENCODE_GUIDS: usize = 40;
pub const FL_INITIALIZE_ENCODER: usize = 96;
pub const FL_CREATE_BITSTREAM_BUFFER: usize = 120;
pub const FL_DESTROY_BITSTREAM_BUFFER: usize = 128;
pub const FL_ENCODE_PICTURE: usize = 136;
pub const FL_LOCK_BITSTREAM: usize = 144;
pub const FL_UNLOCK_BITSTREAM: usize = 152;
pub const FL_MAP_INPUT_RESOURCE: usize = 208;
pub const FL_UNMAP_INPUT_RESOURCE: usize = 216;
pub const FL_DESTROY_ENCODER: usize = 224;
pub const FL_OPEN_ENCODE_SESSION_EX: usize = 240;
pub const FL_REGISTER_RESOURCE: usize = 248;
pub const FL_UNREGISTER_RESOURCE: usize = 256;
pub const FL_GET_LAST_ERROR_STRING: usize = 304;
pub const FL_GET_ENCODE_PRESET_CONFIG_EX: usize = 320;
pub const FL_RECONFIGURE_ENCODER: usize = 264;

// --- NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS ---
pub const OPEN_SESSION_SIZE: usize = 1552;
pub const OS_VERSION: usize = 0;
pub const OS_DEVICE_TYPE: usize = 4;
pub const OS_DEVICE: usize = 8;
pub const OS_API_VERSION: usize = 24;

// --- NV_ENC_PRESET_CONFIG / NV_ENC_CONFIG ---
pub const PRESET_CONFIG_SIZE: usize = 5128;
pub const PC_VERSION: usize = 0;
pub const PC_PRESET_CFG: usize = 8;
pub const CONFIG_SIZE: usize = 3584;
pub const CFG_VERSION: usize = 0;
pub const CFG_PROFILE_GUID: usize = 4;
pub const CFG_GOP_LENGTH: usize = 20;
pub const CFG_FRAME_INTERVAL_P: usize = 24;
pub const CFG_RC_MODE: usize = 44;
pub const CFG_RC_AVERAGE_BITRATE: usize = 60;
pub const CFG_RC_MAX_BITRATE: usize = 64;
pub const CFG_RC_VBV_BUFFER_SIZE: usize = 68;
pub const CFG_RC_VBV_INITIAL_DELAY: usize = 72;
pub const CFG_H264_IDR_PERIOD: usize = 176;
pub const CFG_H264_REPEAT_SPSPPS: Bit = Bit { byte: 169, bit: 4 };
pub const CFG_H264_VUI_SIGNAL_TYPE_PRESENT: usize = 248;
pub const CFG_H264_VUI_VIDEO_FORMAT: usize = 252;
pub const CFG_H264_VUI_FULL_RANGE: usize = 256;
pub const CFG_H264_VUI_COLOUR_DESC_PRESENT: usize = 260;
pub const CFG_H264_VUI_COLOUR_PRIMARIES: usize = 264;
pub const CFG_H264_VUI_TRANSFER: usize = 268;
pub const CFG_H264_VUI_COLOUR_MATRIX: usize = 272;

// --- NV_ENC_INITIALIZE_PARAMS ---
pub const INITIALIZE_PARAMS_SIZE: usize = 1808;
pub const IP_VERSION: usize = 0;
pub const IP_ENCODE_GUID: usize = 4;
pub const IP_PRESET_GUID: usize = 20;
pub const IP_ENCODE_WIDTH: usize = 36;
pub const IP_ENCODE_HEIGHT: usize = 40;
pub const IP_DAR_WIDTH: usize = 44;
pub const IP_DAR_HEIGHT: usize = 48;
pub const IP_FRAME_RATE_NUM: usize = 52;
pub const IP_FRAME_RATE_DEN: usize = 56;
pub const IP_ENABLE_ENCODE_ASYNC: usize = 60;
pub const IP_ENABLE_PTD: usize = 64;
pub const IP_ENCODE_CONFIG: usize = 88;
pub const IP_MAX_ENCODE_WIDTH: usize = 96;
pub const IP_MAX_ENCODE_HEIGHT: usize = 100;
pub const IP_TUNING_INFO: usize = 136;

// --- NV_ENC_RECONFIGURE_PARAMS ---
pub const RECONFIGURE_PARAMS_SIZE: usize = 1824;
pub const RP_VERSION: usize = 0;
pub const RP_REINIT_PARAMS: usize = 8;
pub const RP_RESET_ENCODER: Bit = Bit { byte: 1816, bit: 0 };
pub const RP_FORCE_IDR: Bit = Bit { byte: 1816, bit: 1 };

// --- NV_ENC_REGISTER_RESOURCE ---
pub const REGISTER_RESOURCE_SIZE: usize = 1536;
pub const RR_VERSION: usize = 0;
pub const RR_RESOURCE_TYPE: usize = 4;
pub const RR_WIDTH: usize = 8;
pub const RR_HEIGHT: usize = 12;
pub const RR_PITCH: usize = 16;
pub const RR_SUBRESOURCE_INDEX: usize = 20;
pub const RR_RESOURCE_TO_REGISTER: usize = 24;
pub const RR_REGISTERED_RESOURCE: usize = 32;
pub const RR_BUFFER_FORMAT: usize = 40;
pub const RR_BUFFER_USAGE: usize = 44;

// --- NV_ENC_CREATE_BITSTREAM_BUFFER ---
pub const CREATE_BITSTREAM_BUFFER_SIZE: usize = 776;
pub const CB_VERSION: usize = 0;
pub const CB_BITSTREAM_BUFFER: usize = 16;

// --- NV_ENC_MAP_INPUT_RESOURCE ---
pub const MAP_INPUT_RESOURCE_SIZE: usize = 1544;
pub const MI_VERSION: usize = 0;
pub const MI_REGISTERED_RESOURCE: usize = 16;
pub const MI_MAPPED_RESOURCE: usize = 24;
pub const MI_MAPPED_BUFFER_FMT: usize = 32;

// --- NV_ENC_PIC_PARAMS ---
pub const PIC_PARAMS_SIZE: usize = 3360;
pub const PP_VERSION: usize = 0;
pub const PP_INPUT_WIDTH: usize = 4;
pub const PP_INPUT_HEIGHT: usize = 8;
pub const PP_INPUT_PITCH: usize = 12;
pub const PP_ENCODE_PIC_FLAGS: usize = 16;
pub const PP_FRAME_IDX: usize = 20;
pub const PP_INPUT_TIMESTAMP: usize = 24;
pub const PP_INPUT_BUFFER: usize = 40;
pub const PP_OUTPUT_BITSTREAM: usize = 48;
pub const PP_BUFFER_FMT: usize = 64;
pub const PP_PICTURE_STRUCT: usize = 68;

// --- NV_ENC_LOCK_BITSTREAM ---
pub const LOCK_BITSTREAM_SIZE: usize = 1552;
pub const LB_VERSION: usize = 0;
pub const LB_DO_NOT_WAIT: Bit = Bit { byte: 4, bit: 0 };
pub const LB_OUTPUT_BITSTREAM: usize = 8;
pub const LB_BITSTREAM_SIZE: usize = 36;
pub const LB_BITSTREAM_PTR: usize = 56;
pub const LB_PICTURE_TYPE: usize = 64;

/// Nome do c├│digo de status NVENCSTATUS, para mensagens de erro.
pub fn status_name(status: i32) -> &'static str {
    match status {
        0 => "NV_ENC_SUCCESS",
        1 => "NV_ENC_ERR_NO_ENCODE_DEVICE",
        2 => "NV_ENC_ERR_UNSUPPORTED_DEVICE",
        3 => "NV_ENC_ERR_INVALID_ENCODERDEVICE",
        4 => "NV_ENC_ERR_INVALID_DEVICE",
        5 => "NV_ENC_ERR_DEVICE_NOT_EXIST",
        6 => "NV_ENC_ERR_INVALID_PTR",
        7 => "NV_ENC_ERR_INVALID_EVENT",
        8 => "NV_ENC_ERR_INVALID_PARAM",
        9 => "NV_ENC_ERR_INVALID_CALL",
        10 => "NV_ENC_ERR_OUT_OF_MEMORY",
        11 => "NV_ENC_ERR_ENCODER_NOT_INITIALIZED",
        12 => "NV_ENC_ERR_UNSUPPORTED_PARAM",
        13 => "NV_ENC_ERR_LOCK_BUSY",
        14 => "NV_ENC_ERR_NOT_ENOUGH_BUFFER",
        15 => "NV_ENC_ERR_INVALID_VERSION",
        16 => "NV_ENC_ERR_MAP_FAILED",
        17 => "NV_ENC_ERR_NEED_MORE_INPUT",
        18 => "NV_ENC_ERR_ENCODER_BUSY",
        19 => "NV_ENC_ERR_EVENT_NOT_REGISTERD",
        20 => "NV_ENC_ERR_GENERIC",
        21 => "NV_ENC_ERR_INCOMPATIBLE_CLIENT_KEY",
        22 => "NV_ENC_ERR_UNIMPLEMENTED",
        23 => "NV_ENC_ERR_RESOURCE_REGISTER_FAILED",
        24 => "NV_ENC_ERR_RESOURCE_NOT_REGISTERED",
        25 => "NV_ENC_ERR_RESOURCE_NOT_MAPPED",
        26 => "NV_ENC_ERR_NEED_MORE_OUTPUT",
        _ => "NV_ENC_ERR_DESCONHECIDO",
    }
}
