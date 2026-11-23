//! # fluxo-sinal
//!
//! Servidor de sinalização: o cliente registra a sala do seu código, o host
//! procura a sala, e cada um fica sabendo os endereços do outro para falarem
//! direto (furo de NAT). Vídeo nunca passa por aqui; o servidor não vê o
//! código, só o número da sala.
//!
//! ```text
//! fluxo-sinal --chaves ARQUIVO [--grupos ARQUIVO] [--porta P] [--ip IP]
//! fluxo-sinal --nova-chave ENDEREÇO [--chaves ARQUIVO]
//! fluxo-sinal --novo-grupo NOME --grupos ARQUIVO
//! fluxo-sinal --novo-convite GRUPO --grupos ARQUIVO
//! ```
//!
//! Grupos (ADR-022), separados das chaves: cada grupo tem um convite
//! (`FSSG-…`). Quem tem o convite vê quem do grupo está com o FSS aberto e
//! quem está compartilhando, e só quem é do grupo acha as salas dele. Trocar
//! o convite tira quem ficou com o antigo, sem mexer na chave do servidor.
//!
//! Só atende quem tem uma chave de acesso (`FSS1-…`, ver
//! `fluxo_crypto::ServerKey`); o resto é ignorado sem resposta. Precisa ficar
//! num endereço que os dois alcancem (IP público, porta UDP aberta). Detalhes:
//! `docs/modulos/fluxo-sinal.md`.

use std::collections::HashMap;
use std::io::Write;
use std::net::UdpSocket;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use fluxo_net::{ServerKey, SignalEvent, SignalServer, SIGNAL_PORT};

const AJUDA: &str = "\
fluxo-sinal — servidor de sinalização do Fluxo (apresenta host e cliente)

Uso:
  fluxo-sinal --chaves ARQUIVO [--grupos ARQUIVO] [opções]
                                             atende quem tem uma das chaves
  fluxo-sinal --nova-chave ENDEREÇO [--chaves ARQUIVO]
                                             cria uma chave de acesso e mostra;
                                             com --chaves, guarda no arquivo
  fluxo-sinal --novo-grupo NOME --grupos ARQUIVO
                                             cria um grupo e mostra o convite (FSSG-…)
  fluxo-sinal --novo-convite GRUPO --grupos ARQUIVO
                                             troca o convite do grupo (número
                                             ou nome): o antigo para de valer
Opções:
  --chaves ARQUIVO  chaves aceitas, uma por linha (# começa comentário). Para
                    tirar o acesso de alguém, apague a linha e reinicie
  --grupos ARQUIVO  grupos: número, chave de conferência e nome, um por linha.
                    O servidor guarda só a conferência do convite: não lê os
                    apelidos nem as salas. Depois de mudar, reinicie
  --aberto          sem chaves: atende qualquer um (só para testes)
  --porta P         porta UDP (padrão 7000)
  --ip IP           endereço local onde escutar (padrão 0.0.0.0 = todos)

ENDEREÇO é como os PCs acham este servidor: IP:porta ou nome:porta
(ex.: 203.0.113.5:3478). Ele vai dentro da chave.

Quem compartilha e quem assiste colam a mesma chave nas configurações do FSS
(Conexão), ou usam --chave nos programas:
  fluxo-host --sala --chave FSS1-…
  fluxo-client --sala <código> --chave FSS1-…";

struct Opts {
    porta: u16,
    ip: String,
    chaves: Option<String>,
    nova_chave: Option<String>,
    grupos: Option<String>,
    novo_grupo: Option<String>,
    novo_convite: Option<String>,
    aberto: bool,
}

fn ler_opts() -> Result<Option<Opts>, String> {
    let mut o = Opts {
        porta: SIGNAL_PORT,
        ip: "0.0.0.0".to_string(),
        chaves: None,
        nova_chave: None,
        grupos: None,
        novo_grupo: None,
        novo_convite: None,
        aberto: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut valor = || args.next().ok_or(format!("{a} precisa de um valor"));
        match a.as_str() {
            "--porta" => o.porta = valor()?.parse().map_err(|_| "porta inválida")?,
            "--ip" => o.ip = valor()?,
            "--chaves" => o.chaves = Some(valor()?),
            "--nova-chave" => o.nova_chave = Some(valor()?),
            "--grupos" => o.grupos = Some(valor()?),
            "--novo-grupo" => o.novo_grupo = Some(valor()?),
            "--novo-convite" => o.novo_convite = Some(valor()?),
            "--aberto" => o.aberto = true,
            "--help" | "-h" | "ajuda" => return Ok(None),
            outro => return Err(format!("opção desconhecida: {outro}")),
        }
    }
    Ok(Some(o))
}

/// As chaves de um arquivo: uma por linha (o que vier depois da chave, na
/// mesma linha, é anotação); linhas vazias e com `#` não contam.
fn ler_chaves(texto: &str) -> Result<Vec<ServerKey>, String> {
    linhas(texto)
        .map(|(i, l)| {
            let chave = l.split_whitespace().next().unwrap_or_default();
            ServerKey::parse(chave).map_err(|e| format!("linha {i}: {e}"))
        })
        .collect()
}

/// Um grupo no arquivo: número, chave de conferência do convite e nome.
type Grupo = (u32, [u8; 32], String);

/// Os grupos de um arquivo: `número(hex) conferência(hex) nome`, um por linha.
fn ler_grupos(texto: &str) -> Result<Vec<Grupo>, String> {
    linhas(texto)
        .map(|(i, l)| {
            let mut p = l.splitn(3, char::is_whitespace);
            let id = p.next().and_then(|x| u32::from_str_radix(x, 16).ok());
            let auth = p
                .next()
                .and_then(fluxo_crypto::de_hex)
                .and_then(|b| <[u8; 32]>::try_from(b).ok());
            match (id, auth) {
                (Some(id), Some(auth)) => {
                    let nome = p.next().map(str::trim).filter(|n| !n.is_empty());
                    Ok((
                        id,
                        auth,
                        nome.map_or_else(|| format!("Grupo {id:08x}"), str::to_string),
                    ))
                }
                _ => Err(format!("linha {i}: use número(hex) conferência(hex) nome")),
            }
        })
        .collect()
}

fn linha_de_grupo((id, auth, nome): &Grupo) -> String {
    format!("{id:08x} {} {nome}", fluxo_crypto::hex(auth))
}

fn linhas(texto: &str) -> impl Iterator<Item = (usize, &str)> {
    texto
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
}

/// `--novo-grupo` e `--novo-convite`: muda o arquivo de grupos e devolve o convite.
fn convite(arquivo: &str, novo: Option<&str>, trocar: Option<&str>) -> Result<String, String> {
    let texto = match std::fs::read_to_string(arquivo) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("ler {arquivo}: {e}")),
    };
    let mut grupos = ler_grupos(&texto)?;
    let (convite, aviso) = match (novo, trocar) {
        (Some(nome), _) => {
            let nome = nome.trim();
            if nome.is_empty() {
                return Err("o grupo precisa de um nome".into());
            }
            let g = fluxo_crypto::GroupKey::generate().map_err(|e| e.to_string())?;
            grupos.push((g.id(), g.auth_key(), nome.to_string()));
            let aviso = format!("Grupo {nome} ({:08x}) criado", g.id());
            (g, aviso)
        }
        (None, Some(qual)) => {
            let i = grupos
                .iter()
                .position(|(id, _, nome)| {
                    u32::from_str_radix(qual, 16).is_ok_and(|q| q == *id)
                        || nome.eq_ignore_ascii_case(qual.trim())
                })
                .ok_or(format!("não há grupo {qual} em {arquivo}"))?;
            let g = fluxo_crypto::GroupKey::with_id(grupos[i].0).map_err(|e| e.to_string())?;
            grupos[i].1 = g.auth_key();
            (
                g,
                format!("Convite do grupo {} trocado: o antigo para de valer", grupos[i].2),
            )
        }
        (None, None) => unreachable!("chamado só com --novo-grupo ou --novo-convite"),
    };
    let mut saida = String::from("# Grupos do fluxo-sinal: número, conferência do convite, nome.\n");
    for g in &grupos {
        saida.push_str(&linha_de_grupo(g));
        saida.push('\n');
    }
    std::fs::write(arquivo, saida).map_err(|e| format!("gravar {arquivo}: {e}"))?;
    println!("{aviso}. Reinicie o servidor para valer.");
    Ok(convite.to_string())
}

fn main() -> ExitCode {
    let o = match ler_opts() {
        Ok(Some(o)) => o,
        Ok(None) => {
            println!("{AJUDA}");
            return ExitCode::SUCCESS;
        }
        Err(e) => return erro(&e),
    };

    if o.novo_grupo.is_some() || o.novo_convite.is_some() {
        let Some(arquivo) = &o.grupos else {
            return erro("--novo-grupo e --novo-convite precisam de --grupos ARQUIVO");
        };
        return match convite(arquivo, o.novo_grupo.as_deref(), o.novo_convite.as_deref()) {
            Ok(c) => {
                println!("Convite (mande a quem for do grupo):\n{c}");
                ExitCode::SUCCESS
            }
            Err(e) => erro(&e),
        };
    }

    if let Some(endereco) = &o.nova_chave {
        let chave = match ServerKey::generate(endereco) {
            Ok(c) => c,
            Err(e) => return erro(&format!("endereço {endereco}: {e}")),
        };
        if let Some(arquivo) = &o.chaves {
            let gravar = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(arquivo)
                .and_then(|mut f| writeln!(f, "# {} (id {})\n{chave}", chave.server(), chave.id()));
            if let Err(e) = gravar {
                eprintln!("erro: gravar em {arquivo}: {e}");
                return ExitCode::FAILURE;
            }
            println!(
                "Guardada em {arquivo} (id {}). Reinicie o servidor para valer.",
                chave.id()
            );
        }
        println!("{chave}");
        return ExitCode::SUCCESS;
    }

    let chaves = match (&o.chaves, o.aberto) {
        (Some(arquivo), _) => {
            let lidas = std::fs::read_to_string(arquivo)
                .map_err(|e| format!("ler {arquivo}: {e}"))
                .and_then(|t| ler_chaves(&t));
            match lidas {
                Ok(c) if c.is_empty() => return erro(&format!("{arquivo} não tem nenhuma chave")),
                Ok(c) => c,
                Err(e) => return erro(&e),
            }
        }
        (None, true) => Vec::new(),
        (None, false) => return erro(
            "sem --chaves o servidor atenderia qualquer um; use --chaves ARQUIVO (ou --aberto para testar)",
        ),
    };

    let socket = match UdpSocket::bind((o.ip.as_str(), o.porta)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("erro: abrir a porta UDP {} em {}: {e}", o.porta, o.ip);
            return ExitCode::FAILURE;
        }
    };
    println!(
        "Servidor de sinalização em {} (UDP). Ctrl+C encerra.",
        socket.local_addr().map(|a| a.to_string()).unwrap_or_default()
    );
    match chaves.is_empty() {
        true => println!("  ABERTO: atende qualquer um (--aberto)"),
        false => {
            let ids: Vec<String> = chaves.iter().map(ServerKey::id).collect();
            println!("  {} chave(s) aceita(s): {}", chaves.len(), ids.join(", "));
        }
    }
    let grupos = match &o.grupos {
        None => Vec::new(),
        Some(arquivo) => match std::fs::read_to_string(arquivo)
            .map_err(|e| format!("ler {arquivo}: {e}"))
            .and_then(|t| ler_grupos(&t))
        {
            Ok(g) => g,
            Err(e) => return erro(&e),
        },
    };
    if !grupos.is_empty() {
        let nomes: Vec<String> = grupos
            .iter()
            .map(|(id, _, n)| format!("{n} ({id:08x})"))
            .collect();
        println!("  {} grupo(s): {}", grupos.len(), nomes.join(", "));
    }
    let inicio = Instant::now();
    let nunca = AtomicBool::new(false);
    // O host repete a procura a cada 250 ms, e numa sala aberta procura o
    // tempo todo (cada vez num socket novo): o mesmo acontecimento só volta
    // ao registro depois de um minuto.
    let mut vistos: HashMap<String, Instant> = HashMap::new();
    SignalServer::with_keys(chaves).with_groups(grupos).run(&socket, &nunca, |ev| {
        let agora = Instant::now();
        let chave = match ev {
            SignalEvent::NotFound { room, host } => format!("vazia {room} {}", host.ip()),
            SignalEvent::Ignored { from } => format!("sem chave {}", from.ip()),
            // Cada entrada no grupo aparece (o FSS só reaparece depois de 15 s fora).
            SignalEvent::Member { .. } => format!("{ev:?}{:?}", agora),
            outro => format!("{outro:?}"),
        };
        if vistos
            .get(&chave)
            .is_some_and(|t| agora.duration_since(*t) < REPETIR)
        {
            return;
        }
        if vistos.len() > 10_000 {
            vistos.retain(|_, t| agora.duration_since(*t) < REPETIR);
        }
        vistos.insert(chave, agora);
        let t = inicio.elapsed().as_secs_f64();
        match ev {
            SignalEvent::Registered { room, client } => {
                println!("[{t:8.1} s] sala {room:05X}: cliente em {client}")
            }
            SignalEvent::Found { room, host, client } => {
                println!("[{t:8.1} s] sala {room:05X}: host {host} achou o cliente {client}")
            }
            SignalEvent::NotFound { room, host } => {
                println!(
                    "[{t:8.1} s] sala {room:05X}: host {} procurando, sala vazia",
                    host.ip()
                )
            }
            SignalEvent::RoomTaken { room, client } => {
                println!("[{t:8.1} s] sala {room:05X}: recusada para {client} (cheia)")
            }
            SignalEvent::Ignored { from } => {
                println!("[{t:8.1} s] pacote sem chave válida de {} (ignorado)", from.ip())
            }
            SignalEvent::Member {
                group,
                member,
                from,
                online,
            } => {
                println!(
                    "[{t:8.1} s] grupo {group}: membro {:04x} online de {} ({online} online)",
                    member >> 48,
                    from.ip()
                )
            }
            SignalEvent::Relay { room, host, client } => {
                println!("[{t:8.1} s] sala {room:05X}: repassando entre o host {host} e o cliente {client} (o caminho direto não abriu)")
            }
        }
    });
    ExitCode::SUCCESS
}

/// Um acontecimento igual só volta ao registro depois deste tempo.
const REPETIR: Duration = Duration::from_secs(60);

fn erro(msg: &str) -> ExitCode {
    eprintln!("erro: {msg}\n\n{AJUDA}");
    ExitCode::from(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arquivo_de_chaves() {
        let a = ServerKey::generate("203.0.113.5:3478").unwrap();
        let b = ServerKey::generate("203.0.113.5:3478").unwrap();
        let texto = format!("# amigos\n{a}\n\n  # do notebook:\n  {b}   anotação  \n");
        assert_eq!(ler_chaves(&texto).unwrap(), vec![a, b]);
        assert!(ler_chaves("# só comentário\n").unwrap().is_empty());
        assert!(ler_chaves("FSS1-ERRADA").unwrap_err().starts_with("linha 1"));
    }

    #[test]
    fn criar_grupo_e_trocar_o_convite() {
        let pasta = std::env::temp_dir().join(format!("fluxo-sinal-teste-{}", std::process::id()));
        std::fs::create_dir_all(&pasta).unwrap();
        let arquivo = pasta.join("grupos");
        let arquivo = arquivo.to_str().unwrap();
        let _ = std::fs::remove_file(arquivo);
        let primeiro =
            fluxo_crypto::GroupKey::parse(&convite(arquivo, Some("Amigos da Flame"), None).unwrap()).unwrap();
        convite(arquivo, Some("Trabalho"), None).unwrap();
        let lidos = ler_grupos(&std::fs::read_to_string(arquivo).unwrap()).unwrap();
        assert_eq!(lidos.len(), 2);
        assert_eq!(
            lidos[0],
            (primeiro.id(), primeiro.auth_key(), "Amigos da Flame".to_string())
        );
        // Trocar pelo nome: mesmo número, conferência nova, o antigo não confere mais.
        let novo =
            fluxo_crypto::GroupKey::parse(&convite(arquivo, None, Some("amigos da flame")).unwrap()).unwrap();
        let lidos = ler_grupos(&std::fs::read_to_string(arquivo).unwrap()).unwrap();
        assert_eq!(novo.id(), primeiro.id());
        assert_eq!(lidos[0].1, novo.auth_key());
        assert_ne!(lidos[0].1, primeiro.auth_key());
        assert!(convite(arquivo, None, Some("não existe")).is_err());
        let _ = std::fs::remove_dir_all(&pasta);
    }
}
