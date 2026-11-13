//! WASAPI: [`LoopbackCapture`] (o que o PC toca) e [`AudioOutput`] (tocar).
//!
//! Modo compartilhado, no formato que a placa já usa (o "mix format",
//! normalmente 48 kHz float estéreo); o [`Converter`] leva de/para 48 kHz
//! estéreo. Cada objeto inicializa o COM na thread em que é criado e deve
//! ser usado só nela.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator,
    ISimpleAudioVolume, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_LOOPBACK, DEVICE_STATE_ACTIVE, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Variant::VT_LPWSTR;

use crate::{AudioError, Converter, CHANNELS, FRAME_SAMPLES, SAMPLE_RATE};

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

fn win(context: &'static str) -> impl Fn(windows::core::Error) -> AudioError {
    move |e| AudioError::Windows {
        context,
        message: e.message(),
    }
}

/// Como as amostras da placa estão guardadas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Amostra {
    F32,
    I16,
    I24,
    I32,
}

/// Formato da placa, lido do `WAVEFORMATEX` (que é `packed`: copiado com
/// leitura desalinhada).
#[derive(Debug, Clone, Copy)]
struct Formato {
    taxa: u32,
    canais: usize,
    amostra: Amostra,
    bloco: usize,
}

/// SAFETY: `p` aponta um WAVEFORMATEX válido (do GetMixFormat), e, se a tag
/// for EXTENSIBLE, um WAVEFORMATEXTENSIBLE.
unsafe fn ler_formato(p: *const WAVEFORMATEX) -> Result<Formato, AudioError> {
    let f = std::ptr::read_unaligned(p);
    let tag = match f.wFormatTag {
        WAVE_FORMAT_EXTENSIBLE => {
            let x = std::ptr::read_unaligned(p as *const WAVEFORMATEXTENSIBLE);
            // KSDATAFORMAT_SUBTYPE_PCM / _IEEE_FLOAT: Data1 = 1 / 3.
            let sub = x.SubFormat;
            sub.data1 as u16
        }
        t => t,
    };
    let amostra = match (tag, f.wBitsPerSample) {
        (WAVE_FORMAT_IEEE_FLOAT, 32) => Amostra::F32,
        (WAVE_FORMAT_PCM, 16) => Amostra::I16,
        (WAVE_FORMAT_PCM, 24) => Amostra::I24,
        (WAVE_FORMAT_PCM, 32) => Amostra::I32,
        (t, b) => return Err(AudioError::Format(format!("tag {t}, {b} bits"))),
    };
    Ok(Formato {
        taxa: f.nSamplesPerSec,
        canais: f.nChannels as usize,
        amostra,
        bloco: f.nBlockAlign as usize,
    })
}

/// Inicializa o COM nesta thread (já inicializado: tudo bem).
fn com() {
    // SAFETY: chamada simples; erro "modo diferente" é inofensivo para nós.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

/// Uma saída de som do Windows.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputDevice {
    /// Número para escolher a saída (`LoopbackCapture::new(Some(n))`).
    pub index: usize,
    pub name: String,
    /// É a saída padrão do Windows.
    pub default: bool,
    /// Maior nível de som medido agora (0 = nada tocando nela).
    pub peak: f32,
}

/// `FriendlyName` do dispositivo (`PKEY_Device_FriendlyName`).
const NOME: PROPERTYKEY = PROPERTYKEY {
    fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

fn enumerador() -> Result<IMMDeviceEnumerator, AudioError> {
    com();
    // SAFETY: criação COM padrão.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(win("abrir o áudio"))
}

/// As saídas de som ativas, com o nível de som de cada uma agora (mede por
/// ~300 ms): mostra em qual delas o som está saindo.
pub fn output_devices() -> Result<Vec<OutputDevice>, AudioError> {
    let en = enumerador()?;
    // SAFETY: interfaces COM válidas; strings e PROPVARIANT liberados aqui.
    unsafe {
        let padrao = en
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .and_then(|d| d.GetId())
            .map(|id| {
                let s = id.to_string().unwrap_or_default();
                CoTaskMemFree(Some(id.0 as *const _));
                s
            })
            .unwrap_or_default();
        let lista = en
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(win("listar as saídas de som"))?;
        let mut saidas = Vec::new();
        let mut medidores = Vec::new();
        for i in 0..lista.GetCount().map_err(win("listar as saídas de som"))? {
            let d = lista.Item(i).map_err(win("listar as saídas de som"))?;
            let id = d.GetId().map_err(win("listar as saídas de som"))?;
            let default = id.to_string().unwrap_or_default() == padrao;
            CoTaskMemFree(Some(id.0 as *const _));
            let name = d
                .OpenPropertyStore(STGM_READ)
                .and_then(|p| p.GetValue(&NOME))
                .map(|v| {
                    let s = match v.Anonymous.Anonymous.vt {
                        VT_LPWSTR => v
                            .Anonymous
                            .Anonymous
                            .Anonymous
                            .pwszVal
                            .to_string()
                            .unwrap_or_default(),
                        _ => String::new(),
                    };
                    let mut v = v;
                    let _ = PropVariantClear(&mut v);
                    s
                })
                .unwrap_or_default();
            medidores.push(d.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None).ok());
            saidas.push(OutputDevice {
                index: i as usize,
                name: if name.is_empty() {
                    format!("saída {i}")
                } else {
                    name
                },
                default,
                peak: 0.0,
            });
        }
        for _ in 0..15 {
            for (s, m) in saidas.iter_mut().zip(&medidores) {
                if let Some(v) = m.as_ref().and_then(|m| m.GetPeakValue().ok()) {
                    s.peak = s.peak.max(v);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(saidas)
    }
}

/// Abre a saída de som `indice` (`None`: a padrão) e lê o formato dela.
fn abrir(indice: Option<usize>) -> Result<(IAudioClient, Formato, *mut WAVEFORMATEX), AudioError> {
    let en = enumerador()?;
    // SAFETY: interfaces COM válidas; o formato é liberado por quem chamou.
    unsafe {
        let disp = match indice {
            None => en
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(win("achar a saída de som padrão"))?,
            Some(i) => en
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
                .and_then(|l| l.Item(i as u32))
                .map_err(win("achar a saída de som escolhida"))?,
        };
        let cliente: IAudioClient = disp.Activate(CLSCTX_ALL, None).map_err(win("ativar o áudio"))?;
        let fmt = cliente.GetMixFormat().map_err(win("ler o formato do áudio"))?;
        match ler_formato(fmt) {
            Ok(f) => Ok((cliente, f, fmt)),
            Err(e) => {
                CoTaskMemFree(Some(fmt as *const _));
                Err(e)
            }
        }
    }
}

/// Amostras da placa → f32 intercalado.
fn para_f32(dados: &[u8], f: &Formato) -> Vec<f32> {
    match f.amostra {
        Amostra::F32 => dados
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect(),
        Amostra::I16 => dados
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect(),
        Amostra::I24 => dados
            .chunks_exact(3)
            .map(|b| (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0)
            .collect(),
        Amostra::I32 => dados
            .chunks_exact(4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0)
            .collect(),
    }
}

/// Captura do que o PC está tocando (todos os programas), já em quadros de
/// 10 ms a 48 kHz estéreo.
pub struct LoopbackCapture {
    cliente: IAudioClient,
    captura: IAudioCaptureClient,
    formato: Formato,
    conv: Converter,
    acumulado: Vec<f32>,
    inicio: Instant,
    entregues: u64,
    /// Amostras por canal que vieram da placa (o resto é silêncio completado).
    da_placa: u64,
}

/// Sem nada vindo da placa por mais que isso (nada tocando), completa com
/// silêncio para o fluxo não parar.
const SILENCIO_DEPOIS: Duration = Duration::from_millis(30);

/// Avisa quando a ativação assíncrona do áudio termina (a interface COM que
/// o Windows chama de volta; ágil, para poder vir de qualquer thread).
// Objetos do `implement` já são ágeis (IAgileObject) por padrão.
#[windows_core::implement(windows::Win32::Media::Audio::IActivateAudioInterfaceCompletionHandler)]
struct Ativado(std::sync::mpsc::SyncSender<()>);

impl windows::Win32::Media::Audio::IActivateAudioInterfaceCompletionHandler_Impl for Ativado_Impl {
    fn ActivateCompleted(
        &self,
        _: windows_core::Ref<'_, windows::Win32::Media::Audio::IActivateAudioInterfaceAsyncOperation>,
    ) -> windows_core::Result<()> {
        let _ = self.0.send(());
        Ok(())
    }
}

impl LoopbackCapture {
    /// Captura o que o PC toca **menos** o som do processo `pid` e dos filhos
    /// dele (Windows 10 2004+). O FSS passa o número da própria janela: o som
    /// de quem você está assistindo (um processo filho dela) não volta na sua
    /// transmissão. Sempre da mistura de todas as saídas, em 48 kHz estéreo.
    pub fn excluding_process(pid: u32) -> Result<Self, AudioError> {
        use windows::Win32::Media::Audio::{
            ActivateAudioInterfaceAsync, IActivateAudioInterfaceCompletionHandler,
            AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
            AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
            PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        };
        use windows::Win32::System::Com::StructuredStorage::{
            PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
        };
        use windows::Win32::System::Com::BLOB;
        use windows::Win32::System::Variant::VT_BLOB;
        use windows_core::Interface;

        // SAFETY: COM do processo; parâmetros vivem até o fim da ativação
        // (esperada logo abaixo); o formato é nosso, na pilha.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let params = AUDIOCLIENT_ACTIVATION_PARAMS {
                ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
                Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                    ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                        TargetProcessId: pid,
                        ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
                    },
                },
            };
            // ManuallyDrop: o PROPVARIANT do `windows` se limpa sozinho ao sair de escopo, e
            // liberaria o blob, que é nosso e está na pilha (heap corrompido).
            let pv = std::mem::ManuallyDrop::new(PROPVARIANT {
                Anonymous: PROPVARIANT_0 {
                    Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                        vt: VT_BLOB,
                        wReserved1: 0,
                        wReserved2: 0,
                        wReserved3: 0,
                        Anonymous: PROPVARIANT_0_0_0 {
                            blob: BLOB {
                                cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                                pBlobData: &params as *const _ as *mut u8,
                            },
                        },
                    }),
                },
            });
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            let aviso: IActivateAudioInterfaceCompletionHandler = Ativado(tx).into();
            let op = ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&*pv),
                &aviso,
            )
            .map_err(win("pedir a captura sem o som do FSS (Windows 10 2004+)"))?;
            rx.recv_timeout(Duration::from_secs(5))
                .map_err(|_| AudioError::Windows {
                    context: "ativar a captura sem o som do FSS",
                    message: "o Windows não respondeu".into(),
                })?;
            let mut hr = windows_core::HRESULT(0);
            let mut interface = None;
            op.GetActivateResult(&mut hr, &mut interface)
                .map_err(win("ativar a captura sem o som do FSS"))?;
            hr.ok().map_err(win("ativar a captura sem o som do FSS"))?;
            let cliente: IAudioClient = interface
                .ok_or_else(|| AudioError::Windows {
                    context: "ativar a captura sem o som do FSS",
                    message: "sem interface".into(),
                })?
                .cast()
                .map_err(win("captura sem o som do FSS"))?;
            // Este modo não tem formato de mistura: o nosso, float 48 kHz estéreo.
            let fmt = WAVEFORMATEX {
                wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
                nChannels: CHANNELS as u16,
                nSamplesPerSec: SAMPLE_RATE,
                nAvgBytesPerSec: SAMPLE_RATE * CHANNELS as u32 * 4,
                nBlockAlign: (CHANNELS * 4) as u16,
                wBitsPerSample: 32,
                cbSize: 0,
            };
            cliente
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK,
                    2_000_000,
                    0,
                    &fmt,
                    None,
                )
                .map_err(win("iniciar a captura sem o som do FSS"))?;
            let captura: IAudioCaptureClient = cliente.GetService().map_err(win("captura do som"))?;
            cliente.Start().map_err(win("começar a captura do som"))?;
            let formato = Formato {
                taxa: SAMPLE_RATE,
                canais: CHANNELS,
                amostra: Amostra::F32,
                bloco: CHANNELS * 4,
            };
            Ok(Self {
                cliente,
                captura,
                conv: Converter::new(SAMPLE_RATE, CHANNELS, SAMPLE_RATE, CHANNELS),
                formato,
                acumulado: Vec::new(),
                inicio: Instant::now(),
                entregues: 0,
                da_placa: 0,
            })
        }
    }

    /// Captura da saída de som `dispositivo` (ver [`output_devices`];
    /// `None`: a padrão do Windows).
    pub fn new(dispositivo: Option<usize>) -> Result<Self, AudioError> {
        let (cliente, formato, fmt) = abrir(dispositivo)?;
        // SAFETY: formato vindo do próprio GetMixFormat; liberado logo depois.
        let r = unsafe {
            let r = cliente.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                2_000_000, // 200 ms de buffer (unidades de 100 ns)
                0,
                fmt,
                None,
            );
            CoTaskMemFree(Some(fmt as *const _));
            r
        };
        r.map_err(win("iniciar a captura do som"))?;
        // SAFETY: cliente inicializado acima.
        let captura: IAudioCaptureClient = unsafe { cliente.GetService() }.map_err(win("captura do som"))?;
        unsafe { cliente.Start() }.map_err(win("começar a captura do som"))?;
        Ok(Self {
            cliente,
            captura,
            conv: Converter::new(formato.taxa, formato.canais, SAMPLE_RATE, CHANNELS),
            formato,
            acumulado: Vec::new(),
            inicio: Instant::now(),
            entregues: 0,
            da_placa: 0,
        })
    }

    /// Segundos de som que vieram da placa até agora (o resto foi silêncio
    /// completado porque nada tocava).
    pub fn device_seconds(&self) -> f64 {
        self.da_placa as f64 / self.formato.taxa as f64
    }

    /// Taxa e canais da placa (para mostrar).
    pub fn device_format(&self) -> (u32, usize) {
        (self.formato.taxa, self.formato.canais)
    }

    /// Quadros de 10 ms prontos desde a última chamada (chame a cada ~10 ms).
    /// Nada tocando: quadros de silêncio no ritmo do relógio.
    pub fn read_frames(&mut self) -> Result<Vec<Vec<f32>>, AudioError> {
        let mut chegou = false;
        loop {
            // SAFETY: cliente iniciado; cada GetBuffer tem o seu ReleaseBuffer.
            let n = unsafe { self.captura.GetNextPacketSize() }.map_err(win("ler o som"))?;
            if n == 0 {
                break;
            }
            let (mut dados, mut quadros, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
            unsafe {
                self.captura
                    .GetBuffer(&mut dados, &mut quadros, &mut flags, None, None)
            }
            .map_err(win("ler o som"))?;
            let bytes = quadros as usize * self.formato.bloco;
            let amostras = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || dados.is_null() {
                vec![0.0; quadros as usize * self.formato.canais]
            } else {
                // SAFETY: a placa garante `quadros × bloco` bytes até o ReleaseBuffer.
                para_f32(unsafe { std::slice::from_raw_parts(dados, bytes) }, &self.formato)
            };
            unsafe { self.captura.ReleaseBuffer(quadros) }.map_err(win("ler o som"))?;
            self.da_placa += quadros as u64;
            self.acumulado.extend(self.conv.process(&amostras));
            chegou = true;
        }
        let tamanho = FRAME_SAMPLES * CHANNELS;
        let mut prontos: Vec<Vec<f32>> = Vec::new();
        while self.acumulado.len() >= tamanho {
            prontos.push(self.acumulado.drain(..tamanho).collect());
        }
        // Nada tocando: o Windows não entrega nada. Completa pelo relógio.
        let esperados = (self.inicio.elapsed().as_millis() / 10) as u64;
        let atraso = esperados.saturating_sub(self.entregues + prontos.len() as u64);
        if !chegou && atraso * 10 >= SILENCIO_DEPOIS.as_millis() as u64 {
            prontos.extend((0..atraso).map(|_| vec![0.0; tamanho]));
        }
        self.entregues += prontos.len() as u64;
        Ok(prontos)
    }
}

impl Drop for LoopbackCapture {
    fn drop(&mut self) {
        // SAFETY: cliente iniciado.
        let _ = unsafe { self.cliente.Stop() };
    }
}

/// Toca áudio de 48 kHz estéreo na saída padrão.
pub struct AudioOutput {
    cliente: IAudioClient,
    render: IAudioRenderClient,
    volume: ISimpleAudioVolume,
    formato: Formato,
    conv: Converter,
    tamanho: u32,
    /// Amostras já convertidas que não couberam no buffer da placa.
    pendente: Vec<f32>,
}

impl AudioOutput {
    /// Abre a saída de som `dispositivo` (ver [`output_devices`]; `None`: a
    /// padrão) com volume `nivel` (0.0 a 1.0) só para o Fluxo.
    pub fn new(nivel: f32, dispositivo: Option<usize>) -> Result<Self, AudioError> {
        let (cliente, formato, fmt) = abrir(dispositivo)?;
        if !matches!(formato.amostra, Amostra::F32 | Amostra::I16) {
            // SAFETY: liberando o formato do GetMixFormat.
            unsafe { CoTaskMemFree(Some(fmt as *const _)) };
            return Err(AudioError::Format("saída de som em formato não suportado".into()));
        }
        // SAFETY: formato vindo do próprio GetMixFormat; liberado logo depois.
        let r = unsafe {
            let r = cliente.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 500_000, 0, fmt, None);
            CoTaskMemFree(Some(fmt as *const _));
            r
        };
        r.map_err(win("iniciar a saída de som"))?;
        // SAFETY: cliente inicializado acima.
        unsafe {
            let tamanho = cliente.GetBufferSize().map_err(win("saída de som"))?;
            let render: IAudioRenderClient = cliente.GetService().map_err(win("saída de som"))?;
            let volume: ISimpleAudioVolume = cliente.GetService().map_err(win("volume"))?;
            volume
                .SetMasterVolume(volume_valido(nivel), std::ptr::null())
                .map_err(win("volume"))?;
            cliente.Start().map_err(win("começar a tocar"))?;
            Ok(Self {
                cliente,
                render,
                volume,
                conv: Converter::new(SAMPLE_RATE, CHANNELS, formato.taxa, formato.canais),
                formato,
                tamanho,
                pendente: Vec::new(),
            })
        }
    }

    /// Quanto som já está na fila da placa, em ms.
    pub fn queued_ms(&self) -> f64 {
        // SAFETY: cliente iniciado.
        let fila = unsafe { self.cliente.GetCurrentPadding() }.unwrap_or(0);
        let pend = (self.pendente.len() / self.formato.canais) as u32;
        (fila + pend) as f64 * 1000.0 / self.formato.taxa as f64
    }

    /// Muda o volume do Fluxo (0.0 a 1.0), sem mexer no do sistema.
    pub fn set_volume(&self, v: f32) {
        // SAFETY: interface válida.
        let _ = unsafe { self.volume.SetMasterVolume(volume_valido(v), std::ptr::null()) };
    }

    /// Enfileira um quadro de 48 kHz estéreo (convertido para a placa).
    pub fn write(&mut self, pcm: &[f32]) -> Result<(), AudioError> {
        self.pendente.extend(self.conv.process(pcm));
        // SAFETY: cliente iniciado; GetBuffer/ReleaseBuffer em par, com no
        // máximo o espaço livre.
        unsafe {
            let livre = self.tamanho - self.cliente.GetCurrentPadding().map_err(win("tocar"))?;
            let n = livre.min((self.pendente.len() / self.formato.canais) as u32);
            if n == 0 {
                return Ok(());
            }
            let destino = self.render.GetBuffer(n).map_err(win("tocar"))?;
            let amostras = n as usize * self.formato.canais;
            match self.formato.amostra {
                Amostra::F32 => {
                    std::ptr::copy_nonoverlapping(self.pendente.as_ptr(), destino.cast::<f32>(), amostras)
                }
                _ => {
                    let d = std::slice::from_raw_parts_mut(destino.cast::<i16>(), amostras);
                    for (o, v) in d.iter_mut().zip(&self.pendente) {
                        *o = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
                    }
                }
            }
            self.render.ReleaseBuffer(n, 0).map_err(win("tocar"))?;
            self.pendente.drain(..amostras);
        }
        Ok(())
    }
}

fn volume_valido(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        // SAFETY: cliente iniciado.
        let _ = unsafe { self.cliente.Stop() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amostras_da_placa_viram_f32() {
        let f = |amostra| Formato {
            taxa: 48_000,
            canais: 1,
            amostra,
            bloco: 4,
        };
        assert_eq!(para_f32(&0.5f32.to_le_bytes(), &f(Amostra::F32)), vec![0.5]);
        assert_eq!(para_f32(&i16::MIN.to_le_bytes(), &f(Amostra::I16)), vec![-1.0]);
        // 24 bits: 0x400000 = metade do máximo.
        assert_eq!(para_f32(&[0x00, 0x00, 0x40], &f(Amostra::I24)), vec![0.5]);
        assert_eq!(
            para_f32(&(i32::MIN / 2).to_le_bytes(), &f(Amostra::I32)),
            vec![-0.5]
        );
    }
}
