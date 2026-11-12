//! Lê as linhas que o `fluxo-host` e o `fluxo-client` imprimem: é por elas
//! que a janela fica sabendo dos códigos. O texto é o mesmo que uma pessoa lê
//! no terminal; ao mudar esses `println!`, mude aqui também (os testes abaixo
//! guardam o formato).

/// O que uma linha quer dizer para a janela.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evento {
    /// Host: código da sala, para quem vai assistir.
    Sala(String),
    /// Host, sala: alguém tentou entrar e não pareou; a transmissão segue.
    TentouEntrar(String),
    /// Cliente, mesma rede: código de pareamento e o endereço para o host.
    Pareamento { codigo: String, destino: Option<String> },
    /// Cliente, redes diferentes: código de conexão para mandar a quem compartilha.
    Conexao(String),
    /// Host, redes diferentes: código de resposta para mandar de volta.
    Resposta(String),
    /// Host: resposta para um espectador que entrou depois do primeiro.
    RespostaDe { numero: u32, codigo: String },
    /// Host: quantos estão assistindo agora; `numero` acabou de entrar.
    Conectou { numero: u32, total: usize },
    /// Host: `numero` saiu ou caiu.
    Saiu { numero: u32, total: usize },
    /// Host: não deu para parear com `numero`; o resto segue.
    NaoConectou { numero: u32, motivo: String },
    /// Host: a transmissão começou.
    Transmitindo,
    /// Cliente: o vídeo chegou (`vídeo em AV1 | decoder: …`).
    Recebendo(String),
    /// Host: relatório de um segundo, resumido para quem lê a janela.
    Relatorio(String),
    /// Erro fatal do programa.
    Erro(String),
    /// Qualquer outra coisa: só vai para o registro.
    Outra,
}

/// Interpreta uma linha.
pub fn ler(linha: &str) -> Evento {
    let t = linha.trim();
    if let Some(c) = t.strip_prefix("Código da sala: ") {
        return Evento::Sala(c.trim().to_string());
    }
    if let Some(m) = t.strip_prefix("Alguém tentou entrar e não conectou: ") {
        return Evento::TentouEntrar(m.to_string());
    }
    if let Some(c) = t.strip_prefix("Código de conexão: ") {
        return Evento::Conexao(c.trim().to_string());
    }
    if let Some(c) = t.strip_prefix("Código de resposta: ") {
        return Evento::Resposta(c.trim().to_string());
    }
    // "Código de resposta (espectador 2): 8D9E-66WU-866"
    if let Some(resto) = t.strip_prefix("Código de resposta (espectador ") {
        if let Some((n, c)) = resto.split_once("): ") {
            if let Ok(numero) = n.parse() {
                return Evento::RespostaDe {
                    numero,
                    codigo: c.trim().to_string(),
                };
            }
        }
    }
    // "Espectador 2 conectou (2 no total)", "Espectador 2 saiu (1 no total)",
    // "Espectador 2 não conectou: motivo"
    if let Some(resto) = t.strip_prefix("Espectador ") {
        if let Some((n, depois)) = resto.split_once(' ') {
            if let Ok(numero) = n.parse() {
                if let Some(motivo) = depois.strip_prefix("não conectou: ") {
                    return Evento::NaoConectou {
                        numero,
                        motivo: motivo.to_string(),
                    };
                }
                let total = |d: &str| d.split_whitespace().next().and_then(|x| x.parse().ok());
                if let Some(total) = depois.strip_prefix("conectou (").and_then(total) {
                    return Evento::Conectou { numero, total };
                }
                if let Some(total) = depois.strip_prefix("saiu (").and_then(total) {
                    return Evento::Saiu { numero, total };
                }
            }
        }
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
    if let Some(v) = t.strip_prefix("vídeo em ") {
        return Evento::Recebendo(v.to_string());
    }
    if let Some(e) = t.strip_prefix("erro: ") {
        return Evento::Erro(e.to_string());
    }
    if let Some((n, resto)) = t.split_once(" s: ") {
        if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) {
            return Evento::Relatorio(resumo(resto));
        }
    }
    Evento::Outra
}

/// De `60 fps, 18.2 Mbps (alvo 20.0), perda 0.0%, fila 0 ms, …` deixa só o que
/// interessa a quem compartilha: `60 fps · 18.2 Mbps · perda 0.0% · RTT 1.89 ms`.
fn resumo(relatorio: &str) -> String {
    relatorio
        .split(", ")
        .filter(|p| {
            p.ends_with(" fps") || p.contains(" Mbps") || p.starts_with("perda ") || p.starts_with("RTT ")
        })
        .map(|p| p.split(" (").next().unwrap_or(p).trim())
        .collect::<Vec<_>>()
        .join(" · ")
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
            Evento::Relatorio("60 fps · 18.2 Mbps · perda 0.0%".into())
        );
        assert_eq!(
            ler("erro: o código não confere"),
            Evento::Erro("o código não confere".into())
        );
    }

    #[test]
    fn relatorio_de_verdade_fica_curto() {
        // Linha copiada do host em execução.
        let linha = "     6 s: 60 fps, 15.8 Mbps (alvo 20.0), perda 0.0%, fila 0 ms, encode 2.2 ms, \
                     envio 3.0 ms (fila de envio 1), RTT 1.89 ms, 141 capturas adiadas, 0 pedidos de recuperação";
        assert_eq!(
            ler(linha),
            Evento::Relatorio("60 fps · 15.8 Mbps · perda 0.0% · RTT 1.89 ms".into())
        );
    }

    #[test]
    fn video_chegando_no_cliente() {
        assert_eq!(
            ler("  vídeo em AV1 | decoder: AV1 AV1VideoExtension, acelerado pela GPU (DXVA)"),
            Evento::Recebendo("AV1 | decoder: AV1 AV1VideoExtension, acelerado pela GPU (DXVA)".into())
        );
    }

    #[test]
    fn espectadores_entrando_e_saindo() {
        assert_eq!(
            ler("Código de resposta (espectador 2): 8D9E-66WU-866"),
            Evento::RespostaDe {
                numero: 2,
                codigo: "8D9E-66WU-866".into()
            }
        );
        assert_eq!(
            ler("Espectador 2 conectou (2 no total)"),
            Evento::Conectou { numero: 2, total: 2 }
        );
        assert_eq!(
            ler("Espectador 1 saiu (1 no total)"),
            Evento::Saiu { numero: 1, total: 1 }
        );
        assert_eq!(
            ler("Espectador 3 não conectou: tempo esgotado"),
            Evento::NaoConectou {
                numero: 3,
                motivo: "tempo esgotado".into()
            }
        );
    }

    #[test]
    fn sala_do_host() {
        assert_eq!(ler("Código da sala: K7M4-QX9P"), Evento::Sala("K7M4-QX9P".into()));
        assert_eq!(
            ler("Alguém tentou entrar e não conectou: o código não confere"),
            Evento::TentouEntrar("o código não confere".into())
        );
        // A dica do terminal não é código de pareamento de cliente.
        assert_eq!(ler("  No terminal: fluxo-client --sala K7M4-QX9P"), Evento::Outra);
    }

    #[test]
    fn o_resto_nao_e_evento() {
        assert_eq!(ler("  áudio: S/PDIF (Opus 128 kbps)"), Evento::Outra);
        assert_eq!(ler(""), Evento::Outra);
        // Parece relatório, mas não começa com um número.
        assert_eq!(ler("frames 3 s: nada"), Evento::Outra);
    }
}
