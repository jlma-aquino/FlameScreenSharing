//! # fluxo-sinal
//!
//! Servidor de sinalização: o cliente registra a sala do seu código, o host
//! procura a sala, e cada um fica sabendo os endereços do outro para falarem
//! direto (furo de NAT). Vídeo nunca passa por aqui; o servidor não vê o
//! código, só o número da sala.
//!
//! ```text
//! fluxo-sinal [--porta P] [--ip IP]
//! ```
//!
//! Precisa ficar num endereço que os dois alcancem (IP público, porta UDP
//! aberta). Detalhes: `docs/modulos/fluxo-sinal.md`.

use std::collections::HashMap;
use std::net::UdpSocket;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use fluxo_net::{SignalEvent, SignalServer, SIGNAL_PORT};

const AJUDA: &str = "\
fluxo-sinal — servidor de sinalização do Fluxo (apresenta host e cliente)

Uso:
  fluxo-sinal [opções]

Opções:
  --porta P   porta UDP (padrão 7000)
  --ip IP     endereço local onde escutar (padrão 0.0.0.0 = todos)

Sala (quem compartilha cria o código, quem assiste entra com ele):
  No PC que compartilha:  fluxo-host --sala --sinal <endereço deste servidor>
  No PC que assiste:      fluxo-client --sala <código> --sinal <endereço deste servidor>";

fn main() -> ExitCode {
    let (mut porta, mut ip) = (SIGNAL_PORT, "0.0.0.0".to_string());
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let valor = args.next();
        match (a.as_str(), valor) {
            ("--porta", Some(v)) => match v.parse() {
                Ok(p) => porta = p,
                Err(_) => return erro("porta inválida"),
            },
            ("--ip", Some(v)) => ip = v,
            ("--help" | "-h" | "ajuda", _) => {
                println!("{AJUDA}");
                return ExitCode::SUCCESS;
            }
            (outro, _) => return erro(&format!("opção desconhecida ou sem valor: {outro}")),
        }
    }
    let socket = match UdpSocket::bind((ip.as_str(), porta)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("erro: abrir a porta UDP {porta} em {ip}: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "Servidor de sinalização em {} (UDP). Ctrl+C encerra.",
        socket.local_addr().map(|a| a.to_string()).unwrap_or_default()
    );
    let inicio = Instant::now();
    let nunca = AtomicBool::new(false);
    // O host repete a procura a cada 250 ms, e numa sala aberta procura o
    // tempo todo (cada vez num socket novo): o mesmo acontecimento só volta
    // ao registro depois de um minuto.
    let mut vistos: HashMap<String, Instant> = HashMap::new();
    SignalServer::default().run(&socket, &nunca, |ev| {
        let agora = Instant::now();
        let chave = match ev {
            SignalEvent::NotFound { room, host } => format!("vazia {room} {}", host.ip()),
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
