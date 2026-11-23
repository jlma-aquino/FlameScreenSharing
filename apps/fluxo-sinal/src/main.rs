//! # fluxo-sinal
//!
//! Servidor de sinalização: o cliente registra a sala do seu código, o host
//! procura a sala, e cada um fica sabendo os endereços do outro para falarem
//! direto (furo de NAT). Vídeo nunca passa por aqui; o servidor não vê o
//! código, só o número da sala.
//!
//! ```text
//! fluxo-sinal --chaves ARQUIVO [--porta P] [--ip IP]
//! fluxo-sinal --nova-chave ENDEREÇO [--nome GRUPO] [--chaves ARQUIVO]
//! ```
//!
//! Cada chave é um grupo (ADR-022): quem tem a chave vê quem do grupo está com
//! o FSS aberto e quem está compartilhando.
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
  fluxo-sinal --chaves ARQUIVO [opções]      atende quem tem uma das chaves
  fluxo-sinal --nova-chave ENDEREÇO [--nome GRUPO] [--chaves ARQUIVO]
                                             cria uma chave (um grupo) e mostra;
                                             com --chaves, guarda no arquivo
Opções:
  --chaves ARQUIVO  chaves aceitas, uma por linha (# começa comentário). Cada
                    chave é um grupo: o nome vem depois dela, na mesma linha
                    (FSS1-…  Amigos). Para tirar o acesso de alguém, apague a
                    linha (ou troque a chave do grupo) e reinicie
  --nome GRUPO      nome do grupo da chave nova (padrão: Grupo <id>)
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
    nome: Option<String>,
    aberto: bool,
}

fn ler_opts() -> Result<Option<Opts>, String> {
    let mut o = Opts {
        porta: SIGNAL_PORT,
        ip: "0.0.0.0".to_string(),
        chaves: None,
        nova_chave: None,
        nome: None,
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
            "--nome" => o.nome = Some(valor()?),
            "--aberto" => o.aberto = true,
            "--help" | "-h" | "ajuda" => return Ok(None),
            outro => return Err(format!("opção desconhecida: {outro}")),
        }
    }
    Ok(Some(o))
}

/// As chaves de um arquivo e o nome do grupo de cada uma: uma por linha, o
/// nome depois da chave (sem nome: `Grupo <id>`); linhas vazias e com `#`
/// não contam.
fn ler_chaves(texto: &str) -> Result<Vec<(ServerKey, String)>, String> {
    texto
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|(i, l)| {
            let l = l.trim();
            let (chave, nome) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
            let k = ServerKey::parse(chave).map_err(|e| format!("linha {}: {e}", i + 1))?;
            let nome = match nome.trim() {
                "" => format!("Grupo {}", k.id()),
                n => n.to_string(),
            };
            Ok((k, nome))
        })
        .collect()
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
                .and_then(|mut f| {
                    let nome = o.nome.as_deref().map(str::trim).unwrap_or_default();
                    writeln!(f, "# {} (id {})\n{chave}  {nome}", chave.server(), chave.id())
                });
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
            let ids: Vec<String> = chaves
                .iter()
                .map(|(k, nome)| format!("{} ({nome})", k.id()))
                .collect();
            println!("  {} chave(s) aceita(s): {}", chaves.len(), ids.join(", "));
        }
    }
    let inicio = Instant::now();
    let nunca = AtomicBool::new(false);
    // O host repete a procura a cada 250 ms, e numa sala aberta procura o
    // tempo todo (cada vez num socket novo): o mesmo acontecimento só volta
    // ao registro depois de um minuto.
    let mut vistos: HashMap<String, Instant> = HashMap::new();
    SignalServer::with_groups(chaves).run(&socket, &nunca, |ev| {
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
        let texto = format!("# amigos\n{a}\n\n  # do notebook:\n  {b}   Amigos da Flame  \n");
        let lidas = ler_chaves(&texto).unwrap();
        assert_eq!(
            lidas,
            vec![
                (a.clone(), format!("Grupo {}", a.id())),
                (b, "Amigos da Flame".to_string())
            ]
        );
        assert!(ler_chaves("# só comentário\n").unwrap().is_empty());
        assert!(ler_chaves("FSS1-ERRADA").unwrap_err().starts_with("linha 1"));
    }
}
