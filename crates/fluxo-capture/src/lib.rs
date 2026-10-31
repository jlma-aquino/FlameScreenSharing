//! # fluxo-capture
//!
//! Captura frames da tela usando a **DXGI Desktop Duplication API** do Windows.
//!
//! ## Por que Desktop Duplication
//!
//! - O frame chega como **textura na GPU** (`ID3D11Texture2D`). No pipeline
//!   final ele vai direto para o encoder de hardware sem nunca passar pela
//!   RAM — isso é o que torna 1080p/4K a 60+ fps barato.
//! - O Windows só entrega um frame novo quando a tela **muda**. Tela parada =
//!   zero trabalho. (Consequência: num benchmark com a área de trabalho
//!   parada você verá poucos FPS; abra um vídeo ou jogo para medir.)
//! - Comparação com Windows.Graphics.Capture está no ADR-002 em
//!   `docs/DECISOES.md`.
//!
//! ## Uso
//!
//! ```no_run
//! use fluxo_capture::{list_monitors, DesktopCapturer};
//!
//! let monitores = list_monitors()?;
//! let mut cap = DesktopCapturer::new(0)?; // monitor 0
//! loop {
//!     // Espera até 100 ms por um frame novo.
//!     if let Some(mut frame) = cap.next_frame(100)? {
//!         println!("frame {}x{}", frame.width(), frame.height());
//!         // frame.texture()  -> textura na GPU (para o encoder)
//!         // frame.copy_to_cpu(&mut buf) -> pixels BGRA na RAM (debug/screenshot)
//!     } // <- o frame é devolvido ao Windows aqui (Drop)
//! }
//! # Ok::<(), fluxo_capture::CaptureError>(())
//! ```
//!
//! ## Organização
//!
//! - `lib.rs` (este arquivo): tipos comuns a todas as plataformas.
//! - `dxgi.rs`: implementação real, só compilada no Windows.
//! - `stub.rs`: implementação vazia para outros sistemas (sempre retorna
//!   [`CaptureError::Unsupported`]).

use std::fmt;

#[cfg(windows)]
mod dxgi;
#[cfg(windows)]
pub use dxgi::{enable_dpi_awareness, list_monitors, DesktopCapturer, Frame};

#[cfg(not(windows))]
mod stub;
#[cfg(not(windows))]
pub use stub::{enable_dpi_awareness, list_monitors, DesktopCapturer, Frame};

/// Um monitor que pode ser capturado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Índice global usado em [`DesktopCapturer::new`] (0, 1, 2...).
    pub index: usize,
    /// Nome do dispositivo no Windows, ex.: `\\.\DISPLAY1`.
    pub device_name: String,
    /// Nome da placa de vídeo à qual o monitor está ligado.
    pub adapter_name: String,
    /// Posição do canto superior esquerdo na área de trabalho virtual.
    pub x: i32,
    /// Posição do canto superior esquerdo na área de trabalho virtual.
    pub y: i32,
    /// Largura em pixels físicos.
    pub width: u32,
    /// Altura em pixels físicos.
    pub height: u32,
}

/// Metadados de um frame capturado.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameInfo {
    /// Momento em que o Windows apresentou o frame, em unidades do
    /// `QueryPerformanceCounter`. 0 se só o cursor mudou.
    pub present_time_qpc: i64,
    /// Quantos frames o Windows acumulou desde a última captura. Valor > 1
    /// significa que não fomos rápidos o bastante e perdemos frames.
    pub accumulated_frames: u32,
    /// `true` se a imagem da tela mudou; `false` se foi só o cursor.
    pub desktop_updated: bool,
    /// Posição do ponteiro, quando ela (ou a visibilidade) mudou neste frame.
    pub pointer: Option<PointerPosition>,
}

/// Pixels de um frame copiados para a RAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuImageInfo {
    /// Largura em pixels.
    pub width: u32,
    /// Altura em pixels.
    pub height: u32,
    /// Bytes por linha no buffer de saída (sempre `width * 4`, sem padding).
    pub stride: usize,
}

/// Erros da captura.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    /// Plataforma sem suporte (qualquer coisa que não seja Windows).
    Unsupported,
    /// Não existe monitor com esse índice.
    NoSuchMonitor(usize),
    /// O Windows revogou a captura (troca de resolução, tela de UAC/bloqueio,
    /// jogo entrando em tela cheia exclusiva...). O capturador tenta se
    /// recriar sozinho na próxima chamada de `next_frame`.
    AccessLost,
    /// Outra aplicação já está usando Desktop Duplication neste monitor
    /// além do limite do sistema, ou o recurso não está disponível.
    NotAvailable(String),
    /// Erro genérico do Windows, com o código HRESULT e a mensagem.
    Windows { hresult: i32, message: String },
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => write!(f, "captura de tela só é suportada no Windows"),
            Self::NoSuchMonitor(i) => write!(f, "monitor {i} não existe"),
            Self::AccessLost => write!(f, "acesso à captura perdido (será recriado)"),
            Self::NotAvailable(m) => write!(f, "Desktop Duplication indisponível: {m}"),
            Self::Windows { hresult, message } => {
                write!(f, "erro do Windows 0x{:08X}: {message}", *hresult as u32)
            }
        }
    }
}

impl std::error::Error for CaptureError {}

/// Posição do ponteiro do mouse, em pixels do monitor capturado.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PointerPosition {
    /// Canto superior esquerdo da imagem do ponteiro (pode ser negativo
    /// perto das bordas).
    pub x: i32,
    pub y: i32,
    /// `false` quando o ponteiro está em outro monitor ou escondido.
    pub visible: bool,
}

/// Imagem do ponteiro, pronta para desenhar por cima do vídeo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointerShape {
    pub width: u32,
    pub height: u32,
    /// Ponto que "clica", a partir do canto superior esquerdo.
    pub hot_x: u32,
    pub hot_y: u32,
    /// `width × height × 4` bytes BGRA, alfa não pré-multiplicado.
    pub bgra: Vec<u8>,
}

/// Tipos de ponteiro do Desktop Duplication (`DXGI_OUTDUPL_POINTER_SHAPE_TYPE`).
pub const POINTER_MONOCHROME: u32 = 1;
pub const POINTER_COLOR: u32 = 2;
pub const POINTER_MASKED_COLOR: u32 = 4;

/// Converte a forma do ponteiro como o Windows entrega para BGRA com alfa.
///
/// - **Colorido:** BGRA com alfa, usado como está.
/// - **Monocromático:** máscaras AND e XOR de 1 bit (a altura informada é a
///   soma das duas). AND 1/XOR 0 = transparente; 0/0 = preto; 0/1 = branco.
/// - **Colorido com máscara:** alfa 0 = cor opaca; alfa 255 = "XOR com a
///   tela" (preto aí = transparente).
///
/// "Inverter o que está embaixo" (AND 1/XOR 1, ou XOR com cor) não dá para
/// fazer desenhando por cima: vira preto com contorno branco, que aparece em
/// fundo claro e escuro (é o caso do cursor de texto, o "I").
pub fn convert_pointer_shape(
    kind: u32,
    width: u32,
    height: u32,
    pitch: u32,
    hot: (i32, i32),
    buf: &[u8],
) -> Option<PointerShape> {
    let altura = if kind == POINTER_MONOCHROME {
        height / 2
    } else {
        height
    };
    let (w, h, pitch) = (width as usize, altura as usize, pitch as usize);
    if w == 0 || h == 0 || w > 256 || h > 256 {
        return None;
    }
    let mut bgra = vec![0u8; w * h * 4];
    // Pixels de "inverter", para ganhar o contorno depois.
    let mut inverter = vec![false; w * h];
    match kind {
        POINTER_COLOR | POINTER_MASKED_COLOR => {
            if buf.len() < pitch * (h - 1) + w * 4 {
                return None;
            }
            for y in 0..h {
                for x in 0..w {
                    let o = y * pitch + x * 4;
                    let (b, g, r, a) = (buf[o], buf[o + 1], buf[o + 2], buf[o + 3]);
                    let px = match kind {
                        POINTER_COLOR => [b, g, r, a],
                        _ if a == 0 => [b, g, r, 255],
                        _ if (b, g, r) == (0, 0, 0) => [0, 0, 0, 0],
                        _ => {
                            inverter[y * w + x] = true;
                            [0, 0, 0, 255]
                        }
                    };
                    bgra[(y * w + x) * 4..][..4].copy_from_slice(&px);
                }
            }
        }
        POINTER_MONOCHROME => {
            if buf.len() < pitch * h * 2 {
                return None;
            }
            let bit = |linha: usize, x: usize| buf[linha * pitch + x / 8] & (0x80 >> (x % 8)) != 0;
            for y in 0..h {
                for x in 0..w {
                    let px = match (bit(y, x), bit(y + h, x)) {
                        (true, false) => [0, 0, 0, 0],
                        (false, false) => [0, 0, 0, 255],
                        (false, true) => [255, 255, 255, 255],
                        (true, true) => {
                            inverter[y * w + x] = true;
                            [0, 0, 0, 255]
                        }
                    };
                    bgra[(y * w + x) * 4..][..4].copy_from_slice(&px);
                }
            }
        }
        _ => return None,
    }
    // Contorno branco nos pixels transparentes vizinhos de um "inverter".
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if bgra[i * 4 + 3] != 0 {
                continue;
            }
            let vizinho = [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)].iter().any(|(dx, dy)| {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                nx >= 0
                    && ny >= 0
                    && (nx as usize) < w
                    && (ny as usize) < h
                    && inverter[ny as usize * w + nx as usize]
            });
            if vizinho {
                bgra[i * 4..][..4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
    Some(PointerShape {
        width: w as u32,
        height: h as u32,
        hot_x: hot.0.clamp(0, w as i32 - 1) as u32,
        hot_y: hot.1.clamp(0, h as i32 - 1) as u32,
        bgra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(s: &PointerShape, x: usize, y: usize) -> [u8; 4] {
        let o = (y * s.width as usize + x) * 4;
        s.bgra[o..o + 4].try_into().unwrap()
    }

    #[test]
    fn monocromatico_com_as_quatro_combinacoes() {
        // 4×1: AND = 0011, XOR = 0101 → preto, branco, transparente, inverter.
        let buf = [0b0011_0000, 0b0101_0000];
        let s = convert_pointer_shape(POINTER_MONOCHROME, 4, 2, 1, (1, 0), &buf).unwrap();
        assert_eq!((s.width, s.height, s.hot_x), (4, 1, 1));
        assert_eq!(px(&s, 0, 0), [0, 0, 0, 255]);
        assert_eq!(px(&s, 1, 0), [255, 255, 255, 255]);
        // Transparente, mas vizinho do "inverter": vira contorno branco.
        assert_eq!(px(&s, 2, 0), [255, 255, 255, 255]);
        assert_eq!(px(&s, 3, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn colorido_e_colorido_com_mascara() {
        // 2×1 com pitch 12 (sobra no fim da linha).
        let mut buf = vec![0u8; 12];
        buf[..8].copy_from_slice(&[10, 20, 30, 128, 1, 2, 3, 0]);
        let s = convert_pointer_shape(POINTER_COLOR, 2, 1, 12, (0, 0), &buf).unwrap();
        assert_eq!(s.bgra, vec![10, 20, 30, 128, 1, 2, 3, 0]);

        // Máscara: alfa 0 = opaco; 255 com preto = transparente; 255 com cor = inverter.
        let buf = [9, 8, 7, 0, 0, 0, 0, 255, 50, 50, 50, 255];
        let s = convert_pointer_shape(POINTER_MASKED_COLOR, 3, 1, 12, (0, 0), &buf).unwrap();
        assert_eq!(px(&s, 0, 0), [9, 8, 7, 255]);
        assert_eq!(px(&s, 1, 0), [255, 255, 255, 255], "contorno do vizinho");
        assert_eq!(px(&s, 2, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn tamanhos_impossiveis_ou_buffer_curto() {
        assert!(convert_pointer_shape(POINTER_COLOR, 0, 4, 0, (0, 0), &[]).is_none());
        assert!(convert_pointer_shape(POINTER_COLOR, 300, 4, 1200, (0, 0), &[0; 4800]).is_none());
        assert!(convert_pointer_shape(POINTER_COLOR, 2, 2, 8, (0, 0), &[0; 10]).is_none());
        assert!(convert_pointer_shape(99, 2, 2, 8, (0, 0), &[0; 16]).is_none());
    }
}
