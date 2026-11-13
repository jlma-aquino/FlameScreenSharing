//! Captura de uma **janela** (Windows.Graphics.Capture, Windows 10 1903+):
//! [`WindowCapturer`] e [`list_windows`].
//!
//! O Desktop Duplication (`dxgi.rs`) só pega o monitor inteiro. Para mostrar
//! só um programa, o Windows tem esta outra API, que captura a janela mesmo
//! com outras por cima. O frame também chega como textura na GPU; ele é
//! copiado (na GPU, microssegundos) para uma textura nossa de tamanho par,
//! o que o NV12 do encoder exige, e o frame do Windows volta na hora.
//!
//! O cursor vem desenhado na imagem (a posição à parte que o cliente
//! desenha é a do monitor, que aqui não faz sentido).

use std::time::{Duration, Instant};

use windows::core::{Interface, BOOL};
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BOX,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IsIconic,
    IsWindow, IsWindowVisible, GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW,
};

use crate::CaptureError;

/// Uma janela que dá para compartilhar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    /// O HWND, como número (vai na linha de comando do host).
    pub hwnd: isize,
    pub title: String,
    /// Minimizada: não manda imagem até ser restaurada.
    pub minimized: bool,
}

/// As janelas de programas abertas (visíveis, com título, que não são
/// ferramentas nem janelas escondidas pelo sistema), na ordem do Windows.
pub fn list_windows() -> Vec<WindowInfo> {
    let mut lista: Vec<WindowInfo> = Vec::new();
    extern "system" fn uma(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` é o `&mut Vec` passado a EnumWindows logo abaixo,
        // vivo durante toda a enumeração; o resto só lê a janela.
        unsafe {
            let lista = &mut *(lparam.0 as *mut Vec<WindowInfo>);
            if !IsWindowVisible(hwnd).as_bool() || GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
                return BOOL(1);
            }
            if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                return BOOL(1);
            }
            let mut escondida = 0u32;
            let _ = DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                &mut escondida as *mut u32 as *mut _,
                std::mem::size_of::<u32>() as u32,
            );
            if escondida != 0 {
                return BOOL(1);
            }
            let n = GetWindowTextLengthW(hwnd);
            if n <= 0 {
                return BOOL(1);
            }
            let mut buf = vec![0u16; n as usize + 1];
            let lidos = GetWindowTextW(hwnd, &mut buf);
            let title = String::from_utf16_lossy(&buf[..lidos.max(0) as usize]);
            if !title.trim().is_empty() {
                lista.push(WindowInfo {
                    hwnd: hwnd.0 as isize,
                    title,
                    minimized: IsIconic(hwnd).as_bool(),
                });
            }
            BOOL(1)
        }
    }
    // SAFETY: o callback acima só usa `lista` enquanto EnumWindows roda.
    let _ = unsafe { EnumWindows(Some(uma), LPARAM(&mut lista as *mut Vec<WindowInfo> as isize)) };
    lista
}

/// Captura contínua de uma janela.
pub struct WindowCapturer {
    hwnd: HWND,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    d3d: IDirect3DDevice,
    _item: GraphicsCaptureItem,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    /// Tamanho atual do pool (o da janela no último frame).
    pool_size: SizeInt32,
    /// Cópia de tamanho par do último frame.
    copia: Option<ID3D11Texture2D>,
    tamanho: (u32, u32),
}

fn par(v: i32) -> u32 {
    (v.max(2) as u32) & !1
}

impl WindowCapturer {
    /// Começa a capturar a janela `hwnd` (de [`list_windows`]).
    pub fn new(hwnd: isize) -> Result<Self, CaptureError> {
        let hwnd = HWND(hwnd as *mut _);
        // SAFETY: chamadas COM/WinRT com parâmetros válidos; o HWND é conferido antes.
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() {
                return Err(CaptureError::Window("essa janela não existe mais".into()));
            }
            if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
                return Err(CaptureError::Window(
                    "captura de janela precisa do Windows 10 1903 ou mais novo".into(),
                ));
            }
            let (device, context) = criar_device()?;
            let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&device.cast::<IDXGIDevice>()?)?;
            let d3d: IDirect3DDevice = inspectable.cast()?;
            let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let item: GraphicsCaptureItem = interop.CreateForWindow(hwnd)?;
            let pool_size = item.Size()?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &d3d,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                pool_size,
            )?;
            let session = pool.CreateCaptureSession(&item)?;
            let _ = session.SetIsCursorCaptureEnabled(true);
            // Sem a borda amarela (Windows 11; no 10 não existe e falha: tudo bem).
            let _ = session.SetIsBorderRequired(false);
            session.StartCapture()?;
            Ok(Self {
                hwnd,
                device,
                context,
                d3d,
                _item: item,
                pool,
                session,
                pool_size,
                copia: None,
                tamanho: (par(pool_size.Width), par(pool_size.Height)),
            })
        }
    }

    /// O device D3D11 da captura: o encoder usa este mesmo.
    pub fn d3d_device(&self) -> &ID3D11Device {
        &self.device
    }

    pub fn d3d_context(&self) -> &ID3D11DeviceContext {
        &self.context
    }

    /// Tamanho das imagens entregues (par; segue a janela).
    pub fn size(&self) -> (u32, u32) {
        self.tamanho
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd.0 as isize
    }

    /// Espera até `timeout_ms` por um frame novo (a janela mudou). `Ok(None)`:
    /// nada mudou. Erro: a janela fechou.
    pub fn next_frame(&mut self, timeout_ms: u32) -> Result<Option<WindowFrame>, CaptureError> {
        let limite = Instant::now() + Duration::from_millis(timeout_ms as u64);
        loop {
            // SAFETY: só consulta se o HWND ainda é uma janela.
            if !unsafe { IsWindow(Some(self.hwnd)) }.as_bool() {
                return Err(CaptureError::Window("a janela compartilhada fechou".into()));
            }
            if let Ok(frame) = self.pool.TryGetNextFrame() {
                let tamanho = frame.ContentSize()?;
                if tamanho.Width != self.pool_size.Width || tamanho.Height != self.pool_size.Height {
                    // A janela mudou de tamanho: o pool acompanha; o próximo frame já vem certo.
                    self.pool_size = tamanho;
                    self.pool
                        .Recreate(&self.d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, tamanho)?;
                    continue;
                }
                let (w, h) = (par(tamanho.Width), par(tamanho.Height));
                // Minimizada (ou quase): nada a mostrar.
                if w < 16 || h < 16 {
                    return Ok(None);
                }
                let superficie = frame.Surface()?;
                // SAFETY: a superfície do frame é uma textura D3D11 do nosso device.
                let origem: ID3D11Texture2D = unsafe {
                    superficie
                        .cast::<IDirect3DDxgiInterfaceAccess>()?
                        .GetInterface()?
                };
                let copia = self.copia_de(&origem, w, h)?;
                let caixa = D3D11_BOX {
                    left: 0,
                    top: 0,
                    front: 0,
                    right: w,
                    bottom: h,
                    back: 1,
                };
                // SAFETY: as duas texturas são do mesmo device e formato; a caixa cabe nas duas.
                unsafe {
                    self.context
                        .CopySubresourceRegion(&copia, 0, 0, 0, 0, &origem, 0, Some(&caixa));
                }
                let _ = frame.Close();
                self.tamanho = (w, h);
                return Ok(Some(WindowFrame { texture: copia }));
            }
            if Instant::now() >= limite {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// A textura de cópia, recriada se o tamanho mudou.
    fn copia_de(
        &mut self,
        origem: &ID3D11Texture2D,
        w: u32,
        h: u32,
    ) -> Result<ID3D11Texture2D, CaptureError> {
        if let Some(c) = &self.copia {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: só lê a descrição.
            unsafe { c.GetDesc(&mut d) };
            if (d.Width, d.Height) == (w, h) {
                return Ok(c.clone());
            }
        }
        let mut d = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: só lê a descrição da origem e cria uma textura parecida.
        let nova = unsafe {
            origem.GetDesc(&mut d);
            d.Width = w;
            d.Height = h;
            d.MipLevels = 1;
            d.ArraySize = 1;
            d.Usage = D3D11_USAGE_DEFAULT;
            d.CPUAccessFlags = 0;
            d.MiscFlags = 0;
            let mut t = None;
            self.device.CreateTexture2D(&d, None, Some(&mut t))?;
            t.ok_or_else(|| CaptureError::Window("CreateTexture2D sem textura".into()))?
        };
        self.copia = Some(nova.clone());
        Ok(nova)
    }
}

impl Drop for WindowCapturer {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// Um frame da janela: a nossa cópia, já solta do Windows.
pub struct WindowFrame {
    texture: ID3D11Texture2D,
}

impl WindowFrame {
    pub fn texture(&self) -> &ID3D11Texture2D {
        &self.texture
    }
}

/// Retângulo atual da janela na tela (para mostrar o tamanho na lista).
pub fn window_size(hwnd: isize) -> Option<(u32, u32)> {
    let mut r = RECT::default();
    // SAFETY: só lê o retângulo.
    unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut r) }.ok()?;
    Some(((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32))
}

fn criar_device() -> Result<(ID3D11Device, ID3D11DeviceContext), CaptureError> {
    let niveis: [D3D_FEATURE_LEVEL; 2] = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    for flags in [
        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    ] {
        let (mut device, mut context) = (None, None);
        // SAFETY: criação de device com parâmetros válidos.
        let r = unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                flags,
                Some(&niveis),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };
        if let (Ok(()), Some(d), Some(c)) = (r, device, context) {
            return Ok((d, c));
        }
    }
    Err(CaptureError::Window("não consegui criar o device D3D11".into()))
}
