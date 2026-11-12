//! Aparência da janela: todas as cores e formas vêm daqui, nunca de números
//! soltos no resto do código. Assim, um tema personalizado é só um [`Tema`]
//! diferente — e um [`Tema`] cabe num texto simples (`escuro.destaque =
//! #F26B1D`), que dá para guardar num arquivo, editar à mão e trocar sem
//! recompilar. A janela usa o `tema.txt` da pasta das configurações, se
//! existir ([`carregar`]); senão, o [`Tema::padrao`]. Um editor de temas na
//! janela pode vir depois, gravando o mesmo arquivo.
//!
//! Cada tema tem duas paletas, uma clara e uma escura; a configuração
//! [`Aparencia`] escolhe qual vale (ou segue o sistema).

use eframe::egui::{self, Color32, CornerRadius, Stroke, Theme, ThemePreference};

/// Claro, escuro ou o que o sistema usar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Aparencia {
    Sistema,
    #[default]
    Escuro,
    Claro,
}

impl Aparencia {
    pub const TODAS: [Aparencia; 3] = [Aparencia::Escuro, Aparencia::Claro, Aparencia::Sistema];

    /// Nome para quem usa.
    pub fn rotulo(self) -> &'static str {
        match self {
            Aparencia::Sistema => "Igual ao sistema",
            Aparencia::Escuro => "Escuro",
            Aparencia::Claro => "Claro",
        }
    }

    /// Nome no arquivo de configurações.
    pub fn chave(self) -> &'static str {
        match self {
            Aparencia::Sistema => "sistema",
            Aparencia::Escuro => "escuro",
            Aparencia::Claro => "claro",
        }
    }

    pub fn da_chave(s: &str) -> Option<Self> {
        Self::TODAS.into_iter().find(|a| a.chave() == s.trim())
    }

    fn preferencia(self) -> ThemePreference {
        match self {
            Aparencia::Sistema => ThemePreference::System,
            Aparencia::Escuro => ThemePreference::Dark,
            Aparencia::Claro => ThemePreference::Light,
        }
    }
}

/// As cores de um modo (claro ou escuro). Os nomes são os papéis, não as
/// cores: um tema personalizado muda o valor, e a janela continua sabendo
/// onde usar cada uma.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paleta {
    /// Atrás de tudo (área central).
    pub fundo: Color32,
    /// Barras de cima e de baixo, menu lateral das configurações.
    pub painel: Color32,
    /// Cartões e janelas por cima do fundo.
    pub cartao: Color32,
    /// Botões comuns e campos de texto.
    pub controle: Color32,
    /// Controle com o mouse em cima.
    pub controle_ativo: Color32,
    pub borda: Color32,
    pub texto: Color32,
    pub texto_fraco: Color32,
    /// A cor da marca: botão principal, seleção, links.
    pub destaque: Color32,
    /// Texto em cima do destaque.
    pub sobre_destaque: Color32,
    /// Encerrar, desconectar, erros.
    pub perigo: Color32,
    /// Transmitindo, conectado.
    pub sucesso: Color32,
    /// Avisos que não param nada.
    pub aviso: Color32,
}

/// Os nomes dos campos da [`Paleta`], na ordem do arquivo.
const CAMPOS: [&str; 13] = [
    "fundo",
    "painel",
    "cartao",
    "controle",
    "controle_ativo",
    "borda",
    "texto",
    "texto_fraco",
    "destaque",
    "sobre_destaque",
    "perigo",
    "sucesso",
    "aviso",
];

impl Paleta {
    fn campo_mut(&mut self, nome: &str) -> Option<&mut Color32> {
        Some(match nome {
            "fundo" => &mut self.fundo,
            "painel" => &mut self.painel,
            "cartao" => &mut self.cartao,
            "controle" => &mut self.controle,
            "controle_ativo" => &mut self.controle_ativo,
            "borda" => &mut self.borda,
            "texto" => &mut self.texto,
            "texto_fraco" => &mut self.texto_fraco,
            "destaque" => &mut self.destaque,
            "sobre_destaque" => &mut self.sobre_destaque,
            "perigo" => &mut self.perigo,
            "sucesso" => &mut self.sucesso,
            "aviso" => &mut self.aviso,
            _ => return None,
        })
    }

    fn campo(&self, nome: &str) -> Color32 {
        self.clone().campo_mut(nome).copied().unwrap_or_default()
    }
}

/// Um tema completo: nome, as duas paletas e o arredondamento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tema {
    pub nome: String,
    pub escuro: Paleta,
    pub claro: Paleta,
    /// Raio dos cantos de botões, campos e cartões, em pontos.
    pub raio: u8,
}

const fn cor(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

impl Tema {
    /// O tema do FSS: cinzas neutros e o laranja da chama como destaque.
    pub fn padrao() -> Self {
        Self {
            nome: "FSS".into(),
            escuro: Paleta {
                fundo: cor(0x1C1D21),
                painel: cor(0x24252A),
                cartao: cor(0x2C2D33),
                controle: cor(0x383A41),
                controle_ativo: cor(0x45474F),
                borda: cor(0x3D3F46),
                texto: cor(0xF0F1F3),
                texto_fraco: cor(0xA8ABB3),
                destaque: cor(0xF26B1D),
                sobre_destaque: cor(0xFFFFFF),
                perigo: cor(0xE5484D),
                sucesso: cor(0x2EB67D),
                aviso: cor(0xF0B232),
            },
            claro: Paleta {
                fundo: cor(0xECEDF0),
                painel: cor(0xF7F7F9),
                cartao: cor(0xFFFFFF),
                controle: cor(0xE3E5E9),
                controle_ativo: cor(0xD5D8DE),
                borda: cor(0xD5D8DE),
                texto: cor(0x1E2026),
                texto_fraco: cor(0x5D616B),
                destaque: cor(0xE0590F),
                sobre_destaque: cor(0xFFFFFF),
                perigo: cor(0xD12F35),
                sucesso: cor(0x1E8E5A),
                aviso: cor(0xA86A00),
            },
            raio: 8,
        }
    }

    /// A paleta do modo que a janela está usando agora.
    pub fn paleta(&self, modo: Theme) -> &Paleta {
        match modo {
            Theme::Dark => &self.escuro,
            Theme::Light => &self.claro,
        }
    }

    /// Passa o tema para o egui (os dois modos) e escolhe o modo.
    pub fn aplicar(&self, ctx: &egui::Context, aparencia: Aparencia) {
        ctx.set_visuals_of(Theme::Dark, self.visuals(&self.escuro, true));
        ctx.set_visuals_of(Theme::Light, self.visuals(&self.claro, false));
        for modo in [Theme::Dark, Theme::Light] {
            ctx.style_mut_of(modo, |s| {
                s.spacing.item_spacing = egui::vec2(8.0, 8.0);
                s.spacing.button_padding = egui::vec2(12.0, 6.0);
                s.spacing.interact_size.y = 30.0;
                s.spacing.slider_width = 140.0;
            });
        }
        ctx.set_theme(aparencia.preferencia());
    }

    fn visuals(&self, p: &Paleta, escuro: bool) -> egui::Visuals {
        let mut v = if escuro {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        let raio = CornerRadius::same(self.raio);
        v.panel_fill = p.fundo;
        v.window_fill = p.cartao;
        v.window_stroke = Stroke::new(1.0_f32, p.borda);
        v.window_corner_radius = CornerRadius::same(self.raio.saturating_add(4));
        v.menu_corner_radius = raio;
        v.faint_bg_color = p.cartao;
        v.extreme_bg_color = p.fundo;
        v.text_edit_bg_color = Some(p.fundo);
        v.code_bg_color = p.controle;
        v.hyperlink_color = p.destaque;
        v.warn_fg_color = p.aviso;
        v.error_fg_color = p.perigo;
        v.selection.bg_fill = p.destaque.gamma_multiply(0.45);
        v.selection.stroke = Stroke::new(1.0_f32, p.destaque);
        v.slider_trailing_fill = true;
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = p.cartao;
        w.noninteractive.weak_bg_fill = p.cartao;
        w.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.borda);
        w.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.texto);
        w.noninteractive.corner_radius = raio;
        for (estado, fundo) in [
            (&mut w.inactive, p.controle),
            (&mut w.hovered, p.controle_ativo),
            (&mut w.active, p.controle_ativo),
            (&mut w.open, p.controle_ativo),
        ] {
            estado.bg_fill = fundo;
            estado.weak_bg_fill = fundo;
            estado.bg_stroke = Stroke::new(1.0_f32, p.borda);
            estado.fg_stroke = Stroke::new(1.0_f32, p.texto);
            estado.corner_radius = raio;
            estado.expansion = 0.0;
        }
        w.hovered.bg_stroke = Stroke::new(1.0_f32, p.borda);
        w.active.bg_stroke = Stroke::new(1.0_f32, p.destaque);
        v
    }

    /// O tema em texto, um valor por linha (o formato de [`Tema::de_texto`]).
    pub fn para_texto(&self) -> String {
        let mut s = format!("nome = {}\nraio = {}\n", self.nome, self.raio);
        for (modo, p) in [("escuro", &self.escuro), ("claro", &self.claro)] {
            for campo in CAMPOS {
                let c = p.campo(campo);
                s += &format!("{modo}.{campo} = #{:02X}{:02X}{:02X}\n", c.r(), c.g(), c.b());
            }
        }
        s
    }

    /// Lê um tema em texto. O que faltar vem do [`Tema::padrao`], então um
    /// tema personalizado pode mudar só a cor de destaque. Linhas com `#` no
    /// começo são comentários.
    pub fn de_texto(texto: &str) -> Result<Self, String> {
        let mut t = Self::padrao();
        for (i, linha) in texto.lines().enumerate() {
            let linha = linha.trim();
            if linha.is_empty() || linha.starts_with('#') {
                continue;
            }
            let erro = |m: &str| format!("linha {}: {m}", i + 1);
            let (chave, valor) = linha.split_once('=').ok_or_else(|| erro("falta o ="))?;
            let (chave, valor) = (chave.trim(), valor.trim());
            match chave {
                "nome" => t.nome = valor.to_string(),
                "raio" => t.raio = valor.parse().map_err(|_| erro("raio: use de 0 a 255"))?,
                _ => {
                    let (modo, campo) = chave
                        .split_once('.')
                        .ok_or_else(|| erro("use escuro.CAMPO ou claro.CAMPO"))?;
                    let paleta = match modo {
                        "escuro" => &mut t.escuro,
                        "claro" => &mut t.claro,
                        _ => return Err(erro("use escuro.CAMPO ou claro.CAMPO")),
                    };
                    let alvo = paleta
                        .campo_mut(campo)
                        .ok_or_else(|| erro(&format!("cor desconhecida: {campo}")))?;
                    *alvo = ler_cor(valor).ok_or_else(|| erro("cor: use #RRGGBB"))?;
                }
            }
        }
        Ok(t)
    }
}

/// O arquivo de tema personalizado: `tema.txt`, na pasta das configurações.
pub fn caminho_personalizado() -> Option<std::path::PathBuf> {
    crate::config::caminho().and_then(|c| c.parent().map(|p| p.join("tema.txt")))
}

/// O tema da janela: o do `tema.txt`, se houver, senão o padrão. O texto
/// devolvido explica um `tema.txt` com erro (e aí vale o padrão).
pub fn carregar() -> (Tema, Option<String>) {
    let Some(texto) = caminho_personalizado().and_then(|c| std::fs::read_to_string(c).ok()) else {
        return (Tema::padrao(), None);
    };
    match Tema::de_texto(&texto) {
        Ok(t) => (t, None),
        Err(e) => (
            Tema::padrao(),
            Some(format!("tema.txt: {e}. Usando o tema padrão.")),
        ),
    }
}

/// `#RRGGBB` (o `#` é opcional).
fn ler_cor(s: &str) -> Option<Color32> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    u32::from_str_radix(h, 16).ok().map(cor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texto_de_ida_e_volta() {
        let t = Tema::padrao();
        assert_eq!(Tema::de_texto(&t.para_texto()).unwrap(), t);
    }

    #[test]
    fn tema_personalizado_muda_so_o_que_diz() {
        let t = Tema::de_texto("# meu tema\nnome = Azul\nescuro.destaque = #3366FF\nraio = 2").unwrap();
        assert_eq!(t.nome, "Azul");
        assert_eq!(t.raio, 2);
        assert_eq!(t.escuro.destaque, Color32::from_rgb(0x33, 0x66, 0xFF));
        assert_eq!(t.claro, Tema::padrao().claro);
    }

    #[test]
    fn erros_dizem_a_linha() {
        assert!(Tema::de_texto("escuro.destaque = azul")
            .unwrap_err()
            .starts_with("linha 1"));
        assert!(Tema::de_texto("\nescuro.brilho = #000000")
            .unwrap_err()
            .starts_with("linha 2"));
        assert!(Tema::de_texto("roxo.fundo = #000000").is_err());
        assert!(Tema::de_texto("sem igual").is_err());
    }

    #[test]
    fn aparencia_no_arquivo() {
        for a in Aparencia::TODAS {
            assert_eq!(Aparencia::da_chave(a.chave()), Some(a));
        }
        assert_eq!(Aparencia::da_chave("roxo"), None);
    }
}
