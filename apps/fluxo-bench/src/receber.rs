//! Comando `receber`: o lado cliente provisório (até existir o fluxo-client
//! na fase 3). Recebe o vídeo do `fluxo-host`, mede e grava em `.h264`.

use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use fluxo_net::{ReceiverConfig, VideoReceiver};

use crate::verificar::Decoder;
use crate::Opts;

pub fn cmd_receber(o: &Opts) -> Result<(), Box<dyn Error>> {
    let rx = VideoReceiver::bind(("0.0.0.0", o.porta), ReceiverConfig::default())?;
    let saida = o.saida.clone().unwrap_or_else(|| "recebido.h264".into());
    println!(
        "Escutando em {}. Inicie o fluxo-host com --destino <IP deste PC>:{}",
        rx.local_addr(),
        o.porta
    );
    if o.segundos > 0.0 {
        println!(
            "(mede {} s a partir do primeiro frame; Ctrl+C encerra antes)",
            o.segundos
        );
    }

    let mut decoder = match o.verificar {
        true => {
            let mut d = Decoder::new(0, 0)?;
            d.convert = false; // só contar: converter custaria CPU e atrasaria a recepção
            Some(d)
        }
        false => None,
    };
    let mut arquivo = BufWriter::new(File::create(&saida)?);
    let duracao = (o.segundos > 0.0).then(|| Duration::from_secs_f64(o.segundos));

    let mut inicio: Option<Instant> = None;
    let mut esperando_keyframe = true;
    let (mut frames, mut bytes, mut keyframes, mut pulados, mut decodificados) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut com_perda_antes = 0u64;
    let mut transitos: Vec<i32> = Vec::new();
    let mut proximo_relatorio = Duration::from_secs(1);
    let (mut frames_seg, mut bytes_seg, mut transito_seg, mut transito_max_seg) = (0u64, 0u64, 0i64, 0i32);

    loop {
        if let (Some(t0), Some(d)) = (inicio, duracao) {
            if t0.elapsed() >= d {
                break;
            }
        }
        let recebido = rx.recv_frame(Duration::from_millis(100))?;

        if let Some(f) = recebido {
            if inicio.is_none() {
                let de = rx.stats().peer.map(|p| p.to_string()).unwrap_or_default();
                println!("Recebendo de {de}");
                inicio = Some(Instant::now());
            }
            // O decoder (e quem abrir o arquivo) só consegue começar num keyframe.
            if esperando_keyframe {
                if !f.frame.keyframe {
                    pulados += 1;
                    continue;
                }
                esperando_keyframe = false;
            }
            arquivo.write_all(&f.frame.data)?;
            if let Some(d) = &mut decoder {
                decodificados += d.decode(&f.frame.data)?.len() as u64;
            }
            frames += 1;
            bytes += f.frame.data.len() as u64;
            keyframes += f.frame.keyframe as u64;
            com_perda_antes += f.frame.preceded_by_loss as u64;
            transitos.push(f.transit_us);
            frames_seg += 1;
            bytes_seg += f.frame.data.len() as u64;
            transito_seg += f.transit_us as i64;
            transito_max_seg = transito_max_seg.max(f.transit_us);
        }

        if let Some(t0) = inicio {
            if t0.elapsed() >= proximo_relatorio {
                let s = rx.stats();
                println!(
                    "  {:>3} s: {frames_seg} fps, {:.1} Mbps, trânsito {:.1} ms (máx {:.1}), \
                     perdidos {}, RTT {:.2} ms",
                    proximo_relatorio.as_secs(),
                    bytes_seg as f64 * 8.0 / 1e6,
                    transito_seg as f64 / frames_seg.max(1) as f64 / 1000.0,
                    transito_max_seg as f64 / 1000.0,
                    s.reassembly.frames_lost,
                    s.rtt.smoothed_ms
                );
                (frames_seg, bytes_seg, transito_seg, transito_max_seg) = (0, 0, 0, 0);
                proximo_relatorio += Duration::from_secs(1);
            }
        }
    }
    arquivo.flush()?;
    if let Some(d) = &mut decoder {
        decodificados += d.finish()?.len() as u64;
    }

    let s = rx.stats();
    let r = s.reassembly;
    let total = inicio.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    transitos.sort_unstable();
    let pct = |p: f64| {
        transitos
            .get(((transitos.len().max(1) - 1) as f64 * p).round() as usize)
            .map(|&us| us as f64 / 1000.0)
            .unwrap_or(0.0)
    };
    println!("\nResultado");
    println!("  frames gravados:          {frames} ({keyframes} keyframes), {pulados} pulados antes do 1º keyframe");
    println!(
        "  bitrate médio:            {:.2} Mbps",
        bytes as f64 * 8.0 / total.max(0.001) / 1e6
    );
    println!(
        "  pacotes: {} recebidos, {} duplicados, {} atrasados, {} inválidos",
        r.packets_received, r.duplicate_packets, r.late_packets, s.invalid_packets
    );
    println!(
        "  frames perdidos na rede:  {} (frames que chegaram depois de uma perda: {com_perda_antes})",
        r.frames_lost
    );
    if s.frames_dropped > 0 {
        println!(
            "  descartados na fila:      {} (este programa não leu a tempo)",
            s.frames_dropped
        );
    }
    println!(
        "  trânsito captura→chegada: p50 {:.1} ms, p99 {:.1} ms, máx {:.1} ms (válido com relógios iguais)",
        pct(0.5),
        pct(0.99),
        pct(1.0)
    );
    println!(
        "  RTT:                      {:.2} ms suavizado, {:.2} ms mínimo ({} medições)",
        s.rtt.smoothed_ms, s.rtt.min_ms, s.rtt.samples
    );
    if let Some(d) = &decoder {
        let (w, h) = d.size();
        println!("  decodificados:            {decodificados} de {frames} ({w}x{h})");
    }
    println!("  gravado em {saida}");
    Ok(())
}
