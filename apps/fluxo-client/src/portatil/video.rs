//! Thread do vídeo: rede → OpenH264 → frame mais novo para a janela.

use std::error::Error;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use eframe::egui;
use fluxo_net::{FrameAction, Recovery, RecoveryRequest, VideoCodec, VideoReceiver};
use openh264::decoder::{DecodedYUV, Decoder};
use openh264::formats::YUVSource;

use super::{Compartilhado, Quadro};

/// Durações em ms, com percentis.
#[derive(Default)]
struct Serie(Vec<f64>);

impl Serie {
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

/// Copia o frame do decoder: ele só vale até o próximo `decode`.
fn copiar(d: &DecodedYUV<'_>, capturado_us: u32, dif_relogio_ms: Option<f64>) -> Quadro {
    let (largura, altura) = d.dimensions();
    let (py, pu, _) = d.strides();
    Quadro {
        largura: largura as u32,
        altura: altura as u32,
        y: d.y().to_vec(),
        u: d.u().to_vec(),
        v: d.v().to_vec(),
        passo: (py, pu),
        capturado_us,
        dif_relogio_ms,
    }
}

fn erro_decode(e: openh264::Error) -> Box<dyn Error> {
    format!("OpenH264: {e}").into()
}

/// Laço do vídeo, até a janela fechar ou acabar `--segundos`. Imprime o resumo.
pub fn laco(
    rx: VideoReceiver,
    segundos: f64,
    ctx: egui::Context,
    c: &Compartilhado,
) -> Result<(), Box<dyn Error>> {
    let mut decoder = Decoder::new().map_err(erro_decode)?;
    println!("Escutando em {} | decoder: OpenH264, na CPU", rx.local_addr());

    let duracao = (segundos > 0.0).then(|| Duration::from_secs_f64(segundos));
    let mut inicio: Option<Instant> = None;
    let mut recuperacao = Recovery::new(Instant::now());
    let mut anunciado = false;
    let mut outro_codec = false;
    let mut versao_cursor: Option<u64> = None;
    let mut proximo_titulo = Instant::now() + Duration::from_secs(1);
    let (mut frames_seg, mut bytes_seg) = (0u64, 0u64);
    let (mut recebidos, mut mostrados, mut pulados, mut erros) = (0u64, 0u64, 0u64, 0u64);
    let mut decodes = Serie::default();
    let mut latencias = Serie::default();

    while !c.fechar.load(Ordering::Relaxed) {
        if inicio.zip(duracao).is_some_and(|(t, d)| t.elapsed() >= d) {
            break;
        }
        match recuperacao.request(Instant::now()) {
            RecoveryRequest::Keyframe => {
                rx.request_keyframe();
            }
            RecoveryRequest::Refresh => {
                rx.request_refresh();
            }
            RecoveryRequest::None => {}
        }
        let cursor = rx.cursor();
        if cursor.as_ref().map(|c| c.version) != versao_cursor {
            versao_cursor = cursor.as_ref().map(|c| c.version);
            *c.cursor.lock().unwrap_or_else(|e| e.into_inner()) = cursor;
            ctx.request_repaint();
        }

        let Some(primeiro) = rx.recv_frame(Duration::from_millis(5))? else {
            continue;
        };
        inicio.get_or_insert_with(Instant::now);

        // Decodifica tudo o que chegou (cada P-frame depende do anterior),
        // mas só copia e mostra o mais novo.
        // Com limite de tempo: numa CPU que decodifica mais devagar do que os
        // frames chegam, a fila nunca esvazia, e sem ele nada aparecia na tela.
        let lote = Instant::now();
        let mut chegou = Some(primeiro);
        let mut para_mostrar = None;
        while let Some(f) = chegou.take() {
            recebidos += 1;
            bytes_seg += f.frame.data.len() as u64;
            let codec = f.frame.codec.unwrap_or_default();
            if codec != VideoCodec::H264 {
                // Só acontece se o host forçou o codec com --codec.
                if !outro_codec {
                    eprintln!(
                        "(o host mandou {}; este cliente só decodifica H.264)",
                        codec.name()
                    );
                    outro_codec = true;
                }
                chegou = rx.recv_frame(Duration::ZERO)?;
                continue;
            }
            if !anunciado {
                println!("  vídeo em H.264 | decoder: OpenH264, na CPU");
                anunciado = true;
            }
            match recuperacao.on_frame(&f.frame, Instant::now()) {
                FrameAction::Skip => pulados += 1,
                FrameAction::Decode { show } => {
                    let t = Instant::now();
                    match decoder.decode(&f.frame.data) {
                        Ok(Some(d)) if show => {
                            let dif = rx.stats().clock_offset_ms;
                            para_mostrar = Some(copiar(&d, f.frame.timestamp_us, dif));
                        }
                        Ok(_) => {}
                        Err(e) => {
                            erros += 1;
                            eprintln!("(decode falhou: {e}; pedindo keyframe)");
                            decoder = Decoder::new().map_err(erro_decode)?;
                            recuperacao.on_decode_error(Instant::now());
                        }
                    }
                    decodes.0.push(t.elapsed().as_secs_f64() * 1000.0);
                }
            }
            if lote.elapsed() < Duration::from_millis(33) {
                chegou = rx.recv_frame(Duration::ZERO)?;
            }
        }
        if let Some(q) = para_mostrar {
            *c.quadro.lock().unwrap_or_else(|e| e.into_inner()) = Some(q);
            ctx.request_repaint();
            mostrados += 1;
            frames_seg += 1;
        }

        if Instant::now() >= proximo_titulo {
            let novas = std::mem::take(&mut *c.latencias.lock().unwrap_or_else(|e| e.into_inner()));
            let mut seg = Serie(novas);
            let latencia = match seg.0.is_empty() {
                true => "sincronizando relógios".to_string(),
                false => format!(
                    "captura→tela {:.1} ms (máx {:.1})",
                    seg.media(),
                    seg.percentil(1.0)
                ),
            };
            latencias.0.append(&mut seg.0);
            let s = rx.stats();
            let mbps = bytes_seg as f64 * 8.0 / 1e6;
            *c.titulo.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!(
                "FSS — {frames_seg} fps · {mbps:.1} Mbps · {latencia} · RTT {:.1} ms · perdidos {}{}",
                s.rtt.smoothed_ms,
                s.reassembly.frames_lost,
                if s.encrypted { " · cifrado" } else { "" },
            ));
            *c.painel.lock().unwrap_or_else(|e| e.into_inner()) = format!(
                "{frames_seg} fps · {mbps:.1} Mbps\n{latencia}\nRTT {:.1} ms · fila {:.0} ms\nperdidos {} · FEC {}\n{}F2 esconde",
                s.rtt.smoothed_ms,
                s.queue_delay_ms,
                s.reassembly.frames_lost,
                s.reassembly.fec_recovered,
                if s.encrypted { "cifrado · " } else { "SEM CIFRA · " },
            );
            ctx.request_repaint();
            (frames_seg, bytes_seg) = (0, 0);
            proximo_titulo = Instant::now() + Duration::from_secs(1);
        }
    }

    latencias.0.append(&mut std::mem::take(
        &mut *c.latencias.lock().unwrap_or_else(|e| e.into_inner()),
    ));
    let s = rx.stats();
    let total = inicio.map_or(0.0, |t| t.elapsed().as_secs_f64());
    println!("\nResultado ({total:.1} s)");
    println!(
        "  frames: {recebidos} recebidos, {mostrados} mostrados ({:.1} fps), {pulados} pulados esperando keyframe",
        mostrados as f64 / total.max(0.001)
    );
    println!(
        "  perdidos na rede: {} | reconstruídos pelo FEC: {} fragmentos | erros de decode: {erros} | descartados na fila: {}",
        s.reassembly.frames_lost, s.reassembly.fec_recovered, s.frames_dropped
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
    println!(
        "  RTT:              {:.2} ms suavizado, {:.2} ms mínimo",
        s.rtt.smoothed_ms, s.rtt.min_ms
    );
    // Fim pelo --segundos: a janela também fecha.
    c.fechar.store(true, Ordering::Relaxed);
    ctx.request_repaint();
    Ok(())
}
