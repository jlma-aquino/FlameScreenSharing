//! Thread do vídeo: rede → OpenH264 → frame mais novo para a janela.

use std::error::Error;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use eframe::egui;
use fluxo_net::{clock_diff_us, clock_us, FrameAction, Recovery, RecoveryRequest, VideoCodec, VideoReceiver};
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

/// Vídeo atrasado além disto: descarta a fila e recomeça num keyframe. Melhor
/// um salto do que assistir com segundos de atraso.
const ATRASO_MAXIMO_MS: f64 = 300.0;
/// Intervalo mínimo entre descartes: o keyframe também custa para decodificar.
const ENTRE_DESCARTES: Duration = Duration::from_secs(2);

/// Fração do intervalo entre frames gasta no decode, com média móvel.
#[derive(Default)]
struct Carga {
    decode_ms: f64,
    intervalo_ms: f64,
    ultimo_us: Option<u32>,
}

impl Carga {
    /// Mais um frame: `capturado_us` no relógio do host, e o decode dele.
    fn frame(&mut self, capturado_us: u32, decode_ms: f64) {
        const PESO: f64 = 0.1;
        if let Some(antes) = self.ultimo_us {
            let dif = fluxo_net::clock_diff_us(capturado_us, antes) as f64 / 1000.0;
            // Tela parada: um intervalo enorme não diz nada sobre a capacidade.
            let dif = dif.clamp(0.0, 100.0);
            self.intervalo_ms += (dif - self.intervalo_ms) * PESO;
        }
        self.decode_ms += (decode_ms - self.decode_ms) * PESO;
        self.ultimo_us = Some(capturado_us);
    }

    /// O atraso a informar ao host, em ms. O controlador dele (limite padrão de
    /// 15 ms) só sobe abaixo de 6: 8 ms segura o bitrate onde está, e 20 ms o
    /// faz cair. Assim ele para antes de a CPU engasgar, em vez de subir até
    /// travar e cair de novo.
    fn atraso_ms(&self) -> f64 {
        if self.intervalo_ms <= 0.0 {
            return 0.0;
        }
        match self.decode_ms / self.intervalo_ms {
            c if c >= 0.95 => 20.0,
            c if c >= 0.75 => 8.0,
            _ => 0.0,
        }
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
    let mut carga = Carga::default();
    // Atraso da fila aqui dentro, em média móvel.
    let mut atraso_fila_ms = 0.0f64;
    let mut ultimo_descarte: Option<Instant> = None;
    let (mut descartes, mut frames_descartados) = (0u64, 0u64);

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

        // Tela parada do outro lado não manda frame: o título segue
        // atualizando mesmo assim (abaixo).
        if let Some(primeiro) = rx.recv_frame(Duration::from_millis(5))? {
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
                // Esperando desde que chegou pela rede: passou do limite, alcança.
                let espera_ms = clock_diff_us(clock_us(), f.received_us) as f64 / 1000.0;
                atraso_fila_ms += (espera_ms - atraso_fila_ms) * 0.1;
                let pode_descartar = ultimo_descarte.is_none_or(|t| t.elapsed() >= ENTRE_DESCARTES);
                if espera_ms > ATRASO_MAXIMO_MS && pode_descartar {
                    while rx.recv_frame(Duration::ZERO)?.is_some() {
                        frames_descartados += 1;
                    }
                    frames_descartados += 1;
                    descartes += 1;
                    ultimo_descarte = Some(Instant::now());
                    atraso_fila_ms = 0.0;
                    decoder = Decoder::new().map_err(erro_decode)?;
                    recuperacao.on_decode_error(Instant::now());
                    para_mostrar = None;
                    continue;
                }
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
                        let decode_ms = t.elapsed().as_secs_f64() * 1000.0;
                        decodes.0.push(decode_ms);
                        carga.frame(f.frame.timestamp_us, decode_ms);
                        // A fila já formada pesa mais do que a previsão.
                        let atraso = carga.atraso_ms().max(atraso_fila_ms);
                        rx.set_processing_delay(Duration::from_secs_f64(atraso / 1000.0));
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
        }

        // Antes do primeiro frame fica o texto de espera (código, sala).
        if inicio.is_some() && Instant::now() >= proximo_titulo {
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
    println!("  decode atrasado:  {descartes} descartes de fila, {frames_descartados} frames");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn com_decode(ms: f64) -> Carga {
        let mut c = Carga::default();
        for i in 0..200u32 {
            c.frame(i * 33_333, ms);
        }
        c
    }

    #[test]
    fn carga_decide_entre_livre_segurar_e_baixar() {
        assert_eq!(com_decode(10.0).atraso_ms(), 0.0, "folgado");
        assert_eq!(com_decode(28.0).atraso_ms(), 8.0, "segura");
        assert_eq!(com_decode(40.0).atraso_ms(), 20.0, "baixa");
    }

    #[test]
    fn os_valores_batem_com_o_controlador_do_host() {
        let agora = Instant::now();
        let relatorio = |ms| fluxo_net::NetworkReport {
            loss: Some(0.0),
            queue_delay_ms: ms,
            sent_kbps: None,
        };
        let novo = || {
            let mut c = fluxo_net::BitrateController::new(1000, 20_000, agora);
            c.on_report(&relatorio(100.0), agora); // sai do máximo
            c
        };
        let depois = agora + Duration::from_secs(10);
        let segura = novo().on_report(&relatorio(8.0), depois);
        assert_eq!(segura, None, "8 ms não sobe nem desce");
        let baixa = novo().on_report(&relatorio(20.0), depois);
        assert_eq!(baixa.map(|(_, m)| m), Some(fluxo_net::BitrateChange::DownDelay));
        assert_eq!(
            novo().on_report(&relatorio(0.0), depois).map(|(_, m)| m),
            Some(fluxo_net::BitrateChange::Up)
        );
    }

    #[test]
    fn tela_parada_nao_vira_folga_falsa() {
        let mut c = com_decode(40.0);
        // Um intervalo longo só pesa até 100 ms.
        c.frame(33_333 * 200 + 60_000_000, 40.0);
        assert!(c.intervalo_ms < 40.0);
    }
}
