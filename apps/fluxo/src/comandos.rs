//! Monta a linha de comando do `fluxo-host` e do `fluxo-client` a partir do
//! que foi escolhido na janela, conferindo os códigos antes de abrir qualquer
//! coisa (a mensagem de erro aparece na hora, no campo certo).

use fluxo_net::{ConnectionCode, PairingCode, ReplyCode};

use crate::config::{Configuracoes, CODECS, ENCODERS};

/// Porta padrão do cliente.
const PORTA_PADRAO: u16 = 9000;

/// Opções de resolução: o que aparece na janela e o valor de `--resolucao`.
pub const RESOLUCOES: [(&str, &str); 5] = [
    ("A da tela", "nativa"),
    ("720p", "720p"),
    ("1080p", "1080p"),
    ("1440p", "1440p"),
    ("4K", "4k"),
];

/// Opções de fps.
pub const FPS: [u32; 3] = [30, 60, 120];

/// Como os dois lados se acham.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modo {
    /// Quem compartilha abre uma sala; quem assiste entra com o código, pelo
    /// servidor do FSS.
    Sala,
    /// Redes diferentes, sem servidor: código de conexão e de resposta.
    Internet,
    /// Mesma rede: endereço e código.
    Rede,
}

/// O que a pessoa preencheu para compartilhar a tela.
#[derive(Debug, Clone)]
pub struct Compartilhar {
    pub modo: Modo,
    pub conexao: String,
    pub destino: String,
    pub codigo: String,
    pub monitor: usize,
    /// Começa com o som ligado (dá para mudar durante a transmissão).
    pub audio: bool,
}

impl Default for Compartilhar {
    fn default() -> Self {
        Self {
            modo: Modo::Sala,
            conexao: String::new(),
            destino: String::new(),
            codigo: String::new(),
            monitor: 0,
            audio: true,
        }
    }
}

impl Compartilhar {
    /// Argumentos do `fluxo-host`, ou o que falta arrumar. Qualidade, codec e
    /// servidor vêm das configurações.
    pub fn args(&self, c: &Configuracoes) -> Result<Vec<String>, String> {
        let mut a = Vec::new();
        if self.modo == Modo::Sala {
            a.push("--sala".to_string());
            servidor(&mut a, c);
        } else if self.modo == Modo::Internet {
            let c = self.conexao.trim();
            if c.is_empty() {
                return Err("Cole o código de conexão de quem vai assistir.".into());
            }
            ConnectionCode::parse(c).map_err(|e| format!("Código de conexão: {e}"))?;
            a.extend(["--conexao".to_string(), c.to_string()]);
        } else {
            let d = self.destino.trim();
            if d.is_empty() {
                return Err("Preencha o endereço de quem vai assistir.".into());
            }
            let c = self.codigo.trim();
            PairingCode::parse(c).map_err(|e| format!("Código: {e}"))?;
            a.extend([
                "--destino".to_string(),
                com_porta(d),
                "--codigo".to_string(),
                c.to_string(),
            ]);
        }
        a.extend(["--monitor".to_string(), self.monitor.to_string()]);
        let resolucao = RESOLUCOES[c.resolucao.min(RESOLUCOES.len() - 1)].1;
        if resolucao != "nativa" {
            a.extend(["--resolucao".to_string(), resolucao.to_string()]);
        }
        a.extend(["--fps".to_string(), c.fps.to_string()]);
        if let Some(codec) = CODECS.get(c.codec).and_then(|x| x.1) {
            a.extend(["--codec".to_string(), codec.to_string()]);
        }
        if let Some(encoder) = ENCODERS.get(c.encoder).and_then(|x| x.1) {
            a.extend(["--encoder".to_string(), encoder.to_string()]);
        }
        if c.mbps > 0 {
            a.extend(["--mbps".to_string(), c.mbps.to_string()]);
        }
        if let Some(n) = c.som_de {
            a.extend(["--audio-dispositivo".to_string(), n.to_string()]);
        }
        if !self.audio {
            a.push("--sem-audio".to_string());
        }
        Ok(a)
    }
}

/// `--sinal`, se as configurações trocaram o servidor das salas.
fn servidor(a: &mut Vec<String>, c: &Configuracoes) {
    let s = c.servidor.trim();
    if !s.is_empty() {
        a.extend(["--sinal".to_string(), s.to_string()]);
    }
}

/// `192.168.0.5` vira `192.168.0.5:9000`; quem já tem porta (ou é IPv6 entre
/// colchetes com porta) fica como está.
fn com_porta(destino: &str) -> String {
    match destino.parse::<std::net::SocketAddr>() {
        Ok(_) => destino.to_string(),
        Err(_) if destino.contains(':') && !destino.contains("::") => destino.to_string(),
        Err(_) if destino.contains(':') => format!("[{destino}]:{PORTA_PADRAO}"),
        Err(_) => format!("{destino}:{PORTA_PADRAO}"),
    }
}

/// O que a pessoa escolheu para assistir.
#[derive(Debug, Clone)]
pub struct Assistir {
    pub modo: Modo,
    /// Código da sala de quem compartilha.
    pub sala: String,
}

impl Default for Assistir {
    fn default() -> Self {
        Self {
            modo: Modo::Sala,
            sala: String::new(),
        }
    }
}

impl Assistir {
    /// Argumentos do `fluxo-client`, ou o que falta arrumar. O som sempre
    /// toca (o volume e o mudo mudam durante a transmissão).
    pub fn args(&self, c: &Configuracoes) -> Result<Vec<String>, String> {
        let mut a = Vec::new();
        match self.modo {
            Modo::Sala => {
                let codigo = self.sala.trim();
                if codigo.is_empty() {
                    return Err("Digite o código da sala de quem compartilha.".into());
                }
                PairingCode::parse(codigo).map_err(|e| format!("Código da sala: {e}"))?;
                a.extend(["--sala".to_string(), codigo.to_string()]);
            }
            Modo::Internet => a.push("--internet".to_string()),
            Modo::Rede => {}
        }
        if self.modo == Modo::Sala {
            servidor(&mut a, c);
        }
        a.extend(["--volume".to_string(), c.volume.min(100).to_string()]);
        if let Some(n) = c.tocar_em {
            a.extend(["--saida-som".to_string(), n.to_string()]);
        }
        if !c.estatisticas {
            a.push("--sem-painel".to_string());
        }
        if c.sempre_no_topo {
            a.push("--no-topo".to_string());
        }
        Ok(a)
    }
}

/// Mais um espectador pela internet: a linha para o host, pela entrada padrão.
pub fn linha_espectador_internet(conexao: &str) -> Result<String, String> {
    let c = conexao.trim();
    if c.is_empty() {
        return Err("Cole o código de conexão de quem vai assistir.".into());
    }
    ConnectionCode::parse(c).map_err(|e| format!("Código de conexão: {e}"))?;
    Ok(format!("conexao {c}"))
}

/// Mais um espectador na mesma rede: a linha para o host.
pub fn linha_espectador_rede(destino: &str, codigo: &str) -> Result<String, String> {
    let d = destino.trim();
    if d.is_empty() {
        return Err("Preencha o endereço de quem vai assistir.".into());
    }
    let c = codigo.trim();
    PairingCode::parse(c).map_err(|e| format!("Código: {e}"))?;
    Ok(format!("destino {} {c}", com_porta(d)))
}

/// Confere o código de resposta colado e devolve a linha para o cliente.
pub fn linha_de_resposta(texto: &str) -> Result<String, String> {
    let t = texto.trim();
    if t.is_empty() {
        return Err("Cole o código de resposta de quem compartilha.".into());
    }
    ReplyCode::parse(t).map_err(|e| format!("Código de resposta: {e}"))?;
    Ok(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codigo_valido() -> String {
        PairingCode::generate().unwrap().to_string()
    }

    fn padrao() -> Configuracoes {
        Configuracoes::default()
    }

    #[test]
    fn mesma_rede_vira_destino_e_codigo() {
        let codigo = codigo_valido();
        let c = Compartilhar {
            modo: Modo::Rede,
            destino: " 192.168.0.5 ".into(),
            codigo: codigo.clone(),
            ..Default::default()
        };
        let a = c.args(&padrao()).unwrap();
        assert_eq!(
            a[..4],
            ["--destino", "192.168.0.5:9000", "--codigo", codigo.as_str()]
        );
        assert!(a.windows(2).any(|p| p == ["--fps", "60"]));
        let fora = [
            "--resolucao",
            "--sem-audio",
            "--codec",
            "--encoder",
            "--mbps",
            "--sinal",
        ];
        assert!(!a.iter().any(|x| fora.contains(&x.as_str())));
    }

    #[test]
    fn configuracoes_viram_argumentos() {
        let c = Compartilhar {
            modo: Modo::Rede,
            destino: "192.168.0.5:9100".into(),
            codigo: codigo_valido(),
            monitor: 1,
            audio: false,
            ..Default::default()
        };
        let cfg = Configuracoes {
            resolucao: 3,
            fps: 120,
            codec: 3,
            encoder: 1,
            mbps: 40,
            som_de: Some(2),
            ..padrao()
        };
        let a = c.args(&cfg).unwrap();
        assert_eq!(a[1], "192.168.0.5:9100");
        for par in [
            ["--monitor", "1"],
            ["--resolucao", "1440p"],
            ["--fps", "120"],
            ["--codec", "av1"],
            ["--encoder", "nvenc"],
            ["--mbps", "40"],
            ["--audio-dispositivo", "2"],
        ] {
            assert!(a.windows(2).any(|p| p == par), "{par:?}");
        }
        assert!(a.iter().any(|x| x == "--sem-audio"));
    }

    #[test]
    fn campo_vazio_ou_codigo_errado_explica_o_que_falta() {
        let vazio = Compartilhar {
            modo: Modo::Internet,
            ..Default::default()
        };
        assert!(vazio.args(&padrao()).unwrap_err().contains("código de conexão"));
        let errado = Compartilhar {
            modo: Modo::Internet,
            conexao: "AAAA-BBBB".into(),
            ..Default::default()
        };
        assert!(errado
            .args(&padrao())
            .unwrap_err()
            .starts_with("Código de conexão:"));
        let sem_destino = Compartilhar {
            modo: Modo::Rede,
            codigo: codigo_valido(),
            ..Default::default()
        };
        assert!(sem_destino.args(&padrao()).unwrap_err().contains("endereço"));
        assert!(linha_de_resposta("  ").is_err());
        assert!(linha_de_resposta("XXXX").is_err());
    }

    #[test]
    fn linhas_para_mais_um_espectador() {
        let codigo = codigo_valido();
        assert_eq!(
            linha_espectador_rede("10.0.0.2", &codigo).unwrap(),
            format!("destino 10.0.0.2:9000 {codigo}")
        );
        assert!(linha_espectador_rede("", &codigo).is_err());
        assert!(linha_espectador_rede("10.0.0.2", "x").is_err());
        assert!(linha_espectador_internet(" ").is_err());
        assert!(linha_espectador_internet("AAAA").is_err());
    }

    #[test]
    fn portas_e_ipv6() {
        assert_eq!(com_porta("10.0.0.1"), "10.0.0.1:9000");
        assert_eq!(com_porta("10.0.0.1:7"), "10.0.0.1:7");
        assert_eq!(com_porta("pc-da-sala"), "pc-da-sala:9000");
        assert_eq!(com_porta("[2001:db8::1]:7"), "[2001:db8::1]:7");
        assert_eq!(com_porta("2001:db8::1"), "[2001:db8::1]:9000");
    }

    #[test]
    fn assistir() {
        let codigo = codigo_valido();
        let a = Assistir {
            sala: format!(" {codigo} "),
            ..Default::default()
        };
        assert_eq!(
            a.args(&padrao()).unwrap(),
            ["--sala", codigo.as_str(), "--volume", "100"]
        );
        assert!(Assistir::default()
            .args(&padrao())
            .unwrap_err()
            .contains("código da sala"));
        let i = Assistir {
            modo: Modo::Internet,
            ..Default::default()
        };
        assert_eq!(i.args(&padrao()).unwrap(), ["--internet", "--volume", "100"]);
        let cfg = Configuracoes {
            volume: 30,
            tocar_em: Some(1),
            estatisticas: false,
            sempre_no_topo: true,
            servidor: "203.0.113.5:3478".into(),
            ..padrao()
        };
        let b = Assistir {
            modo: Modo::Sala,
            sala: codigo.clone(),
        };
        assert_eq!(
            b.args(&cfg).unwrap(),
            [
                "--sala",
                codigo.as_str(),
                "--sinal",
                "203.0.113.5:3478",
                "--volume",
                "30",
                "--saida-som",
                "1",
                "--sem-painel",
                "--no-topo"
            ]
        );
        // O servidor só vale para a sala.
        let r = Assistir {
            modo: Modo::Rede,
            ..Default::default()
        };
        assert!(!r.args(&cfg).unwrap().iter().any(|x| x == "--sinal"));
    }

    #[test]
    fn sala_nao_pede_nada() {
        let a = Compartilhar::default().args(&padrao()).unwrap();
        assert_eq!(a[0], "--sala");
        assert!(!a
            .iter()
            .any(|x| x == "--codigo" || x == "--conexao" || x == "--sinal"));
        let cfg = Configuracoes {
            servidor: "meu.servidor:7000".into(),
            ..padrao()
        };
        let b = Compartilhar::default().args(&cfg).unwrap();
        assert_eq!(b[..3], ["--sala", "--sinal", "meu.servidor:7000"]);
    }
}
