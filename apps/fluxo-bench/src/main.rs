//! # fluxo-bench
//!
//! Ferramenta de linha de comando para validar captura, encode, decode e
//! rede isoladamente.
//!
//! ```text
//! fluxo-bench listar
//! fluxo-bench bench      [--monitor N] [--segundos S] [--copiar-cpu]
//! fluxo-bench screenshot [--monitor N] [--saida arquivo.bmp]
//! fluxo-bench encoders   [--monitor N]
//! fluxo-bench encode     [--monitor N] [--segundos S] [--encoder auto|nvenc|mf]
//!                        [--mbps M] [--fps F] [--keyframe-a-cada S] [--saida arquivo.h264]
//!                        [--verificar] [--refresh N] [--perder-a-cada S]
//! fluxo-bench decode    [--monitor N] [--segundos S] [--encoder auto|nvenc|mf] [--mbps M]
//! fluxo-bench receber    [--porta P] [--segundos S] [--saida arquivo.h264] [--verificar] [--sinal SERVIDOR | --internet]
//! fluxo-bench enviar     (--destino IP:PORTA | --sinal SERVIDOR | --conexao C) [--mbps M] [--fps F] [--segundos S] [--fec K]
//! fluxo-bench nat
//! ```
//!
//! Detalhes e como interpretar os números: `docs/modulos/fluxo-bench.md`.

mod bmp;
#[cfg(windows)]
mod encode;
mod enviar;
mod nat;
mod receber;
#[cfg(windows)]
mod verificar;

use std::error::Error;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use fluxo_capture::{enable_dpi_awareness, list_monitors, CaptureError, DesktopCapturer};
use fluxo_encode::BackendChoice;

/// Opções aceitas pelos comandos.
struct Opts {
    monitor: usize,
    segundos: f64,
    copiar_cpu: bool,
    /// Arquivo de saída; cada comando tem seu padrão.
    saida: Option<String>,
    encoder: BackendChoice,
    mbps: f64,
    fps: u32,
    keyframe_a_cada: f64,
    verificar: bool,
    porta: u16,
    /// Destino do `enviar` (IP:PORTA).
    destino: Option<String>,
    /// Servidor de sinalização do `receber` (registra) e do `enviar` (procura).
    sinal: Option<String>,
    /// Código de conexão do `receber --internet` (o `enviar` conecta sem servidor).
    conexao: Option<String>,
    /// TTL dos pedidos do `enviar --conexao` até o furo do receber chegar (0 = normal).
    ttl_furo: u32,
    /// `receber` sem servidor: mostra um código de conexão (STUN público).
    internet: bool,
    /// `receber --internet` sem tentar abrir a porta no roteador por UPnP.
    sem_upnp: bool,
    /// FEC do `enviar`: 1 paridade a cada K pacotes (0 = sem).
    fec: u8,
    /// Código de pareamento do `receber` (padrão: sorteado) e do `enviar`.
    codigo: Option<String>,
    /// `receber`/`enviar` sem pareamento nem cifra.
    sem_cifra: bool,
    /// Intra-refresh em ondas de N frames (0 = desligado).
    refresh: u32,
    /// No encode com `--verificar`, descarta um frame a cada S segundos
    /// (0 = nunca) e mede a recuperação do decoder.
    perder_a_cada: f64,
}

fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        monitor: 0,
        segundos: 10.0,
        copiar_cpu: false,
        saida: None,
        encoder: BackendChoice::Auto,
        mbps: 20.0,
        fps: 60,
        keyframe_a_cada: 5.0,
        verificar: false,
        porta: 9000,
        destino: None,
        sinal: None,
        conexao: None,
        ttl_furo: fluxo_net::PUNCH_TTL,
        internet: false,
        sem_upnp: false,
        fec: 10,
        codigo: None,
        sem_cifra: false,
        refresh: 0,
        perder_a_cada: 0.0,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut valor = || it.next().ok_or(format!("{a} precisa de um valor"));
        match a.as_str() {
            "--monitor" => o.monitor = valor()?.parse().map_err(|_| "monitor inválido")?,
            "--segundos" => o.segundos = valor()?.parse().map_err(|_| "segundos inválido")?,
            "--saida" => o.saida = Some(valor()?.clone()),
            "--copiar-cpu" => o.copiar_cpu = true,
            "--verificar" => o.verificar = true,
            "--porta" => o.porta = valor()?.parse().map_err(|_| "porta inválida")?,
            "--destino" => o.destino = Some(valor()?.clone()),
            "--sinal" => o.sinal = Some(valor()?.clone()),
            "--conexao" => o.conexao = Some(valor()?.clone()),
            "--ttl-furo" => o.ttl_furo = valor()?.parse().map_err(|_| "ttl-furo inválido")?,
            "--internet" => o.internet = true,
            "--sem-upnp" => o.sem_upnp = true,
            "--fec" => o.fec = valor()?.parse().map_err(|_| "fec inválido (0 a 255)")?,
            "--codigo" => o.codigo = Some(valor()?.clone()),
            "--sem-cifra" => o.sem_cifra = true,
            "--encoder" => o.encoder = valor()?.parse()?,
            "--mbps" => o.mbps = valor()?.parse().map_err(|_| "mbps inválido")?,
            "--fps" => o.fps = valor()?.parse().map_err(|_| "fps inválido")?,
            "--keyframe-a-cada" => {
                o.keyframe_a_cada = valor()?.parse().map_err(|_| "keyframe-a-cada inválido")?
            }
            "--refresh" => o.refresh = valor()?.parse().map_err(|_| "refresh inválido")?,
            "--perder-a-cada" => o.perder_a_cada = valor()?.parse().map_err(|_| "perder-a-cada inválido")?,
            outro => return Err(format!("opção desconhecida: {outro}")),
        }
    }
    Ok(o)
}

const AJUDA: &str = "\
fluxo-bench — testes de captura e encode do Fluxo

Comandos:
  listar                      lista os monitores disponíveis
  bench      [opções]         mede quantos frames por segundo a captura entrega
  screenshot [opções]         salva um frame como imagem BMP
  encoders   [opções]         mostra quais encoders funcionam nesta máquina
  encode     [opções]         captura + encode: mede tempos e grava um .h264
  decode     [opções]         captura + encode + decode: mede o decoder do cliente
  receber    [opções]         recebe o vídeo do fluxo-host, mede e grava um .h264
  enviar     [opções]         manda frames sintéticos ao receber (carga fixa, sem captura)
  nat                         mostra como o NAT desta rede trata UDP (furo de NAT passa?)

Opções:
  --monitor N           índice do monitor (padrão 0; veja 'listar')
  --segundos S          duração do bench/encode/receber (padrão 10; no receber,
                        conta a partir do primeiro frame; 0 = sem fim)
  --copiar-cpu          no bench, também copia cada frame para a RAM e mede o custo
  --saida ARQ           arquivo de saída (padrão captura.bmp / captura.h264 /
                        recebido.h264)
  --encoder E           auto, nvenc ou mf (padrão auto: NVENC em NVIDIA,
                        senão Media Foundation)
  --mbps M              bitrate alvo do encode (padrão 20)
  --fps F               fps informado ao encoder (padrão 60)
  --keyframe-a-cada S   no encode, força um keyframe a cada S segundos para
                        medir o custo (padrão 5; 0 = só o primeiro)
  --verificar           no encode, decodifica cada frame com o decoder do
                        Windows, mede o PSNR e salva o último frame em BMP;
                        no receber, confere que todo frame decodifica
  --porta P             porta UDP do receber (padrão 9000)
  --destino IP:PORTA    destino do enviar (o PC que roda o receber)
  --sinal SERVIDOR      servidor de sinalização (fluxo-sinal): o receber se registra
                        nele e o enviar acha o receber por ele (redes diferentes)
  --internet            no receber: redes diferentes sem servidor; mostra um código de
                        conexão e pede no terminal o código de resposta do enviar
  --sem-upnp            no receber --internet: não abre a porta no roteador por UPnP
  --conexao C           no enviar: o código de conexão do receber --internet
  --ttl-furo N          no enviar --conexao: TTL dos pedidos até o furo do receber chegar
                        (padrão 12; 0 = TTL normal). Alto o bastante para passar o NAT
                        daqui, baixo o bastante para morrer antes do NAT de lá
  --fec K               no enviar: 1 paridade a cada K pacotes (padrão 10; 0 = sem)
  --codigo XXXX-XXXX    código de pareamento: o receber mostra um sorteado (ou usa
                        este); o enviar precisa do código do receber
  --sem-cifra           receber/enviar sem pareamento nem cifra (os dois lados)
  --refresh N           no encode, liga o intra-refresh em ondas de N frames (NVENC)
  --perder-a-cada S     no encode com --verificar, descarta um frame a cada S
                        segundos (o decoder não o recebe) e pede a recuperação:
                        uma onda de intra-refresh com --refresh, senão um
                        keyframe. Mede quanto o decoder leva para se recuperar

Dica: com a tela parada o Windows não gera frames. Durante bench/encode,
deixe um vídeo ou jogo rodando a 60 fps no monitor capturado.";

fn main() -> ExitCode {
    enable_dpi_awareness();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        println!("{AJUDA}");
        return ExitCode::SUCCESS;
    };
    let opts = match parse_opts(&args[1..]) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("erro: {e}\n\n{AJUDA}");
            return ExitCode::from(2);
        }
    };

    let resultado: Result<(), Box<dyn Error>> = match cmd.as_str() {
        "listar" => cmd_listar().map_err(Into::into),
        "bench" => cmd_bench(&opts).map_err(Into::into),
        "screenshot" => cmd_screenshot(&opts).map_err(Into::into),
        #[cfg(windows)]
        "encoders" => encode::cmd_encoders(&opts),
        #[cfg(windows)]
        "encode" => encode::cmd_encode(&opts),
        #[cfg(windows)]
        "decode" => encode::cmd_decode(&opts),
        "receber" => receber::cmd_receber(&opts),
        "enviar" => enviar::cmd_enviar(&opts),
        "nat" => nat::cmd_nat(&opts),
        "ajuda" | "--help" | "-h" => {
            println!("{AJUDA}");
            Ok(())
        }
        outro => {
            eprintln!("comando desconhecido: {outro}\n\n{AJUDA}");
            return ExitCode::from(2);
        }
    };

    match resultado {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_listar() -> Result<(), CaptureError> {
    let monitores = list_monitors()?;
    if monitores.is_empty() {
        println!("Nenhum monitor encontrado.");
    }
    for m in monitores {
        println!(
            "[{}] {}  {}x{} em ({}, {})  —  {}",
            m.index, m.device_name, m.width, m.height, m.x, m.y, m.adapter_name
        );
    }
    Ok(())
}

/// Estatísticas simples de uma série de durações.
struct Serie(Vec<Duration>);

impl Serie {
    fn media_ms(&self) -> f64 {
        if self.0.is_empty() {
            return 0.0;
        }
        self.0.iter().map(Duration::as_secs_f64).sum::<f64>() / self.0.len() as f64 * 1000.0
    }
    fn percentil_ms(&mut self, p: f64) -> f64 {
        if self.0.is_empty() {
            return 0.0;
        }
        self.0.sort_unstable();
        let i = ((self.0.len() - 1) as f64 * p).round() as usize;
        self.0[i].as_secs_f64() * 1000.0
    }
}

fn cmd_bench(o: &Opts) -> Result<(), CaptureError> {
    let mut cap = DesktopCapturer::new(o.monitor)?;
    let m = cap.monitor().clone();
    println!(
        "Capturando monitor {} ({}x{}, {}) por {} s{}...",
        m.index,
        m.width,
        m.height,
        m.adapter_name,
        o.segundos,
        if o.copiar_cpu { " com cópia para RAM" } else { "" }
    );

    let duracao = Duration::from_secs_f64(o.segundos);
    let inicio = Instant::now();
    let mut ultimo_frame: Option<Instant> = None;
    let mut intervalos = Serie(Vec::new());
    let mut copias = Serie(Vec::new());
    let mut buf = Vec::new();

    let (mut imagens, mut so_cursor, mut timeouts, mut acumulados, mut perdas_acesso) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut proximo_relatorio = Duration::from_secs(1);
    let mut imagens_no_segundo = 0u64;

    while inicio.elapsed() < duracao {
        let frame = match cap.next_frame(100) {
            Ok(f) => f,
            Err(CaptureError::AccessLost) => {
                perdas_acesso += 1;
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(e) => return Err(e),
        };
        let Some(mut frame) = frame else {
            timeouts += 1;
            continue;
        };

        let info = frame.info();
        if !info.desktop_updated {
            so_cursor += 1;
            continue;
        }

        let agora = Instant::now();
        if let Some(ant) = ultimo_frame {
            intervalos.0.push(agora - ant);
        }
        ultimo_frame = Some(agora);
        imagens += 1;
        imagens_no_segundo += 1;
        acumulados += info.accumulated_frames.saturating_sub(1) as u64;

        if o.copiar_cpu {
            let t = Instant::now();
            frame.copy_to_cpu(&mut buf)?;
            copias.0.push(t.elapsed());
        }
        drop(frame); // devolve o frame ao Windows o quanto antes

        if inicio.elapsed() >= proximo_relatorio {
            println!("  {:>3} s: {imagens_no_segundo} fps", proximo_relatorio.as_secs());
            imagens_no_segundo = 0;
            proximo_relatorio += Duration::from_secs(1);
        }
    }

    let total = inicio.elapsed().as_secs_f64();
    println!("\nResultado");
    println!(
        "  frames com imagem nova:   {imagens}  ({:.1} fps médio)",
        imagens as f64 / total
    );
    println!("  updates só de cursor:     {so_cursor}");
    println!("  esperas sem mudança:      {timeouts}  (blocos de 100 ms)");
    println!("  frames perdidos (Windows acumulou): {acumulados}");
    println!("  perdas de acesso:         {perdas_acesso}");
    println!(
        "  intervalo entre frames:   média {:.2} ms, p50 {:.2} ms, p99 {:.2} ms",
        intervalos.media_ms(),
        intervalos.percentil_ms(0.50),
        intervalos.percentil_ms(0.99)
    );
    if o.copiar_cpu {
        println!(
            "  cópia GPU→RAM:            média {:.2} ms, p99 {:.2} ms",
            copias.media_ms(),
            copias.percentil_ms(0.99)
        );
    }
    if imagens as f64 / total < 5.0 {
        println!(
            "\nPoucos frames? A tela provavelmente estava parada. Rode de novo com um vídeo ou jogo aberto."
        );
    }
    Ok(())
}

fn cmd_screenshot(o: &Opts) -> Result<(), CaptureError> {
    let mut cap = DesktopCapturer::new(o.monitor)?;
    // O primeiro frame após criar a duplicação sempre vem com a tela inteira.
    // Tentamos por até 2 s, ignorando updates só de cursor.
    let limite = Instant::now() + Duration::from_secs(2);
    let mut buf = Vec::new();
    let saida = o.saida.as_deref().unwrap_or("captura.bmp");
    while Instant::now() < limite {
        if let Some(mut frame) = cap.next_frame(200)? {
            if !frame.info().desktop_updated {
                continue;
            }
            let img = frame.copy_to_cpu(&mut buf)?;
            drop(frame);
            let arquivo = std::fs::File::create(saida)
                .and_then(|f| bmp::write_bmp32(std::io::BufWriter::new(f), img.width, img.height, &buf));
            return match arquivo {
                Ok(()) => {
                    println!("Salvo {saida} ({}x{})", img.width, img.height);
                    Ok(())
                }
                Err(e) => Err(CaptureError::NotAvailable(format!(
                    "falha ao gravar arquivo: {e}"
                ))),
            };
        }
    }
    Err(CaptureError::NotAvailable("nenhum frame recebido em 2 s".into()))
}
