//! Quem assiste vê o vídeo no lugar desta janela: ao entrar, a janela do
//! vídeo (do `fluxo-client`) abre na mesma posição e tamanho, e esta se
//! esconde; quando o vídeo fecha, esta volta, onde o vídeo estava. Para quem
//! usa é uma janela só, e o vídeo continua no caminho rápido do cliente
//! (Direct3D direto na tela), sem passar pelo egui.
//!
//! Por HWND e Win32: com a janela escondida o egui não se redesenha, então
//! a volta é feita da thread que lê a saída do cliente. Fora do Windows não
//! faz nada (o cliente portátil abre a janela dele, separada).

/// Posição e tamanho externos de uma janela: x, y, largura, altura.
pub type Retangulo = (i32, i32, i32, i32);

/// `Janela: X,Y,L,A`, que o cliente imprime ao fechar.
pub fn ler_retangulo(linha: &str) -> Option<(i32, i32, i32, i32)> {
    let resto = linha.trim().strip_prefix("Janela: ")?;
    let n: Vec<i32> = resto
        .split(',')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    match n[..] {
        [x, y, l, a] if l > 0 && a > 0 => Some((x, y, l, a)),
        _ => None,
    }
}

#[cfg(windows)]
mod win {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, GetWindowRect, SetForegroundWindow, SetWindowPos, ShowWindow, SWP_NOZORDER,
        SW_HIDE, SW_SHOW,
    };

    fn hwnd(h: isize) -> HWND {
        HWND(h as *mut _)
    }

    /// O HWND da janela do eframe.
    pub fn hwnd_de(cc: &eframe::CreationContext<'_>) -> Option<isize> {
        match cc.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
            _ => None,
        }
    }

    /// Posição e tamanho externos (x, y, largura, altura), em pixels de tela.
    pub fn retangulo(h: isize) -> Option<(i32, i32, i32, i32)> {
        let mut r = RECT::default();
        // SAFETY: só lê o retângulo de uma janela; HWND inválido dá erro.
        unsafe { GetWindowRect(hwnd(h), &mut r) }.ok()?;
        Some((r.left, r.top, r.right - r.left, r.bottom - r.top))
    }

    pub fn esconder(h: isize) {
        // SAFETY: ShowWindow aceita qualquer HWND (inválido: não faz nada).
        let _ = unsafe { ShowWindow(hwnd(h), SW_HIDE) };
    }

    /// Mostra de novo, onde o vídeo estava (se souber).
    pub fn mostrar(h: isize, onde: Option<(i32, i32, i32, i32)>) {
        // SAFETY: chamadas de janela; funcionam de qualquer thread.
        unsafe {
            if let Some((x, y, l, a)) = onde {
                let _ = SetWindowPos(hwnd(h), None, x, y, l, a, SWP_NOZORDER);
            }
            let _ = ShowWindow(hwnd(h), SW_SHOW);
            let _ = SetForegroundWindow(hwnd(h));
        }
    }

    /// Deixa a janela do processo `pid` vir para a frente (o Windows só
    /// deixa quem está em primeiro plano passar o foco adiante).
    pub fn permitir_foco(pid: u32) {
        // SAFETY: só muda a permissão de foco.
        let _ = unsafe { AllowSetForegroundWindow(pid) };
    }
}

#[cfg(windows)]
pub use win::*;

#[cfg(not(windows))]
mod outro {
    pub fn hwnd_de(_: &eframe::CreationContext<'_>) -> Option<isize> {
        None
    }
    pub fn retangulo(_: isize) -> Option<(i32, i32, i32, i32)> {
        None
    }
    pub fn esconder(_: isize) {}
    pub fn mostrar(_: isize, _: Option<(i32, i32, i32, i32)>) {}
    pub fn permitir_foco(_: u32) {}
}

#[cfg(not(windows))]
pub use outro::*;

#[cfg(test)]
mod tests {
    use super::ler_retangulo;

    #[test]
    fn linha_do_cliente_ao_fechar() {
        assert_eq!(ler_retangulo("Janela: -8,0,1296,739"), Some((-8, 0, 1296, 739)));
        assert_eq!(
            ler_retangulo("  Janela: 10, 20, 800, 600 "),
            Some((10, 20, 800, 600))
        );
        assert_eq!(ler_retangulo("Janela: 1,2,3"), None);
        assert_eq!(ler_retangulo("Janela: 1,2,0,4"), None);
        assert_eq!(ler_retangulo("Volume: 30%"), None);
    }
}
