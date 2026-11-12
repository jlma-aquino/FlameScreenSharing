//! # fluxo-client
//!
//! Lado de quem assiste: rede → decode → tela.
//!
//! ```text
//! fluxo-client [--sala CÓDIGO] [--porta P] [--vsync] [--segundos S] [--largura L] [--altura A] [--no-topo]
//!              [--sem-painel] [--codigo XXXX-XXXX] [--sem-cifra] [--sinal SERVIDOR] [--internet [--usar-upnp]]
//!              [--sem-audio] [--volume V]
//! ```
//!
//! Escuta numa porta UDP, decodifica com o `fluxo-decode` e mostra numa
//! janela (F11 ou Alt+Enter: tela cheia). As estatísticas aparecem num painel
//! sobre a imagem (F2 esconde) e no título da janela, uma vez por segundo, e
//! um resumo no terminal ao fechar.
//!
//! Detalhes e como ler os números: `docs/modulos/fluxo-client.md`.

// No Linux, sem a opção `portatil`, só avisa como compilar.
#![cfg_attr(not(any(windows, feature = "portatil")), allow(dead_code))]

// A barra de controles: lógica pura (testada em qualquer sistema), usada no Windows.
#[cfg(any(windows, test))]
mod controles;
#[cfg(windows)]
mod exibir;
#[cfg(windows)]
mod overlay;
#[cfg(feature = "portatil")]
mod portatil;
#[cfg(windows)]
mod present;
#[cfg(windows)]
mod window;

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering as Ordem};

/// Volume do som do host, 0 a 100. Começa no `--volume` e muda com o vídeo
/// rodando, quando chega `volume N` pela entrada padrão (é assim que a
/// janela do FSS mexe nele, sem reiniciar nada).
static VOLUME: AtomicU32 = AtomicU32::new(100);

/// Mudo (botão da barra sobre o vídeo, ou M): o volume escolhido fica guardado.
static MUDO: AtomicBool = AtomicBool::new(false);

/// O volume que toca agora, de 0 a 1 (0 no mudo).
pub(crate) fn volume() -> f32 {
    match MUDO.load(Ordem::Relaxed) {
        true => 0.0,
        false => VOLUME.load(Ordem::Relaxed) as f32 / 100.0,
    }
}

/// O volume escolhido, 0 a 100 (o mudo não mexe nele).
#[cfg_attr(not(windows), allow(dead_code))]
fn volume_pct() -> u32 {
    VOLUME.load(Ordem::Relaxed)
}

/// Muda o volume (limitado a 0..=100).
#[cfg_attr(not(windows), allow(dead_code))]
fn mudar_volume(v: i64) {
    VOLUME.store(v.clamp(0, 100) as u32, Ordem::Relaxed);
}

/// `volume N` → `Some(Ok(N))`; outra linha → `None`.
fn ler_volume(linha: &str) -> Option<Result<u32, String>> {
    let resto = linha.trim().strip_prefix("volume")?;
    Some(match resto.trim().parse::<u32>() {
        Ok(v) if v <= 100 => Ok(v),
        _ => Err("volume: use um número de 0 a 100".to_string()),
    })
}

/// `X,Y,L,A` (posição e tamanho da janela).
fn ler_janela(s: &str) -> Result<(i32, i32, i32, i32), String> {
    let n: Vec<i32> = s
        .split(',')
        .map(|p| p.trim().parse())
        .collect::<Result<_, _>>()
        .map_err(|_| "janela: use X,Y,LARGURA,ALTURA")?;
    match n[..] {
        [x, y, l, a] if l >= 200 && a >= 150 => Ok((x, y, l, a)),
        [_, _, _, _] => Err("janela: no mínimo 200 x 150".into()),
        _ => Err("janela: use X,Y,LARGURA,ALTURA".into()),
    }
}

const AJUDA: &str = "\
fluxo-client — mostra a tela transmitida por um fluxo-host

Uso:
  fluxo-client --sala CÓDIGO --chave FSS1-… [opções]
  fluxo-client [opções]

Opções:
  --sala CÓDIGO   entra na sala de quem compartilha (fluxo-host --sala mostra o
                  código), pelo servidor da --chave
  --chave FSS1-…  chave de acesso ao servidor de sinalização (a mesma de quem
                  compartilha): o endereço dele e o segredo que assina os pedidos
  --segredos-pela-entrada
                        lê da entrada, antes de tudo, linhas --opcao valor até a linha fim
                        (a chave e os códigos fora da linha de comando, que outros
                        programas do PC conseguem ler)
  --porta P       porta UDP onde escutar (padrão 9000)
  --vsync         sincroniza com o monitor (sem tearing; até 1 quadro a mais de atraso)
  --segundos S    fecha sozinho depois de S segundos do primeiro frame (padrão 0 = não)
  --largura L     largura inicial da janela (padrão 1280)
  --altura A      altura inicial da janela (padrão 720)
  --no-topo       janela sempre por cima das outras
  --codigo X      usa este código de pareamento (padrão: um novo, sorteado)
  --sem-cifra     TESTE (só em compilação de teste, não na de produção): sem pareamento nem cifra (o host também)
  --sinal S       registra o código no servidor de sinalização S (fluxo-sinal), para
                  um host em outra rede achar este PC: fluxo-host --sinal S --codigo ...
  --internet      redes diferentes, sem servidor: mostra um código de conexão para
                  mandar a quem compartilha (fluxo-host --conexao ...) e pede no
                  terminal o código de resposta que ele mandar de volta
  --usar-upnp     com --internet: abre a porta no roteador por UPnP (desligado por
                  padrão; a porta vale 1 hora, renovada enquanto o programa roda)
  --sem-audio     não toca o som do host
  --volume V      volume do som do host, 0 a 100 (padrão 100; só do Fluxo)
  --saida-som N   toca na saída de som N (padrão: a do Windows; lista em fluxo-bench audio)
  --sem-painel    começa sem o painel de estatísticas sobre a imagem (F2 mostra)
  --janela X,Y,L,A  posição e tamanho da janela (externos, em pixels de tela); ao
                  fechar, imprime Janela: X,Y,L,A (a janela do FSS volta ali)

Na janela: F11, F, Alt+Enter ou clique duplo alterna tela cheia; Esc sai da tela
cheia; F2 mostra/esconde o painel de estatísticas; M liga e desliga o som; setas
para cima e para baixo (ou a roda do mouse) mudam o volume. Com o mouse em
cima, aparece a barra de controles embaixo; parado, ela some (o cursor junto).
No terminal, com o vídeo rodando:  volume N  (0 a 100) muda o volume na hora.
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
    /// Chave de acesso ao servidor (endereço + segredo que assina os pedidos).
    chave: Option<fluxo_net::ServerKey>,
    /// Código da sala de quem compartilha.
    sala: Option<String>,
    internet: bool,
    usar_upnp: bool,
    sem_audio: bool,
    /// Volume do som do host, 0 a 100 (só do Fluxo, não o do sistema).
    volume: f32,
    /// Saída de som onde tocar (`None`: a padrão do Windows).
    saida_som: Option<usize>,
    sem_painel: bool,
    /// Retângulo externo da janela (x, y, largura, altura).
    janela: Option<(i32, i32, i32, i32)>,
    /// TESTE no Windows: usa o cliente do Linux (`--features portatil`).
    portatil: bool,
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
        chave: None,
        sala: None,
        internet: false,
        usar_upnp: false,
        sem_audio: false,
        volume: 100.0,
        saida_som: None,
        sem_painel: false,
        portatil: false,
        janela: None,
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
            // Só em compilação de teste: na de produção a cifra não desliga.
            "--sem-cifra" if cfg!(debug_assertions) => o.sem_cifra = true,
            "--sinal" => o.sinal = Some(valor()?.clone()),
            "--chave" => o.chave = Some(fluxo_net::ServerKey::parse(valor()?).map_err(|e| e.to_string())?),
            "--sala" => o.sala = Some(valor()?.clone()),
            "--internet" => o.internet = true,
            "--usar-upnp" => o.usar_upnp = true,
            "--sem-audio" => o.sem_audio = true,
            "--volume" => o.volume = valor()?.parse().map_err(|_| "volume inválido (0 a 100)")?,
            "--saida-som" => o.saida_som = Some(valor()?.parse().map_err(|_| "saida-som inválida")?),
            "--sem-painel" => o.sem_painel = true,
            "--portatil" => o.portatil = true,
            "--janela" => o.janela = Some(ler_janela(valor()?)?),
            "--help" | "-h" | "ajuda" => return Err(String::new()),
            outro => return Err(format!("opção desconhecida: {outro}")),
        }
    }
    if let (Some(k), None) = (&o.chave, &o.sinal) {
        o.sinal = Some(k.server().to_string());
    }
    if let Some(sala) = &o.sala {
        if o.internet || o.sem_cifra || o.codigo.is_some() {
            return Err("--sala já é o código: sem --codigo, --internet ou --sem-cifra".into());
        }
        fluxo_net::PairingCode::parse(sala).map_err(|e| format!("código da sala: {e}"))?;
        o.codigo = Some(sala.clone());
        if o.sinal.is_none() {
            return Err("--sala precisa da chave do servidor (--chave FSS1-…) ou de --sinal".into());
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
    // Segredos (chave, códigos) pela entrada, fora da linha de comando (fluxo-net, segredos.rs).
    let args = match fluxo_net::args_with_secrets(std::env::args().skip(1).collect()) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("erro: {e}");
            return ExitCode::from(2);
        }
    };
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
    VOLUME.store(opts.volume.clamp(0.0, 100.0).round() as u32, Ordem::Relaxed);
    match run(&opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(all(not(windows), feature = "portatil"))]
fn run(o: &Opts) -> Result<(), Box<dyn std::error::Error>> {
    let p = preparar(o)?;
    portatil::run(o, p.rx, p.titulo, p.espera)
}

#[cfg(not(any(windows, feature = "portatil")))]
fn run(_: &Opts) -> Result<(), Box<dyn std::error::Error>> {
    Err("fora do Windows, compile com: cargo build --release -p fluxo-client --features portatil".into())
}

/// O que o cliente abre antes da janela, igual em todo sistema.
struct Preparado {
    rx: fluxo_net::VideoReceiver,
    /// Título da janela até chegar vídeo.
    titulo: String,
    /// Texto na janela até chegar vídeo (código e endereço para o host).
    espera: String,
}

/// Codecs que este cliente decodifica: o que o Windows tiver; o portátil, só H.264.
fn codecs_aqui(o: &Opts) -> u8 {
    #[cfg(windows)]
    if !o.portatil {
        return fluxo_decode::decodable_codecs();
    }
    let _ = o;
    fluxo_net::VideoCodec::H264.bit()
}

/// Rede e códigos: abre os sockets, mostra no terminal o que mandar ao host e
/// fica ouvindo os códigos de resposta.
fn preparar(o: &Opts) -> Result<Preparado, Box<dyn std::error::Error>> {
    use fluxo_net::{PairingCode, ReceiverConfig, VideoReceiver};

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
        true => Some(fluxo_net::Internet::open(o.porta, o.usar_upnp)?),
        false => None,
    };
    let (v4, v6, keepalive) = match &internet {
        Some(i) => (
            i.v4.try_clone()?,
            i.v6.as_ref().map(std::net::UdpSocket::try_clone).transpose()?,
            Some(i.public.server),
        ),
        // Pela sinalização, IPv6 também: o endereço dele vai ao host pelo
        // servidor, e costuma passar onde o IPv4 trava.
        None => (
            std::net::UdpSocket::bind(("0.0.0.0", o.porta))?,
            servidor.and(fluxo_net::ipv6_socket_for_signal()),
            None,
        ),
    };
    let rx = VideoReceiver::with_sockets(
        v4,
        v6,
        ReceiverConfig {
            pairing_code: codigo.clone(),
            signal_server: servidor,
            signal_key: o.chave.clone(),
            keepalive_to: keepalive,
            on_pairing: internet
                .as_ref()
                .map(|_| fluxo_net::PairingObserver::print_to_terminal()),
            // O host escolhe o codec entre os que este PC decodifica.
            codecs: codecs_aqui(o),
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
    let titulo = match (&codigo, &conexao) {
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
            format!("FSS — código de conexão {cc} · cole a resposta do host")
        }
        (Some(c), None) if o.sala.is_some() => {
            println!("Entrando na sala {c}…");
            format!("FSS — sala {c} · esperando quem compartilha")
        }
        (Some(c), None) => {
            println!("Código de pareamento: {c}");
            println!("No PC que compartilha:  fluxo-host {endereco} --codigo {c}");
            format!("FSS — código {c} · aguardando o host ({endereco})")
        }
        (None, None) => {
            println!("TESTE: sem pareamento nem cifra (--sem-cifra)");
            format!("FSS — aguardando o host ({endereco}, sem cifra)")
        }
    };
    let espera = match (&codigo, &conexao) {
        (_, Some(cc)) => format!("código de conexão\n{cc}\ncole a resposta\ndo host"),
        (Some(c), None) if o.sala.is_some() => format!("sala {c}\nesperando quem\ncompartilha"),
        (Some(c), None) => format!("código {c}\nhost: {endereco}\n--codigo {c}"),
        (None, None) => format!("aguardando o host\n{endereco} (sem cifra)"),
    };
    // Códigos de resposta só valem com código de conexão; o volume, sempre.
    ler_entrada(conexao.is_some().then(|| rx.puncher()));
    Ok(Preparado { rx, titulo, espera })
}

/// Thread da janela: só processa mensagens e repassa comandos; rede, decode
/// e apresentação rodam na thread de exibição ([`exibir`]), que continua
/// mostrando frames enquanto a janela é arrastada.
#[cfg(windows)]
fn run(o: &Opts) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::exibir::{exibir, Compartilhado, Config};
    use crate::window::Window;

    #[cfg(feature = "portatil")]
    if o.portatil {
        let p = preparar(o)?;
        return portatil::run(o, p.rx, p.titulo, p.espera);
    }
    #[cfg(not(feature = "portatil"))]
    if o.portatil {
        return Err("compile com --features portatil".into());
    }
    crate::window::enable_dpi_awareness();
    let Preparado { rx, titulo, espera } = preparar(o)?;
    let mut janela = Window::new(&titulo, o.largura, o.altura, o.no_topo, o.janela)?;
    let comp = Arc::new(Compartilhado::default());
    comp.painel.store(!o.sem_painel, Ordering::Relaxed);
    // Primeiro desenho já: texto de espera e barra, antes de qualquer imagem.
    comp.redesenhar.store(true, Ordering::Relaxed);
    let cfg = Config {
        hwnd: janela.hwnd().0 as isize,
        tamanho: janela.client_size(),
        vsync: o.vsync,
        segundos: o.segundos,
        espera,
    };
    // Áudio numa thread própria: toca no ritmo da placa, sem esperar o vídeo.
    let audio = match (o.sem_audio, rx.take_audio()) {
        (false, Some(fila)) => {
            let (c, saida) = (comp.clone(), o.saida_som);
            Some(
                std::thread::Builder::new()
                    .name("fluxo-audio".into())
                    .spawn(move || tocar_audio(fila, saida, &c))?,
            )
        }
        _ => None,
    };
    let c = comp.clone();
    let exibicao = std::thread::Builder::new()
        .name("fluxo-exibicao".into())
        .spawn(move || exibir(rx, cfg, &c).map_err(|e| e.to_string()))?;

    // Barra de controles sobre o vídeo (controles.rs): estado aqui, desenho
    // na thread de exibição a partir do `Desenho` mais recente.
    let layout = |j: &Window| {
        let (w, h) = j.client_size();
        controles::Layout::new(w, h, j.escala())
    };
    let mut barra = controles::Controles::new(layout(&janela), Instant::now());
    let mut desenhado: Option<controles::Desenho> = None;

    while !exibicao.is_finished() {
        janela.wait_messages(Duration::from_millis(10));
        let agora = Instant::now();
        if janela.closed() {
            comp.fechar.store(true, Ordering::Relaxed);
        }
        let mut alternar_tela_cheia = janela.take_fullscreen_toggle();
        let mut alternar_painel = janela.take_painel_toggle();
        if janela.take_mudo_toggle() {
            MUDO.fetch_xor(true, Ordering::Relaxed);
        }
        let passos = janela.take_volume_passos();
        if passos != 0 {
            mudar_volume(volume_pct() as i64 + passos as i64 * controles::PASSO_VOLUME as i64);
        }
        for e in janela.take_entradas() {
            match barra.entrada(e, volume_pct(), agora) {
                Some(controles::Acao::AlternarMudo) => {
                    MUDO.fetch_xor(true, Ordering::Relaxed);
                }
                Some(controles::Acao::Volume(v)) => {
                    MUDO.store(false, Ordering::Relaxed);
                    mudar_volume(v as i64);
                }
                Some(controles::Acao::AlternarEstatisticas) => alternar_painel = !alternar_painel,
                Some(controles::Acao::AlternarTelaCheia) => alternar_tela_cheia = !alternar_tela_cheia,
                Some(controles::Acao::Sair) => {
                    janela.close();
                    comp.fechar.store(true, Ordering::Relaxed);
                }
                None => {}
            }
        }
        if alternar_painel {
            comp.painel.fetch_xor(true, Ordering::Relaxed);
        }
        if alternar_tela_cheia {
            janela.toggle_fullscreen();
        }
        if janela.take_resized() {
            *comp.tamanho.lock().unwrap_or_else(|e| e.into_inner()) = Some(janela.client_size());
            barra.layout = layout(&janela);
        }
        // Antes da primeira imagem, a barra fica (é por ela que dá para sair).
        barra.tempo(agora, !comp.tem_imagem.load(Ordering::Relaxed));
        janela.set_esconder_cursor(barra.esconder_cursor());
        let desenho = barra.visivel.then(|| controles::Desenho {
            layout: barra.layout,
            sob_o_mouse: barra.sob_o_mouse,
            volume: volume_pct(),
            mudo: MUDO.load(Ordering::Relaxed),
            estatisticas: comp.painel.load(Ordering::Relaxed),
            tela_cheia: janela.is_fullscreen(),
        });
        if desenho != desenhado {
            desenhado = desenho;
            *comp.controles.lock().unwrap_or_else(|e| e.into_inner()) = desenho;
            comp.redesenhar.store(true, Ordering::Relaxed);
        }
        if let Some(t) = comp.titulo.lock().unwrap_or_else(|e| e.into_inner()).take() {
            janela.set_title(&t);
        }
    }
    // Para a janela do FSS voltar onde o vídeo estava (fora da tela cheia).
    let (x, y, w, h) = janela.retangulo_normal();
    println!("Janela: {x},{y},{w},{h}");
    // A janela do FSS (que nos abriu) pode voltar para a frente: o Windows só
    // deixa quem está em primeiro plano passar o foco adiante.
    // SAFETY: só muda a permissão de foco.
    let _ = unsafe {
        windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
            windows::Win32::UI::WindowsAndMessaging::ASFW_ANY,
        )
    };
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

/// Lê a entrada padrão numa thread (a janela e o vídeo seguem): `volume N`
/// e, com código de conexão (`furos`), os códigos de resposta do host,
/// furando o NAT na direção de cada um.
fn ler_entrada(furos: Option<fluxo_net::Puncher>) {
    std::thread::spawn(move || {
        for linha in std::io::stdin().lines() {
            let Ok(linha) = linha else {
                return; // sem terminal
            };
            if linha.trim().is_empty() {
                continue;
            }
            match ler_volume(&linha) {
                Some(Ok(v)) => {
                    VOLUME.store(v, Ordem::Relaxed);
                    println!("Volume: {v}%");
                    continue;
                }
                Some(Err(e)) => {
                    println!("{e}");
                    continue;
                }
                None => {}
            }
            let Some(furos) = &furos else {
                println!("(não entendi \"{}\": use volume 0 a 100)", linha.trim());
                continue;
            };
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
    saida: Option<usize>,
    c: &exibir::Compartilhado,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    let mut atual = volume();
    let mut tocador = fluxo_audio::AudioPlayer::new(atual, saida)?;
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
        if volume() != atual {
            atual = volume();
            tocador.set_volume(atual);
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

#[cfg(test)]
mod tests {
    use super::{ler_janela, ler_volume};

    #[test]
    fn janela_pela_linha_de_comando() {
        assert_eq!(ler_janela("-8,0,1296,739"), Ok((-8, 0, 1296, 739)));
        assert!(ler_janela("1,2,3").is_err());
        assert!(ler_janela("0,0,100,100").is_err());
        assert!(ler_janela("a,b,c,d").is_err());
    }

    #[test]
    fn volume_pela_entrada() {
        assert_eq!(ler_volume("volume 40"), Some(Ok(40)));
        assert_eq!(ler_volume(" volume 0 "), Some(Ok(0)));
        assert!(matches!(ler_volume("volume 101"), Some(Err(_))));
        assert!(matches!(ler_volume("volume"), Some(Err(_))));
        // Códigos de resposta seguem para o furo do NAT.
        assert_eq!(ler_volume("8D9E-66WU-866"), None);
    }
}
