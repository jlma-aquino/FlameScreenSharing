//! # fluxo-client
//!
//! Lado de quem assiste: rede → decode → tela.
//!
//! ```text
//! fluxo-client [--porta P] [--vsync] [--segundos S] [--largura L] [--altura A] [--no-topo]
//!              [--sem-painel] [--codigo XXXX-XXXX] [--sem-cifra] [--sinal SERVIDOR] [--internet]
//!              [--sem-audio] [--volume V]
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
  --sinal S       registra o código no servidor de sinalização S (fluxo-sinal), para
                  um host em outra rede achar este PC: fluxo-host --sinal S --codigo ...
  --internet      redes diferentes, sem servidor: mostra um código de conexão para
                  mandar a quem compartilha (fluxo-host --conexao ...) e pede no
                  terminal o código de resposta que ele mandar de volta
  --sem-upnp      com --internet: não abre a porta no roteador por UPnP
  --sem-audio     não toca o som do host
  --volume V      volume do som do host, 0 a 100 (padrão 100; só do Fluxo)
  --saida-som N   toca na saída de som N (padrão: a do Windows; lista em fluxo-bench audio)
  --sem-painel    começa sem o painel de estatísticas sobre a imagem (F2 mostra)

Na janela: F11 ou Alt+Enter alterna tela cheia; Esc sai da tela cheia;
F2 mostra/esconde o painel de estatísticas.
No PC que compartilha:  fluxo-host --destino <IP deste PC>:9000 --codigo <código>
  (redes diferentes:    fluxo-host --sinal <servidor> --codigo <código>)";

struct Opts {
    porta: u16,
    vsync: bool,
    segundos: f64,
    largura: u32,
    altura: u32,
    no_topo: bool,
    codigo: Option<String>,
    sem_cifra: bool,
    sinal: Option<String>,
    internet: bool,
    sem_upnp: bool,
    sem_audio: bool,
    /// Volume do som do host, 0 a 100 (só do Fluxo, não o do sistema).
    volume: f32,
    /// Saída de som onde tocar (`None`: a padrão do Windows).
    saida_som: Option<usize>,
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
        sinal: None,
        internet: false,
        sem_upnp: false,
        sem_audio: false,
        volume: 100.0,
        saida_som: None,
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
            "--sinal" => o.sinal = Some(valor()?.clone()),
            "--internet" => o.internet = true,
            "--sem-upnp" => o.sem_upnp = true,
            "--sem-audio" => o.sem_audio = true,
            "--volume" => o.volume = valor()?.parse().map_err(|_| "volume inválido (0 a 100)")?,
            "--saida-som" => o.saida_som = Some(valor()?.parse().map_err(|_| "saida-som inválida")?),
            "--sem-painel" => o.sem_painel = true,
            "--help" | "-h" | "ajuda" => return Err(String::new()),
            outro => return Err(format!("opção desconhecida: {outro}")),
        }
    }
    if o.internet && (o.sinal.is_some() || o.sem_cifra) {
        return Err("--internet não combina com --sinal nem --sem-cifra".into());
    }
    if o.sinal.is_some() && o.sem_cifra {
        return Err("--sinal precisa do código (sem --sem-cifra)".into());
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
    let servidor = o
        .sinal
        .as_deref()
        .map(fluxo_net::resolve_signal_server)
        .transpose()?;
    // Sem servidor (--internet): endereços públicos pelo STUN (IPv4 e IPv6)
    // e porta aberta por UPnP, se der; só valem para estes sockets, que são
    // os que recebem o vídeo.
    let internet = match o.internet {
        true => Some(fluxo_net::Internet::open(o.porta, !o.sem_upnp)?),
        false => None,
    };
    let (v4, v6, keepalive) = match &internet {
        Some(i) => (
            i.v4.try_clone()?,
            i.v6.as_ref().map(std::net::UdpSocket::try_clone).transpose()?,
            Some(i.public.server),
        ),
        None => (std::net::UdpSocket::bind(("0.0.0.0", o.porta))?, None, None),
    };
    let rx = VideoReceiver::with_sockets(
        v4,
        v6,
        ReceiverConfig {
            pairing_code: codigo.clone(),
            signal_server: servidor,
            keepalive_to: keepalive,
            on_pairing: internet
                .as_ref()
                .map(|_| fluxo_net::PairingObserver::print_to_terminal()),
            // O host escolhe o codec entre os que este Windows decodifica.
            codecs: fluxo_decode::decodable_codecs(),
            ..ReceiverConfig::default()
        },
    )?;
    let conexao = match (&internet, &codigo) {
        (Some(i), Some(c)) => Some(i.connection_code(c.clone())?),
        _ => None,
    };
    // Como o host acha este PC: pelo servidor (o que foi digitado em
    // --sinal) ou pelo IP da rede local.
    let endereco = match (&o.sinal, fluxo_net::local_ip()) {
        (Some(s), _) => format!("--sinal {s}"),
        (None, Some(ip)) => format!("--destino {ip}:{}", o.porta),
        (None, None) => format!("--destino <IP deste PC>:{}", o.porta),
    };
    let espera = match (&codigo, &conexao) {
        (_, Some(cc)) => {
            println!("Código de conexão: {cc}");
            for linha in internet.iter().flat_map(|i| i.summary()) {
                println!("  {linha}");
            }
            println!("  Mande para quem vai compartilhar:  fluxo-host --conexao {cc}");
            match cc.open {
                true => println!("Ele conecta direto; se não conectar, cole aqui o código de resposta dele:"),
                false => println!("Depois cole aqui o código de resposta que ele mandar de volta:"),
            }
            ler_respostas(rx.puncher());
            format!("Fluxo — código de conexão {cc} · cole a resposta do host no terminal")
        }
        (Some(c), None) => {
            println!("Código de pareamento: {c}");
            println!("No PC que compartilha:  fluxo-host {endereco} --codigo {c}");
            format!("Fluxo — código {c} · aguardando o host ({endereco})")
        }
        (None, None) => {
            println!("TESTE: sem pareamento nem cifra (--sem-cifra)");
            format!("Fluxo — aguardando o host ({endereco}, sem cifra)")
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
        espera: match (&codigo, &conexao) {
            (_, Some(cc)) => format!("código de conexão\n{cc}\ncole a resposta do\nhost no terminal"),
            (Some(c), None) => format!("código {c}\nhost: {endereco}\n--codigo {c}"),
            (None, None) => format!("aguardando o host\n{endereco} (sem cifra)"),
        },
    };
    // Áudio numa thread própria: toca no ritmo da placa, sem esperar o vídeo.
    let audio = match (o.sem_audio, rx.take_audio()) {
        (false, Some(fila)) => {
            let (c, volume, saida) = (comp.clone(), o.volume / 100.0, o.saida_som);
            Some(
                std::thread::Builder::new()
                    .name("fluxo-audio".into())
                    .spawn(move || tocar_audio(fila, volume, saida, &c))?,
            )
        }
        _ => None,
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
    comp.fechar.store(true, Ordering::Relaxed);
    match audio.map(|t| t.join()) {
        Some(Ok(Ok(resumo))) => println!("  {resumo}"),
        Some(Ok(Err(e))) => println!("  áudio desligado: {e}"),
        Some(Err(_)) => println!("  áudio: a thread terminou com pânico"),
        None => {}
    }
    Ok(())
}

/// Lê do terminal os códigos de resposta do host (numa thread: a janela e o
/// vídeo seguem) e fura o NAT na direção de cada um.
#[cfg(windows)]
fn ler_respostas(furos: fluxo_net::Puncher) {
    std::thread::spawn(move || {
        for linha in std::io::stdin().lines() {
            let Ok(linha) = linha else {
                return; // sem terminal
            };
            if linha.trim().is_empty() {
                continue;
            }
            match fluxo_net::ReplyCode::parse(&linha) {
                Ok(r) => {
                    let destinos = r.candidates();
                    furos.punch(&destinos);
                    let lista: Vec<String> = destinos.iter().map(|d| d.to_string()).collect();
                    println!("Abrindo caminho até o host em {}…", lista.join(" e "));
                }
                Err(e) => println!("{e}. Cole de novo:"),
            }
        }
    });
}

/// Thread de áudio: tira os quadros da fila da rede e toca no ritmo da placa
/// (buffer de jitter, Opus, saída padrão), até a janela fechar.
#[cfg(windows)]
fn tocar_audio(
    fila: fluxo_net::AudioStream,
    volume: f32,
    saida: Option<usize>,
    c: &exibir::Compartilhado,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    let mut tocador = fluxo_audio::AudioPlayer::new(volume, saida)?;
    // Para quando a janela fecha ou quando a rede termina (a exibição soltou
    // o VideoReceiver): os dois são o fim normal.
    'tocar: while !c.fechar.load(Ordering::Relaxed) {
        // Espera curta: a placa precisa ser alimentada a cada poucos ms.
        let mut espera = Duration::from_millis(3);
        loop {
            match fila.recv(espera) {
                Ok(Some(a)) => tocador.push(a.seq, a.data),
                Ok(None) => break,
                Err(fluxo_net::NetError::Closed) => break 'tocar,
                Err(e) => return Err(e.into()),
            }
            espera = Duration::ZERO;
        }
        tocador.pump(Instant::now())?;
    }
    let s = tocador.stats();
    Ok(format!(
        "áudio: {} quadros tocados, {} perdidos ({} refeitos pelo FEC), {} vezes sem som para tocar, \
         buffer final {} quadros de 10 ms",
        s.jitter.played, s.jitter.lost, s.fec, s.jitter.underruns, s.jitter.target
    ))
}
