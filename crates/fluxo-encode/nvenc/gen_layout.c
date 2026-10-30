/*
 * Gera src/nvenc/layout.rs a partir do cabeçalho oficial nvEncodeAPI.h.
 *
 * Por que existe: as structs do NVENC são grandes, cheias de campos
 * reservados, unions e bitfields. Transcrevê-las à mão para Rust é fácil de
 * errar e o erro só aparece como crash ou vídeo corrompido. Em vez disso o
 * compilador C (que lê o cabeçalho de verdade) imprime tamanhos, offsets,
 * versões, constantes e GUIDs, e o Rust trata as structs como blocos de bytes
 * acessados por esses offsets.
 *
 * Rodar: nvenc/gerar_layout.ps1 (precisa do Visual Studio Build Tools).
 * Ao trocar de versão do cabeçalho, rode de novo e faça commit do layout.rs.
 */
#include <stdio.h>
#include <string.h>
#include <stddef.h>

#include "nvEncodeAPI.h"

#define SIZE(N, T) printf("pub const %s: usize = %u;\n", N, (unsigned)sizeof(T))
#define OFF(N, T, F) printf("pub const %s: usize = %u;\n", N, (unsigned)offsetof(T, F))
#define U32(N, V) printf("pub const %s: u32 = 0x%08X;\n", N, (unsigned)(V))
#define I32(N, V) printf("pub const %s: i32 = %d;\n", N, (int)(V))

/* Bitfield de 1 bit: liga o campo numa struct zerada e procura o bit aceso. */
#define BIT(N, T, F)                                                          \
    do {                                                                      \
        static T s_;                                                          \
        const unsigned char *p_ = (const unsigned char *)&s_;                 \
        size_t i_;                                                            \
        int b_;                                                               \
        memset(&s_, 0, sizeof s_);                                            \
        s_.F = 1;                                                             \
        for (i_ = 0; i_ < sizeof s_; i_++) {                                  \
            if (p_[i_]) {                                                     \
                for (b_ = 0; b_ < 8; b_++)                                    \
                    if (p_[i_] & (1u << b_)) break;                           \
                printf("pub const %s: Bit = Bit { byte: %u, bit: %d };\n", N, \
                       (unsigned)i_, b_);                                     \
                break;                                                        \
            }                                                                 \
        }                                                                     \
    } while (0)

static void guid(const char *name, const GUID *g) {
    printf("pub const %s: GUID = GUID::from_values(0x%08lX, 0x%04X, 0x%04X, "
           "[0x%02X, 0x%02X, 0x%02X, 0x%02X, 0x%02X, 0x%02X, 0x%02X, 0x%02X]);\n",
           name, (unsigned long)g->Data1, g->Data2, g->Data3, g->Data4[0], g->Data4[1],
           g->Data4[2], g->Data4[3], g->Data4[4], g->Data4[5], g->Data4[6], g->Data4[7]);
}

#define STATUS(S) printf("        %d => \"%s\",\n", (int)S, #S)

int main(void) {
    printf("// GERADO por crates/fluxo-encode/nvenc/gen_layout.c a partir de\n");
    printf("// nvEncodeAPI.h %d.%d. NÃO EDITE À MÃO: rode nvenc/gerar_layout.ps1.\n",
           NVENCAPI_MAJOR_VERSION, NVENCAPI_MINOR_VERSION);
    printf("#![allow(dead_code)]\n\n");
    printf("use windows::core::GUID;\n\n");
    printf("/// Posição de um bitfield de 1 bit dentro de uma struct.\n");
    printf("#[derive(Debug, Clone, Copy)]\n");
    printf("pub struct Bit {\n    pub byte: usize,\n    pub bit: u8,\n}\n\n");

    printf("// --- Versões ---\n");
    printf("pub const API_MAJOR: u32 = %d;\n", NVENCAPI_MAJOR_VERSION);
    printf("pub const API_MINOR: u32 = %d;\n", NVENCAPI_MINOR_VERSION);
    U32("NVENCAPI_VERSION", NVENCAPI_VERSION);
    U32("FUNCTION_LIST_VER", NV_ENCODE_API_FUNCTION_LIST_VER);
    U32("OPEN_SESSION_VER", NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER);
    U32("PRESET_CONFIG_VER", NV_ENC_PRESET_CONFIG_VER);
    U32("CONFIG_VER", NV_ENC_CONFIG_VER);
    U32("INITIALIZE_PARAMS_VER", NV_ENC_INITIALIZE_PARAMS_VER);
    U32("REGISTER_RESOURCE_VER", NV_ENC_REGISTER_RESOURCE_VER);
    U32("CREATE_BITSTREAM_BUFFER_VER", NV_ENC_CREATE_BITSTREAM_BUFFER_VER);
    U32("MAP_INPUT_RESOURCE_VER", NV_ENC_MAP_INPUT_RESOURCE_VER);
    U32("PIC_PARAMS_VER", NV_ENC_PIC_PARAMS_VER);
    U32("LOCK_BITSTREAM_VER", NV_ENC_LOCK_BITSTREAM_VER);
    U32("RECONFIGURE_PARAMS_VER", NV_ENC_RECONFIGURE_PARAMS_VER);

    printf("\n// --- Constantes ---\n");
    U32("INFINITE_GOPLENGTH", NVENC_INFINITE_GOPLENGTH);
    U32("DEVICE_TYPE_DIRECTX", NV_ENC_DEVICE_TYPE_DIRECTX);
    U32("INPUT_RESOURCE_TYPE_DIRECTX", NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX);
    U32("INPUT_IMAGE", NV_ENC_INPUT_IMAGE);
    U32("BUFFER_FORMAT_NV12", NV_ENC_BUFFER_FORMAT_NV12);
    U32("RC_CBR", NV_ENC_PARAMS_RC_CBR);
    U32("TUNING_ULTRA_LOW_LATENCY", NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY);
    U32("PIC_FLAG_FORCEIDR", NV_ENC_PIC_FLAG_FORCEIDR);
    U32("PIC_FLAG_OUTPUT_SPSPPS", NV_ENC_PIC_FLAG_OUTPUT_SPSPPS);
    U32("PIC_STRUCT_FRAME", NV_ENC_PIC_STRUCT_FRAME);
    U32("PIC_TYPE_I", NV_ENC_PIC_TYPE_I);
    U32("PIC_TYPE_IDR", NV_ENC_PIC_TYPE_IDR);
    U32("VUI_VIDEO_FORMAT_UNSPECIFIED", NV_ENC_VUI_VIDEO_FORMAT_UNSPECIFIED);
    U32("VUI_COLOR_PRIMARIES_BT709", NV_ENC_VUI_COLOR_PRIMARIES_BT709);
    U32("VUI_TRANSFER_CHARACTERISTIC_BT709", NV_ENC_VUI_TRANSFER_CHARACTERISTIC_BT709);
    U32("VUI_MATRIX_COEFFS_BT709", NV_ENC_VUI_MATRIX_COEFFS_BT709);
    I32("SUCCESS", NV_ENC_SUCCESS);
    I32("ERR_NEED_MORE_INPUT", NV_ENC_ERR_NEED_MORE_INPUT);

    printf("\n// --- GUIDs ---\n");
    guid("CODEC_H264_GUID", &NV_ENC_CODEC_H264_GUID);
    guid("H264_PROFILE_HIGH_GUID", &NV_ENC_H264_PROFILE_HIGH_GUID);
    guid("PRESET_P1_GUID", &NV_ENC_PRESET_P1_GUID);
    guid("PRESET_P2_GUID", &NV_ENC_PRESET_P2_GUID);
    guid("PRESET_P3_GUID", &NV_ENC_PRESET_P3_GUID);
    guid("PRESET_P4_GUID", &NV_ENC_PRESET_P4_GUID);
    guid("PRESET_P5_GUID", &NV_ENC_PRESET_P5_GUID);
    guid("PRESET_P6_GUID", &NV_ENC_PRESET_P6_GUID);
    guid("PRESET_P7_GUID", &NV_ENC_PRESET_P7_GUID);

    printf("\n// --- NV_ENCODE_API_FUNCTION_LIST ---\n");
    SIZE("FUNCTION_LIST_SIZE", NV_ENCODE_API_FUNCTION_LIST);
    OFF("FL_VERSION", NV_ENCODE_API_FUNCTION_LIST, version);
    OFF("FL_GET_ENCODE_GUID_COUNT", NV_ENCODE_API_FUNCTION_LIST, nvEncGetEncodeGUIDCount);
    OFF("FL_GET_ENCODE_GUIDS", NV_ENCODE_API_FUNCTION_LIST, nvEncGetEncodeGUIDs);
    OFF("FL_INITIALIZE_ENCODER", NV_ENCODE_API_FUNCTION_LIST, nvEncInitializeEncoder);
    OFF("FL_CREATE_BITSTREAM_BUFFER", NV_ENCODE_API_FUNCTION_LIST, nvEncCreateBitstreamBuffer);
    OFF("FL_DESTROY_BITSTREAM_BUFFER", NV_ENCODE_API_FUNCTION_LIST, nvEncDestroyBitstreamBuffer);
    OFF("FL_ENCODE_PICTURE", NV_ENCODE_API_FUNCTION_LIST, nvEncEncodePicture);
    OFF("FL_LOCK_BITSTREAM", NV_ENCODE_API_FUNCTION_LIST, nvEncLockBitstream);
    OFF("FL_UNLOCK_BITSTREAM", NV_ENCODE_API_FUNCTION_LIST, nvEncUnlockBitstream);
    OFF("FL_MAP_INPUT_RESOURCE", NV_ENCODE_API_FUNCTION_LIST, nvEncMapInputResource);
    OFF("FL_UNMAP_INPUT_RESOURCE", NV_ENCODE_API_FUNCTION_LIST, nvEncUnmapInputResource);
    OFF("FL_DESTROY_ENCODER", NV_ENCODE_API_FUNCTION_LIST, nvEncDestroyEncoder);
    OFF("FL_OPEN_ENCODE_SESSION_EX", NV_ENCODE_API_FUNCTION_LIST, nvEncOpenEncodeSessionEx);
    OFF("FL_REGISTER_RESOURCE", NV_ENCODE_API_FUNCTION_LIST, nvEncRegisterResource);
    OFF("FL_UNREGISTER_RESOURCE", NV_ENCODE_API_FUNCTION_LIST, nvEncUnregisterResource);
    OFF("FL_GET_LAST_ERROR_STRING", NV_ENCODE_API_FUNCTION_LIST, nvEncGetLastErrorString);
    OFF("FL_GET_ENCODE_PRESET_CONFIG_EX", NV_ENCODE_API_FUNCTION_LIST, nvEncGetEncodePresetConfigEx);
    OFF("FL_RECONFIGURE_ENCODER", NV_ENCODE_API_FUNCTION_LIST, nvEncReconfigureEncoder);

    printf("\n// --- NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS ---\n");
    SIZE("OPEN_SESSION_SIZE", NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS);
    OFF("OS_VERSION", NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS, version);
    OFF("OS_DEVICE_TYPE", NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS, deviceType);
    OFF("OS_DEVICE", NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS, device);
    OFF("OS_API_VERSION", NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS, apiVersion);

    printf("\n// --- NV_ENC_PRESET_CONFIG / NV_ENC_CONFIG ---\n");
    SIZE("PRESET_CONFIG_SIZE", NV_ENC_PRESET_CONFIG);
    OFF("PC_VERSION", NV_ENC_PRESET_CONFIG, version);
    OFF("PC_PRESET_CFG", NV_ENC_PRESET_CONFIG, presetCfg);
    SIZE("CONFIG_SIZE", NV_ENC_CONFIG);
    OFF("CFG_VERSION", NV_ENC_CONFIG, version);
    OFF("CFG_PROFILE_GUID", NV_ENC_CONFIG, profileGUID);
    OFF("CFG_GOP_LENGTH", NV_ENC_CONFIG, gopLength);
    OFF("CFG_FRAME_INTERVAL_P", NV_ENC_CONFIG, frameIntervalP);
    OFF("CFG_RC_MODE", NV_ENC_CONFIG, rcParams.rateControlMode);
    OFF("CFG_RC_AVERAGE_BITRATE", NV_ENC_CONFIG, rcParams.averageBitRate);
    OFF("CFG_RC_MAX_BITRATE", NV_ENC_CONFIG, rcParams.maxBitRate);
    OFF("CFG_RC_VBV_BUFFER_SIZE", NV_ENC_CONFIG, rcParams.vbvBufferSize);
    OFF("CFG_RC_VBV_INITIAL_DELAY", NV_ENC_CONFIG, rcParams.vbvInitialDelay);
    OFF("CFG_H264_IDR_PERIOD", NV_ENC_CONFIG, encodeCodecConfig.h264Config.idrPeriod);
    BIT("CFG_H264_REPEAT_SPSPPS", NV_ENC_CONFIG, encodeCodecConfig.h264Config.repeatSPSPPS);
    OFF("CFG_H264_VUI_SIGNAL_TYPE_PRESENT", NV_ENC_CONFIG,
        encodeCodecConfig.h264Config.h264VUIParameters.videoSignalTypePresentFlag);
    OFF("CFG_H264_VUI_VIDEO_FORMAT", NV_ENC_CONFIG,
        encodeCodecConfig.h264Config.h264VUIParameters.videoFormat);
    OFF("CFG_H264_VUI_FULL_RANGE", NV_ENC_CONFIG,
        encodeCodecConfig.h264Config.h264VUIParameters.videoFullRangeFlag);
    OFF("CFG_H264_VUI_COLOUR_DESC_PRESENT", NV_ENC_CONFIG,
        encodeCodecConfig.h264Config.h264VUIParameters.colourDescriptionPresentFlag);
    OFF("CFG_H264_VUI_COLOUR_PRIMARIES", NV_ENC_CONFIG,
        encodeCodecConfig.h264Config.h264VUIParameters.colourPrimaries);
    OFF("CFG_H264_VUI_TRANSFER", NV_ENC_CONFIG,
        encodeCodecConfig.h264Config.h264VUIParameters.transferCharacteristics);
    OFF("CFG_H264_VUI_COLOUR_MATRIX", NV_ENC_CONFIG,
        encodeCodecConfig.h264Config.h264VUIParameters.colourMatrix);

    printf("\n// --- NV_ENC_INITIALIZE_PARAMS ---\n");
    SIZE("INITIALIZE_PARAMS_SIZE", NV_ENC_INITIALIZE_PARAMS);
    OFF("IP_VERSION", NV_ENC_INITIALIZE_PARAMS, version);
    OFF("IP_ENCODE_GUID", NV_ENC_INITIALIZE_PARAMS, encodeGUID);
    OFF("IP_PRESET_GUID", NV_ENC_INITIALIZE_PARAMS, presetGUID);
    OFF("IP_ENCODE_WIDTH", NV_ENC_INITIALIZE_PARAMS, encodeWidth);
    OFF("IP_ENCODE_HEIGHT", NV_ENC_INITIALIZE_PARAMS, encodeHeight);
    OFF("IP_DAR_WIDTH", NV_ENC_INITIALIZE_PARAMS, darWidth);
    OFF("IP_DAR_HEIGHT", NV_ENC_INITIALIZE_PARAMS, darHeight);
    OFF("IP_FRAME_RATE_NUM", NV_ENC_INITIALIZE_PARAMS, frameRateNum);
    OFF("IP_FRAME_RATE_DEN", NV_ENC_INITIALIZE_PARAMS, frameRateDen);
    OFF("IP_ENABLE_ENCODE_ASYNC", NV_ENC_INITIALIZE_PARAMS, enableEncodeAsync);
    OFF("IP_ENABLE_PTD", NV_ENC_INITIALIZE_PARAMS, enablePTD);
    OFF("IP_ENCODE_CONFIG", NV_ENC_INITIALIZE_PARAMS, encodeConfig);
    OFF("IP_MAX_ENCODE_WIDTH", NV_ENC_INITIALIZE_PARAMS, maxEncodeWidth);
    OFF("IP_MAX_ENCODE_HEIGHT", NV_ENC_INITIALIZE_PARAMS, maxEncodeHeight);
    OFF("IP_TUNING_INFO", NV_ENC_INITIALIZE_PARAMS, tuningInfo);

    printf("\n// --- NV_ENC_RECONFIGURE_PARAMS ---\n");
    SIZE("RECONFIGURE_PARAMS_SIZE", NV_ENC_RECONFIGURE_PARAMS);
    OFF("RP_VERSION", NV_ENC_RECONFIGURE_PARAMS, version);
    OFF("RP_REINIT_PARAMS", NV_ENC_RECONFIGURE_PARAMS, reInitEncodeParams);
    BIT("RP_RESET_ENCODER", NV_ENC_RECONFIGURE_PARAMS, resetEncoder);
    BIT("RP_FORCE_IDR", NV_ENC_RECONFIGURE_PARAMS, forceIDR);

    printf("\n// --- NV_ENC_REGISTER_RESOURCE ---\n");
    SIZE("REGISTER_RESOURCE_SIZE", NV_ENC_REGISTER_RESOURCE);
    OFF("RR_VERSION", NV_ENC_REGISTER_RESOURCE, version);
    OFF("RR_RESOURCE_TYPE", NV_ENC_REGISTER_RESOURCE, resourceType);
    OFF("RR_WIDTH", NV_ENC_REGISTER_RESOURCE, width);
    OFF("RR_HEIGHT", NV_ENC_REGISTER_RESOURCE, height);
    OFF("RR_PITCH", NV_ENC_REGISTER_RESOURCE, pitch);
    OFF("RR_SUBRESOURCE_INDEX", NV_ENC_REGISTER_RESOURCE, subResourceIndex);
    OFF("RR_RESOURCE_TO_REGISTER", NV_ENC_REGISTER_RESOURCE, resourceToRegister);
    OFF("RR_REGISTERED_RESOURCE", NV_ENC_REGISTER_RESOURCE, registeredResource);
    OFF("RR_BUFFER_FORMAT", NV_ENC_REGISTER_RESOURCE, bufferFormat);
    OFF("RR_BUFFER_USAGE", NV_ENC_REGISTER_RESOURCE, bufferUsage);

    printf("\n// --- NV_ENC_CREATE_BITSTREAM_BUFFER ---\n");
    SIZE("CREATE_BITSTREAM_BUFFER_SIZE", NV_ENC_CREATE_BITSTREAM_BUFFER);
    OFF("CB_VERSION", NV_ENC_CREATE_BITSTREAM_BUFFER, version);
    OFF("CB_BITSTREAM_BUFFER", NV_ENC_CREATE_BITSTREAM_BUFFER, bitstreamBuffer);

    printf("\n// --- NV_ENC_MAP_INPUT_RESOURCE ---\n");
    SIZE("MAP_INPUT_RESOURCE_SIZE", NV_ENC_MAP_INPUT_RESOURCE);
    OFF("MI_VERSION", NV_ENC_MAP_INPUT_RESOURCE, version);
    OFF("MI_REGISTERED_RESOURCE", NV_ENC_MAP_INPUT_RESOURCE, registeredResource);
    OFF("MI_MAPPED_RESOURCE", NV_ENC_MAP_INPUT_RESOURCE, mappedResource);
    OFF("MI_MAPPED_BUFFER_FMT", NV_ENC_MAP_INPUT_RESOURCE, mappedBufferFmt);

    printf("\n// --- NV_ENC_PIC_PARAMS ---\n");
    SIZE("PIC_PARAMS_SIZE", NV_ENC_PIC_PARAMS);
    OFF("PP_VERSION", NV_ENC_PIC_PARAMS, version);
    OFF("PP_INPUT_WIDTH", NV_ENC_PIC_PARAMS, inputWidth);
    OFF("PP_INPUT_HEIGHT", NV_ENC_PIC_PARAMS, inputHeight);
    OFF("PP_INPUT_PITCH", NV_ENC_PIC_PARAMS, inputPitch);
    OFF("PP_ENCODE_PIC_FLAGS", NV_ENC_PIC_PARAMS, encodePicFlags);
    OFF("PP_FRAME_IDX", NV_ENC_PIC_PARAMS, frameIdx);
    OFF("PP_INPUT_TIMESTAMP", NV_ENC_PIC_PARAMS, inputTimeStamp);
    OFF("PP_INPUT_BUFFER", NV_ENC_PIC_PARAMS, inputBuffer);
    OFF("PP_OUTPUT_BITSTREAM", NV_ENC_PIC_PARAMS, outputBitstream);
    OFF("PP_BUFFER_FMT", NV_ENC_PIC_PARAMS, bufferFmt);
    OFF("PP_PICTURE_STRUCT", NV_ENC_PIC_PARAMS, pictureStruct);

    printf("\n// --- NV_ENC_LOCK_BITSTREAM ---\n");
    SIZE("LOCK_BITSTREAM_SIZE", NV_ENC_LOCK_BITSTREAM);
    OFF("LB_VERSION", NV_ENC_LOCK_BITSTREAM, version);
    BIT("LB_DO_NOT_WAIT", NV_ENC_LOCK_BITSTREAM, doNotWait);
    OFF("LB_OUTPUT_BITSTREAM", NV_ENC_LOCK_BITSTREAM, outputBitstream);
    OFF("LB_BITSTREAM_SIZE", NV_ENC_LOCK_BITSTREAM, bitstreamSizeInBytes);
    OFF("LB_BITSTREAM_PTR", NV_ENC_LOCK_BITSTREAM, bitstreamBufferPtr);
    OFF("LB_PICTURE_TYPE", NV_ENC_LOCK_BITSTREAM, pictureType);

    printf("\n/// Nome do código de status NVENCSTATUS, para mensagens de erro.\n");
    printf("pub fn status_name(status: i32) -> &'static str {\n    match status {\n");
    STATUS(NV_ENC_SUCCESS);
    STATUS(NV_ENC_ERR_NO_ENCODE_DEVICE);
    STATUS(NV_ENC_ERR_UNSUPPORTED_DEVICE);
    STATUS(NV_ENC_ERR_INVALID_ENCODERDEVICE);
    STATUS(NV_ENC_ERR_INVALID_DEVICE);
    STATUS(NV_ENC_ERR_DEVICE_NOT_EXIST);
    STATUS(NV_ENC_ERR_INVALID_PTR);
    STATUS(NV_ENC_ERR_INVALID_EVENT);
    STATUS(NV_ENC_ERR_INVALID_PARAM);
    STATUS(NV_ENC_ERR_INVALID_CALL);
    STATUS(NV_ENC_ERR_OUT_OF_MEMORY);
    STATUS(NV_ENC_ERR_ENCODER_NOT_INITIALIZED);
    STATUS(NV_ENC_ERR_UNSUPPORTED_PARAM);
    STATUS(NV_ENC_ERR_LOCK_BUSY);
    STATUS(NV_ENC_ERR_NOT_ENOUGH_BUFFER);
    STATUS(NV_ENC_ERR_INVALID_VERSION);
    STATUS(NV_ENC_ERR_MAP_FAILED);
    STATUS(NV_ENC_ERR_NEED_MORE_INPUT);
    STATUS(NV_ENC_ERR_ENCODER_BUSY);
    STATUS(NV_ENC_ERR_EVENT_NOT_REGISTERD);
    STATUS(NV_ENC_ERR_GENERIC);
    STATUS(NV_ENC_ERR_INCOMPATIBLE_CLIENT_KEY);
    STATUS(NV_ENC_ERR_UNIMPLEMENTED);
    STATUS(NV_ENC_ERR_RESOURCE_REGISTER_FAILED);
    STATUS(NV_ENC_ERR_RESOURCE_NOT_REGISTERED);
    STATUS(NV_ENC_ERR_RESOURCE_NOT_MAPPED);
    STATUS(NV_ENC_ERR_NEED_MORE_OUTPUT);
    printf("        _ => \"NV_ENC_ERR_DESCONHECIDO\",\n    }\n}\n");
    return 0;
}
