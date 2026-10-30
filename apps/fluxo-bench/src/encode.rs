//! Comandos `encoders`, `encode` e `decode`: testam o fluxo-encode (e o
//! fluxo-decode) com frames reais da captura.

use std::collections::VecDeque;
use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use fluxo_capture::{CaptureError, DesktopCapturer};
use fluxo_encode::{gpu_vendor, open_encoder, probe_backend, Backend, EncoderConfig};

use crate::verificar::{psnr, Decoder};
use crate::{bmp, Opts, Serie};

/// Estado do `--verificar`: decodifica cada frame e compara com o original.
struct Verificacao {
    decoder: Decoder,
    /// Originais (BGRA) esperando o frame decodificado correspondente.
    originais: VecDeque<Vec<u8>>,
    psnrs: Vec<f64>,
    ultimo: Option<(Vec<u8>, Vec<u8>)>,
}

impl Verificacao {
    fn comparar(&mut self, decodificados: Vec<Vec<u8>>) {
        for dec in decodificados {
            if let Some(orig) = self.originais.pop_front() {
                self.psnrs.push(psnr(&orig, &dec));
                self.ultimo = Some((orig, dec));
            }
        }
    }
}

fn config(o: &Opts, cap: &DesktopCapturer) -> EncoderConfig {
    let m = cap.monitor();
    EncoderConfig {
        fps: o.fps,
        bitrate_kbps: (o.mbps * 1000.0).round() as u32,
        backend: o.encoder,
        ..EncoderConfig::new(m.width, m.height)
    }
}

/// Lista os backends e diz se cada um funciona nesta máquina.
pub fn cmd_encoders(o: &Opts) -> Result<(), Box<dyn Error>> {
    let cap = DesktopCapturer::new(o.monitor)?;
    let m = cap.monitor().clone();
    let cfg = config(o, &cap);
    println!(
        "Monitor {} ({}x{}) na placa {} — fabricante {:?}\n",
        m.index,
        m.width,
        m.height,
        m.adapter_name,
        gpu_vendor(cap.d3d_device())?
    );
    for b in Backend::ALL {
        match probe_backend(cap.d3d_device(), &cfg, b) {
            Ok(mut enc) => {
                println!("  [ok]  {:<6} {}", b.id(), enc.name());
                // O bitrate adaptativo precisa mudar o bitrate em tempo real.
                let metade = enc.bitrate_kbps() / 2;
                match enc.set_bitrate(metade) {
                    Ok(()) => println!(
                        "               bitrate em tempo real: ok ({} → {metade} kbps)",
                        cfg.bitrate_kbps
                    ),
                    Err(e) => println!("               bitrate em tempo real: NÃO ({e})"),
                }
            }
            Err(e) => println!("  [--]  {:<6} {e}", b.id()),
        }
    }
    let aberto = open_encoder(cap.d3d_device(), &cfg)?;
    println!("\n--encoder {} usaria: {}", cfg.backend, aberto.encoder.name());
    for (b, motivo) in &aberto.skipped {
        println!("  (pulou {b}: {motivo})");
    }
    Ok(())
}

/// Captura, codifica, mede e grava o resultado num arquivo .h264.
pub fn cmd_encode(o: &Opts) -> Result<(), Box<dyn Error>> {
    let mut cap = DesktopCapturer::new(o.monitor)?;
    let cfg = config(o, &cap);
    let aberto = open_encoder(cap.d3d_device(), &cfg)?;
    for (b, motivo) in &aberto.skipped {
        println!("(pulando {b}: {motivo})");
    }
    let mut enc = aberto.encoder;
    let saida = o.saida.clone().unwrap_or_else(|| "captura.h264".into());
    println!(
        "Codificando monitor {} ({}x{}) com {}\n  alvo {} Mbps a {} fps, {} s → {saida}",
        o.monitor,
        cfg.width,
        cfg.height,
        enc.name(),
        o.mbps,
        cfg.fps,
        o.segundos
    );

    let mut verif = match o.verificar {
        true => Some(Verificacao {
            decoder: Decoder::new(cfg.width, cfg.height)?,
            originais: VecDeque::new(),
            psnrs: Vec::new(),
            ultimo: None,
        }),
        false => None,
    };
    let mut copia = Vec::new();

    let mut arquivo = BufWriter::new(File::create(&saida)?);
    let duracao = Duration::from_secs_f64(o.segundos);
    let keyframe_a_cada = (o.keyframe_a_cada > 0.0).then(|| Duration::from_secs_f64(o.keyframe_a_cada));
    let inicio = Instant::now();
    let mut ultimo_keyframe = inicio;

    let mut tempos = Serie(Vec::new());
    let mut tempos_keyframe = Serie(Vec::new());
    let (mut entradas, mut saidas, mut bytes, mut bytes_keyframe, mut keyframes) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut maior_frame = 0usize;
    let mut proximo_relatorio = Duration::from_secs(1);
    let (mut frames_no_segundo, mut bytes_no_segundo) = (0u64, 0u64);

    while inicio.elapsed() < duracao {
        let mut frame = match cap.next_frame(100) {
            Ok(Some(f)) => f,
            Ok(None) => continue,
            Err(CaptureError::AccessLost) => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        if !frame.info().desktop_updated {
            continue;
        }
        if let Some(v) = &mut verif {
            frame.copy_to_cpu(&mut copia)?;
            v.originais.push_back(copia.clone());
        }

        let pedir_keyframe = keyframe_a_cada.is_some_and(|p| ultimo_keyframe.elapsed() >= p);
        if pedir_keyframe {
            ultimo_keyframe = Instant::now();
        }
        let t = Instant::now();
        let pacotes = enc.encode(frame.texture(), pedir_keyframe)?;
        let dt = t.elapsed();
        drop(frame); // devolve o frame ao Windows o quanto antes
        entradas += 1;

        for p in pacotes {
            if let Some(v) = &mut verif {
                let dec = v.decoder.decode(&p.data)?;
                v.comparar(dec);
            }
            arquivo.write_all(&p.data)?;
            saidas += 1;
            bytes += p.data.len() as u64;
            bytes_no_segundo += p.data.len() as u64;
            maior_frame = maior_frame.max(p.data.len());
            if p.keyframe {
                keyframes += 1;
                bytes_keyframe += p.data.len() as u64;
                tempos_keyframe.0.push(dt);
            } else {
                tempos.0.push(dt);
            }
        }
        frames_no_segundo += 1;

        if inicio.elapsed() >= proximo_relatorio {
            println!(
                "  {:>3} s: {frames_no_segundo} fps, {:.1} Mbps",
                proximo_relatorio.as_secs(),
                bytes_no_segundo as f64 * 8.0 / 1e6
            );
            (frames_no_segundo, bytes_no_segundo) = (0, 0);
            proximo_relatorio += Duration::from_secs(1);
        }
    }
    arquivo.flush()?;

    let total = inicio.elapsed().as_secs_f64();
    let p_frames = saidas.saturating_sub(keyframes);
    println!("\nResultado ({})", enc.name());
    println!("  frames: {entradas} entregues, {saidas} codificados ({keyframes} keyframes)");
    println!(
        "  tempo de encode (P):      média {:.2} ms, p50 {:.2} ms, p99 {:.2} ms",
        tempos.media_ms(),
        tempos.percentil_ms(0.50),
        tempos.percentil_ms(0.99)
    );
    if keyframes > 0 {
        println!(
            "  tempo de encode (keyframe): média {:.2} ms, máx {:.2} ms",
            tempos_keyframe.media_ms(),
            tempos_keyframe.percentil_ms(1.0)
        );
    }
    let alvo_por_frame = cfg.bitrate_bps() as f64 / 8.0 / cfg.fps as f64;
    if p_frames > 0 {
        println!(
            "  tamanho médio (P):        {:.1} KB  (alvo do bitrate: {:.1} KB/frame)",
            (bytes - bytes_keyframe) as f64 / p_frames as f64 / 1024.0,
            alvo_por_frame / 1024.0
        );
    }
    if keyframes > 0 {
        println!(
            "  tamanho médio (keyframe): {:.1} KB",
            bytes_keyframe as f64 / keyframes as f64 / 1024.0
        );
    }
    println!(
        "  maior frame:              {:.1} KB",
        maior_frame as f64 / 1024.0
    );
    println!(
        "  bitrate real:             {:.2} Mbps",
        bytes as f64 * 8.0 / total / 1e6
    );
    if let Some(mut v) = verif {
        let resto = v.decoder.finish()?;
        v.comparar(resto);
        let n = v.psnrs.len();
        let media = v.psnrs.iter().sum::<f64>() / n.max(1) as f64;
        let minimo = v.psnrs.iter().copied().fold(f64::INFINITY, f64::min);
        println!("\nVerificação (decoder H.264 do Windows)");
        println!("  frames decodificados:     {n} de {saidas}");
        if n > 0 {
            println!("  PSNR vs. original:        média {media:.1} dB, mínimo {minimo:.1} dB");
        }
        if let Some((orig, dec)) = &v.ultimo {
            for (sufixo, img) in [("original", orig), ("decodificado", dec)] {
                let nome = format!("{saida}.{sufixo}.bmp");
                bmp::write_bmp32(BufWriter::new(File::create(&nome)?), cfg.width, cfg.height, img)?;
                println!("  último frame salvo em     {nome}");
            }
        }
    }
    println!("\nAbra o vídeo no VLC (arraste o arquivo) ou: ffplay -f h264 {saida}");
    if (entradas as f64) / total < 5.0 {
        println!(
            "Poucos frames? A tela provavelmente estava parada. Rode de novo com um vídeo ou jogo aberto."
        );
    }
    Ok(())
}

/// Captura → encode → decode na mesma máquina, sem rede: mede o decoder
/// isolado (num device separado, como no cliente).
pub fn cmd_decode(o: &Opts) -> Result<(), Box<dyn Error>> {
    let mut cap = DesktopCapturer::new(o.monitor)?;
    let cfg = config(o, &cap);
    let mut enc = open_encoder(cap.d3d_device(), &cfg)?.encoder;
    let device = fluxo_decode::create_device()?;
    let mut dec = fluxo_decode::VideoDecoder::new(&device)?;
    println!(
        "Encode: {}\nDecode: {}\n{} s com a tela do monitor {}...",
        enc.name(),
        dec.name(),
        o.segundos,
        o.monitor
    );

    let duracao = Duration::from_secs_f64(o.segundos);
    let inicio = Instant::now();
    let mut tempos = Serie(Vec::new());
    let (mut codificados, mut decodificados) = (0u64, 0u64);
    let mut tamanho = (0, 0);
    while inicio.elapsed() < duracao {
        let frame = match cap.next_frame(100) {
            Ok(Some(f)) => f,
            Ok(None) => continue,
            Err(CaptureError::AccessLost) => continue,
            Err(e) => return Err(e.into()),
        };
        if !frame.info().desktop_updated {
            continue;
        }
        let pacotes = enc.encode(frame.texture(), false)?;
        drop(frame);
        for p in pacotes {
            codificados += 1;
            let t = Instant::now();
            let saida = dec.decode(&p.data)?;
            tempos.0.push(t.elapsed());
            if let Some(f) = saida {
                decodificados += 1;
                tamanho = (f.width(), f.height());
            }
        }
    }

    println!("\nResultado ({})", dec.name());
    println!(
        "  frames: {codificados} codificados, {decodificados} decodificados ({}x{})",
        tamanho.0, tamanho.1
    );
    println!(
        "  tempo de decode (CPU):    média {:.2} ms, p50 {:.2} ms, p99 {:.2} ms",
        tempos.media_ms(),
        tempos.percentil_ms(0.50),
        tempos.percentil_ms(0.99)
    );
    println!("  (com DXVA, parte do trabalho termina na GPU depois; o fluxo-client mede até a tela)");
    if decodificados + 1 < codificados {
        println!("  Atenção: o decoder está segurando frames — latência extra de um frame por frame retido.");
    }
    Ok(())
}
