//! A janela (egui) e o desenho do vídeo: os três planos do I420 vão para a GPU
//! como texturas, e um shader converte para RGB. Assim a conversão de cor não
//! pesa na CPU, que já está decodificando.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, Rect};
use eframe::glow::{self, HasContext};
use fluxo_net::{clock_diff_us, clock_us, CursorShape};

use super::{Compartilhado, Quadro};

const VERTICES: &str = r#"
out vec2 uv;
void main() {
    // Quatro cantos, sem buffer: a imagem inteira, de cima para baixo.
    vec2 p = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
    uv = vec2(p.x, 1.0 - p.y);
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

// BT.709, faixa limitada: a conversão que o host fez na ida.
const FRAGMENTOS: &str = r#"
in vec2 uv;
out vec4 cor;
uniform sampler2D plano_y;
uniform sampler2D plano_u;
uniform sampler2D plano_v;
void main() {
    float y = (texture(plano_y, uv).r - 0.0627) * 1.1644;
    float u = texture(plano_u, uv).r - 0.5;
    float v = texture(plano_v, uv).r - 0.5;
    cor = vec4(y + 1.7927 * v, y - 0.2132 * u - 0.5329 * v, y + 2.1124 * u, 1.0);
}
"#;

/// O que existe na GPU. Tudo é usado só na thread da janela, dentro do desenho.
struct Render {
    programa: glow::Program,
    vao: glow::VertexArray,
    texturas: [glow::Texture; 3],
    /// Tamanho das texturas agora.
    tamanho: (u32, u32),
    pendente: Option<Quadro>,
}

impl Render {
    fn new(gl: &glow::Context) -> Result<Self, String> {
        let versao = match gl.version().is_embedded {
            true => "#version 300 es\nprecision mediump float;\n",
            false => "#version 140\n",
        };
        // SAFETY: chamadas OpenGL na thread do contexto, durante a criação da
        // janela; só objetos criados aqui.
        unsafe {
            let programa = gl.create_program()?;
            let mut shaders = Vec::new();
            for (tipo, fonte) in [
                (glow::VERTEX_SHADER, VERTICES),
                (glow::FRAGMENT_SHADER, FRAGMENTOS),
            ] {
                let s = gl.create_shader(tipo)?;
                gl.shader_source(s, &format!("{versao}{fonte}"));
                gl.compile_shader(s);
                if !gl.get_shader_compile_status(s) {
                    return Err(format!("shader: {}", gl.get_shader_info_log(s)));
                }
                gl.attach_shader(programa, s);
                shaders.push(s);
            }
            gl.link_program(programa);
            if !gl.get_program_link_status(programa) {
                return Err(format!("programa: {}", gl.get_program_info_log(programa)));
            }
            for s in shaders {
                gl.detach_shader(programa, s);
                gl.delete_shader(s);
            }
            gl.use_program(Some(programa));
            for (i, nome) in ["plano_y", "plano_u", "plano_v"].iter().enumerate() {
                let local = gl.get_uniform_location(programa, nome);
                gl.uniform_1_i32(local.as_ref(), i as i32);
            }
            gl.use_program(None);
            let vao = gl.create_vertex_array()?;
            let mut texturas = Vec::new();
            for _ in 0..3 {
                let t = gl.create_texture()?;
                gl.bind_texture(glow::TEXTURE_2D, Some(t));
                for (p, v) in [
                    (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                    (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
                }
                texturas.push(t);
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            Ok(Self {
                programa,
                vao,
                texturas: [texturas[0], texturas[1], texturas[2]],
                tamanho: (0, 0),
                pendente: None,
            })
        }
    }

    /// Sobe para a GPU o frame novo, se houver.
    fn subir(&mut self, gl: &glow::Context) {
        let Some(q) = self.pendente.take() else { return };
        let novo_tamanho = self.tamanho != (q.largura, q.altura);
        self.tamanho = (q.largura, q.altura);
        let (w, h) = (q.largura as i32, q.altura as i32);
        let planos: [(&[u8], usize, i32, i32); 3] = [
            (&q.y, q.passo.0, w, h),
            (&q.u, q.passo.1, w / 2, h / 2),
            (&q.v, q.passo.1, w / 2, h / 2),
        ];
        // Formato de um canal: R8 no OpenGL 3 e no ES 3.
        // SAFETY: cada plano tem `passo × altura` bytes, e o OpenGL lê só isso
        // (UNPACK_ROW_LENGTH diz o passo); na thread do contexto.
        unsafe {
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            for (i, (dados, passo, pw, ph)) in planos.into_iter().enumerate() {
                gl.bind_texture(glow::TEXTURE_2D, Some(self.texturas[i]));
                gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, passo as i32);
                if novo_tamanho {
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::R8 as i32,
                        pw,
                        ph,
                        0,
                        glow::RED,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(None),
                    );
                }
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    pw,
                    ph,
                    glow::RED,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(dados)),
                );
            }
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
    }

    fn desenhar(&mut self, gl: &glow::Context) {
        self.subir(gl);
        if self.tamanho == (0, 0) {
            return;
        }
        // SAFETY: objetos deste contexto; o egui restaura o estado depois da
        // chamada de desenho dele.
        unsafe {
            gl.disable(glow::BLEND);
            gl.use_program(Some(self.programa));
            gl.bind_vertex_array(Some(self.vao));
            for (i, t) in self.texturas.iter().enumerate() {
                gl.active_texture(glow::TEXTURE0 + i as u32);
                gl.bind_texture(glow::TEXTURE_2D, Some(*t));
            }
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }

    fn apagar(&self, gl: &glow::Context) {
        // SAFETY: a janela está fechando; nada mais usa estes objetos.
        unsafe {
            gl.delete_program(self.programa);
            gl.delete_vertex_array(self.vao);
            for t in self.texturas {
                gl.delete_texture(t);
            }
        }
    }
}

/// Maior retângulo com a proporção do vídeo dentro de `area`, no centro.
fn encaixar(area: Rect, largura: u32, altura: u32) -> Rect {
    let escala = (area.width() / largura.max(1) as f32).min(area.height() / altura.max(1) as f32);
    Rect::from_center_size(
        area.center(),
        egui::vec2(largura as f32 * escala, altura as f32 * escala),
    )
}

/// A forma do cursor em imagem do egui: BGRA vira RGBA.
fn imagem_do_cursor(f: &CursorShape) -> egui::ColorImage {
    let rgba: Vec<u8> = f
        .bgra
        .chunks_exact(4)
        .flat_map(|p| [p[2], p[1], p[0], p[3]])
        .collect();
    egui::ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &rgba)
}

pub struct Tela {
    comp: Arc<Compartilhado>,
    render: Arc<Mutex<Render>>,
    /// Tamanho do vídeo (0 até o primeiro frame).
    video: (u32, u32),
    espera: String,
    painel: bool,
    tela_cheia: bool,
    /// Textura do cursor, e de qual forma ela é.
    cursor: Option<(Arc<CursorShape>, egui::TextureHandle)>,
}

impl Tela {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        comp: Arc<Compartilhado>,
        espera: String,
        painel: bool,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let gl = cc.gl.as_ref().ok_or("sem contexto OpenGL")?;
        Ok(Self {
            comp,
            render: Arc::new(Mutex::new(Render::new(gl)?)),
            video: (0, 0),
            espera,
            painel,
            tela_cheia: false,
            cursor: None,
        })
    }

    /// Pega o frame novo e registra a latência dele até aqui.
    fn pegar_quadro(&mut self) {
        let novo = self.comp.quadro.lock().unwrap_or_else(|e| e.into_inner()).take();
        let Some(q) = novo else { return };
        self.video = (q.largura, q.altura);
        if let Some(dif) = q.dif_relogio_ms {
            let ms = clock_diff_us(clock_us(), q.capturado_us) as f64 / 1000.0 + dif;
            self.comp
                .latencias
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(ms);
        }
        self.render.lock().unwrap_or_else(|e| e.into_inner()).pendente = Some(q);
    }

    fn teclas(&mut self, ctx: &egui::Context) {
        let (f2, cheia, esc) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::F2),
                i.key_pressed(egui::Key::F11) || (i.modifiers.alt && i.key_pressed(egui::Key::Enter)),
                i.key_pressed(egui::Key::Escape),
            )
        });
        self.painel ^= f2;
        if cheia || (esc && self.tela_cheia) {
            self.tela_cheia = !self.tela_cheia;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.tela_cheia));
        }
    }

    fn desenhar_cursor(&mut self, ctx: &egui::Context, ui: &egui::Ui, imagem: Rect) {
        let c = self.comp.cursor.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(c) = c.filter(|c| c.visible) else { return };
        let Some(forma) = c.shape else { return };
        let igual = self.cursor.as_ref().is_some_and(|(f, _)| Arc::ptr_eq(f, &forma));
        if !igual {
            let textura = ctx.load_texture("cursor", imagem_do_cursor(&forma), egui::TextureOptions::LINEAR);
            self.cursor = Some((forma.clone(), textura));
        }
        let Some((_, textura)) = &self.cursor else { return };
        let escala = imagem.width() / self.video.0.max(1) as f32;
        let canto = imagem.min + egui::vec2(c.x as f32, c.y as f32) * escala;
        let tamanho = egui::vec2(forma.width as f32, forma.height as f32) * escala;
        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        // Recortado à imagem: nada de cursor por cima das barras pretas.
        ui.painter().with_clip_rect(imagem).image(
            textura.id(),
            Rect::from_min_size(canto, tamanho),
            uv,
            Color32::WHITE,
        );
    }
}

impl eframe::App for Tela {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if self.comp.fechar.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.pegar_quadro();
        self.teclas(ctx);
        if let Some(t) = self.comp.titulo.lock().unwrap_or_else(|e| e.into_inner()).take() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(t));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(ctx, |ui| {
                let area = ui.max_rect();
                if self.video == (0, 0) {
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(Color32::LIGHT_GRAY, &self.espera);
                    });
                    return;
                }
                let imagem = encaixar(area, self.video.0, self.video.1);
                let render = self.render.clone();
                let desenho = eframe::egui_glow::CallbackFn::new(move |_, painter| {
                    render
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .desenhar(painter.gl());
                });
                ui.painter().add(egui::PaintCallback {
                    rect: imagem,
                    callback: Arc::new(desenho),
                });
                self.desenhar_cursor(ctx, ui, imagem);
                if self.painel {
                    let texto = self.comp.painel.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    egui::Area::new(egui::Id::new("painel"))
                        .fixed_pos(area.min + egui::vec2(8.0, 8.0))
                        .show(ctx, |ui| {
                            egui::Frame::popup(ui.style()).show(ui, |ui| ui.small(texto));
                        });
                }
            });
    }

    fn on_exit(&mut self, gl: Option<&glow::Context>) {
        self.comp.fechar.store(true, Ordering::Relaxed);
        if let Some(gl) = gl {
            self.render.lock().unwrap_or_else(|e| e.into_inner()).apagar(gl);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imagem_encaixa_sem_deformar_e_fica_no_centro() {
        let area = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 1000.0));
        let r = encaixar(area, 1920, 1080);
        assert_eq!(r.width(), 1000.0);
        assert!((r.height() - 562.5).abs() < 0.01);
        assert_eq!(r.center(), area.center());
    }

    #[test]
    fn cursor_de_bgra_para_rgba() {
        let f = CursorShape {
            width: 1,
            height: 1,
            hot_x: 0,
            hot_y: 0,
            bgra: vec![10, 20, 30, 255],
        };
        assert_eq!(
            imagem_do_cursor(&f).pixels[0],
            Color32::from_rgba_unmultiplied(30, 20, 10, 255)
        );
    }
}
