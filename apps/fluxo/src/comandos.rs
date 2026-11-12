//! Monta a linha de comando do `fluxo-host` e do `fluxo-client` a partir do
//! que foi escolhido na janela, conferindo os códigos antes de abrir qualquer
//! coisa (a mensagem de erro aparece na hora, no campo certo).

use fluxo_net::{ConnectionCode, PairingCode, ReplyCode};

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
    /// Índice em [`RESOLUCOES`].
    pub resolucao: usize,
    pub fps: u32,
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
            resolucao: 0,
            fps: 60,
            audio: true,
        }
    }
}

impl Compartilhar {
    /// Argumentos do `fluxo-host`, ou o que falta arrumar.
    pub fn args(&self) -> Result<Vec<String>, String> {
        let mut a = Vec::new();
        if self.modo == Modo::Sala {
            a.push("--sala".to_string());
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
        let resolucao = RESOLUCOES[self.resolucao.min(RESOLUCOES.len() - 1)].1;
        if resolucao != "nativa" {
            a.extend(["--resolucao".to_string(), resolucao.to_string()]);
        }
        a.extend(["--fps".to_string(), self.fps.to_string()]);
        if !self.audio {
            a.push("--sem-audio".to_string());
        }
        Ok(a)
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
    pub audio: bool,
    /// Volume do Fluxo, 0 a 100.
    pub volume: u32,
}

impl Default for Assistir {
    fn default() -> Self {
        Self {
            modo: Modo::Sala,
            sala: String::new(),
            audio: true,
            volume: 100,
        }
    }
}

impl Assistir {
    /// Argumentos do `fluxo-client`, ou o que falta arrumar.
    pub fn args(&self) -> Result<Vec<String>, String> {
        let mut a = Vec::new();
        match self.modo {
            Modo::Sala => {
                let c = self.sala.trim();
                if c.is_empty() {
                    return Err("Digite o código da sala de quem compartilha.".into());
                }
                PairingCode::parse(c).map_err(|e| format!("Código da sala: {e}"))?;
                a.extend(["--sala".to_string(), c.to_string()]);
            }
            Modo::Internet => a.push("--internet".to_string()),
            Modo::Rede => {}
        }
        match self.audio {
            true => a.extend(["--volume".to_string(), self.volume.min(100).to_string()]),
            false => a.push("--sem-audio".to_string()),
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

    #[test]
    fn mesma_rede_vira_destino_e_codigo() {
        let codigo = codigo_valido();
        let c = Compartilhar {
            modo: Modo::Rede,
            destino: " 192.168.0.5 ".into(),
            codigo: codigo.clone(),
            ..Default::default()
        };
        let a = c.args().unwrap();
        assert_eq!(
            a[..4],
            ["--destino", "192.168.0.5:9000", "--codigo", codigo.as_str()]
        );
        assert!(a.windows(2).any(|p| p == ["--fps", "60"]));
        assert!(!a.iter().any(|x| x == "--resolucao" || x == "--sem-audio"));
    }

    #[test]
    fn opcoes_viram_argumentos() {
        let c = Compartilhar {
            modo: Modo::Rede,
            destino: "192.168.0.5:9100".into(),
            codigo: codigo_valido(),
            monitor: 1,
            resolucao: 3,
            fps: 120,
            audio: false,
            ..Default::default()
        };
        let a = c.args().unwrap();
        assert_eq!(a[1], "192.168.0.5:9100");
        assert!(a.windows(2).any(|p| p == ["--monitor", "1"]));
        assert!(a.windows(2).any(|p| p == ["--resolucao", "1440p"]));
        assert!(a.windows(2).any(|p| p == ["--fps", "120"]));
        assert!(a.iter().any(|x| x == "--sem-audio"));
    }

    #[test]
    fn campo_vazio_ou_codigo_errado_explica_o_que_falta() {
        let vazio = Compartilhar {
            modo: Modo::Internet,
            ..Default::default()
        };
        assert!(vazio.args().unwrap_err().contains("código de conexão"));
        let errado = Compartilhar {
            modo: Modo::Internet,
            conexao: "AAAA-BBBB".into(),
            ..Default::default()
        };
        assert!(errado.args().unwrap_err().starts_with("Código de conexão:"));
        let sem_destino = Compartilhar {
            modo: Modo::Rede,
            codigo: codigo_valido(),
            ..Default::default()
        };
        assert!(sem_destino.args().unwrap_err().contains("endereço"));
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
        assert_eq!(a.args().unwrap(), ["--sala", codigo.as_str(), "--volume", "100"]);
        assert!(Assistir::default().args().unwrap_err().contains("código da sala"));
        let i = Assistir {
            modo: Modo::Internet,
            ..Default::default()
        };
        assert_eq!(i.args().unwrap(), ["--internet", "--volume", "100"]);
        let b = Assistir {
            modo: Modo::Rede,
            audio: false,
            volume: 30,
            ..Default::default()
        };
        assert_eq!(b.args().unwrap(), ["--sem-audio"]);
    }

    #[test]
    fn sala_nao_pede_nada() {
        let a = Compartilhar::default().args().unwrap();
        assert_eq!(a[0], "--sala");
        assert!(!a.iter().any(|x| x == "--codigo" || x == "--conexao"));
    }
}
