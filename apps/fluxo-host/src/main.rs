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

mod limite;

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
  --fps F               máximo de frames por segundo (padrão 60); capturas mais
                        rápidas esperam o próximo horário, sem perder a última
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

/// Contadores do relatório por segundo.
#[cfg(windows)]
#[derive(Default)]
struct Contadores {
    frames: u64,
    bytes: u64,
    encode: std::time::Duration,
    /// Capturas que chegaram cedo demais para o limite de fps.
    adiadas: u64,
    /// Saiu um keyframe desde a última vez que o laço olhou.
    keyframe: bool,
}

/// Codifica `textura` e enfileira o resultado na rede. Devolve o tamanho
/// novo se a resolução mudou (o encoder precisa ser recriado).
#[cfg(windows)]
fn codificar(
    enc: &mut dyn fluxo_encode::Encoder,
    tx: &fluxo_net::VideoSender,
    textura: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    capturado_us: u32,
    pedir_keyframe: bool,
    c: &mut Contadores,
) -> Result<Option<(u32, u32)>, Box<dyn std::error::Error>> {
    use fluxo_encode::EncodeError;

    let t = std::time::Instant::now();
    let pacotes = match enc.encode(textura, pedir_keyframe) {
        Ok(p) => p,
        Err(EncodeError::SizeMismatch { got, .. }) => return Ok(Some(got)),
        Err(e) => return Err(e.into()),
    };
    c.encode += t.elapsed();
    for p in pacotes {
        c.keyframe |= p.keyframe;
        c.frames += 1;
        c.bytes += p.data.len() as u64;
        tx.send_frame(p.data, p.keyframe, capturado_us)?;
    }
    Ok(None)
}

/// Copia `origem` (textura da captura, que só vale enquanto o frame existir)
/// para `destino`, recriando-a se o tamanho mudou. Cópia na GPU: microssegundos.
#[cfg(windows)]
fn guardar_copia(
    device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
    context: &windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    origem: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    destino: &mut Option<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>,
) -> windows::core::Result<()> {
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    };

    let mut d = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: GetDesc só escreve em `d`; CreateTexture2D com descritor
    // derivado da origem; CopyResource entre texturas de mesmo tamanho e formato.
    unsafe {
        origem.GetDesc(&mut d);
        let serve = destino.as_ref().is_some_and(|t| {
            let mut a = D3D11_TEXTURE2D_DESC::default();
            t.GetDesc(&mut a);
            (a.Width, a.Height, a.Format) == (d.Width, d.Height, d.Format)
        });
        if !serve {
            let desc = D3D11_TEXTURE2D_DESC {
                MipLevels: 1,
                ArraySize: 1,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
                ..d
            };
            let mut nova = None;
            device.CreateTexture2D(&desc, None, Some(&mut nova))?;
            *destino = nova;
        }
        if let Some(t) = destino.as_ref() {
            context.CopyResource(t, origem);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn run(o: &Opts) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, Instant};

    use fluxo_capture::{enable_dpi_awareness, CaptureError, DesktopCapturer};
    use fluxo_encode::{open_encoder, EncoderConfig};
    use fluxo_net::{clock_us, SenderConfig, VideoSender};

    use crate::limite::FpsLimiter;

    enable_dpi_awareness();
    let mut cap = DesktopCapturer::new(o.monitor)?;
    // Cópias: o frame capturado prende o capturador enquanto existe.
    let device = cap.d3d_device().clone();
    let context = cap.d3d_context().clone();
    let bitrate_bps = (o.mbps * 1e6).round() as u64;
    let config_para = |w: u32, h: u32| EncoderConfig {
        fps: o.fps,
        bitrate_kbps: (bitrate_bps / 1000) as u32,
        backend: o.encoder,
        ..EncoderConfig::new(w, h)
    };
    let m = cap.monitor().clone();
    let aberto = open_encoder(&device, &config_para(m.width, m.height))?;
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
        "Transmitindo monitor {} ({}x{}) de {} para {}\n  {}\n  {} Mbps, até {} fps, pacing {:.0} Mbps, keyframe a cada {} s",
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
    let mut limite = FpsLimiter::new(o.fps);
    // Frame que chegou cedo demais: cópia na GPU + momento da captura.
    let mut pendente: Option<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D> = None;
    let mut pendente_us = 0u32;
    let mut seg = Contadores::default();
    let mut total = Contadores::default();

    while !duracao.is_some_and(|d| inicio.elapsed() >= d) {
        // Com um pendente, espera no máximo até o horário dele.
        let espera_ms = match limite.deadline() {
            Some(d) => d
                .saturating_duration_since(Instant::now())
                .as_micros()
                .div_ceil(1000) as u32,
            None => 100,
        };
        let frame = match cap.next_frame(espera_ms) {
            Ok(f) => f,
            Err(CaptureError::AccessLost) => {
                std::thread::sleep(Duration::from_millis(50));
                None
            }
            Err(e) => return Err(e.into()),
        };

        let mut pedir_keyframe = keyframe_a_cada.is_some_and(|p| ultimo_keyframe.elapsed() >= p);
        let mut recriar = None;
        if let Some(frame) = frame.filter(|f| f.info().desktop_updated) {
            // Momento da captura, no relógio que o cliente usa para medir o trânsito.
            let capturado_us = clock_us();
            if limite.on_frame(Instant::now()) {
                recriar = codificar(
                    &mut *enc,
                    &tx,
                    frame.texture(),
                    capturado_us,
                    pedir_keyframe,
                    &mut seg,
                )?;
                pedir_keyframe = false;
            } else {
                guardar_copia(&device, &context, frame.texture(), &mut pendente)?;
                pendente_us = capturado_us;
                seg.adiadas += 1;
            }
            drop(frame); // devolve o frame ao Windows o quanto antes
        }
        if recriar.is_none() && limite.on_tick(Instant::now()) {
            if let Some(t) = &pendente {
                recriar = codificar(&mut *enc, &tx, t, pendente_us, pedir_keyframe, &mut seg)?;
            }
        }
        if std::mem::take(&mut seg.keyframe) {
            ultimo_keyframe = Instant::now();
        }
        if let Some((w, h)) = recriar {
            println!("(resolução mudou para {w}x{h}; recriando o encoder, que começa com keyframe)");
            enc = open_encoder(&device, &config_para(w, h))?.encoder;
            pendente = None;
            limite = FpsLimiter::new(o.fps);
        }

        if inicio.elapsed() >= proximo_relatorio {
            let s = tx.stats();
            println!(
                "  {:>4} s: {} fps ({} capturas adiadas), {:.1} Mbps, encode {:.1} ms, envio {:.1} ms (fila {}), RTT {}",
                proximo_relatorio.as_secs(),
                seg.frames,
                seg.adiadas,
                seg.bytes as f64 * 8.0 / 1e6,
                seg.encode.as_secs_f64() * 1000.0 / seg.frames.max(1) as f64,
                s.last_frame_send_ms,
                s.queue_len,
                match s.rtt.samples {
                    0 => "— (cliente não respondeu)".to_string(),
                    _ => format!("{:.2} ms", s.rtt.smoothed_ms),
                }
            );
            total.frames += seg.frames;
            total.bytes += seg.bytes;
            total.adiadas += seg.adiadas;
            seg = Contadores::default();
            proximo_relatorio += Duration::from_secs(1);
        }
    }

    total.frames += seg.frames;
    total.bytes += seg.bytes;
    total.adiadas += seg.adiadas;
    drop(enc);
    let s = tx.stats();
    drop(tx); // espera a fila de envio esvaziar
    let t = inicio.elapsed().as_secs_f64();
    println!("\nResultado");
    println!(
        "  frames: {} ({:.1} fps médio, limite {}), {:.2} Mbps médio; {} capturas adiadas pelo limite",
        total.frames,
        total.frames as f64 / t,
        o.fps,
        total.bytes as f64 * 8.0 / t / 1e6,
        total.adiadas
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
