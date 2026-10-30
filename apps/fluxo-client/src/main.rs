//! # fluxo-client
//!
//! Lado de quem assiste: rede → decode → tela.
//!
//! ```text
//! fluxo-client [--porta P] [--vsync] [--segundos S] [--largura L] [--altura A] [--no-topo]
//! ```
//!
//! Escuta numa porta UDP, decodifica com o `fluxo-decode` e mostra numa
//! janela (F11 ou Alt+Enter: tela cheia). As estatísticas aparecem no título
//! da janela, uma vez por segundo, e um resumo no terminal ao fechar.
//!
//! Detalhes e como ler os números: `docs/modulos/fluxo-client.md`.

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

Na janela: F11 ou Alt+Enter alterna tela cheia; Esc sai da tela cheia.
No PC que compartilha:  fluxo-host --destino <IP deste PC>:9000";

struct Opts {
    porta: u16,
    vsync: bool,
    segundos: f64,
    largura: u32,
    altura: u32,
    no_topo: bool,
}

fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        porta: 9000,
        vsync: false,
        segundos: 0.0,
        largura: 1280,
        altura: 720,
        no_topo: false,
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

/// Durações em ms, com percentis.
#[cfg(windows)]
#[derive(Default)]
struct Serie(Vec<f64>);

#[cfg(windows)]
impl Serie {
    fn push(&mut self, ms: f64) {
        self.0.push(ms);
    }
    fn media(&self) -> f64 {
        self.0.iter().sum::<f64>() / self.0.len().max(1) as f64
    }
    fn percentil(&mut self, p: f64) -> f64 {
        if self.0.is_empty() {
            return 0.0;
        }
        self.0.sort_by(f64::total_cmp);
        self.0[((self.0.len() - 1) as f64 * p).round() as usize]
    }
}

#[cfg(windows)]
fn run(o: &Opts) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, Instant};

    use fluxo_decode::{create_device, VideoDecoder};
    use fluxo_net::{clock_diff_us, clock_us, ReceiverConfig, VideoReceiver};

    use crate::present::Presenter;
    use crate::window::Window;

    let rx = VideoReceiver::bind(("0.0.0.0", o.porta), ReceiverConfig::default())?;
    let device = create_device()?;
    let mut decoder = VideoDecoder::new(&device)?;
    let mut janela = Window::new(
        &format!("Fluxo — aguardando o host na porta {}", o.porta),
        o.largura,
        o.altura,
        o.no_topo,
    )?;
    let mut tela = Presenter::new(&device, janela.hwnd(), janela.client_size(), o.vsync)?;
    println!(
        "Escutando em {} | decoder: {} | {}",
        rx.local_addr(),
        decoder.name(),
        if tela.tearing() {
            "sem vsync (tearing permitido)"
        } else if o.vsync {
            "com vsync"
        } else {
            "sem vsync (o sistema não permite tearing nesta janela)"
        }
    );

    let duracao = (o.segundos > 0.0).then(|| Duration::from_secs_f64(o.segundos));
    let mut inicio: Option<Instant> = None;
    let mut esperando_keyframe = true;
    // Quando a perda atual foi notada, para medir quanto a imagem ficou congelada.
    let mut perda_desde: Option<Instant> = None;
    let mut recuperacoes = Serie::default();
    let mut perdas = 0u64;
    let mut proximo_titulo = Instant::now() + Duration::from_secs(1);

    // Estatísticas: por segundo (título) e totais (resumo).
    let (mut frames_seg, mut bytes_seg) = (0u64, 0u64);
    let mut latencia_seg = Serie::default();
    let mut latencias = Serie::default();
    let mut decodes = Serie::default();
    let (mut recebidos, mut mostrados, mut pulados, mut erros_decode) = (0u64, 0u64, 0u64, 0u64);

    while !janela.closed() {
        if let (Some(t0), Some(d)) = (inicio, duracao) {
            if t0.elapsed() >= d {
                break;
            }
        }
        janela.pump();
        if janela.take_fullscreen_toggle() {
            janela.toggle_fullscreen();
        }
        if janela.take_resized() {
            tela.resize(janela.client_size())?;
        }

        // Sem imagem decodificável (largada ou perda): pede um keyframe. Pode
        // chamar a cada volta: o fluxo-net segura pedidos repetidos.
        if esperando_keyframe {
            rx.request_keyframe();
        }

        // Espera curta para a janela continuar responsiva sem frames.
        let Some(primeiro) = rx.recv_frame(Duration::from_millis(2))? else {
            if Instant::now() >= proximo_titulo {
                proximo_titulo += Duration::from_secs(1);
            }
            continue;
        };
        inicio.get_or_insert_with(Instant::now);

        // Decodifica tudo o que chegou (cada P-frame depende do anterior),
        // mas só mostra o mais novo: se o cliente atrasou, ele alcança.
        let mut chegou = Some(primeiro);
        let mut para_mostrar = None;
        while let Some(f) = chegou.take() {
            recebidos += 1;
            bytes_seg += f.frame.data.len() as u64;
            // Um frame se perdeu antes deste: os P-frames seguintes apontam
            // para uma imagem que o decoder não tem. Em vez de mostrar imagem
            // corrompida, congela a última boa e pede um keyframe.
            if f.frame.preceded_by_loss && !f.frame.keyframe && !esperando_keyframe {
                perdas += 1;
                esperando_keyframe = true;
                perda_desde = Some(Instant::now());
                rx.request_keyframe();
            }
            if esperando_keyframe && !f.frame.keyframe {
                pulados += 1;
            } else {
                esperando_keyframe = false;
                if let Some(t) = perda_desde.take() {
                    recuperacoes.push(t.elapsed().as_secs_f64() * 1000.0);
                }
                let t = Instant::now();
                match decoder.decode(&f.frame.data) {
                    Ok(Some(d)) => para_mostrar = Some((d, f.frame.timestamp_us)),
                    Ok(None) => {}
                    Err(e) => {
                        // Bitstream estragado: recomeça no próximo keyframe
                        // com um decoder limpo.
                        erros_decode += 1;
                        eprintln!("(decode falhou: {e}; pedindo keyframe)");
                        decoder = VideoDecoder::new(&device)?;
                        esperando_keyframe = true;
                        perda_desde = Some(Instant::now());
                        rx.request_keyframe();
                    }
                }
                decodes.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            chegou = rx.recv_frame(Duration::ZERO)?;
        }

        if let Some((d, capturado_us)) = para_mostrar {
            tela.present(&d)?;
            drop(d); // devolve a superfície ao decoder
            mostrados += 1;
            frames_seg += 1;
            // Captura no host → frame entregue ao monitor aqui. A captura vem
            // no relógio do host; somando a diferença entre os relógios
            // (medida pelos pings) ela vira o nosso relógio.
            if let Some(dif_ms) = rx.stats().clock_offset_ms {
                let ms = clock_diff_us(clock_us(), capturado_us) as f64 / 1000.0 + dif_ms;
                latencia_seg.push(ms);
                latencias.push(ms);
            }
        }

        if Instant::now() >= proximo_titulo {
            let s = rx.stats();
            let latencia = match latencia_seg.0.is_empty() {
                true => "sincronizando relógios".to_string(),
                false => format!(
                    "captura→tela {:.1} ms (máx {:.1})",
                    latencia_seg.media(),
                    latencia_seg.percentil(1.0)
                ),
            };
            janela.set_title(&format!(
                "Fluxo — {frames_seg} fps · {:.1} Mbps · {latencia} · RTT {:.1} ms · perdidos {}",
                bytes_seg as f64 * 8.0 / 1e6,
                s.rtt.smoothed_ms,
                s.reassembly.frames_lost,
            ));
            (frames_seg, bytes_seg) = (0, 0);
            latencia_seg = Serie::default();
            proximo_titulo = Instant::now() + Duration::from_secs(1);
        }
    }

    let s = rx.stats();
    let total = inicio.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    println!("\nResultado ({:.1} s)", total);
    println!(
        "  frames: {recebidos} recebidos, {mostrados} mostrados ({:.1} fps), {pulados} pulados esperando keyframe",
        mostrados as f64 / total.max(0.001)
    );
    println!(
        "  perdidos na rede: {} | reconstruídos pelo FEC: {} fragmentos | erros de decode: {erros_decode} | descartados na fila: {}",
        s.reassembly.frames_lost, s.reassembly.fec_recovered, s.frames_dropped
    );
    println!(
        "  recuperação:      {perdas} perdas, {} keyframes pedidos; imagem congelada média {:.1} ms, máx {:.1} ms",
        s.keyframe_requests,
        recuperacoes.media(),
        recuperacoes.percentil(1.0)
    );
    println!(
        "  decode (CPU):     média {:.2} ms, p99 {:.2} ms",
        decodes.media(),
        decodes.percentil(0.99)
    );
    println!(
        "  captura → tela:   p50 {:.1} ms, p99 {:.1} ms, máx {:.1} ms",
        latencias.percentil(0.5),
        latencias.percentil(0.99),
        latencias.percentil(1.0)
    );
    if let (Some(dif), Some(erro)) = (s.clock_offset_ms, s.clock_error_ms) {
        println!("  relógios:         host {dif:+.1} ms em relação a este PC (sincronizados pelos pings, erro ±{erro:.1} ms)");
    }
    println!(
        "  RTT:              {:.2} ms suavizado, {:.2} ms mínimo",
        s.rtt.smoothed_ms, s.rtt.min_ms
    );
    Ok(())
}
