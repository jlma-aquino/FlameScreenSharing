//! Comando `audio`: captura o que o PC toca, passa pelo Opus de ida e volta
//! e mostra o resultado — sem rede e, por padrão, sem tocar nada.
//!
//! `--saida x.wav` grava o áudio decodificado; `--volume V` também o toca
//! pela saída padrão no volume V (0 a 100; com 0, testa a reprodução em
//! silêncio).

use std::error::Error;

use crate::Opts;

#[cfg(not(windows))]
pub fn cmd_audio(_: &Opts) -> Result<(), Box<dyn Error>> {
    Err("a captura de som (WASAPI) só existe no Windows".into())
}

#[cfg(windows)]
pub fn cmd_audio(o: &Opts) -> Result<(), Box<dyn Error>> {
    use std::time::{Duration, Instant};

    use fluxo_audio::{
        output_devices, AudioOutput, Decoder, Encoder, LoopbackCapture, CHANNELS, DEFAULT_BITRATE,
        SAMPLE_RATE,
    };

    use crate::wav::WavWriter;

    // As saídas de som, com o nível agora: mostra onde o som está saindo.
    println!("Saídas de som (nível agora):");
    for d in output_devices()? {
        println!(
            "  {} {:<45} {:>5.2}{}",
            d.index,
            d.name,
            d.peak,
            if d.default { "  (padrão)" } else { "" }
        );
    }
    let mut cap = LoopbackCapture::new(o.dispositivo)?;
    let (taxa, canais) = cap.device_format();
    println!(
        "Capturando o que o PC toca ({taxa} Hz, {canais} canais → Opus 48 kHz estéreo, {} kbps) por {} s",
        DEFAULT_BITRATE / 1000,
        o.segundos
    );
    let mut enc = Encoder::new(DEFAULT_BITRATE, 5)?;
    let mut dec = Decoder::new()?;
    let mut wav = o
        .saida
        .as_deref()
        .map(|s| WavWriter::create(s, SAMPLE_RATE, CHANNELS as u16))
        .transpose()?;
    let mut saida = o.volume.map(|v| AudioOutput::new(v / 100.0)).transpose()?;

    let (mut quadros, mut silencio, mut bytes, mut pico, mut soma) = (0u64, 0u64, 0usize, 0f32, 0f64);
    let mut maior_encode = Duration::ZERO;
    let inicio = Instant::now();
    while inicio.elapsed().as_secs_f64() < o.segundos {
        std::thread::sleep(Duration::from_millis(5));
        for q in cap.read_frames()? {
            quadros += 1;
            if q.iter().all(|&v| v == 0.0) {
                silencio += 1;
            }
            for v in &q {
                pico = pico.max(v.abs());
                soma += (*v as f64) * (*v as f64);
            }
            let t = Instant::now();
            let pacote = enc.encode(&q)?;
            maior_encode = maior_encode.max(t.elapsed());
            bytes += pacote.len();
            let pcm = dec.decode(Some(&pacote))?;
            if let Some(w) = wav.as_mut() {
                w.write(&pcm)?;
            }
            if let Some(s) = saida.as_mut() {
                s.write(&pcm)?;
            }
        }
    }
    let duracao = inicio.elapsed().as_secs_f64();
    let rms = (soma / (quadros.max(1) as f64 * 960.0)).sqrt();
    println!(
        "  quadros: {quadros} ({:.0}/s; esperado 100), {silencio} só de silêncio; {:.1} s vieram da placa",
        quadros as f64 / duracao,
        cap.device_seconds()
    );
    println!(
        "  nível: pico {:.2}, RMS {:.3}{}",
        pico,
        rms,
        if pico == 0.0 { " (nada tocando)" } else { "" }
    );
    println!(
        "  Opus: {:.0} kbps médio, encode mais lento {:.2} ms por quadro de 10 ms",
        bytes as f64 * 8.0 / duracao / 1000.0,
        maior_encode.as_secs_f64() * 1000.0
    );
    if let Some(s) = &saida {
        println!(
            "  tocando com volume {:.0}%: {:.1} ms na fila da placa",
            o.volume.unwrap_or(0.0),
            s.queued_ms()
        );
    }
    if let (Some(w), Some(caminho)) = (wav, &o.saida) {
        let s = w.seconds(SAMPLE_RATE);
        w.finish()?;
        println!("  gravado em {caminho} ({s:.1} s)");
    }
    Ok(())
}
