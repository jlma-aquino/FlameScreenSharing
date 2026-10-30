//! Implementação Windows: DXGI Desktop Duplication + Direct3D 11.
//!
//! ## Ciclo de vida
//!
//! ```text
//! DesktopCapturer::new(i)
//!   ├─ encontra a placa (adapter) e a saída (output) do monitor i
//!   ├─ cria um ID3D11Device NESSA placa  (obrigatório: a duplicação precisa
//!   │                                     do device da GPU dona do monitor)
//!   └─ IDXGIOutput1::DuplicateOutput      → IDXGIOutputDuplication
//!
//! next_frame(timeout)
//!   ├─ AcquireNextFrame                   → textura do frame (na GPU)
//!   │    ├─ WAIT_TIMEOUT  → Ok(None)      (tela não mudou)
//!   │    └─ ACCESS_LOST   → recria na próxima chamada
//!   └─ devolve Frame (guarda); ao sair de escopo chama ReleaseFrame
//! ```
//!
//! Regra importante da API: só pode existir **um** frame adquirido por vez,
//! e o Windows não entrega o próximo enquanto este não for liberado. O tipo
//! [`Frame`] garante isso em tempo de compilação: ele empresta o capturador
//! de forma exclusiva (`&mut`) e libera o frame no `Drop`.

use windows::core::Interface;
use windows::Win32::Foundation::{E_ACCESSDENIED, HMODULE};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1, IDXGIOutputDuplication,
    IDXGIResource, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_CURRENTLY_AVAILABLE, DXGI_ERROR_NOT_FOUND,
    DXGI_ERROR_SESSION_DISCONNECTED, DXGI_ERROR_UNSUPPORTED, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO,
};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};

use crate::{CaptureError, CpuImageInfo, FrameInfo, MonitorInfo};

impl From<windows::core::Error> for CaptureError {
    fn from(e: windows::core::Error) -> Self {
        CaptureError::Windows {
            hresult: e.code().0,
            message: e.message().to_string(),
        }
    }
}

/// Faz o processo enxergar pixels físicos em telas com escala (125%, 150%...).
///
/// Sem isso, o Windows "mente" as coordenadas e tamanhos dos monitores para
/// programas antigos. Chame **uma vez, no início do `main`**, antes de
/// qualquer outra coisa. Chamar de novo é inofensivo.
pub fn enable_dpi_awareness() {
    // Falha se já foi definido (ex.: pelo manifesto do .exe) — tudo bem.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

/// Converte uma string UTF-16 terminada em zero (formato do Windows).
fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// Percorre todas as placas e saídas ligadas à área de trabalho, chamando
/// `visit` para cada uma, na mesma ordem usada pelos índices de monitor.
/// Para cedo se `visit` devolver `Some`.
fn walk_outputs<T>(
    mut visit: impl FnMut(&IDXGIAdapter1, &IDXGIOutput, MonitorInfo) -> Option<T>,
) -> Result<Option<T>, CaptureError> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut index = 0usize;
        let mut ai = 0u32;
        loop {
            let adapter = match factory.EnumAdapters1(ai) {
                Ok(a) => a,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(e) => return Err(e.into()),
            };
            let adapter_name = wide_to_string(&adapter.GetDesc1()?.Description);

            let mut oi = 0u32;
            loop {
                let output = match adapter.EnumOutputs(oi) {
                    Ok(o) => o,
                    Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                    Err(e) => return Err(e.into()),
                };
                oi += 1;

                let d = output.GetDesc()?;
                if !d.AttachedToDesktop.as_bool() {
                    continue;
                }
                let r = d.DesktopCoordinates;
                let info = MonitorInfo {
                    index,
                    device_name: wide_to_string(&d.DeviceName),
                    adapter_name: adapter_name.clone(),
                    x: r.left,
                    y: r.top,
                    width: (r.right - r.left).max(0) as u32,
                    height: (r.bottom - r.top).max(0) as u32,
                };
                index += 1;
                if let Some(found) = visit(&adapter, &output, info) {
                    return Ok(Some(found));
                }
            }
            ai += 1;
        }
        Ok(None)
    }
}

/// Lista os monitores disponíveis para captura.
///
/// O índice de cada [`MonitorInfo`] é o que se passa para
/// [`DesktopCapturer::new`]. A ordem é: primeira placa de vídeo, suas saídas;
/// segunda placa, suas saídas; e assim por diante.
pub fn list_monitors() -> Result<Vec<MonitorInfo>, CaptureError> {
    let mut all = Vec::new();
    walk_outputs::<()>(|_, _, info| {
        all.push(info);
        None
    })?;
    Ok(all)
}

/// Cria o device D3D11 na placa indicada.
fn create_device(adapter: &IDXGIAdapter1) -> Result<(ID3D11Device, ID3D11DeviceContext), CaptureError> {
    // VIDEO_SUPPORT habilita as APIs de vídeo do D3D11 (conversão BGRA→NV12
    // do encoder e Media Foundation). Se o driver recusar, segue sem ela: a
    // captura funciona igual e o encoder reporta o problema ao ser criado.
    let com_video = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
    create_device_with_flags(adapter, com_video)
        .or_else(|_| create_device_with_flags(adapter, D3D11_CREATE_DEVICE_BGRA_SUPPORT))
}

fn create_device_with_flags(
    adapter: &IDXGIAdapter1,
    flags: D3D11_CREATE_DEVICE_FLAG,
) -> Result<(ID3D11Device, ID3D11DeviceContext), CaptureError> {
    let levels: [D3D_FEATURE_LEVEL; 2] = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    unsafe {
        // Com um adapter explícito, o tipo de driver tem que ser UNKNOWN.
        // BGRA_SUPPORT é necessário para interoperar com Direct2D/encoders.
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            flags,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    match (device, context) {
        (Some(d), Some(c)) => Ok((d, c)),
        _ => Err(CaptureError::NotAvailable(
            "D3D11CreateDevice não devolveu o device".into(),
        )),
    }
}

/// Traduz erros de `DuplicateOutput` para algo acionável.
fn map_duplicate_error(e: windows::core::Error) -> CaptureError {
    let code = e.code();
    if code == DXGI_ERROR_NOT_CURRENTLY_AVAILABLE {
        CaptureError::NotAvailable("limite de capturas simultâneas deste monitor atingido".into())
    } else if code == DXGI_ERROR_UNSUPPORTED {
        CaptureError::NotAvailable(
            "o driver não suporta; em notebooks com duas GPUs, rode o programa na GPU ligada ao monitor"
                .into(),
        )
    } else if code == E_ACCESSDENIED || code == DXGI_ERROR_SESSION_DISCONNECTED {
        // Tela segura (UAC, Ctrl+Alt+Del, bloqueio) ou sessão remota trocada.
        CaptureError::AccessLost
    } else {
        e.into()
    }
}

/// Texturas de cópia para a CPU, reaproveitadas entre frames.
struct Staging {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
}

/// Captura contínua de um monitor.
///
/// Crie um por monitor. Não é `Send`: use sempre na mesma thread em que foi
/// criado (o pipeline terá uma thread dedicada só para captura).
pub struct DesktopCapturer {
    monitor: MonitorInfo,
    output: IDXGIOutput1,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    /// `None` depois de um ACCESS_LOST; recriado em `next_frame`.
    duplication: Option<IDXGIOutputDuplication>,
    staging: Option<Staging>,
}

impl DesktopCapturer {
    /// Começa a capturar o monitor de índice `monitor_index`
    /// (ver [`list_monitors`]).
    pub fn new(monitor_index: usize) -> Result<Self, CaptureError> {
        let found = walk_outputs(|adapter, output, info| {
            (info.index == monitor_index).then(|| (adapter.clone(), output.clone(), info))
        })?;
        let (adapter, output, monitor) = found.ok_or(CaptureError::NoSuchMonitor(monitor_index))?;

        let (device, context) = create_device(&adapter)?;
        let output: IDXGIOutput1 = output.cast()?;

        let mut cap = Self {
            monitor,
            output,
            device,
            context,
            duplication: None,
            staging: None,
        };
        cap.recreate_duplication()?;
        Ok(cap)
    }

    /// Informações do monitor sendo capturado.
    pub fn monitor(&self) -> &MonitorInfo {
        &self.monitor
    }

    /// O device D3D11 usado na captura. O encoder de hardware deve usar
    /// **este mesmo device** para receber as texturas sem cópia.
    pub fn d3d_device(&self) -> &ID3D11Device {
        &self.device
    }

    /// O contexto imediato do device (para cópias/conversões na GPU).
    pub fn d3d_context(&self) -> &ID3D11DeviceContext {
        &self.context
    }

    fn recreate_duplication(&mut self) -> Result<(), CaptureError> {
        self.duplication = None;
        let dup = unsafe { self.output.DuplicateOutput(&self.device) }.map_err(map_duplicate_error)?;
        self.duplication = Some(dup);
        Ok(())
    }

    /// Espera até `timeout_ms` por um frame novo.
    ///
    /// - `Ok(Some(frame))`: chegou frame (imagem nova ou só o cursor mexeu;
    ///   veja [`FrameInfo::desktop_updated`]).
    /// - `Ok(None)`: nada mudou na tela dentro do prazo.
    /// - `Err(AccessLost)`: captura interrompida; chame de novo depois de um
    ///   tempinho e ela é recriada automaticamente.
    pub fn next_frame(&mut self, timeout_ms: u32) -> Result<Option<Frame<'_>>, CaptureError> {
        if self.duplication.is_none() {
            self.recreate_duplication()?;
        }
        let dup = self.duplication.clone().expect("duplicação recém-criada");

        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        match unsafe { dup.AcquireNextFrame(timeout_ms, &mut info, &mut resource) } {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                self.duplication = None;
                return Err(CaptureError::AccessLost);
            }
            Err(e) => return Err(e.into()),
        }

        // A partir daqui o frame está adquirido: qualquer erro precisa
        // liberá-lo antes de retornar.
        let texture = match resource.map(|r| r.cast::<ID3D11Texture2D>()) {
            Some(Ok(t)) => t,
            Some(Err(e)) => {
                let _ = unsafe { dup.ReleaseFrame() };
                return Err(e.into());
            }
            None => {
                let _ = unsafe { dup.ReleaseFrame() };
                return Err(CaptureError::NotAvailable("AcquireNextFrame sem textura".into()));
            }
        };

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };

        // Mantém o tamanho do monitor atualizado (mudança de resolução).
        self.monitor.width = desc.Width;
        self.monitor.height = desc.Height;

        Ok(Some(Frame {
            cap: self,
            texture,
            desc,
            info: FrameInfo {
                present_time_qpc: info.LastPresentTime,
                accumulated_frames: info.AccumulatedFrames,
                desktop_updated: info.LastPresentTime != 0,
            },
        }))
    }
}

/// Um frame adquirido. Enquanto ele existir, nenhum outro frame chega.
/// Solte-o (saia do escopo) o quanto antes.
pub struct Frame<'a> {
    cap: &'a mut DesktopCapturer,
    texture: ID3D11Texture2D,
    desc: D3D11_TEXTURE2D_DESC,
    info: FrameInfo,
}

impl Frame<'_> {
    /// Metadados do frame.
    pub fn info(&self) -> FrameInfo {
        self.info
    }

    /// Largura em pixels.
    pub fn width(&self) -> u32 {
        self.desc.Width
    }

    /// Altura em pixels.
    pub fn height(&self) -> u32 {
        self.desc.Height
    }

    /// A textura do frame na GPU (formato BGRA 8 bits).
    ///
    /// **Válida só enquanto o `Frame` existir.** Para usá-la depois (ex.:
    /// encoder assíncrono), copie para uma textura sua com
    /// `CopyResource` antes de soltar o frame.
    pub fn texture(&self) -> &ID3D11Texture2D {
        &self.texture
    }

    /// Copia os pixels para a RAM em `out` (BGRA, linhas contíguas, sem
    /// padding). Lento (~2–5 ms em 1080p): use só para debug/screenshot,
    /// nunca no caminho principal de streaming.
    pub fn copy_to_cpu(&mut self, out: &mut Vec<u8>) -> Result<CpuImageInfo, CaptureError> {
        if self.desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(CaptureError::NotAvailable(format!(
                "formato de pixel {:?} não suportado em copy_to_cpu",
                self.desc.Format
            )));
        }
        let (w, h) = (self.desc.Width, self.desc.Height);
        let cap = &mut *self.cap;

        // (Re)cria a textura de staging se o tamanho mudou.
        let reuse = matches!(&cap.staging, Some(s) if s.width == w && s.height == h);
        if !reuse {
            let sdesc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: self.desc.Format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut tex: Option<ID3D11Texture2D> = None;
            unsafe { cap.device.CreateTexture2D(&sdesc, None, Some(&mut tex))? };
            let texture = tex.ok_or_else(|| CaptureError::NotAvailable("CreateTexture2D vazio".into()))?;
            cap.staging = Some(Staging {
                texture,
                width: w,
                height: h,
            });
        }
        let staging = &cap.staging.as_ref().expect("staging criada acima").texture;

        let row_bytes = w as usize * 4;
        out.resize(row_bytes * h as usize, 0);

        unsafe {
            cap.context.CopyResource(staging, &self.texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            cap.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            // A GPU pode alinhar cada linha (RowPitch ≥ largura*4):
            // copiamos linha a linha para tirar esse padding.
            let src = mapped.pData as *const u8;
            for y in 0..h as usize {
                std::ptr::copy_nonoverlapping(
                    src.add(y * mapped.RowPitch as usize),
                    out.as_mut_ptr().add(y * row_bytes),
                    row_bytes,
                );
            }
            cap.context.Unmap(staging, 0);
        }

        Ok(CpuImageInfo {
            width: w,
            height: h,
            stride: row_bytes,
        })
    }
}

impl Drop for Frame<'_> {
    fn drop(&mut self) {
        let lost = match &self.cap.duplication {
            Some(dup) => matches!(
                unsafe { dup.ReleaseFrame() },
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST
            ),
            None => false,
        };
        if lost {
            self.cap.duplication = None;
        }
    }
}
