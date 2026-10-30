//! # fluxo-bench
//!
//! Ferramenta de linha de comando para validar a captura de tela antes de
//! existir o resto do pipeline.
//!
//! ```text
//! fluxo-bench listar
//! fluxo-bench bench      [--monitor N] [--segundos S] [--copiar-cpu]
//! fluxo-bench screenshot [--monitor N] [--saida arquivo.bmp]
//! ```
//!
//! Detalhes e como interpretar os números: `docs/modulos/fluxo-bench.md`.

mod bmp;

use std::process::ExitCode;
use std::time::{Duration, Instant};

use fluxo_capture::{enable_dpi_awareness, list_monitors, CaptureError, DesktopCapturer};

/// Opções aceitas pelos comandos.
struct Opts {
    monitor: usize,
    segundos: f64,
    copiar_cpu: bool,
    saida: String,
}

fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        monitor: 0,
        segundos: 10.0,
        copiar_cpu: false,
        saida: "captura.bmp".into(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut valor = || it.next().ok_or(format!("{a} precisa de um valor"));
        match a.as_str() {
            "--monitor" => o.monitor = valor()?.parse().map_err(|_| "monitor inválido")?,
            "--segundos" => o.segundos = valor()?.parse().map_err(|_| "segundos inválido")?,
            "--saida" => o.saida = valor()?.clone(),
            "--copiar-cpu" => o.copiar_cpu = true,
            outro => return Err(format!("opção desconhecida: {outro}")),
        }
    }
    Ok(o)
}

const AJUDA: &str = "\
fluxo-bench — testes de captura do Fluxo

Comandos:
  listar                      lista os monitores disponíveis
  bench      [opções]         mede quantos frames por segundo a captura entrega
  screenshot [opções]         salva um frame como imagem BMP

Opções:
  --monitor N     índice do monitor (padrão 0; veja 'listar')
  --segundos S    duração do bench (padrão 10)
  --copiar-cpu    no bench, também copia cada frame para a RAM e mede o custo
  --saida ARQ     arquivo do screenshot (padrão captura.bmp)

Dica: com a tela parada o Windows não gera frames. Durante o bench,
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

    let resultado = match cmd.as_str() {
        "listar" => cmd_listar(),
        "bench" => cmd_bench(&opts),
        "screenshot" => cmd_screenshot(&opts),
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
    while Instant::now() < limite {
        if let Some(mut frame) = cap.next_frame(200)? {
            if !frame.info().desktop_updated {
                continue;
            }
            let img = frame.copy_to_cpu(&mut buf)?;
            drop(frame);
            let arquivo = std::fs::File::create(&o.saida)
                .and_then(|f| bmp::write_bmp32(std::io::BufWriter::new(f), img.width, img.height, &buf));
            return match arquivo {
                Ok(()) => {
                    println!("Salvo {} ({}x{})", o.saida, img.width, img.height);
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
