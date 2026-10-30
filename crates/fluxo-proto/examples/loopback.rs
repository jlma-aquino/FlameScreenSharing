//! Teste de carga do protocolo via UDP em `127.0.0.1`.
//!
//! Simula um stream de vídeo: frames "codificados" de tamanho realista a
//! 60 fps, fragmentados, enviados por UDP, remontados do outro lado. Opcionalmente
//! descarta pacotes de propósito para simular uma rede ruim.
//!
//! Uso:
//! ```text
//! cargo run -p fluxo-proto --release --example loopback -- [--segundos 5] [--mbps 20] [--fps 60] [--perda 0.5]
//! ```
//! `--perda` é a porcentagem de pacotes descartados antes do envio.

use std::net::UdpSocket;
use std::thread;
use std::time::{Duration, Instant};

use fluxo_proto::{Fragmenter, PacketHeader, Reassembler, DEFAULT_MAX_PAYLOAD};

struct Args {
    segundos: f64,
    mbps: f64,
    fps: f64,
    perda_pct: f64,
}

fn parse_args() -> Args {
    let mut a = Args {
        segundos: 5.0,
        mbps: 20.0,
        fps: 60.0,
        perda_pct: 0.0,
    };
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i + 1 < v.len() {
        let n: f64 = v[i + 1].parse().expect("valor numérico esperado");
        match v[i].as_str() {
            "--segundos" => a.segundos = n,
            "--mbps" => a.mbps = n,
            "--fps" => a.fps = n,
            "--perda" => a.perda_pct = n,
            outro => panic!("argumento desconhecido: {outro}"),
        }
        i += 2;
    }
    a
}

/// Gerador pseudoaleatório minúsculo (xorshift) para não depender de crates.
struct Rng(u64);
impl Rng {
    fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn main() -> std::io::Result<()> {
    let args = parse_args();
    let rx = UdpSocket::bind("127.0.0.1:0")?;
    rx.set_read_timeout(Some(Duration::from_millis(500)))?;
    let rx_addr = rx.local_addr()?;

    let bytes_por_frame = (args.mbps * 1_000_000.0 / 8.0 / args.fps) as usize;
    let total_frames = (args.segundos * args.fps) as u32;
    println!(
        "Enviando {total_frames} frames de ~{} KB a {} fps ({} Mbps), perda simulada {}%",
        bytes_por_frame / 1024,
        args.fps,
        args.mbps,
        args.perda_pct
    );

    // ---------- Envio (thread separada, como seria no host) ----------
    let perda = args.perda_pct / 100.0;
    let intervalo = Duration::from_secs_f64(1.0 / args.fps);
    let sender = thread::spawn(move || -> std::io::Result<(u64, u64)> {
        let tx = UdpSocket::bind("127.0.0.1:0")?;
        tx.connect(rx_addr)?;
        let mut frag = Fragmenter::new(DEFAULT_MAX_PAYLOAD);
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let inicio = Instant::now();
        let (mut enviados, mut descartados) = (0u64, 0u64);

        for n in 0..total_frames {
            // Keyframe a cada 2 s, 5x maior (como num encoder real).
            let key = n % (args.fps as u32 * 2) == 0;
            let tamanho = if key { bytes_por_frame * 5 } else { bytes_por_frame };
            let frame = vec![(n % 251) as u8; tamanho];
            let ts = inicio.elapsed().as_micros() as u32;

            let (_, pacotes) = frag.fragment(&frame, key, ts).expect("fragmentar");
            for p in &pacotes {
                if rng.next_f64() < perda {
                    descartados += 1;
                    continue;
                }
                tx.send(p)?;
                enviados += 1;
            }

            // Ritmo de 60 fps.
            let alvo = intervalo * (n + 1);
            if let Some(espera) = alvo.checked_sub(inicio.elapsed()) {
                thread::sleep(espera);
            }
        }
        Ok((enviados, descartados))
    });

    // ---------- Recepção (como seria no cliente) ----------
    let mut reasm = Reassembler::default();
    let mut buf = vec![0u8; 65_536];
    let inicio = Instant::now();
    let mut frames_ok = 0u64;
    let mut frames_corrompidos = 0u64;
    let mut ultimo_relatorio = Instant::now();

    loop {
        let n = match rx.recv(&mut buf) {
            Ok(n) => n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if sender.is_finished() {
                    break;
                }
                continue;
            }
            Err(e) => return Err(e),
        };
        let Ok((h, payload)) = PacketHeader::decode(&buf[..n]) else {
            continue;
        };
        if let Some(f) = reasm.push(&h, payload) {
            // Confere o conteúdo: todos os bytes devem ser iguais a (id % 251).
            if f.data.iter().all(|&b| b == (f.frame_id % 251) as u8) {
                frames_ok += 1;
            } else {
                frames_corrompidos += 1;
            }
        }
        if ultimo_relatorio.elapsed() >= Duration::from_secs(1) {
            let s = reasm.stats();
            println!(
                "  t={:4.1}s  frames ok={:5}  perdidos={:4}  pacotes={}",
                inicio.elapsed().as_secs_f64(),
                frames_ok,
                s.frames_lost,
                s.packets_received
            );
            ultimo_relatorio = Instant::now();
        }
    }

    let (enviados, descartados) = sender.join().expect("thread de envio")?;
    let s = reasm.stats();
    println!("\nResultado");
    println!("  pacotes enviados:        {enviados} (descartados de propósito: {descartados})");
    println!("  pacotes recebidos:       {}", s.packets_received);
    println!("  frames completos:        {frames_ok} de {total_frames}");
    println!("  frames perdidos:         {}", s.frames_lost);
    println!("  frames corrompidos:      {frames_corrompidos}");
    println!(
        "  duplicados / atrasados:  {} / {}",
        s.duplicate_packets, s.late_packets
    );
    Ok(())
}
