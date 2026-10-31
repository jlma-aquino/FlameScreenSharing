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
        intra_refresh_frames: o.refresh,
        ..EncoderConfig::new(m.width, m.height)
    }
}

/// Uma recuperação em andamento depois de um frame descartado de propósito
/// (`--perder-a-cada`).
struct Recuperacao {
    /// Frames decodificados desde a perda (o último é o que limpou a imagem).
    frames: u32,
    psnr_min: f64,
    bytes: u64,
    maior: usize,
}

/// Resultado de uma recuperação concluída.
struct Recuperada {
    frames: u32,
    psnr_min: f64,
    /// PSNR do frame que fechou a recuperação (keyframe ou fim da onda).
    psnr_fim: f64,
    bytes: u64,
    maior: usize,
    /// PSNR entre o frame que fechou a recuperação e o mesmo frame num
    /// decoder que não perdeu nada (99 = idênticos): mede se a recuperação
    /// apagou de fato todo o estrago, sem misturar a qualidade do encode.
    igual_fim: f64,
    /// O mesmo, pior valor nos frames seguintes (resto de erro que se
    /// propaga mostraria aqui).
    igual_depois: f64,
}

/// Frames observados depois de cada recuperação.
const OBSERVAR_DEPOIS: u32 = 30;

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

    // Perda simulada: descarta um frame, pede a recuperação e mede.
    let perder_a_cada =
        (o.perder_a_cada > 0.0 && verif.is_some()).then(|| Duration::from_secs_f64(o.perder_a_cada));
    let mut proxima_perda = perder_a_cada.map(|p| Instant::now() + p);
    // Decoder de referência: recebe todos os frames, inclusive os
    // "perdidos". O decodificado com perda é comparado com ele.
    let mut referencia = match perder_a_cada {
        Some(_) => Some(Decoder::new(cfg.width, cfg.height)?),
        None => None,
    };
    // Recuperação recente ainda observada: (índice em `recuperadas`, frames restantes).
    let mut observando: Option<(usize, u32)> = None;
    let mut perder_proximo = false;
    let mut keyframe_pela_perda = false;
    let mut por_onda = false;
    let mut recuperando: Option<Recuperacao> = None;
    let mut recuperadas: Vec<Recuperada> = Vec::new();
    let mut erros_decode = 0u32;
    let mut psnr_normal = Vec::new();

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

        let pedir_keyframe = keyframe_a_cada.is_some_and(|p| ultimo_keyframe.elapsed() >= p)
            || std::mem::take(&mut keyframe_pela_perda);
        if pedir_keyframe {
            ultimo_keyframe = Instant::now();
        }
        if recuperando.is_none() && proxima_perda.is_some_and(|t| Instant::now() >= t) {
            perder_proximo = true;
            proxima_perda = perder_a_cada.map(|p| Instant::now() + p);
        }
        let t = Instant::now();
        let pacotes = enc.encode(frame.texture(), pedir_keyframe)?;
        let dt = t.elapsed();
        drop(frame); // devolve o frame ao Windows o quanto antes
        entradas += 1;

        for p in pacotes {
            if let Some(v) = &mut verif {
                if std::mem::take(&mut perder_proximo) {
                    // O decoder nunca vê este frame, como se a rede o perdesse.
                    v.originais.pop_back();
                    if let Some(r) = &mut referencia {
                        let _ = r.decode(&p.data);
                    }
                    observando = None;
                    por_onda = enc.start_intra_refresh();
                    keyframe_pela_perda = !por_onda;
                    recuperando = Some(Recuperacao {
                        frames: 0,
                        psnr_min: f64::INFINITY,
                        bytes: 0,
                        maior: 0,
                    });
                } else {
                    let antes = v.psnrs.len();
                    let dec = v.decoder.decode(&p.data);
                    let sem_perda = referencia.as_mut().map(|r| r.decode(&p.data));
                    let igual = match (&dec, &sem_perda) {
                        (Ok(a), Some(Ok(b))) => a.first().zip(b.first()).map(|(a, b)| psnr(a, b)),
                        _ => None,
                    };
                    match dec {
                        Ok(dec) => v.comparar(dec),
                        Err(_) => {
                            erros_decode += 1;
                            v.originais.pop_back();
                        }
                    }
                    let psnr = (v.psnrs.len() > antes).then(|| v.psnrs[v.psnrs.len() - 1]);
                    match &mut recuperando {
                        Some(r) => {
                            r.frames += 1;
                            r.psnr_min = r.psnr_min.min(psnr.unwrap_or(0.0));
                            r.bytes += p.data.len() as u64;
                            r.maior = r.maior.max(p.data.len());
                            let limpo = if por_onda { p.refresh_end } else { p.keyframe };
                            if limpo {
                                recuperadas.push(Recuperada {
                                    frames: r.frames,
                                    psnr_min: r.psnr_min,
                                    psnr_fim: psnr.unwrap_or(0.0),
                                    bytes: r.bytes,
                                    maior: r.maior,
                                    igual_fim: igual.unwrap_or(0.0),
                                    igual_depois: 99.0,
                                });
                                recuperando = None;
                                observando = Some((recuperadas.len() - 1, OBSERVAR_DEPOIS));
                            }
                        }
                        None => {
                            psnr_normal.extend(psnr);
                            if let Some((i, resto)) = &mut observando {
                                let r = &mut recuperadas[*i];
                                r.igual_depois = r.igual_depois.min(igual.unwrap_or(0.0));
                                *resto -= 1;
                                if *resto == 0 {
                                    observando = None;
                                }
                            }
                        }
                    }
                }
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
    if perder_a_cada.is_some() {
        let n = recuperadas.len().max(1) as f64;
        let media = |f: &dyn Fn(&Recuperada) -> f64| recuperadas.iter().map(f).sum::<f64>() / n;
        let normal = psnr_normal.iter().sum::<f64>() / psnr_normal.len().max(1) as f64;
        println!(
            "\nPerdas simuladas ({}): {} recuperadas{}",
            match (o.refresh, por_onda || recuperadas.is_empty() && o.refresh > 0) {
                (k, true) if k > 0 => format!("recuperação por onda de intra-refresh de {k} frames"),
                _ => "recuperação por keyframe".to_string(),
            },
            recuperadas.len(),
            match recuperando {
                Some(_) => " (1 ainda em andamento no fim)",
                None => "",
            }
        );
        if !recuperadas.is_empty() {
            println!(
                "  frames até a imagem limpa: média {:.1}, máx {}",
                media(&|r| r.frames as f64),
                recuperadas.iter().map(|r| r.frames).max().unwrap_or(0)
            );
            println!("  PSNR normal:               média {normal:.1} dB");
            println!(
                "  PSNR durante a recuperação: mínimo médio {:.1} dB",
                media(&|r| r.psnr_min)
            );
            println!(
                "  PSNR do frame que limpou:  média {:.1} dB, pior {:.1} dB",
                media(&|r| r.psnr_fim),
                recuperadas
                    .iter()
                    .map(|r| r.psnr_fim)
                    .fold(f64::INFINITY, f64::min)
            );
            println!(
                "  frames da recuperação:     média {:.1} KB, maior {:.1} KB",
                media(&|r| r.bytes as f64 / r.frames.max(1) as f64) / 1024.0,
                recuperadas.iter().map(|r| r.maior).max().unwrap_or(0) as f64 / 1024.0
            );
        }
        if !recuperadas.is_empty() {
            let identicas = recuperadas.iter().filter(|r| r.igual_fim >= 99.0).count();
            println!(
                "  vs. decoder sem perda:     {identicas} de {} recuperações idênticas no fim (99 dB = idêntico); \
                 média {:.1} dB, pior {:.1} dB",
                recuperadas.len(),
                media(&|r| r.igual_fim),
                recuperadas.iter().map(|r| r.igual_fim).fold(f64::INFINITY, f64::min)
            );
            println!(
                "  {OBSERVAR_DEPOIS} frames seguintes:       pior {:.1} dB",
                recuperadas
                    .iter()
                    .map(|r| r.igual_depois)
                    .fold(f64::INFINITY, f64::min)
            );
        }
        if erros_decode > 0 {
            println!("  erros do decoder:          {erros_decode}");
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
