//! Comando `enviar`: manda frames sintéticos ao `fluxo-bench receber`, sem
//! captura nem encode, a um bitrate fixo.
//!
//! Serve para medir a rede com carga controlada: com a tela parada o host
//! real quase não gera tráfego. Usa o mesmo `VideoSender` do host (pacing a
//! 2,5× o bitrate, FEC), então as perdas e rajadas medidas valem para ele.
//! Todo frame vai marcado como keyframe: o receptor nunca pula frames
//! esperando um, e cada frame perdido conta uma vez só.

use std::error::Error;
use std::time::{Duration, Instant};

use fluxo_net::{clock_us, SenderConfig, VideoSender};

use crate::Opts;

pub fn cmd_enviar(o: &Opts) -> Result<(), Box<dyn Error>> {
    let destino = o
        .destino
        .as_deref()
        .ok_or("enviar precisa de --destino IP:PORTA")?;
    let bitrate_bps = (o.mbps * 1e6).round() as u64;
    let fps = o.fps.max(1);
    let tamanho = (bitrate_bps / 8 / fps as u64).max(1) as usize;
    let cfg = SenderConfig {
        fec_group: o.fec,
        ..SenderConfig::for_bitrate(bitrate_bps)
    };
    let pacing = cfg.pacing_bps;
    let tx = VideoSender::connect("0.0.0.0:0", destino, cfg)?;
    println!(
        "Enviando para {} : {} Mbps ({} fps × {:.1} KB), pacing {:.0} Mbps, FEC {}, {} s",
        tx.peer_addr(),
        o.mbps,
        fps,
        tamanho as f64 / 1024.0,
        pacing as f64 / 1e6,
        match o.fec {
            0 => "desligado".to_string(),
            k => format!("1 a cada {k}"),
        },
        o.segundos
    );

    // Bytes pseudoaleatórios: o conteúdo não importa para a rede.
    let mut semente = 0x9E37_79B9_7F4A_7C15u64;
    let frame: Vec<u8> = (0..tamanho)
        .map(|_| {
            semente ^= semente << 13;
            semente ^= semente >> 7;
            semente ^= semente << 17;
            semente as u8
        })
        .collect();

    let intervalo = Duration::from_secs_f64(1.0 / fps as f64);
    let inicio = Instant::now();
    let duracao = Duration::from_secs_f64(o.segundos.max(0.1));
    let mut proximo = inicio;
    let mut proximo_relatorio = Duration::from_secs(1);
    let mut enviados = 0u64;
    while inicio.elapsed() < duracao {
        let agora = Instant::now();
        if agora < proximo {
            std::thread::sleep(proximo - agora);
        }
        proximo += intervalo;
        tx.send_frame(frame.clone(), true, clock_us())?;
        enviados += 1;
        // O relatório do cliente chega sozinho; aqui só o mostramos.
        let _ = tx.take_report();

        if inicio.elapsed() >= proximo_relatorio {
            let s = tx.stats();
            println!(
                "  {:>3} s: perda {}, fila {:.0} ms, RTT {:.2} ms, fila de envio {}",
                proximo_relatorio.as_secs(),
                s.last_loss
                    .map_or("—".to_string(), |p| format!("{:.2}%", p * 100.0)),
                s.queue_delay_ms,
                s.rtt.smoothed_ms,
                s.queue_len
            );
            proximo_relatorio += Duration::from_secs(1);
        }
    }
    let s = tx.stats();
    drop(tx); // espera a fila de envio esvaziar
    println!(
        "\nEnviados: {enviados} frames, {} pacotes, {:.2} Mbps; RTT {:.2} ms suavizado, {:.2} ms mínimo",
        s.packets_sent,
        s.bytes_sent as f64 * 8.0 / inicio.elapsed().as_secs_f64() / 1e6,
        s.rtt.smoothed_ms,
        s.rtt.min_ms
    );
    Ok(())
}
