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

use std::net::UdpSocket;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use fluxo_net::{SignalEvent, SignalServer, SIGNAL_PORT};

const AJUDA: &str = "\
fluxo-sinal — servidor de sinalização do Fluxo (apresenta host e cliente)

Uso:
  fluxo-sinal [opções]

Opções:
  --porta P   porta UDP (padrão 7000)
  --ip IP     endereço local onde escutar (padrão 0.0.0.0 = todos)

No PC que assiste:      fluxo-client --sinal <endereço deste servidor>
No PC que compartilha:  fluxo-host --sinal <endereço deste servidor> --codigo <código>";

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
    // O host repete a procura a cada 250 ms: mostra só o que muda.
    let mut ultimo: Option<SignalEvent> = None;
    SignalServer::default().run(&socket, &nunca, |ev| {
        if ultimo.as_ref() == Some(ev) {
            return;
        }
        ultimo = Some(ev.clone());
        let t = inicio.elapsed().as_secs_f64();
        match ev {
            SignalEvent::Registered { room, client } => {
                println!("[{t:8.1} s] sala {room:05X}: cliente em {client}")
            }
            SignalEvent::Found { room, host, client } => {
                println!("[{t:8.1} s] sala {room:05X}: host {host} achou o cliente {client}")
            }
            SignalEvent::NotFound { room, host } => {
                println!("[{t:8.1} s] sala {room:05X}: host {host} procurou, sala vazia")
            }
            SignalEvent::RoomTaken { room, client } => {
                println!("[{t:8.1} s] sala {room:05X}: recusada para {client} (ocupada)")
            }
        }
    });
    ExitCode::SUCCESS
}

fn erro(msg: &str) -> ExitCode {
    eprintln!("erro: {msg}\n\n{AJUDA}");
    ExitCode::from(2)
}
