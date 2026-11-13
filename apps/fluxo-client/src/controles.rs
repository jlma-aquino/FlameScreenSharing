//! A barra de controles sobre o vídeo, como a do YouTube: mudo, volume,
//! voltar ao FSS, estatísticas, tela cheia e sair. Ela aparece quando o mouse mexe e some
//! depois de [`SOME_DEPOIS`] parado (o cursor some junto).
//!
//! Aqui fica só a parte que não depende do Windows: onde cada peça está
//! ([`Layout`]), qual botão está sob o mouse e o que fazer com cada evento
//! ([`Controles`]). A janela (`window.rs`) junta os eventos do mouse, e a
//! thread de exibição desenha (`overlay.rs`) a partir de um [`Desenho`].

use std::time::{Duration, Instant};

/// Com o mouse parado (e fora da barra) por este tempo, a barra some.
pub const SOME_DEPOIS: Duration = Duration::from_millis(2500);

/// Altura da barra e tamanho dos botões, em pixels a 100% de escala.
const ALTURA: f32 = 56.0;
const BOTAO: f32 = 40.0;
const ESPACO: f32 = 8.0;
const MARGEM: f32 = 12.0;
const TRILHO: f32 = 110.0;
const SAIR: f32 = 84.0;

/// Passo do volume na roda do mouse e nas setas.
pub const PASSO_VOLUME: u32 = 5;

/// Um retângulo em pixels da área de desenho.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Retangulo {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Retangulo {
    fn novo(x: f32, y: f32, largura: f32, altura: f32) -> Self {
        Self {
            x0: x,
            y0: y,
            x1: x + largura,
            y1: y + altura,
        }
    }

    pub fn contem(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }

    pub fn largura(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn centro_y(&self) -> f32 {
        (self.y0 + self.y1) / 2.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Botao {
    Mudo,
    Volume,
    /// Traz a janela do FSS de volta (sem fechar o vídeo).
    Inicio,
    Estatisticas,
    TelaCheia,
    Sair,
}

/// Onde cada peça fica, para uma área de desenho e uma escala (DPI).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Layout {
    pub barra: Retangulo,
    pub mudo: Retangulo,
    /// Onde o volume responde ao mouse (mais alto que o trilho desenhado).
    pub volume: Retangulo,
    /// O trilho do volume, desenhado.
    pub trilho: Retangulo,
    /// Onde escrever "75%".
    pub texto_volume: Retangulo,
    pub inicio: Retangulo,
    pub estatisticas: Retangulo,
    pub tela_cheia: Retangulo,
    pub sair: Retangulo,
    pub escala: f32,
}

impl Layout {
    pub fn new(largura: u32, altura: u32, escala: f32) -> Self {
        let e = escala.max(0.5);
        let (w, h) = (largura as f32, altura as f32);
        let barra = Retangulo::novo(0.0, (h - ALTURA * e).max(0.0), w, ALTURA * e);
        let topo = barra.centro_y() - BOTAO * e / 2.0;
        let quadrado = |x: f32| Retangulo::novo(x, topo, BOTAO * e, BOTAO * e);

        let mudo = quadrado(MARGEM * e);
        let x_trilho = mudo.x1 + ESPACO * e;
        let volume = Retangulo::novo(x_trilho, topo, TRILHO * e, BOTAO * e);
        let trilho = Retangulo::novo(x_trilho, barra.centro_y() - 2.0 * e, TRILHO * e, 4.0 * e);
        let texto_volume = Retangulo::novo(volume.x1 + ESPACO * e, topo, 48.0 * e, BOTAO * e);

        let sair = Retangulo::novo(w - MARGEM * e - SAIR * e, topo, SAIR * e, BOTAO * e);
        let tela_cheia = quadrado(sair.x0 - ESPACO * e - BOTAO * e);
        let estatisticas = quadrado(tela_cheia.x0 - ESPACO * e - BOTAO * e);
        let inicio = quadrado(estatisticas.x0 - ESPACO * e - BOTAO * e);
        Self {
            barra,
            mudo,
            volume,
            trilho,
            texto_volume,
            inicio,
            estatisticas,
            tela_cheia,
            sair,
            escala: e,
        }
    }

    /// O botão sob o ponto, se houver.
    pub fn botao_em(&self, x: f32, y: f32) -> Option<Botao> {
        [
            (self.mudo, Botao::Mudo),
            (self.volume, Botao::Volume),
            (self.inicio, Botao::Inicio),
            (self.estatisticas, Botao::Estatisticas),
            (self.tela_cheia, Botao::TelaCheia),
            (self.sair, Botao::Sair),
        ]
        .into_iter()
        .find(|(r, _)| r.contem(x, y))
        .map(|(_, b)| b)
    }

    /// O volume (0 a 100) para o mouse na posição `x` do trilho.
    pub fn volume_em(&self, x: f32) -> u32 {
        let t = ((x - self.trilho.x0) / self.trilho.largura().max(1.0)).clamp(0.0, 1.0);
        (t * 100.0).round() as u32
    }
}

/// O que a janela recebeu do mouse e do teclado.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Entrada {
    Mover(f32, f32),
    Apertar(f32, f32),
    Soltar(f32, f32),
    CliqueDuplo(f32, f32),
    /// Roda do mouse: positivo para cima.
    Roda(i32),
    /// O mouse saiu da janela.
    Saiu,
}

/// O que a janela deve fazer depois de um evento.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acao {
    AlternarMudo,
    Volume(u32),
    AlternarEstatisticas,
    MostrarFss,
    AlternarTelaCheia,
    Sair,
}

/// O estado da barra, na thread da janela.
#[derive(Debug, Clone)]
pub struct Controles {
    pub layout: Layout,
    pub visivel: bool,
    pub sob_o_mouse: Option<Botao>,
    /// Arrastando o volume.
    arrastando: bool,
    /// Botão apertado (a ação sai ao soltar em cima dele).
    apertado: Option<Botao>,
    ultimo_movimento: Instant,
    dentro: bool,
}

impl Controles {
    pub fn new(layout: Layout, agora: Instant) -> Self {
        Self {
            layout,
            visivel: true,
            sob_o_mouse: None,
            arrastando: false,
            apertado: None,
            ultimo_movimento: agora,
            dentro: false,
        }
    }

    /// Trata um evento; devolve o que fazer (volume calculado aqui, em cima
    /// de `volume` atual).
    pub fn entrada(&mut self, e: Entrada, volume: u32, agora: Instant) -> Option<Acao> {
        match e {
            Entrada::Mover(x, y) => {
                self.ultimo_movimento = agora;
                self.visivel = true;
                self.dentro = true;
                self.sob_o_mouse = self.layout.botao_em(x, y);
                self.arrastando.then(|| Acao::Volume(self.layout.volume_em(x)))
            }
            Entrada::Apertar(x, y) => {
                self.ultimo_movimento = agora;
                let estava_visivel = self.visivel;
                self.visivel = true;
                let botao = self.layout.botao_em(x, y).filter(|_| estava_visivel);
                match botao {
                    Some(Botao::Volume) => {
                        self.arrastando = true;
                        Some(Acao::Volume(self.layout.volume_em(x)))
                    }
                    b => {
                        self.apertado = b;
                        None
                    }
                }
            }
            Entrada::Soltar(x, y) => {
                self.arrastando = false;
                let apertado = self.apertado.take();
                let solto = self.layout.botao_em(x, y);
                match apertado.filter(|b| Some(*b) == solto) {
                    Some(Botao::Mudo) => Some(Acao::AlternarMudo),
                    Some(Botao::Estatisticas) => Some(Acao::AlternarEstatisticas),
                    Some(Botao::Inicio) => Some(Acao::MostrarFss),
                    Some(Botao::TelaCheia) => Some(Acao::AlternarTelaCheia),
                    Some(Botao::Sair) => Some(Acao::Sair),
                    _ => None,
                }
            }
            // Clique duplo no vídeo (fora da barra) alterna a tela cheia.
            Entrada::CliqueDuplo(x, y) => {
                (!self.visivel || !self.layout.barra.contem(x, y)).then_some(Acao::AlternarTelaCheia)
            }
            Entrada::Roda(passos) => {
                self.ultimo_movimento = agora;
                self.visivel = true;
                let novo = volume as i64 + passos as i64 * PASSO_VOLUME as i64;
                Some(Acao::Volume(novo.clamp(0, 100) as u32))
            }
            Entrada::Saiu => {
                self.dentro = false;
                self.sob_o_mouse = None;
                if !self.arrastando {
                    self.visivel = false;
                }
                None
            }
        }
    }

    /// Esconde a barra com o mouse parado. `sempre`: fica visível (antes da
    /// primeira imagem, por exemplo). Devolve `true` se mudou.
    pub fn tempo(&mut self, agora: Instant, sempre: bool) -> bool {
        let antes = self.visivel;
        let ocupado = self.arrastando || self.sob_o_mouse.is_some();
        if sempre {
            self.visivel = true;
        } else if self.visivel && !ocupado && agora.duration_since(self.ultimo_movimento) >= SOME_DEPOIS {
            self.visivel = false;
        }
        antes != self.visivel
    }

    /// O cursor some junto com a barra, só com o mouse sobre o vídeo.
    pub fn esconder_cursor(&self) -> bool {
        self.dentro && !self.visivel
    }
}

/// O que a thread de exibição precisa para desenhar a barra.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Desenho {
    pub layout: Layout,
    pub sob_o_mouse: Option<Botao>,
    pub volume: u32,
    pub mudo: bool,
    pub estatisticas: bool,
    pub tela_cheia: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agora() -> Instant {
        Instant::now()
    }

    #[test]
    fn pecas_cabem_e_nao_se_sobrepoem() {
        for (w, h, e) in [(1280, 720, 1.0), (640, 360, 1.0), (2560, 1440, 1.5)] {
            let l = Layout::new(w, h, e);
            assert_eq!(l.barra.y1, h as f32);
            let ordem = [l.mudo, l.volume, l.texto_volume];
            for par in ordem.windows(2) {
                assert!(par[0].x1 <= par[1].x0, "{par:?}");
            }
            assert!(l.texto_volume.x1 <= l.inicio.x0 && l.inicio.x1 <= l.estatisticas.x0);
            assert!(l.estatisticas.x1 <= l.tela_cheia.x0 && l.tela_cheia.x1 <= l.sair.x0);
            assert!(l.sair.x1 <= w as f32);
        }
    }

    #[test]
    fn botao_e_volume_sob_o_mouse() {
        let l = Layout::new(1280, 720, 1.0);
        let meio = |r: Retangulo| ((r.x0 + r.x1) / 2.0, r.centro_y());
        let (x, y) = meio(l.sair);
        assert_eq!(l.botao_em(x, y), Some(Botao::Sair));
        let (x, y) = meio(l.mudo);
        assert_eq!(l.botao_em(x, y), Some(Botao::Mudo));
        assert_eq!(l.botao_em(640.0, 100.0), None);
        assert_eq!(l.volume_em(l.trilho.x0 - 10.0), 0);
        assert_eq!(l.volume_em(l.trilho.x1 + 10.0), 100);
        assert_eq!(l.volume_em((l.trilho.x0 + l.trilho.x1) / 2.0), 50);
    }

    #[test]
    fn clique_vale_ao_soltar_no_mesmo_botao() {
        let t = agora();
        let mut c = Controles::new(Layout::new(1280, 720, 1.0), t);
        let s = c.layout.sair;
        let (x, y) = (s.x0 + 5.0, s.centro_y());
        assert_eq!(c.entrada(Entrada::Apertar(x, y), 50, t), None);
        assert_eq!(c.entrada(Entrada::Soltar(x, y), 50, t), Some(Acao::Sair));
        // Apertou no Sair e soltou fora: nada.
        c.entrada(Entrada::Apertar(x, y), 50, t);
        assert_eq!(c.entrada(Entrada::Soltar(10.0, 10.0), 50, t), None);
    }

    #[test]
    fn arrastar_o_volume() {
        let t = agora();
        let mut c = Controles::new(Layout::new(1280, 720, 1.0), t);
        let tr = c.layout.trilho;
        assert_eq!(
            c.entrada(Entrada::Apertar(tr.x0, tr.centro_y()), 80, t),
            Some(Acao::Volume(0))
        );
        // Arrastando, vale mesmo fora do trilho.
        assert_eq!(
            c.entrada(Entrada::Mover(tr.x1 + 50.0, 10.0), 0, t),
            Some(Acao::Volume(100))
        );
        c.entrada(Entrada::Soltar(tr.x1 + 50.0, 10.0), 100, t);
        assert_eq!(c.entrada(Entrada::Mover(tr.x0, tr.centro_y()), 100, t), None);
    }

    #[test]
    fn roda_muda_o_volume_dentro_dos_limites() {
        let t = agora();
        let mut c = Controles::new(Layout::new(1280, 720, 1.0), t);
        assert_eq!(c.entrada(Entrada::Roda(1), 98, t), Some(Acao::Volume(100)));
        assert_eq!(c.entrada(Entrada::Roda(-2), 50, t), Some(Acao::Volume(40)));
        assert_eq!(c.entrada(Entrada::Roda(-3), 5, t), Some(Acao::Volume(0)));
    }

    #[test]
    fn some_com_o_mouse_parado_e_volta_ao_mexer() {
        let t = agora();
        let mut c = Controles::new(Layout::new(1280, 720, 1.0), t);
        c.entrada(Entrada::Mover(640.0, 300.0), 50, t);
        assert!(!c.tempo(t + Duration::from_secs(1), false));
        assert!(c.tempo(t + SOME_DEPOIS, false));
        assert!(!c.visivel && c.esconder_cursor());
        // Primeiro clique com a barra escondida só a mostra.
        let s = c.layout.sair;
        let t2 = t + SOME_DEPOIS + Duration::from_secs(1);
        c.entrada(Entrada::Apertar(s.x0 + 5.0, s.centro_y()), 50, t2);
        assert_eq!(c.entrada(Entrada::Soltar(s.x0 + 5.0, s.centro_y()), 50, t2), None);
        assert!(c.visivel);
        // Com o mouse em cima de um botão, não some.
        let m = c.layout.mudo;
        c.entrada(Entrada::Mover(m.x0 + 5.0, m.centro_y()), 50, t2);
        assert!(!c.tempo(t2 + SOME_DEPOIS * 3, false));
        // Antes da primeira imagem, fica sempre.
        c.entrada(Entrada::Mover(640.0, 300.0), 50, t2);
        assert!(!c.tempo(t2 + SOME_DEPOIS * 3, true));
        // Saiu da janela: some na hora, sem esconder o cursor de ninguém.
        c.entrada(Entrada::Saiu, 50, t2);
        assert!(!c.visivel && !c.esconder_cursor());
    }

    #[test]
    fn clique_duplo_no_video_alterna_tela_cheia() {
        let t = agora();
        let mut c = Controles::new(Layout::new(1280, 720, 1.0), t);
        assert_eq!(
            c.entrada(Entrada::CliqueDuplo(640.0, 300.0), 50, t),
            Some(Acao::AlternarTelaCheia)
        );
        let b = c.layout.barra;
        assert_eq!(c.entrada(Entrada::CliqueDuplo(640.0, b.centro_y()), 50, t), None);
    }
}
