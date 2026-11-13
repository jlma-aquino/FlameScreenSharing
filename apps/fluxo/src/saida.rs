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
    /// Host: passou a transmitir esta tela (número a partir de 1, como na janela).
    Tela(usize),
    /// Host: passou a transmitir só esta janela (HWND).
    Janela(isize),
    /// Host: o som do PC está indo (ou não).
    Som(bool),
    /// Host: vídeo pausado (ou transmitindo de novo).
    Pausa(bool),
    /// Host: resolução e fps depois de uma mudança (`1280x720, até 30 fps`).
    Qualidade(String),
    /// Host: o que está valendo (`AV1 · 1920x1080 · até 60 fps · até 20 Mbps`).
    Configuracao(String),
    /// Cliente: volume agora, 0 a 100.
    Volume(u32),
    /// Um pedido da janela não deu certo; a transmissão segue.
    Recusado(String),
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
                // "Espectador 2 expulso (1 no total)": para a janela, saiu.
                if let Some(total) = depois.strip_prefix("expulso (").and_then(total) {
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
    // "Tela: monitor 2 (2560x1440)"
    if let Some(resto) = t.strip_prefix("Tela: monitor ") {
        if let Some(n) = resto.split_whitespace().next().and_then(|n| n.parse().ok()) {
            return Evento::Tela(n);
        }
    }
    match t {
        "Som: ligado" => return Evento::Som(true),
        "Som: desligado" => return Evento::Som(false),
        "Vídeo: pausado" => return Evento::Pausa(true),
        "Vídeo: transmitindo" => return Evento::Pausa(false),
        _ => {}
    }
    if let Some(c) = t.strip_prefix("Configuração: ") {
        return Evento::Configuracao(c.to_string());
    }
    if let Some(q) = t.strip_prefix("Qualidade: ") {
        return Evento::Qualidade(q.to_string());
    }
    if let Some(v) = t
        .strip_prefix("Volume: ")
        .and_then(|v| v.trim_end_matches('%').parse().ok())
    {
        return Evento::Volume(v);
    }
    if let Some(e) = t.strip_prefix("Não consegui trocar de tela: ") {
        return Evento::Recusado(format!("Não deu para trocar de tela: {e}"));
    }
    // "Tela: janela 132456 (1280x720)"
    if let Some(resto) = t.strip_prefix("Tela: janela ") {
        if let Some(h) = resto.split_whitespace().next().and_then(|n| n.parse().ok()) {
            return Evento::Janela(h);
        }
    }
    if t.starts_with("Transmitindo monitor") || t.starts_with("Transmitindo janela") {
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

/// Mascara os IPs da internet num texto (para "Copiar detalhes"): IPv4
/// público vira `177.73.*.*`, IPv6 público vira `2804:14c:*`. Endereços da
/// rede local e o loopback ficam (não identificam ninguém de fora). A porta,
/// quando houver, fica.
pub fn mascarar_ips(texto: &str) -> String {
    use std::net::{IpAddr, Ipv4Addr};
    let publico_v4 =
        |ip: Ipv4Addr| !(ip.is_private() || ip.is_loopback() || ip.is_unspecified() || ip.is_link_local());
    let mascara = |token: &str| -> Option<String> {
        // "1.2.3.4", "1.2.3.4:5000", "[2001:db8::1]:5000", "2001:db8::1"
        let (ip, porta) = match token.strip_prefix('[') {
            Some(resto) => {
                let (ip, depois) = resto.split_once(']')?;
                (ip, depois)
            }
            None => match token.rsplit_once(':') {
                Some((ip, p)) if ip.contains('.') && p.bytes().all(|b| b.is_ascii_digit()) => {
                    (ip, &token[ip.len()..])
                }
                _ => (token, ""),
            },
        };
        let novo = match ip.parse::<IpAddr>().ok()? {
            IpAddr::V4(v4) if publico_v4(v4) => {
                let o = v4.octets();
                format!("{}.{}.*.*", o[0], o[1])
            }
            IpAddr::V6(v6) if (v6.segments()[0] & 0xE000) == 0x2000 => {
                let s = v6.segments();
                let curto = format!("{:x}:{:x}:*", s[0], s[1]);
                if token.starts_with('[') {
                    format!("[{curto}]")
                } else {
                    curto
                }
            }
            _ => return None,
        };
        Some(format!("{novo}{porta}"))
    };
    let mut saida = String::with_capacity(texto.len());
    let mut token = String::new();
    let parte_de_ip = |c: char| c.is_ascii_hexdigit() || matches!(c, '.' | ':' | '[' | ']');
    let fechar = |token: &mut String, saida: &mut String| {
        if !token.is_empty() {
            saida.push_str(&mascara(token).unwrap_or_else(|| token.clone()));
            token.clear();
        }
    };
    for ch in texto.chars() {
        if parte_de_ip(ch) {
            token.push(ch);
        } else {
            fechar(&mut token, &mut saida);
            saida.push(ch);
        }
    }
    fechar(&mut token, &mut saida);
    saida
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
    fn respostas_aos_botoes() {
        assert_eq!(ler("Tela: monitor 2 (2560x1440)"), Evento::Tela(2));
        assert_eq!(ler("Tela: janela 132456 (1280x720)"), Evento::Janela(132456));
        assert_eq!(
            ler("Transmitindo janela 132456 (1280x720) de 0.0.0.0:1 para 1.2.3.4:9000"),
            Evento::Transmitindo
        );
        assert_eq!(ler("Som: desligado"), Evento::Som(false));
        assert_eq!(ler("Vídeo: pausado"), Evento::Pausa(true));
        assert_eq!(ler("Vídeo: transmitindo"), Evento::Pausa(false));
        assert_eq!(
            ler("Qualidade: 1280x720, até 30 fps"),
            Evento::Qualidade("1280x720, até 30 fps".into())
        );
        assert_eq!(ler("Volume: 35%"), Evento::Volume(35));
        assert_eq!(
            ler("Configuração: AV1 · 1920x1080 · até 60 fps · até 20 Mbps"),
            Evento::Configuracao("AV1 · 1920x1080 · até 60 fps · até 20 Mbps".into())
        );
        assert!(matches!(
            ler("Não consegui trocar de tela: monitor 7 não existe"),
            Evento::Recusado(_)
        ));
    }

    #[test]
    fn ips_mascarados_para_copiar() {
        let t = "Transmitindo de 177.73.10.20:5000 para [2804:14c:65a1:9b00::7]:9000 e 192.168.1.5:9000, \
                 pedido de 8.8.8.8, local 127.0.0.1:1 e fe80::1, sala 3F04B, 60 fps, v1.2";
        let m = mascarar_ips(t);
        assert!(m.contains("177.73.*.*:5000"), "{m}");
        assert!(m.contains("[2804:14c:*]:9000"), "{m}");
        assert!(m.contains("8.8.*.*"), "{m}");
        assert!(m.contains("192.168.1.5:9000") && m.contains("127.0.0.1:1"), "{m}");
        assert!(
            m.contains("fe80::1") && m.contains("sala 3F04B") && m.contains("v1.2"),
            "{m}"
        );
        assert!(!m.contains("10.20") && !m.contains("65a1"), "{m}");
    }

    #[test]
    fn expulso_e_saida() {
        assert_eq!(
            ler("Espectador 2 expulso (1 no total)"),
            Evento::Saiu { numero: 2, total: 1 }
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
