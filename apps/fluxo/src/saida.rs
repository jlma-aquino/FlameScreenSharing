//! Lê as linhas que o `fluxo-host` e o `fluxo-client` imprimem: é por elas
//! que a janela fica sabendo dos códigos. O texto é o mesmo que uma pessoa lê
//! no terminal; ao mudar esses `println!`, mude aqui também (os testes abaixo
//! guardam o formato).

/// O que uma linha quer dizer para a janela.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evento {
    /// Cliente, mesma rede: código de pareamento e o endereço para o host.
    Pareamento { codigo: String, destino: Option<String> },
    /// Cliente, redes diferentes: código de conexão para mandar a quem compartilha.
    Conexao(String),
    /// Host, redes diferentes: código de resposta para mandar de volta.
    Resposta(String),
    /// Host: a transmissão começou.
    Transmitindo,
    /// Host: relatório de um segundo (`  12 s: 60 fps, …`).
    Relatorio(String),
    /// Erro fatal do programa.
    Erro(String),
    /// Qualquer outra coisa: só vai para o registro.
    Outra,
}

/// Interpreta uma linha.
pub fn ler(linha: &str) -> Evento {
    let t = linha.trim();
    if let Some(c) = t.strip_prefix("Código de conexão: ") {
        return Evento::Conexao(c.trim().to_string());
    }
    if let Some(c) = t.strip_prefix("Código de resposta: ") {
        return Evento::Resposta(c.trim().to_string());
    }
    if let Some(c) = t.strip_prefix("Código de pareamento: ") {
        return Evento::Pareamento {
            codigo: c.trim().to_string(),
            destino: None,
        };
    }
    // "No PC que compartilha:  fluxo-host --destino 192.168.0.5:9000 --codigo K7M4-QX9P"
    if t.starts_with("No PC que compartilha:") {
        let mut partes = t.split_whitespace();
        let destino = partes.by_ref().find(|p| *p == "--destino").and(partes.next());
        let codigo = t.split_whitespace().skip_while(|p| *p != "--codigo").nth(1);
        if let Some(codigo) = codigo {
            return Evento::Pareamento {
                codigo: codigo.to_string(),
                // Sem IP descoberto, o cliente escreve "<IP deste PC>:9000".
                destino: destino.filter(|d| !d.starts_with('<')).map(str::to_string),
            };
        }
    }
    if t.starts_with("Transmitindo monitor") {
        return Evento::Transmitindo;
    }
    if let Some(e) = t.strip_prefix("erro: ") {
        return Evento::Erro(e.to_string());
    }
    let relatorio = t
        .split_once(" s: ")
        .is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    if relatorio {
        return Evento::Relatorio(t.to_string());
    }
    Evento::Outra
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codigos_do_cliente_e_do_host() {
        assert_eq!(
            ler("Código de conexão: NT4P-PSZE-8D9E"),
            Evento::Conexao("NT4P-PSZE-8D9E".into())
        );
        assert_eq!(
            ler("Código de resposta: 8D9E-66WU-866"),
            Evento::Resposta("8D9E-66WU-866".into())
        );
        assert_eq!(
            ler("Código de pareamento: K7M4-QX9P"),
            Evento::Pareamento {
                codigo: "K7M4-QX9P".into(),
                destino: None
            }
        );
    }

    #[test]
    fn endereco_do_cliente_na_mesma_rede() {
        assert_eq!(
            ler("No PC que compartilha:  fluxo-host --destino 192.168.0.5:9000 --codigo K7M4-QX9P"),
            Evento::Pareamento {
                codigo: "K7M4-QX9P".into(),
                destino: Some("192.168.0.5:9000".into())
            }
        );
    }

    #[test]
    fn sem_ip_descoberto_nao_inventa_destino() {
        assert_eq!(
            ler("No PC que compartilha:  fluxo-host --destino <IP deste PC>:9000 --codigo K7M4-QX9P"),
            Evento::Pareamento {
                codigo: "K7M4-QX9P".into(),
                destino: None
            }
        );
    }

    #[test]
    fn linhas_do_host() {
        assert_eq!(
            ler("Transmitindo monitor 0 (1920x1080) de 0.0.0.0:1 para 1.2.3.4:9000"),
            Evento::Transmitindo
        );
        assert_eq!(
            ler("    12 s: 60 fps, 18.2 Mbps (alvo 20.0), perda 0.0%"),
            Evento::Relatorio("12 s: 60 fps, 18.2 Mbps (alvo 20.0), perda 0.0%".into())
        );
        assert_eq!(
            ler("erro: o código não confere"),
            Evento::Erro("o código não confere".into())
        );
    }

    #[test]
    fn o_resto_nao_e_evento() {
        assert_eq!(ler("  áudio: S/PDIF (Opus 128 kbps)"), Evento::Outra);
        assert_eq!(ler(""), Evento::Outra);
        // Parece relatório, mas não começa com um número.
        assert_eq!(ler("frames 3 s: nada"), Evento::Outra);
    }
}
