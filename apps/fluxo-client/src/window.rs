//! Janela Win32 mínima: redimensionável, tela cheia sem bordas (F11, Alt+Enter,
//! F ou clique duplo; Esc sai), F2 mostra/esconde o painel, M liga e desliga o
//! som, setas mudam o volume, e o mouse vai para a barra de controles
//! (`controles.rs`).
//!
//! A *window procedure* do Windows é um callback; ela só anota o que
//! aconteceu em [`State`], e o laço principal reage depois de `pump()`.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, GetStockObject, MonitorFromWindow, BLACK_BRUSH, HBRUSH, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_DOWN, VK_ESCAPE, VK_F11,
    VK_F2, VK_RETURN, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::controles::Entrada;

const CLASS: PCWSTR = w!("FluxoClient");

/// O mouse saiu da janela (pedido com `TrackMouseEvent`); fica em `Win32_UI_Controls`,
/// que o resto do projeto não usa.
const WM_MOUSELEAVE: u32 = 0x02A3;

/// O que a window procedure anotou desde a última olhada.
#[derive(Default)]
struct State {
    closed: Cell<bool>,
    resized: Cell<bool>,
    toggle_fullscreen: Cell<bool>,
    toggle_painel: Cell<bool>,
    toggle_mudo: Cell<bool>,
    /// Setas: passos de volume (+ sobe).
    volume_passos: Cell<i32>,
    fullscreen: Cell<bool>,
    /// Mouse e roda, na ordem em que chegaram.
    entradas: RefCell<Vec<Entrada>>,
    /// Já pediu ao Windows o aviso de quando o mouse sair.
    rastreando: Cell<bool>,
    /// Cursor escondido sobre o vídeo (barra escondida, mouse parado).
    sem_cursor: Cell<bool>,
}

pub struct Window {
    hwnd: HWND,
    /// Endereço fixo (Box) guardado em GWLP_USERDATA; vive até DestroyWindow.
    state: Box<State>,
    /// Estilo e posição antes de entrar em tela cheia.
    saved: Option<(isize, RECT)>,
}

/// Deixa o processo saber o DPI de cada monitor: sem isso, numa tela com
/// escala (125%, 150%) o Windows desenharia o vídeo pequeno e esticaria,
/// borrado. Chamar antes de criar qualquer janela.
pub fn enable_dpi_awareness() {
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    // SAFETY: só muda um modo do processo; falha (já definido) não importa.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

impl Window {
    /// `topmost`: a janela fica sempre por cima das outras (útil para
    /// acompanhar a transmissão enquanto usa o PC, e porque uma janela aberta
    /// por um programa em segundo plano não pode tomar o foco sozinha).
    /// `posicao`: retângulo externo (x, y, largura, altura) em pixels de
    /// tela; sem ela, `width` × `height` de área de desenho onde o Windows
    /// quiser.
    pub fn new(
        title: &str,
        width: u32,
        height: u32,
        topmost: bool,
        posicao: Option<(i32, i32, i32, i32)>,
    ) -> windows::core::Result<Self> {
        let state = Box::<State>::default();
        // SAFETY: registro de classe e criação de janela com parâmetros
        // válidos; `state` sobrevive à janela (destruída no Drop).
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                // Clique duplo alterna a tela cheia.
                style: CS_DBLCLKS,
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                // Fundo preto: sem clarão branco antes do primeiro frame.
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassExW(&wc); // 0 se já registrada: tudo bem

            let (x, y, largura, altura) = match posicao {
                Some(p) => p,
                None => {
                    let mut rect = RECT {
                        left: 0,
                        top: 0,
                        right: width as i32,
                        bottom: height as i32,
                    };
                    AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, false)?;
                    (
                        CW_USEDEFAULT,
                        CW_USEDEFAULT,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                    )
                }
            };
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
                x,
                y,
                largura,
                altura,
                None,
                None,
                Some(instance.into()),
                Some(&*state as *const State as *const c_void),
            )?;
            // Aberto por outro programa (terminal, script), o Windows aplica
            // na primeira exibição o modo de quem nos lançou — às vezes
            // minimizado. A segunda chamada vale sempre: janela normal.
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
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

    /// Espera até chegar uma mensagem (ou `timeout`) e processa todas as
    /// pendentes.
    pub fn wait_messages(&self, timeout: std::time::Duration) {
        // SAFETY: espera sem handles, só pela fila de mensagens desta thread.
        unsafe {
            MsgWaitForMultipleObjects(None, false, timeout.as_millis() as u32, QS_ALLINPUT);
        }
        self.pump();
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

    /// Fecha como se o X tivesse sido clicado (botão Sair da barra).
    pub fn close(&self) {
        self.state.closed.set(true);
    }

    /// `true` uma vez depois de cada mudança de tamanho.
    pub fn take_resized(&self) -> bool {
        self.state.resized.replace(false)
    }

    /// `true` uma vez depois de cada F2 (mostra/esconde o painel de estatísticas).
    pub fn take_painel_toggle(&self) -> bool {
        self.state.toggle_painel.replace(false)
    }

    /// `true` uma vez depois de F11 / Alt+Enter / F / Esc em tela cheia.
    pub fn take_fullscreen_toggle(&self) -> bool {
        self.state.toggle_fullscreen.replace(false)
    }

    /// `true` uma vez depois de cada M.
    pub fn take_mudo_toggle(&self) -> bool {
        self.state.toggle_mudo.replace(false)
    }

    /// Passos de volume das setas desde a última olhada.
    pub fn take_volume_passos(&self) -> i32 {
        self.state.volume_passos.replace(0)
    }

    /// Eventos do mouse desde a última olhada.
    pub fn take_entradas(&self) -> Vec<Entrada> {
        std::mem::take(&mut *self.state.entradas.borrow_mut())
    }

    pub fn is_fullscreen(&self) -> bool {
        self.state.fullscreen.get()
    }

    /// Esconde (ou mostra) o cursor sobre o vídeo, já.
    pub fn set_esconder_cursor(&self, esconder: bool) {
        if self.state.sem_cursor.replace(esconder) == esconder {
            return;
        }
        // SAFETY: a janela é desta thread; SetCursor vale até o próximo
        // WM_SETCURSOR, que repete a escolha.
        unsafe {
            let cursor = if esconder {
                None
            } else {
                LoadCursorW(None, IDC_ARROW).ok()
            };
            SetCursor(cursor);
        }
    }

    /// Escala do monitor da janela (1,0 = 100%).
    pub fn escala(&self) -> f32 {
        // SAFETY: janela válida.
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        if dpi == 0 {
            1.0
        } else {
            dpi as f32 / 96.0
        }
    }

    /// Retângulo externo da janela fora da tela cheia (x, y, largura, altura).
    pub fn retangulo_normal(&self) -> (i32, i32, i32, i32) {
        let r = match &self.saved {
            Some((_, r)) => *r,
            None => {
                let mut r = RECT::default();
                // SAFETY: janela válida; só escreve em `r`.
                let _ = unsafe { GetWindowRect(self.hwnd, &mut r) };
                r
            }
        };
        (r.left, r.top, r.right - r.left, r.bottom - r.top)
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

/// Posição do mouse numa mensagem de mouse (pode ser negativa, fora da janela).
fn ponto(lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16;
    (x as f32, y as f32)
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
        let anotar = |e: Entrada| s.entradas.borrow_mut().push(e);
        match msg {
            WM_CLOSE => {
                // Quem fecha é o laço principal (Drop), depois de terminar o frame.
                s.closed.set(true);
                return LRESULT(0);
            }
            WM_SIZE => s.resized.set(true),
            WM_ERASEBKGND => return LRESULT(1), // a swapchain pinta tudo
            WM_DPICHANGED => {
                // Mudou de monitor (ou de escala): o Windows sugere o tamanho novo.
                let r = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                return LRESULT(0);
            }
            WM_SETCURSOR if s.sem_cursor.get() && (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
                SetCursor(None);
                return LRESULT(1);
            }
            WM_MOUSEMOVE => {
                if !s.rastreando.replace(true) {
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut t);
                }
                let (x, y) = ponto(lparam);
                anotar(Entrada::Mover(x, y));
            }
            WM_MOUSELEAVE => {
                s.rastreando.set(false);
                anotar(Entrada::Saiu);
            }
            WM_LBUTTONDOWN => {
                // Segura o mouse: arrastar o volume para fora da janela continua valendo.
                SetCapture(hwnd);
                let (x, y) = ponto(lparam);
                anotar(Entrada::Apertar(x, y));
            }
            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                let (x, y) = ponto(lparam);
                anotar(Entrada::Soltar(x, y));
            }
            WM_LBUTTONDBLCLK => {
                let (x, y) = ponto(lparam);
                anotar(Entrada::CliqueDuplo(x, y));
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16;
                // Touchpad manda passos menores que 120: cada um ainda vale um passo.
                let passos = match delta as i32 / 120 {
                    0 => (delta as i32).signum(),
                    p => p.clamp(-5, 5),
                };
                anotar(Entrada::Roda(passos));
            }
            WM_KEYDOWN if tecla == VK_F11.0 || tecla == b'F' as u16 => s.toggle_fullscreen.set(true),
            WM_KEYDOWN if tecla == VK_F2.0 => s.toggle_painel.set(true),
            WM_KEYDOWN if tecla == b'M' as u16 => s.toggle_mudo.set(true),
            WM_KEYDOWN if tecla == VK_UP.0 => s.volume_passos.set(s.volume_passos.get() + 1),
            WM_KEYDOWN if tecla == VK_DOWN.0 => s.volume_passos.set(s.volume_passos.get() - 1),
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
