//! # fluxo-host
//!
//! Lado de quem compartilha a tela: captura → encode → rede.
//!
//! ```text
//! fluxo-host --destino IP:PORTA [--monitor N] [--encoder auto|nvenc|mf]
//!            [--mbps M] [--fps F] [--pacing-mbps P] [--keyframe-a-cada S]
//!            [--segundos S] [--porta-local P]
//! ```
//!
//! Por enquanto o destino é informado à mão (o cliente escuta numa porta;
//! descoberta e NAT ficam para a fase 5), e keyframes saem a cada poucos
//! segundos para que um cliente que entrar depois consiga começar (o pedido
//! de keyframe pelo cliente vem na fase 4).
//!
//! Detalhes e como ler a saída: `docs/modulos/fluxo-host.md`.

use std::process::ExitCode;

const AJUDA: &str = "\
fluxo-host — transmite a tela para um cliente Fluxo

Uso:
  fluxo-host --destino IP:PORTA [opções]

Opções:
  --destino IP:PORTA    para onde enviar (obrigatório), ex.: 192.168.0.20:9000
  --monitor N           monitor a transmitir (padrão 0)
  --encoder E           auto, nvenc ou mf (padrão auto)
  --mbps M              bitrate do vídeo (padrão 20)
  --fps F               fps informado ao encoder (padrão 60)
  --pacing-mbps P       taxa de envio dos pacotes (padrão 2,5 × mbps; 0 = sem pacing)
  --keyframe-a-cada S   keyframe periódico em segundos (padrão 2; 0 = só o primeiro)
  --segundos S          encerra depois de S segundos (padrão 0 = até Ctrl+C)
  --porta-local P       porta UDP de saída (padrão: automática)

Para testar sem cliente, rode antes em outro terminal (ou outro PC):
  fluxo-bench receber --porta 9000";

#[derive(Debug, Clone)]
struct Opts {
    destino: String,
    monitor: usize,
    encoder: fluxo_encode::BackendChoice,
    mbps: f64,
    fps: u32,
    pacing_mbps: Option<f64>,
    keyframe_a_cada: f64,
    segundos: f64,
    porta_local: u16,
}

fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        destino: String::new(),
        monitor: 0,
        encoder: Default::default(),
        mbps: 20.0,
        fps: 60,
        pacing_mbps: None,
        keyframe_a_cada: 2.0,
        segundos: 0.0,
        porta_local: 0,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut valor = || it.next().ok_or(format!("{a} precisa de um valor"));
        match a.as_str() {
            "--destino" => o.destino = valor()?.clone(),
            "--monitor" => o.monitor = valor()?.parse().map_err(|_| "monitor inválido")?,
            "--encoder" => o.encoder = valor()?.parse()?,
            "--mbps" => o.mbps = valor()?.parse().map_err(|_| "mbps inválido")?,
            "--fps" => o.fps = valor()?.parse().map_err(|_| "fps inválido")?,
            "--pacing-mbps" => o.pacing_mbps = Some(valor()?.parse().map_err(|_| "pacing-mbps inválido")?),
            "--keyframe-a-cada" => {
                o.keyframe_a_cada = valor()?.parse().map_err(|_| "keyframe-a-cada inválido")?
            }
            "--segundos" => o.segundos = valor()?.parse().map_err(|_| "segundos inválido")?,
            "--porta-local" => o.porta_local = valor()?.parse().map_err(|_| "porta-local inválida")?,
            "--help" | "-h" | "ajuda" => return Err(String::new()),
            outro => return Err(format!("opção desconhecida: {outro}")),
        }
    }
    if o.destino.is_empty() {
        return Err("falta --destino IP:PORTA".into());
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
    Err("o fluxo-host só roda no Windows".into())
}

#[cfg(windows)]
fn run(o: &Opts) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, Instant};

    use fluxo_capture::{enable_dpi_awareness, CaptureError, DesktopCapturer};
    use fluxo_encode::{open_encoder, EncodeError, EncoderConfig};
    use fluxo_net::{clock_us, SenderConfig, VideoSender};

    enable_dpi_awareness();
    let mut cap = DesktopCapturer::new(o.monitor)?;
    let bitrate_bps = (o.mbps * 1e6).round() as u64;
    let config_para = |w: u32, h: u32| EncoderConfig {
        fps: o.fps,
        bitrate_kbps: (bitrate_bps / 1000) as u32,
        backend: o.encoder,
        ..EncoderConfig::new(w, h)
    };
    let m = cap.monitor().clone();
    let aberto = open_encoder(cap.d3d_device(), &config_para(m.width, m.height))?;
    for (b, motivo) in &aberto.skipped {
        println!("(pulando {b}: {motivo})");
    }
    let mut enc = aberto.encoder;

    let net_cfg = SenderConfig {
        pacing_bps: o
            .pacing_mbps
            .map(|p| (p * 1e6).round() as u64)
            .unwrap_or(SenderConfig::for_bitrate(bitrate_bps).pacing_bps),
        ..SenderConfig::for_bitrate(bitrate_bps)
    };
    let pacing_mbps = net_cfg.pacing_bps as f64 / 1e6;
    let tx = VideoSender::connect(("0.0.0.0", o.porta_local), o.destino.as_str(), net_cfg)?;

    println!(
        "Transmitindo monitor {} ({}x{}) de {} para {}\n  {}\n  {} Mbps a {} fps, pacing {:.0} Mbps, keyframe a cada {} s",
        m.index,
        m.width,
        m.height,
        tx.local_addr(),
        tx.peer_addr(),
        enc.name(),
        o.mbps,
        o.fps,
        pacing_mbps,
        o.keyframe_a_cada
    );
    if o.segundos <= 0.0 {
        println!("  Ctrl+C para encerrar.");
    }

    let duracao = (o.segundos > 0.0).then(|| Duration::from_secs_f64(o.segundos));
    let keyframe_a_cada = (o.keyframe_a_cada > 0.0).then(|| Duration::from_secs_f64(o.keyframe_a_cada));
    let inicio = Instant::now();
    let mut ultimo_keyframe = inicio;
    let mut proximo_relatorio = Duration::from_secs(1);
    let (mut frames_seg, mut bytes_seg, mut encode_seg) = (0u64, 0u64, Duration::ZERO);
    let (mut frames_total, mut bytes_total) = (0u64, 0u64);

    while !duracao.is_some_and(|d| inicio.elapsed() >= d) {
        let frame = match cap.next_frame(100) {
            Ok(Some(f)) => Some(f),
            Ok(None) => None,
            Err(CaptureError::AccessLost) => {
                std::thread::sleep(Duration::from_millis(50));
                None
            }
            Err(e) => return Err(e.into()),
        };

        let mut recriar = None;
        if let Some(frame) = frame.filter(|f| f.info().desktop_updated) {
            // Momento da captura, no relógio que o cliente usa para medir o trânsito.
            let capturado_us = clock_us();
            let pedir_keyframe = keyframe_a_cada.is_some_and(|p| ultimo_keyframe.elapsed() >= p);
            let t = Instant::now();
            let pacotes = match enc.encode(frame.texture(), pedir_keyframe) {
                Ok(p) => p,
                Err(EncodeError::SizeMismatch { got, .. }) => {
                    // A resolução mudou: o encoder é recriado logo abaixo,
                    // depois de soltar o frame (ele prende o capturador).
                    recriar = Some(got);
                    Vec::new()
                }
                Err(e) => return Err(e.into()),
            };
            encode_seg += t.elapsed();
            drop(frame); // devolve o frame ao Windows o quanto antes

            for p in pacotes {
                if p.keyframe {
                    ultimo_keyframe = Instant::now();
                }
                frames_seg += 1;
                bytes_seg += p.data.len() as u64;
                tx.send_frame(p.data, p.keyframe, capturado_us)?;
            }
        }
        if let Some((w, h)) = recriar {
            println!("(resolução mudou para {w}x{h}; recriando o encoder, que começa com keyframe)");
            enc = open_encoder(cap.d3d_device(), &config_para(w, h))?.encoder;
        }

        if inicio.elapsed() >= proximo_relatorio {
            let s = tx.stats();
            println!(
                "  {:>4} s: {frames_seg} fps, {:.1} Mbps, encode {:.1} ms, envio {:.1} ms (fila {}), RTT {}",
                proximo_relatorio.as_secs(),
                bytes_seg as f64 * 8.0 / 1e6,
                encode_seg.as_secs_f64() * 1000.0 / frames_seg.max(1) as f64,
                s.last_frame_send_ms,
                s.queue_len,
                match s.rtt.samples {
                    0 => "— (cliente não respondeu)".to_string(),
                    _ => format!("{:.2} ms", s.rtt.smoothed_ms),
                }
            );
            frames_total += frames_seg;
            bytes_total += bytes_seg;
            (frames_seg, bytes_seg, encode_seg) = (0, 0, Duration::ZERO);
            proximo_relatorio += Duration::from_secs(1);
        }
    }

    frames_total += frames_seg;
    bytes_total += bytes_seg;
    drop(enc);
    let s = tx.stats();
    drop(tx); // espera a fila de envio esvaziar
    let total = inicio.elapsed().as_secs_f64();
    println!("\nResultado");
    println!(
        "  frames: {frames_total} ({:.1} fps médio), {:.2} Mbps médio",
        frames_total as f64 / total,
        bytes_total as f64 * 8.0 / total / 1e6
    );
    println!(
        "  rede: {} pacotes, {} erros de envio, maior tempo de envio de um frame {:.1} ms",
        s.packets_sent, s.send_errors, s.max_frame_send_ms
    );
    println!(
        "  RTT: {:.2} ms suavizado, {:.2} ms mínimo ({} medições)",
        s.rtt.smoothed_ms, s.rtt.min_ms, s.rtt.samples
    );
    Ok(())
}
