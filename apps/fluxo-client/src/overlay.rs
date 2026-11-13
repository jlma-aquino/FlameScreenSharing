//! Painel de estatísticas desenhado sobre a imagem (Direct2D + DirectWrite).
//!
//! Desenha direto no back buffer da swapchain, depois do Video Processor e
//! antes do `Present`: um retângulo escuro semitransparente no canto com o
//! texto por cima. O texto só é refeito quando muda (1×/s); desenhar o
//! layout pronto em cada frame é barato. O cursor do host também é desenhado
//! aqui, antes do painel, a partir de um bitmap refeito só quando a forma muda.
//! Por último, a barra de controles (`controles.rs`), quando visível.

use std::sync::Arc;

use fluxo_net::CursorShape;
use windows::core::PCWSTR;
use windows::core::{w, Interface, Result};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_GRADIENT_STOP,
    D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1DeviceContext, ID2D1Factory1, ID2D1LinearGradientBrush,
    ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE,
    D2D1_EXTEND_MODE_CLAMP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_GAMMA_2_2,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_METRICS,
};

use crate::controles::{Botao, Desenho, Retangulo};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface, IDXGISwapChain1};

/// Distância do painel até o canto da imagem, e do texto até a borda do painel.
const MARGEM: f32 = 12.0;
const RECUO: f32 = 8.0;

/// Ícones da fonte Segoe MDL2 Assets (vem com o Windows 10 e 11).
const ICONE_SOM: &str = "\u{E767}";
const ICONE_MUDO: &str = "\u{E74F}";
const ICONE_ESTATISTICAS: &str = "\u{E9D9}";
const ICONE_INICIO: &str = "\u{E80F}";
const ICONE_TELA_CHEIA: &str = "\u{E740}";
const ICONE_SAIR_TELA_CHEIA: &str = "\u{E73F}";

/// Pincéis e fontes da barra de controles (fontes refeitas quando a escala muda).
struct Barra {
    branco: ID2D1SolidColorBrush,
    realce: ID2D1SolidColorBrush,
    trilho: ID2D1SolidColorBrush,
    destaque: ID2D1SolidColorBrush,
    perigo: ID2D1SolidColorBrush,
    degrade: ID2D1LinearGradientBrush,
    fontes: Option<(f32, IDWriteTextFormat, IDWriteTextFormat, IDWriteTextFormat)>,
}

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
    barra: Barra,
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
            let paradas = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: cor(0.0, 0.0, 0.0, 0.0),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: cor(0.0, 0.0, 0.0, 0.75),
                },
            ];
            // A versão simples (do ID2D1RenderTarget), em sRGB.
            let alvo: &ID2D1RenderTarget = &contexto;
            let paradas =
                alvo.CreateGradientStopCollection(&paradas, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
            let barra = Barra {
                branco: contexto.CreateSolidColorBrush(&cor(1.0, 1.0, 1.0, 1.0), None)?,
                realce: contexto.CreateSolidColorBrush(&cor(1.0, 1.0, 1.0, 0.18), None)?,
                trilho: contexto.CreateSolidColorBrush(&cor(1.0, 1.0, 1.0, 0.35), None)?,
                // As mesmas cores do tema da janela do FSS (apps/fluxo/src/tema.rs).
                destaque: contexto.CreateSolidColorBrush(&cor(0.949, 0.420, 0.114, 1.0), None)?,
                perigo: contexto.CreateSolidColorBrush(&cor(0.898, 0.282, 0.302, 1.0), None)?,
                degrade: contexto.CreateLinearGradientBrush(
                    &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES::default(),
                    None,
                    &paradas,
                )?,
                fontes: None,
            };
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
                barra,
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
        controles: Option<&Desenho>,
        limpar: bool,
    ) -> Result<()> {
        if let Some(d) = controles {
            self.preparar_fontes(d.layout.escala)?;
        }
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
            if limpar {
                self.contexto.Clear(Some(&D2D1_COLOR_F {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                }));
            }
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
            if let Some(d) = controles {
                self.desenhar_controles(d);
            }
            self.contexto.EndDraw(None, None)
        }
    }

    /// Fontes da barra no tamanho da escala atual.
    fn preparar_fontes(&mut self, escala: f32) -> Result<()> {
        if self.barra.fontes.as_ref().is_some_and(|(e, ..)| *e == escala) {
            return Ok(());
        }
        let fonte = |nome: PCWSTR, peso: DWRITE_FONT_WEIGHT, tamanho: f32| -> Result<IDWriteTextFormat> {
            // SAFETY: fábrica e parâmetros válidos.
            unsafe {
                let f = self.dwrite.CreateTextFormat(
                    nome,
                    None,
                    peso,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    tamanho * escala,
                    w!("pt-BR"),
                )?;
                f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                Ok(f)
            }
        };
        let icones = fonte(w!("Segoe MDL2 Assets"), DWRITE_FONT_WEIGHT_NORMAL, 18.0)?;
        let texto = fonte(w!("Segoe UI"), DWRITE_FONT_WEIGHT_NORMAL, 14.0)?;
        let forte = fonte(w!("Segoe UI"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 14.0)?;
        self.barra.fontes = Some((escala, icones, texto, forte));
        Ok(())
    }

    /// A barra de controles, dentro de BeginDraw/EndDraw.
    fn desenhar_controles(&self, d: &Desenho) {
        let Some((_, icones, texto, forte)) = &self.barra.fontes else {
            return;
        };
        let l = &d.layout;
        let e = l.escala;
        let ret = |r: &Retangulo| D2D_RECT_F {
            left: r.x0,
            top: r.y0,
            right: r.x1,
            bottom: r.y1,
        };
        let arredondado = |r: &Retangulo, raio: f32| D2D1_ROUNDED_RECT {
            rect: ret(r),
            radiusX: raio,
            radiusY: raio,
        };
        let escrever = |s: &str, fonte: &IDWriteTextFormat, r: &Retangulo, pincel: &ID2D1SolidColorBrush| {
            let utf16: Vec<u16> = s.encode_utf16().collect();
            // SAFETY: texto, fonte e pincel válidos, dentro de BeginDraw.
            unsafe {
                self.contexto.DrawText(
                    &utf16,
                    fonte,
                    &ret(r),
                    pincel,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
        };
        let b = &self.barra;
        // SAFETY: desenho dentro de BeginDraw/EndDraw, na thread do device.
        unsafe {
            // Degradê com o dobro da altura da barra: some para cima, e a
            // imagem continua visível atrás dos botões.
            let topo = (2.0 * l.barra.y0 - l.barra.y1).max(0.0);
            b.degrade
                .SetStartPoint(windows_numerics::Vector2 { X: 0.0, Y: topo });
            b.degrade.SetEndPoint(windows_numerics::Vector2 {
                X: 0.0,
                Y: l.barra.y1,
            });
            let fundo = Retangulo { y0: topo, ..l.barra };
            self.contexto.FillRectangle(&ret(&fundo), &b.degrade);

            // Realce do botão sob o mouse.
            let realce = match d.sob_o_mouse {
                Some(Botao::Mudo) => Some(l.mudo),
                Some(Botao::Estatisticas) => Some(l.estatisticas),
                Some(Botao::Inicio) => Some(l.inicio),
                Some(Botao::TelaCheia) => Some(l.tela_cheia),
                _ => None,
            };
            if let Some(r) = realce {
                self.contexto
                    .FillRoundedRectangle(&arredondado(&r, 6.0 * e), &b.realce);
            }

            let sem_som = d.mudo || d.volume == 0;
            escrever(
                if sem_som { ICONE_MUDO } else { ICONE_SOM },
                icones,
                &l.mudo,
                &b.branco,
            );

            // Volume: trilho, parte cheia e bolinha.
            let raio = l.trilho.y1 - l.trilho.y0;
            self.contexto
                .FillRoundedRectangle(&arredondado(&l.trilho, raio), &b.trilho);
            let nivel = if d.mudo { 0 } else { d.volume };
            let x = l.trilho.x0 + l.trilho.largura() * nivel as f32 / 100.0;
            let cheio = Retangulo { x1: x, ..l.trilho };
            self.contexto
                .FillRoundedRectangle(&arredondado(&cheio, raio), &b.destaque);
            let bolinha = D2D1_ELLIPSE {
                point: windows_numerics::Vector2 {
                    X: x,
                    Y: l.trilho.centro_y(),
                },
                radiusX: 7.0 * e,
                radiusY: 7.0 * e,
            };
            self.contexto.FillEllipse(&bolinha, &b.branco);
            escrever(&format!("{nivel}%"), texto, &l.texto_volume, &b.branco);

            escrever(ICONE_INICIO, icones, &l.inicio, &b.branco);
            let estatisticas = if d.estatisticas { &b.destaque } else { &b.branco };
            escrever(ICONE_ESTATISTICAS, icones, &l.estatisticas, estatisticas);
            let tela = if d.tela_cheia {
                ICONE_SAIR_TELA_CHEIA
            } else {
                ICONE_TELA_CHEIA
            };
            escrever(tela, icones, &l.tela_cheia, &b.branco);

            let sair_fundo = if d.sob_o_mouse == Some(Botao::Sair) {
                &b.destaque
            } else {
                &b.perigo
            };
            self.contexto
                .FillRoundedRectangle(&arredondado(&l.sair, 6.0 * e), sair_fundo);
            escrever("Sair", forte, &l.sair, &b.branco);
        }
    }
}
