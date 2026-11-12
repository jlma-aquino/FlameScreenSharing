//! As configurações padrão, guardadas entre uma vez e outra num arquivo de
//! texto simples (`chave = valor`, uma por linha), na pasta de configurações
//! do usuário: `%APPDATA%\FlameScreenSharing\configuracoes.txt` no Windows,
//! `~/.config/flamescreensharing/configuracoes.txt` no Linux.
//!
//! Chave desconhecida ou valor estranho no arquivo não impede de abrir: vale
//! o padrão daquela chave. Temas personalizados, quando existirem, ficam na
//! pasta `temas` ao lado (ver `tema.rs`).

use std::path::PathBuf;

use crate::comandos::{FPS, RESOLUCOES};
use crate::tema::Aparencia;

/// Codecs: o que aparece na janela e o valor de `--codec` (`None`: automático).
pub const CODECS: [(&str, Option<&str>); 4] = [
    ("Automático", None),
    ("H.264 (o mais compatível)", Some("h264")),
    ("HEVC / H.265", Some("hevc")),
    ("AV1 (mais qualidade por Mbps)", Some("av1")),
];

/// Encoders: o que aparece na janela e o valor de `--encoder`.
pub const ENCODERS: [(&str, Option<&str>); 3] = [
    ("Automático", None),
    ("Placa NVIDIA (NVENC)", Some("nvenc")),
    ("Windows (Media Foundation)", Some("mf")),
];

/// Faixa do bitrate máximo escolhido à mão, em Mbps.
pub const MBPS: std::ops::RangeInclusive<u32> = 2..=100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configuracoes {
    pub aparencia: Aparencia,

    // Compartilhar
    pub monitor: usize,
    pub som: bool,
    /// Índice em [`RESOLUCOES`].
    pub resolucao: usize,
    pub fps: u32,
    /// Índice em [`CODECS`].
    pub codec: usize,
    /// Índice em [`ENCODERS`].
    pub encoder: usize,
    /// Bitrate máximo; 0 = automático (pela resolução e fps).
    pub mbps: u32,
    /// De qual saída de som capturar (`None`: a padrão do Windows).
    pub som_de: Option<usize>,

    // Assistir
    pub volume: u32,
    /// Em qual saída tocar (`None`: a padrão do Windows).
    pub tocar_em: Option<usize>,
    /// Painel de estatísticas sobre o vídeo.
    pub estatisticas: bool,
    pub sempre_no_topo: bool,
    /// Pela internet trocando códigos: abrir a porta no roteador por UPnP.
    pub upnp: bool,

    // Conexão
    /// Chave de acesso ao servidor das salas (`FSS1-…`), como colada.
    pub chave: String,

    // Sobre
    /// Ao abrir, procura versão nova e, achando, baixa e instala sozinho.
    pub atualizar_sozinho: bool,
}

impl Default for Configuracoes {
    fn default() -> Self {
        Self {
            aparencia: Aparencia::default(),
            monitor: 0,
            som: true,
            resolucao: 0,
            fps: 60,
            codec: 0,
            encoder: 0,
            mbps: 0,
            som_de: None,
            volume: 100,
            tocar_em: None,
            estatisticas: true,
            sempre_no_topo: false,
            upnp: false,
            chave: String::new(),
            atualizar_sozinho: true,
        }
    }
}

/// Onde fica o arquivo (`None`: sistema sem pasta de configurações).
pub fn caminho() -> Option<PathBuf> {
    let pasta = if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?).join("FlameScreenSharing")
    } else {
        match std::env::var_os("XDG_CONFIG_HOME") {
            Some(x) if !x.is_empty() => PathBuf::from(x),
            _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
        }
        .join("flamescreensharing")
    };
    Some(pasta.join("configuracoes.txt"))
}

impl Configuracoes {
    /// As do arquivo, ou as padrão se ainda não houver.
    pub fn carregar() -> Self {
        caminho()
            .and_then(|c| std::fs::read_to_string(c).ok())
            .map(|t| Self::de_texto(&t))
            .unwrap_or_default()
    }

    pub fn salvar(&self) -> std::io::Result<()> {
        let c = caminho().ok_or_else(|| std::io::Error::other("sem pasta de configurações"))?;
        if let Some(p) = c.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(c, self.para_texto())
    }

    pub fn para_texto(&self) -> String {
        let opcional = |o: Option<usize>| o.map_or("padrao".to_string(), |n| n.to_string());
        let sim = |b: bool| if b { "sim" } else { "nao" };
        format!(
            "# Configurações do FlameScreenSharing (a janela grava sozinha)\n\
             aparencia = {}\n\
             monitor = {}\n\
             som = {}\n\
             resolucao = {}\n\
             fps = {}\n\
             codec = {}\n\
             encoder = {}\n\
             mbps = {}\n\
             som_de = {}\n\
             volume = {}\n\
             tocar_em = {}\n\
             estatisticas = {}\n\
             sempre_no_topo = {}\n\
             upnp = {}\n\
             chave = {}\n\
             atualizacoes = {}\n",
            self.aparencia.chave(),
            self.monitor,
            sim(self.som),
            RESOLUCOES[self.resolucao.min(RESOLUCOES.len() - 1)].1,
            self.fps,
            CODECS[self.codec.min(CODECS.len() - 1)].1.unwrap_or("auto"),
            ENCODERS[self.encoder.min(ENCODERS.len() - 1)].1.unwrap_or("auto"),
            if self.mbps == 0 {
                "auto".to_string()
            } else {
                self.mbps.to_string()
            },
            opcional(self.som_de),
            self.volume,
            opcional(self.tocar_em),
            sim(self.estatisticas),
            sim(self.sempre_no_topo),
            sim(self.upnp),
            self.chave.split_whitespace().collect::<String>(),
            sim(self.atualizar_sozinho),
        )
    }

    pub fn de_texto(texto: &str) -> Self {
        let mut c = Self::default();
        let sim = |v: &str| match v {
            "sim" => Some(true),
            "nao" | "não" => Some(false),
            _ => None,
        };
        let opcional = |v: &str| match v {
            "padrao" | "padrão" => Some(None),
            n => n.parse().ok().map(Some),
        };
        let indice = |lista: &[Option<&str>], v: &str| lista.iter().position(|x| x.unwrap_or("auto") == v);
        for linha in texto.lines() {
            let linha = linha.trim();
            if linha.starts_with('#') {
                continue;
            }
            let Some((chave, valor)) = linha.split_once('=') else {
                continue;
            };
            let v = valor.trim();
            match chave.trim() {
                "aparencia" => c.aparencia = Aparencia::da_chave(v).unwrap_or(c.aparencia),
                "monitor" => c.monitor = v.parse().unwrap_or(c.monitor),
                "som" => c.som = sim(v).unwrap_or(c.som),
                "resolucao" => c.resolucao = RESOLUCOES.iter().position(|r| r.1 == v).unwrap_or(c.resolucao),
                "fps" => c.fps = v.parse().ok().filter(|f| FPS.contains(f)).unwrap_or(c.fps),
                "codec" => {
                    let lista: Vec<_> = CODECS.iter().map(|x| x.1).collect();
                    c.codec = indice(&lista, v).unwrap_or(c.codec)
                }
                "encoder" => {
                    let lista: Vec<_> = ENCODERS.iter().map(|x| x.1).collect();
                    c.encoder = indice(&lista, v).unwrap_or(c.encoder)
                }
                "mbps" => {
                    c.mbps = match v {
                        "auto" => 0,
                        n => n.parse().ok().filter(|m| MBPS.contains(m)).unwrap_or(c.mbps),
                    }
                }
                "som_de" => c.som_de = opcional(v).unwrap_or(c.som_de),
                "volume" => c.volume = v.parse().ok().filter(|v| *v <= 100).unwrap_or(c.volume),
                "tocar_em" => c.tocar_em = opcional(v).unwrap_or(c.tocar_em),
                "estatisticas" => c.estatisticas = sim(v).unwrap_or(c.estatisticas),
                "sempre_no_topo" => c.sempre_no_topo = sim(v).unwrap_or(c.sempre_no_topo),
                "upnp" => c.upnp = sim(v).unwrap_or(c.upnp),
                "chave" => c.chave = v.to_string(),
                "atualizacoes" => c.atualizar_sozinho = sim(v).unwrap_or(c.atualizar_sozinho),
                _ => {}
            }
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ida_e_volta() {
        let c = Configuracoes {
            aparencia: Aparencia::Claro,
            monitor: 1,
            som: false,
            resolucao: 2,
            fps: 120,
            codec: 3,
            encoder: 1,
            mbps: 35,
            som_de: Some(2),
            volume: 40,
            tocar_em: Some(1),
            estatisticas: false,
            sempre_no_topo: true,
            upnp: true,
            chave: "FSS1-ABCDE-FGHJK".into(),
            atualizar_sozinho: false,
        };
        assert_eq!(Configuracoes::de_texto(&c.para_texto()), c);
        let p = Configuracoes::default();
        assert_eq!(Configuracoes::de_texto(&p.para_texto()), p);
    }

    #[test]
    fn arquivo_estranho_nao_impede_de_abrir() {
        let c =
            Configuracoes::de_texto("fps = 999\nvolume = -3\nlixo\naparencia = roxo\nmbps = 1\nnova = 1\n");
        assert_eq!(c, Configuracoes::default());
        assert_eq!(Configuracoes::de_texto(""), Configuracoes::default());
    }
}
