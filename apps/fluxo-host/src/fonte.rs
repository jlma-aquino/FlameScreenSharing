//! O que está sendo transmitido: um monitor inteiro (Desktop Duplication) ou
//! uma janela (Windows.Graphics.Capture). O laço do vídeo usa os dois do mesmo
//! jeito: uma textura na GPU por frame, no device da própria captura.

use fluxo_capture::{
    CaptureError, DesktopCapturer, Frame, PointerPosition, PointerShape, WindowCapturer, WindowFrame,
};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};

/// Monitor (índice de `--monitor`) ou janela (HWND).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alvo {
    Tela(usize),
    Janela(isize),
}

pub enum Fonte {
    Tela(DesktopCapturer),
    Janela(WindowCapturer),
}

impl Fonte {
    pub fn abrir(alvo: Alvo) -> Result<Self, CaptureError> {
        Ok(match alvo {
            Alvo::Tela(n) => Fonte::Tela(DesktopCapturer::new(n)?),
            Alvo::Janela(h) => Fonte::Janela(WindowCapturer::new(h)?),
        })
    }

    pub fn alvo(&self) -> Alvo {
        match self {
            Fonte::Tela(c) => Alvo::Tela(c.monitor().index),
            Fonte::Janela(c) => Alvo::Janela(c.hwnd()),
        }
    }

    /// Tamanho das imagens capturadas.
    pub fn tamanho(&self) -> (u32, u32) {
        match self {
            Fonte::Tela(c) => (c.monitor().width, c.monitor().height),
            Fonte::Janela(c) => c.size(),
        }
    }

    pub fn d3d_device(&self) -> &ID3D11Device {
        match self {
            Fonte::Tela(c) => c.d3d_device(),
            Fonte::Janela(c) => c.d3d_device(),
        }
    }

    pub fn d3d_context(&self) -> &ID3D11DeviceContext {
        match self {
            Fonte::Tela(c) => c.d3d_context(),
            Fonte::Janela(c) => c.d3d_context(),
        }
    }

    /// Solta o que impediria outra captura do mesmo monitor (ver
    /// `DesktopCapturer::release_duplication`); a de janela não prende nada.
    pub fn soltar(&mut self) {
        if let Fonte::Tela(c) = self {
            c.release_duplication();
        }
    }

    /// Para a linha "Tela: …" que a janela do FSS lê (`apps/fluxo/src/saida.rs`).
    pub fn linha(&self) -> String {
        let (w, h) = self.tamanho();
        match self {
            Fonte::Tela(c) => format!("Tela: monitor {} ({w}x{h})", c.monitor().index + 1),
            Fonte::Janela(c) => format!("Tela: janela {} ({w}x{h})", c.hwnd()),
        }
    }

    /// Para a linha "Transmitindo …".
    pub fn descricao(&self) -> String {
        match self {
            Fonte::Tela(c) => format!("monitor {}", c.monitor().index),
            Fonte::Janela(c) => format!("janela {}", c.hwnd()),
        }
    }

    pub fn proximo(&mut self, espera_ms: u32) -> Result<Option<Quadro<'_>>, CaptureError> {
        Ok(match self {
            Fonte::Tela(c) => c.next_frame(espera_ms)?.map(Quadro::Tela),
            Fonte::Janela(c) => c.next_frame(espera_ms)?.map(Quadro::Janela),
        })
    }
}

pub enum Quadro<'a> {
    Tela(Frame<'a>),
    Janela(WindowFrame),
}

impl Quadro<'_> {
    pub fn texture(&self) -> &ID3D11Texture2D {
        match self {
            Quadro::Tela(f) => f.texture(),
            Quadro::Janela(f) => f.texture(),
        }
    }

    /// A imagem mudou (no monitor, um frame pode ser só o cursor mexendo).
    pub fn atualizou(&self) -> bool {
        match self {
            Quadro::Tela(f) => f.info().desktop_updated,
            Quadro::Janela(_) => true,
        }
    }

    /// Forma nova do cursor (só no monitor: na janela ele vem na imagem).
    pub fn take_pointer_shape(&mut self) -> Option<PointerShape> {
        match self {
            Quadro::Tela(f) => f.take_pointer_shape(),
            Quadro::Janela(_) => None,
        }
    }

    pub fn ponteiro(&self) -> Option<PointerPosition> {
        match self {
            Quadro::Tela(f) => f.info().pointer,
            Quadro::Janela(_) => None,
        }
    }
}
