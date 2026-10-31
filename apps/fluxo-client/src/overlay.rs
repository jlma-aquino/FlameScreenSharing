//! Painel de estatísticas desenhado sobre a imagem (Direct2D + DirectWrite).
//!
//! Desenha direto no back buffer da swapchain, depois do Video Processor e
//! antes do `Present`: um retângulo escuro semitransparente no canto com o
//! texto por cima. O texto só é refeito quando muda (1×/s); desenhar o
//! layout pronto em cada frame é barato. O cursor do host também é desenhado
//! aqui, antes do painel, a partir de um bitmap refeito só quando a forma muda.

use std::sync::Arc;

use fluxo_net::CursorShape;
use windows::core::{w, Interface, Result};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
    D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1DeviceContext, ID2D1Factory1, ID2D1SolidColorBrush,
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_NONE,
    D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_INTERPOLATION_MODE_LINEAR,
    D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_METRICS,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface, IDXGISwapChain1};

/// Distância do painel até o canto da imagem, e do texto até a borda do painel.
const MARGEM: f32 = 12.0;
const RECUO: f32 = 8.0;

pub struct Overlay {
    contexto: ID2D1DeviceContext,
    dwrite: IDWriteFactory,
    formato: IDWriteTextFormat,
    fundo: ID2D1SolidColorBrush,
    letra: ID2D1SolidColorBrush,
    /// Bitmap do back buffer; solto antes de redimensionar a swapchain.
    alvo: Option<ID2D1Bitmap1>,
    texto: Option<(IDWriteTextLayout, f32, f32)>,
    /// Bitmap do cursor e a forma de que ele veio (pelo endereço do `Arc`).
    cursor: Option<(usize, ID2D1Bitmap1)>,
}

impl Overlay {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        // SAFETY: criação de objetos Direct2D/DirectWrite com parâmetros
        // válidos, sobre o mesmo device D3D11 da swapchain.
        unsafe {
            let fabrica: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d = fabrica.CreateDevice(&device.cast::<IDXGIDevice>()?)?;
            let contexto = d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let cor = |r, g, b, a| D2D1_COLOR_F { r, g, b, a };
            let fundo = contexto.CreateSolidColorBrush(&cor(0.0, 0.0, 0.0, 0.6), None)?;
            let letra = contexto.CreateSolidColorBrush(&cor(1.0, 1.0, 1.0, 1.0), None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let formato = dwrite.CreateTextFormat(
                w!("Consolas"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                15.0,
                w!("pt-BR"),
            )?;
            Ok(Self {
                contexto,
                dwrite,
                formato,
                fundo,
                letra,
                alvo: None,
                texto: None,
                cursor: None,
            })
        }
    }

    /// Troca o texto do painel (várias linhas, separadas por `\n`).
    pub fn set_text(&mut self, texto: &str) -> Result<()> {
        let utf16: Vec<u16> = texto.encode_utf16().collect();
        // SAFETY: texto e formato válidos; `m` só é escrito.
        unsafe {
            let layout = self
                .dwrite
                .CreateTextLayout(&utf16, &self.formato, 2000.0, 2000.0)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            self.texto = Some((layout, m.width, m.height));
        }
        Ok(())
    }

    /// Solta o back buffer: chame antes de `ResizeBuffers`.
    pub fn release_target(&mut self) {
        // SAFETY: tira o alvo do contexto antes de soltar o bitmap.
        unsafe { self.contexto.SetTarget(None) };
        self.alvo = None;
    }

    /// Imagem do cursor do host (refeita só quando a forma muda).
    pub fn set_cursor_shape(&mut self, forma: &Arc<CursorShape>) -> Result<()> {
        let id = Arc::as_ptr(forma) as usize;
        if self.cursor.as_ref().is_some_and(|(i, _)| *i == id) {
            return Ok(());
        }
        // O Direct2D desenha com alfa pré-multiplicado.
        let mut px = forma.bgra.clone();
        for p in px.chunks_exact_mut(4) {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let tamanho = D2D_SIZE_U {
            width: forma.width as u32,
            height: forma.height as u32,
        };
        // SAFETY: `px` tem largura × altura × 4 bytes, lidos durante a chamada.
        let bitmap = unsafe {
            self.contexto
                .CreateBitmap(tamanho, Some(px.as_ptr().cast()), forma.width as u32 * 4, &props)?
        };
        self.cursor = Some((id, bitmap));
        Ok(())
    }

    /// Desenha no back buffer atual o cursor (em `cursor.0`, recortado a
    /// `cursor.1`, a área do vídeo) e, com `painel`, o painel por cima. No
    /// flip model do D3D11 o buffer 0 é sempre o back buffer da vez, então o
    /// bitmap criado sobre ele vale até o próximo resize.
    pub fn draw(
        &mut self,
        swapchain: &IDXGISwapChain1,
        painel: bool,
        cursor: Option<(D2D_RECT_F, D2D_RECT_F)>,
    ) -> Result<()> {
        // SAFETY: bitmap criado sobre o back buffer desta swapchain; desenho
        // entre BeginDraw/EndDraw na mesma thread do device.
        unsafe {
            if self.alvo.is_none() {
                let superficie: IDXGISurface = swapchain.GetBuffer(0)?;
                let props = D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_IGNORE,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                    colorContext: std::mem::ManuallyDrop::new(None),
                };
                let bitmap = self
                    .contexto
                    .CreateBitmapFromDxgiSurface(&superficie, Some(&props))?;
                self.contexto.SetTarget(&bitmap);
                self.alvo = Some(bitmap);
            }
            self.contexto.BeginDraw();
            if let (Some((destino, recorte)), Some((_, bitmap))) = (cursor, &self.cursor) {
                self.contexto
                    .PushAxisAlignedClip(&recorte, D2D1_ANTIALIAS_MODE_ALIASED);
                self.contexto.DrawBitmap(
                    bitmap,
                    Some(&destino),
                    1.0,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    None,
                    None,
                );
                self.contexto.PopAxisAlignedClip();
            }
            if let (true, Some((layout, largura, altura))) = (painel, &self.texto) {
                let caixa = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: MARGEM,
                        top: MARGEM,
                        right: MARGEM + largura + 2.0 * RECUO,
                        bottom: MARGEM + altura + 2.0 * RECUO,
                    },
                    radiusX: 6.0,
                    radiusY: 6.0,
                };
                self.contexto.FillRoundedRectangle(&caixa, &self.fundo);
                self.contexto.DrawTextLayout(
                    windows_numerics::Vector2 {
                        X: MARGEM + RECUO,
                        Y: MARGEM + RECUO,
                    },
                    layout,
                    &self.letra,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
            self.contexto.EndDraw(None, None)
        }
    }
}
