//! Implementação vazia para sistemas que não são Windows.
//!
//! Tem exatamente a mesma API pública de `dxgi.rs`, mas tudo retorna
//! [`CaptureError::Unsupported`]. Existe para o workspace inteiro compilar
//! (e os testes dos outros crates rodarem) em Linux/macOS.
//!
//! Se um dia houver captura em Linux (PipeWire) ou macOS
//! (ScreenCaptureKit), ela entra como um módulo novo ao lado deste.

use crate::{CaptureError, CpuImageInfo, FrameInfo, MonitorInfo};

/// Sem efeito fora do Windows.
pub fn enable_dpi_awareness() {}

/// Sempre `Err(Unsupported)` fora do Windows.
pub fn list_monitors() -> Result<Vec<MonitorInfo>, CaptureError> {
    Err(CaptureError::Unsupported)
}

/// Capturador (indisponível nesta plataforma).
pub struct DesktopCapturer {
    _private: (),
}

impl DesktopCapturer {
    /// Sempre `Err(Unsupported)` fora do Windows.
    pub fn new(_monitor_index: usize) -> Result<Self, CaptureError> {
        Err(CaptureError::Unsupported)
    }

    /// Informações do monitor capturado.
    pub fn monitor(&self) -> &MonitorInfo {
        unreachable!("DesktopCapturer não pode ser criado fora do Windows")
    }

    /// Sempre `Err(Unsupported)` fora do Windows.
    pub fn next_frame(&mut self, _timeout_ms: u32) -> Result<Option<Frame<'_>>, CaptureError> {
        Err(CaptureError::Unsupported)
    }
}

/// Frame capturado (indisponível nesta plataforma).
pub struct Frame<'a> {
    _cap: &'a mut DesktopCapturer,
}

impl Frame<'_> {
    /// Metadados do frame.
    pub fn info(&self) -> FrameInfo {
        unreachable!()
    }
    /// Largura em pixels.
    pub fn width(&self) -> u32 {
        unreachable!()
    }
    /// Altura em pixels.
    pub fn height(&self) -> u32 {
        unreachable!()
    }
    /// Copia os pixels BGRA para a RAM.
    pub fn copy_to_cpu(&mut self, _out: &mut Vec<u8>) -> Result<CpuImageInfo, CaptureError> {
        Err(CaptureError::Unsupported)
    }
}

/// Mantém a API idêntica à do Windows, onde `Frame` libera o frame no Drop.
impl Drop for Frame<'_> {
    fn drop(&mut self) {}
}
