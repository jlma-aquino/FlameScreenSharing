//! FFI mínima com `nvEncodeAPI64.dll` (instalada pelo driver NVIDIA).
//!
//! As structs do NVENC são tratadas como blocos de bytes ([`Blob`]) lidos e
//! escritos pelos offsets de `layout.rs`, que é **gerado** a partir do
//! cabeçalho oficial (ver `nvenc/gen_layout.c`). Assim nenhum layout é
//! transcrito à mão.

use std::ffi::{c_char, c_void, CStr};
use std::sync::OnceLock;

use windows::core::{s, w, GUID};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use super::layout::*;
use crate::{Backend, EncodeError};

/// Bloco de bytes com o tamanho e alinhamento de uma struct do NVENC.
/// Começa zerado, que é o valor "padrão" que a API espera nos campos não usados.
#[repr(C, align(8))]
pub(crate) struct Blob<const N: usize>([u8; N]);

impl<const N: usize> Blob<N> {
    /// Cria zerado, no heap (algumas structs passam de 5 KB).
    pub fn new() -> Box<Self> {
        Box::new(Self([0; N]))
    }

    pub fn bytes(&self) -> &[u8; N] {
        &self.0
    }

    pub fn bytes_mut(&mut self) -> &mut [u8; N] {
        &mut self.0
    }

    pub fn as_mut_ptr(&mut self) -> *mut c_void {
        self.0.as_mut_ptr().cast()
    }

    pub fn set_u32(&mut self, off: usize, v: u32) {
        self.0[off..off + 4].copy_from_slice(&v.to_ne_bytes());
    }

    pub fn get_u32(&self, off: usize) -> u32 {
        u32::from_ne_bytes(self.0[off..off + 4].try_into().expect("fatia de 4 bytes"))
    }

    pub fn set_u64(&mut self, off: usize, v: u64) {
        self.0[off..off + 8].copy_from_slice(&v.to_ne_bytes());
    }

    pub fn set_ptr(&mut self, off: usize, p: *mut c_void) {
        self.0[off..off + 8].copy_from_slice(&(p as usize).to_ne_bytes());
    }

    pub fn get_ptr(&self, off: usize) -> *mut c_void {
        usize::from_ne_bytes(self.0[off..off + 8].try_into().expect("fatia de 8 bytes")) as *mut c_void
    }

    pub fn set_guid(&mut self, off: usize, g: &GUID) {
        self.set_u32(off, g.data1);
        self.0[off + 4..off + 6].copy_from_slice(&g.data2.to_ne_bytes());
        self.0[off + 6..off + 8].copy_from_slice(&g.data3.to_ne_bytes());
        self.0[off + 8..off + 16].copy_from_slice(&g.data4);
    }

    pub fn set_bit(&mut self, b: Bit, on: bool) {
        if on {
            self.0[b.byte] |= 1 << b.bit;
        } else {
            self.0[b.byte] &= !(1 << b.bit);
        }
    }
}

// Assinaturas das funções da API. NVENCAPI é __stdcall, que no x64 é a
// convenção padrão — "system" cobre os dois casos.
/// O que GetProcAddress devolve (FARPROC), antes de ganhar o tipo certo.
type FarProc = unsafe extern "system" fn() -> isize;
type FnGetMaxVersion = unsafe extern "system" fn(*mut u32) -> i32;
type FnCreateInstance = unsafe extern "system" fn(*mut c_void) -> i32;
type FnOpenSessionEx = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32;
/// Forma da maioria das funções: (encoder, ponteiro para params ou handle).
type FnEncArg = unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32;
type FnEnc = unsafe extern "system" fn(*mut c_void) -> i32;
type FnGuidCount = unsafe extern "system" fn(*mut c_void, *mut u32) -> i32;
type FnGuids = unsafe extern "system" fn(*mut c_void, *mut GUID, u32, *mut u32) -> i32;
type FnPresetConfigEx = unsafe extern "system" fn(*mut c_void, GUID, GUID, u32, *mut c_void) -> i32;
type FnLastError = unsafe extern "system" fn(*mut c_void) -> *const c_char;

/// Funções da lista que usamos; todas conferidas na carga.
const USED: [(usize, &str); 16] = [
    (FL_OPEN_ENCODE_SESSION_EX, "nvEncOpenEncodeSessionEx"),
    (FL_GET_ENCODE_GUID_COUNT, "nvEncGetEncodeGUIDCount"),
    (FL_GET_ENCODE_GUIDS, "nvEncGetEncodeGUIDs"),
    (FL_GET_ENCODE_PRESET_CONFIG_EX, "nvEncGetEncodePresetConfigEx"),
    (FL_INITIALIZE_ENCODER, "nvEncInitializeEncoder"),
    (FL_CREATE_BITSTREAM_BUFFER, "nvEncCreateBitstreamBuffer"),
    (FL_DESTROY_BITSTREAM_BUFFER, "nvEncDestroyBitstreamBuffer"),
    (FL_REGISTER_RESOURCE, "nvEncRegisterResource"),
    (FL_UNREGISTER_RESOURCE, "nvEncUnregisterResource"),
    (FL_MAP_INPUT_RESOURCE, "nvEncMapInputResource"),
    (FL_UNMAP_INPUT_RESOURCE, "nvEncUnmapInputResource"),
    (FL_ENCODE_PICTURE, "nvEncEncodePicture"),
    (FL_LOCK_BITSTREAM, "nvEncLockBitstream"),
    (FL_UNLOCK_BITSTREAM, "nvEncUnlockBitstream"),
    (FL_DESTROY_ENCODER, "nvEncDestroyEncoder"),
    (FL_GET_LAST_ERROR_STRING, "nvEncGetLastErrorString"),
];

/// A DLL do driver, carregada uma vez por processo e nunca descarregada.
struct Library {
    create_instance: FnCreateInstance,
}

static LIBRARY: OnceLock<Result<Library, String>> = OnceLock::new();

impl Library {
    fn get() -> Result<&'static Library, EncodeError> {
        LIBRARY
            .get_or_init(Self::load)
            .as_ref()
            .map_err(|reason| unavailable(reason.clone()))
    }

    fn load() -> Result<Library, String> {
        // SAFETY: os símbolos são os exportados pela DLL do driver, com as
        // assinaturas do cabeçalho 12.1.
        unsafe {
            let dll = LoadLibraryW(w!("nvEncodeAPI64.dll"))
                .map_err(|_| "nvEncodeAPI64.dll não encontrada (sem driver NVIDIA)".to_string())?;

            let get_max = GetProcAddress(dll, s!("NvEncodeAPIGetMaxSupportedVersion"))
                .ok_or("driver sem NvEncodeAPIGetMaxSupportedVersion")?;
            let get_max = std::mem::transmute::<FarProc, FnGetMaxVersion>(get_max);
            let mut max = 0u32;
            let st = get_max(&mut max);
            if st != SUCCESS {
                return Err(format!("NvEncodeAPIGetMaxSupportedVersion: {}", status_name(st)));
            }
            // Formato da versão devolvida: (major << 4) | minor.
            if max < ((API_MAJOR << 4) | API_MINOR) {
                return Err(format!(
                    "o driver suporta a API NVENC até {}.{}; o Fluxo precisa da {API_MAJOR}.{API_MINOR} \
                     (atualize o driver NVIDIA para a versão 531 ou mais nova)",
                    max >> 4,
                    max & 0xF
                ));
            }

            let create = GetProcAddress(dll, s!("NvEncodeAPICreateInstance"))
                .ok_or("driver sem NvEncodeAPICreateInstance")?;
            Ok(Library {
                create_instance: std::mem::transmute::<FarProc, FnCreateInstance>(create),
            })
        }
    }
}

/// Uma tabela de funções do NVENC.
///
/// Cada encoder pede a sua ao driver em vez de compartilhar uma por
/// processo: medimos que, depois que o MFT H.264 da própria NVIDIA (backend
/// Media Foundation) roda no processo, uma tabela antiga passa a recusar
/// sessões novas com `NV_ENC_ERR_INVALID_VERSION`. Pedir a tabela é barato.
pub(crate) struct Api {
    list: Box<Blob<FUNCTION_LIST_SIZE>>,
}

impl Api {
    pub fn new() -> Result<Api, EncodeError> {
        let lib = Library::get()?;
        let mut list = Blob::<FUNCTION_LIST_SIZE>::new();
        list.set_u32(FL_VERSION, FUNCTION_LIST_VER);
        // SAFETY: o driver preenche a struct do tamanho gerado de layout.rs.
        let st = unsafe { (lib.create_instance)(list.as_mut_ptr()) };
        if st != SUCCESS {
            return Err(unavailable(format!(
                "NvEncodeAPICreateInstance: {}",
                status_name(st)
            )));
        }
        for (off, nome) in USED {
            if list.get_ptr(off).is_null() {
                return Err(unavailable(format!("o driver não forneceu {nome}")));
            }
        }
        Ok(Api { list })
    }

    /// Ponteiro de função na posição `off` da lista.
    ///
    /// # Safety
    /// `F` precisa ser o tipo de função correto para essa posição, e `off`
    /// precisa estar em [`USED`] (garantido não nulo na carga).
    unsafe fn func<F: Copy>(&self, off: usize) -> F {
        let p = self.list.get_ptr(off);
        debug_assert!(!p.is_null());
        std::mem::transmute_copy(&p)
    }

    /// Chama uma função da forma (encoder, argumento).
    ///
    /// # Safety
    /// `off` tem que ser de uma função com essa forma, e `arg` o ponteiro
    /// esperado por ela (struct com `version` preenchido, ou handle).
    pub unsafe fn call(&self, off: usize, enc: *mut c_void, arg: *mut c_void) -> i32 {
        self.func::<FnEncArg>(off)(enc, arg)
    }

    /// # Safety
    /// `params` aponta para uma NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS válida.
    pub unsafe fn open_session(&self, params: *mut c_void, enc: &mut *mut c_void) -> i32 {
        self.func::<FnOpenSessionEx>(FL_OPEN_ENCODE_SESSION_EX)(params, enc)
    }

    /// # Safety
    /// `enc` é uma sessão aberta e ainda não destruída.
    pub unsafe fn destroy_encoder(&self, enc: *mut c_void) -> i32 {
        self.func::<FnEnc>(FL_DESTROY_ENCODER)(enc)
    }

    /// Codecs suportados pela GPU desta sessão.
    ///
    /// # Safety
    /// `enc` é uma sessão aberta.
    pub unsafe fn encode_guids(&self, enc: *mut c_void) -> Result<Vec<GUID>, EncodeError> {
        let mut count = 0u32;
        let st = self.func::<FnGuidCount>(FL_GET_ENCODE_GUID_COUNT)(enc, &mut count);
        self.check(enc, "nvEncGetEncodeGUIDCount", st)?;
        let mut guids = vec![GUID::zeroed(); count as usize];
        let mut got = 0u32;
        let st = self.func::<FnGuids>(FL_GET_ENCODE_GUIDS)(enc, guids.as_mut_ptr(), count, &mut got);
        self.check(enc, "nvEncGetEncodeGUIDs", st)?;
        guids.truncate(got as usize);
        Ok(guids)
    }

    /// # Safety
    /// `enc` é uma sessão aberta; `preset` é uma NV_ENC_PRESET_CONFIG com as
    /// versões preenchidas.
    pub unsafe fn preset_config(
        &self,
        enc: *mut c_void,
        codec: GUID,
        preset: GUID,
        tuning: u32,
        out: *mut c_void,
    ) -> i32 {
        self.func::<FnPresetConfigEx>(FL_GET_ENCODE_PRESET_CONFIG_EX)(enc, codec, preset, tuning, out)
    }

    /// Converte um status em erro, anexando a explicação do driver.
    pub fn check(&self, enc: *mut c_void, call: &'static str, status: i32) -> Result<(), EncodeError> {
        if status == SUCCESS {
            return Ok(());
        }
        let detail = if enc.is_null() {
            String::new()
        } else {
            // SAFETY: a sessão está aberta; o driver devolve uma string C
            // terminada em zero que vale até a próxima chamada.
            unsafe {
                let p = self.func::<FnLastError>(FL_GET_LAST_ERROR_STRING)(enc);
                if p.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(p).to_string_lossy().into_owned()
                }
            }
        };
        Err(EncodeError::Nvenc {
            call,
            status: status_name(status).to_string(),
            detail,
        })
    }
}

fn unavailable(reason: String) -> EncodeError {
    EncodeError::BackendUnavailable {
        backend: Backend::Nvenc,
        reason,
    }
}
