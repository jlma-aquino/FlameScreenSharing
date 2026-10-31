//! # fluxo-client
//!
//! Lado de quem assiste: rede → decode → tela.
//!
//! ```text
//! fluxo-client [--porta P] [--vsync] [--segundos S] [--largura L] [--altura A] [--no-topo]
//!              [--sem-painel]
//! ```
//!
//! Escuta numa porta UDP, decodifica com o `fluxo-decode` e mostra numa
//! janela (F11 ou Alt+Enter: tela cheia). As estatísticas aparecem num painel
//! sobre a imagem (F2 esconde) e no título da janela, uma vez por segundo, e
//! um resumo no terminal ao fechar.
//!
//! Detalhes e como ler os números: `docs/modulos/fluxo-client.md`.

#[cfg(windows)]
mod exibir;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod present;
#[cfg(windows)]
mod window;

use std::process::ExitCode;

const AJUDA: &str = "\
fluxo-client — mostra a tela transmitida por um fluxo-host

Uso:
  fluxo-client [opções]

Opções:
  --porta P       porta UDP onde escutar (padrão 9000)
  --vsync         sincroniza com o monitor (sem tearing; até 1 quadro a mais de atraso)
  --segundos S    fecha sozinho depois de S segundos do primeiro frame (padrão 0 = não)
  --largura L     largura inicial da janela (padrão 1280)
  --altura A      altura inicial da janela (padrão 720)
  --no-topo       janela sempre por cima das outras
  --codigo X      usa este código de pareamento (padrão: um novo, sorteado)
  --sem-cifra     TESTE: sem pareamento nem cifra (o host também)
  --sem-painel    começa sem o painel de estatísticas sobre a imagem (F2 mostra)

Na janela: F11 ou Alt+Enter alterna tela cheia; Esc sai da tela cheia;
F2 mostra/esconde o painel de estatísticas.
No PC que compartilha:  fluxo-host --destino <IP deste PC>:9000";

struct Opts {
    porta: u16,
    vsync: bool,
    segundos: f64,
    largura: u32,
    altura: u32,
    no_topo: bool,
    codigo: Option<String>,
    sem_cifra: bool,
    sem_painel: bool,
}

fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        porta: 9000,
        vsync: false,
        segundos: 0.0,
        largura: 1280,
        altura: 720,
        no_topo: false,
        codigo: None,
        sem_cifra: false,
        sem_painel: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut valor = || it.next().ok_or(format!("{a} precisa de um valor"));
        match a.as_str() {
            "--porta" => o.porta = valor()?.parse().map_err(|_| "porta inválida")?,
            "--vsync" => o.vsync = true,
            "--segundos" => o.segundos = valor()?.parse().map_err(|_| "segundos inválido")?,
            "--largura" => o.largura = valor()?.parse().map_err(|_| "largura inválida")?,
            "--altura" => o.altura = valor()?.parse().map_err(|_| "altura inválida")?,
            "--no-topo" => o.no_topo = true,
            "--codigo" => o.codigo = Some(valor()?.clone()),
            "--sem-cifra" => o.sem_cifra = true,
            "--sem-painel" => o.sem_painel = true,
            "--help" | "-h" | "ajuda" => return Err(String::new()),
            outro => return Err(format!("opção desconhecida: {outro}")),
        }
    }
    Ok(o)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = match parse_opts(&args) {
        Ok(o) => o,
        Err(e) if e.is_empty() => {
            println!("{AJUDA}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("erro: {e}\n\n{AJUDA}");
            return ExitCode::from(2);
        }
    };
    match run(&opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn run(_: &Opts) -> Result<(), Box<dyn std::error::Error>> {
    Err("o fluxo-client só roda no Windows".into())
}

/// Thread da janela: só processa mensagens e repassa comandos; rede, decode
/// e apresentação rodam na thread de exibição ([`exibir`]), que continua
/// mostrando frames enquanto a janela é arrastada.
#[cfg(windows)]
fn run(o: &Opts) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;

    use fluxo_net::{PairingCode, ReceiverConfig, VideoReceiver};

    use crate::exibir::{exibir, Compartilhado, Config};
    use crate::window::Window;

    // Código de pareamento: o host digita este, junto com o nosso endereço.
    let codigo = match (&o.codigo, o.sem_cifra) {
        (_, true) => None,
        (Some(c), false) => Some(PairingCode::parse(c)?),
        (None, false) => Some(PairingCode::generate()?),
    };
    let rx = VideoReceiver::bind(
        ("0.0.0.0", o.porta),
        ReceiverConfig {
            pairing_code: codigo.clone(),
            ..ReceiverConfig::default()
        },
    )?;
    let endereco = match ip_local() {
        Some(ip) => format!("{ip}:{}", o.porta),
        None => format!("<IP deste PC>:{}", o.porta),
    };
    let espera = match &codigo {
        Some(c) => {
            println!("Código de pareamento: {c}");
            println!("No PC que compartilha:  fluxo-host --destino {endereco} --codigo {c}");
            format!("Fluxo — código {c} · aguardando o host em {endereco}")
        }
        None => {
            println!("TESTE: sem pareamento nem cifra (--sem-cifra)");
            format!("Fluxo — aguardando o host em {endereco} (sem cifra)")
        }
    };
    let mut janela = Window::new(&espera, o.largura, o.altura, o.no_topo)?;
    let comp = Arc::new(Compartilhado::default());
    comp.painel.store(!o.sem_painel, Ordering::Relaxed);
    let cfg = Config {
        hwnd: janela.hwnd().0 as isize,
        tamanho: janela.client_size(),
        vsync: o.vsync,
        segundos: o.segundos,
        espera: match &codigo {
            Some(c) => format!("código {c}\nhost: --destino {endereco}\n--codigo {c}"),
            None => format!("aguardando o host em\n{endereco} (sem cifra)"),
        },
    };
    let c = comp.clone();
    let exibicao = std::thread::Builder::new()
        .name("fluxo-exibicao".into())
        .spawn(move || exibir(rx, cfg, &c).map_err(|e| e.to_string()))?;

    while !exibicao.is_finished() {
        janela.wait_messages(Duration::from_millis(10));
        if janela.closed() {
            comp.fechar.store(true, Ordering::Relaxed);
        }
        if janela.take_painel_toggle() {
            comp.painel.fetch_xor(true, Ordering::Relaxed);
        }
        if janela.take_fullscreen_toggle() {
            janela.toggle_fullscreen();
        }
        if janela.take_resized() {
            *comp.tamanho.lock().unwrap_or_else(|e| e.into_inner()) = Some(janela.client_size());
        }
        if let Some(t) = comp.titulo.lock().unwrap_or_else(|e| e.into_inner()).take() {
            janela.set_title(&t);
        }
    }
    // A swapchain (na outra thread) já foi solta; agora a janela pode fechar.
    exibicao
        .join()
        .map_err(|_| "a thread de exibição terminou com pânico")??;
    Ok(())
}

/// Endereço provável deste PC na rede local: o da interface que o sistema
/// usaria para chegar à internet. `connect` num socket UDP só escolhe a rota;
/// nenhum pacote sai.
#[cfg(windows)]
fn ip_local() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:53").ok()?;
    Some(s.local_addr().ok()?.ip())
}
