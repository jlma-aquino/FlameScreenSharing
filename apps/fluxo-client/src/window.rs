//! Janela Win32 mínima: redimensionável, tela cheia sem bordas (F11 ou
//! Alt+Enter; Esc sai), e eventos lidos pelo laço principal.
//!
//! A *window procedure* do Windows é um callback; ela só anota o que
//! aconteceu em [`State`], e o laço principal reage depois de `pump()`.

use std::cell::Cell;
use std::ffi::c_void;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, GetStockObject, MonitorFromWindow, BLACK_BRUSH, HBRUSH, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_F11, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS: PCWSTR = w!("FluxoClient");

/// O que a window procedure anotou desde a última olhada.
#[derive(Default)]
struct State {
    closed: Cell<bool>,
    resized: Cell<bool>,
    toggle_fullscreen: Cell<bool>,
    fullscreen: Cell<bool>,
}

pub struct Window {
    hwnd: HWND,
    /// Endereço fixo (Box) guardado em GWLP_USERDATA; vive até DestroyWindow.
    state: Box<State>,
    /// Estilo e posição antes de entrar em tela cheia.
    saved: Option<(isize, RECT)>,
}

impl Window {
    /// `topmost`: a janela fica sempre por cima das outras (útil para
    /// acompanhar a transmissão enquanto usa o PC, e porque uma janela aberta
    /// por um programa em segundo plano não pode tomar o foco sozinha).
    pub fn new(title: &str, width: u32, height: u32, topmost: bool) -> windows::core::Result<Self> {
        let state = Box::<State>::default();
        // SAFETY: registro de classe e criação de janela com parâmetros
        // válidos; `state` sobrevive à janela (destruída no Drop).
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                // Fundo preto: sem clarão branco antes do primeiro frame.
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassExW(&wc); // 0 se já registrada: tudo bem

            let mut rect = RECT {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            };
            AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, false)?;
            let ex_style = if topmost {
                WS_EX_TOPMOST
            } else {
                WINDOW_EX_STYLE::default()
            };
            let hwnd = CreateWindowExW(
                ex_style,
                CLASS,
                &HSTRING::from(title),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                rect.right - rect.left,
                rect.bottom - rect.top,
                None,
                None,
                Some(instance.into()),
                Some(&*state as *const State as *const c_void),
            )?;
            Ok(Self {
                hwnd,
                state,
                saved: None,
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Processa todas as mensagens pendentes, sem bloquear.
    pub fn pump(&self) {
        let mut msg = MSG::default();
        // SAFETY: laço de mensagens padrão.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    pub fn closed(&self) -> bool {
        self.state.closed.get()
    }

    /// `true` uma vez depois de cada mudança de tamanho.
    pub fn take_resized(&self) -> bool {
        self.state.resized.replace(false)
    }

    /// `true` uma vez depois de F11 / Alt+Enter / Esc em tela cheia.
    pub fn take_fullscreen_toggle(&self) -> bool {
        self.state.toggle_fullscreen.replace(false)
    }

    /// Tamanho da área de desenho, em pixels.
    pub fn client_size(&self) -> (u32, u32) {
        let mut r = RECT::default();
        // SAFETY: janela válida; só escreve em `r`.
        let _ = unsafe { GetClientRect(self.hwnd, &mut r) };
        ((r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32)
    }

    pub fn set_title(&self, title: &str) {
        // SAFETY: janela válida, string terminada em zero (HSTRING).
        let _ = unsafe { SetWindowTextW(self.hwnd, &HSTRING::from(title)) };
    }

    /// Alterna entre janela normal e tela cheia sem bordas no monitor atual.
    pub fn toggle_fullscreen(&mut self) {
        // SAFETY: chamadas de janela sobre um HWND válido.
        unsafe {
            if let Some((style, r)) = self.saved.take() {
                SetWindowLongPtrW(self.hwnd, GWL_STYLE, style);
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_FRAMECHANGED | SWP_NOZORDER,
                );
                self.state.fullscreen.set(false);
                return;
            }
            let mut r = RECT::default();
            let _ = GetWindowRect(self.hwnd, &mut r);
            self.saved = Some((GetWindowLongPtrW(self.hwnd, GWL_STYLE), r));
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST), &mut mi);
            let m = mi.rcMonitor;
            SetWindowLongPtrW(self.hwnd, GWL_STYLE, (WS_POPUP | WS_VISIBLE).0 as isize);
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOP),
                m.left,
                m.top,
                m.right - m.left,
                m.bottom - m.top,
                SWP_FRAMECHANGED,
            );
            self.state.fullscreen.set(true);
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: destrói a janela antes de `state` ser liberado.
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: protocolo padrão de window procedure. O ponteiro em
    // GWLP_USERDATA é o `State` passado em CreateWindowExW, que vive até
    // DestroyWindow (Drop de Window).
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let Some(s) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State).as_ref() else {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        let tecla = wparam.0 as u16;
        match msg {
            WM_CLOSE => {
                // Quem fecha é o laço principal (Drop), depois de terminar o frame.
                s.closed.set(true);
                return LRESULT(0);
            }
            WM_SIZE => s.resized.set(true),
            WM_ERASEBKGND => return LRESULT(1), // a swapchain pinta tudo
            WM_KEYDOWN if tecla == VK_F11.0 => s.toggle_fullscreen.set(true),
            WM_KEYDOWN if tecla == VK_ESCAPE.0 && s.fullscreen.get() => s.toggle_fullscreen.set(true),
            WM_SYSKEYDOWN if tecla == VK_RETURN.0 => {
                s.toggle_fullscreen.set(true);
                return LRESULT(0); // sem o "bip" do Alt+Enter
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}
