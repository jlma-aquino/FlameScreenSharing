//! Comando `receber`: recebe o vídeo do `fluxo-host`, mede e grava em
//! `.h264`, sem janela.
//!
//! É o único comando que roda **fora do Windows** (só usa o `fluxo-net`):
//! serve para testar a rede com um segundo PC Linux/macOS como receptor.
//! Lá, `--verificar` não existe (o decoder é do Windows); confira o arquivo
//! gravado com `ffplay` ou VLC.

use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::net::UdpSocket;
use std::time::{Duration, Instant};

use fluxo_net::{
    ConnectionCode, FrameAction, PairingCode, Puncher, ReceiverConfig, Recovery, RecoveryRequest, ReplyCode,
    VideoReceiver,
};

#[cfg(windows)]
use crate::verificar::Decoder;
use crate::Opts;

/// Fora do Windows não há decoder: `--verificar` vira um aviso.
#[cfg(not(windows))]
struct Decoder;

#[cfg(not(windows))]
impl Decoder {
    fn decode(&mut self, _: &[u8]) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
        Ok(Vec::new())
    }
    fn finish(&mut self) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
        Ok(Vec::new())
    }
    fn size(&self) -> (u32, u32) {
        (0, 0)
    }
}

#[cfg(windows)]
fn abrir_decoder() -> Result<Option<Decoder>, Box<dyn Error>> {
    let mut d = Decoder::new(0, 0)?;
    d.convert = false; // só contar: converter custaria CPU e atrasaria a recepção
    Ok(Some(d))
}

#[cfg(not(windows))]
fn abrir_decoder() -> Result<Option<Decoder>, Box<dyn Error>> {
    println!("(--verificar só existe no Windows; confira o arquivo gravado com ffplay ou VLC)");
    Ok(None)
}

pub fn cmd_receber(o: &Opts) -> Result<(), Box<dyn Error>> {
    let codigo = match (&o.codigo, o.sem_cifra) {
        (_, true) => None,
        (Some(c), false) => Some(PairingCode::parse(c)?),
        (None, false) => Some(PairingCode::generate()?),
    };
    let servidor = o
        .sinal
        .as_deref()
        .map(fluxo_net::resolve_signal_server)
        .transpose()?;
    if (servidor.is_some() || o.internet) && codigo.is_none() {
        return Err("--sinal e --internet precisam do código (sem --sem-cifra)".into());
    }
    if servidor.is_some() && o.internet {
        return Err("use --sinal ou --internet, não os dois".into());
    }
    // Sem servidor: o endereço público vem do STUN, e só vale para este socket.
    let socket = UdpSocket::bind(("0.0.0.0", o.porta))?;
    let publico = match o.internet {
        true => Some(fluxo_net::discover_public_addr(&socket, fluxo_net::STUN_SERVERS)?),
        false => None,
    };
    let rx = VideoReceiver::with_socket(
        socket,
        ReceiverConfig {
            pairing_code: codigo.clone(),
            signal_server: servidor,
            keepalive_to: publico.as_ref().map(|p| p.server),
            on_pairing: publico
                .as_ref()
                .map(|_| fluxo_net::PairingObserver::print_to_terminal()),
            ..ReceiverConfig::default()
        },
    )?;
    let saida = o.saida.clone().unwrap_or_else(|| "recebido.h264".into());
    match (&codigo, &o.sinal, &publico) {
        (Some(c), _, Some(p)) => {
            let cc = ConnectionCode {
                pairing: c.clone(),
                addr: p.v4()?,
            };
            println!(
                "Escutando em {} (público {}; {}).",
                rx.local_addr(),
                p.addr,
                p.summary()
            );
            println!("Código de conexão: {cc}");
            println!("  No PC que envia: fluxo-bench enviar --conexao {cc}");
            if let Some(aviso) = p.warning() {
                println!("  {aviso}");
            }
            println!("Depois cole aqui o código de resposta que ele mostrar:");
            ler_respostas(rx.puncher());
        }
        (Some(c), Some(s), None) => println!(
            "Escutando em {}, código de pareamento {c}. No host: --sinal {s} --codigo {c}",
            rx.local_addr(),
        ),
        (Some(c), None, None) => println!(
            "Escutando em {}, código de pareamento {c}. No host: --destino <IP deste PC>:{} --codigo {c}",
            rx.local_addr(),
            o.porta
        ),
        (None, ..) => println!(
            "Escutando em {} SEM cifra. No host: --destino <IP deste PC>:{} --sem-cifra",
            rx.local_addr(),
            o.porta
        ),
    }
    if o.segundos > 0.0 {
        println!(
            "(mede {} s a partir do primeiro frame; Ctrl+C encerra antes)",
            o.segundos
        );
    }

    let mut decoder = match o.verificar {
        true => abrir_decoder()?,
        false => None,
    };
    let mut arquivo = BufWriter::new(File::create(&saida)?);
    let duracao = (o.segundos > 0.0).then(|| Duration::from_secs_f64(o.segundos));

    let mut inicio: Option<Instant> = None;
    // Largada e perdas: o que gravar e o que pedir ao host (ver fluxo_net::Recovery).
    let mut recuperacao = Recovery::new(Instant::now());
    let mut congelados: Vec<f64> = Vec::new();
    let mut ja_limpo = false;
    let (mut frames, mut bytes, mut keyframes, mut pulados, mut decodificados, mut sujos) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    // Latência captura → chegada, com os relógios sincronizados pelos
    // Ping/Pong. Os frames de antes da 1ª sincronização (~500 ms) ficam fora.
    let mut latencias: Vec<i32> = Vec::new();
    let mut proximo_relatorio = Duration::from_secs(1);
    let (mut frames_seg, mut bytes_seg) = (0u64, 0u64);
    let mut latencias_seg: Vec<i32> = Vec::new();
    let mut avisou_registro = false;

    loop {
        if let (Some(t0), Some(d)) = (inicio, duracao) {
            if t0.elapsed() >= d {
                break;
            }
        }
        // Sem imagem limpa (largada ou depois de uma perda): pede ao host.
        // O fluxo-net segura pedidos repetidos.
        match recuperacao.request(Instant::now()) {
            RecoveryRequest::Keyframe => {
                rx.request_keyframe();
            }
            RecoveryRequest::Refresh => {
                rx.request_refresh();
            }
            RecoveryRequest::None => {}
        }
        if servidor.is_some() && !avisou_registro {
            let s = rx.stats();
            if let Some(publico) = s.public_addr {
                println!("Registrado no servidor de sinalização; endereço público {publico}");
                avisou_registro = true;
            } else if s.room_taken {
                return Err(
                    "o servidor recusou a sala deste código (outro cliente nela); use outro código".into(),
                );
            }
        }
        let recebido = rx.recv_frame(Duration::from_millis(20))?;

        if let Some(f) = recebido {
            if inicio.is_none() {
                let de = rx.stats().peer.map(|p| p.to_string()).unwrap_or_default();
                println!("Recebendo de {de}");
                inicio = Some(Instant::now());
            }
            // O arquivo só pode começar num keyframe. Depois de uma perda,
            // grava tudo: quem abrir o arquivo vê o estrago até a onda de
            // intra-refresh (ou o keyframe) pedida limpar a imagem.
            let sujo_desde = recuperacao.dirty_since();
            match recuperacao.on_frame(&f.frame, Instant::now()) {
                FrameAction::Skip => {
                    pulados += 1;
                    continue;
                }
                FrameAction::Decode { show } => sujos += !show as u64,
            }
            if let (Some(t), true) = (sujo_desde, recuperacao.is_clean()) {
                if ja_limpo {
                    congelados.push(t.elapsed().as_secs_f64() * 1000.0);
                }
                ja_limpo = true;
            }
            arquivo.write_all(&f.frame.data)?;
            if let Some(d) = &mut decoder {
                decodificados += d.decode(&f.frame.data)?.len() as u64;
            }
            frames += 1;
            bytes += f.frame.data.len() as u64;
            keyframes += f.frame.keyframe as u64;
            frames_seg += 1;
            bytes_seg += f.frame.data.len() as u64;
            if let Some(l) = f.latency_us {
                latencias.push(l);
                latencias_seg.push(l);
            }
        }

        if let Some(t0) = inicio {
            if t0.elapsed() >= proximo_relatorio {
                let s = rx.stats();
                println!(
                    "  {:>3} s: {frames_seg} fps, {:.1} Mbps, latência {}, perdidos {}, RTT {:.2} ms",
                    proximo_relatorio.as_secs(),
                    bytes_seg as f64 * 8.0 / 1e6,
                    match latencias_seg.iter().max() {
                        Some(&max) => format!(
                            "{:.1} ms (máx {:.1})",
                            latencias_seg.iter().map(|&l| l as f64).sum::<f64>()
                                / latencias_seg.len() as f64
                                / 1000.0,
                            max as f64 / 1000.0
                        ),
                        None if s.clock_offset_ms.is_none() => "— (sincronizando relógios)".to_string(),
                        None => "—".to_string(),
                    },
                    s.reassembly.frames_lost,
                    s.rtt.smoothed_ms
                );
                (frames_seg, bytes_seg) = (0, 0);
                latencias_seg.clear();
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
    latencias.sort_unstable();
    let pct = |p: f64| {
        latencias
            .get(((latencias.len().max(1) - 1) as f64 * p).round() as usize)
            .map(|&us| us as f64 / 1000.0)
            .unwrap_or(0.0)
    };
    println!("\nResultado");
    println!(
        "  frames gravados:          {frames} ({keyframes} keyframes), {pulados} pulados esperando keyframe"
    );
    println!(
        "  bitrate médio:            {:.2} Mbps",
        bytes as f64 * 8.0 / total.max(0.001) / 1e6
    );
    println!(
        "  pacotes: {} recebidos, {} duplicados, {} atrasados, {} inválidos",
        r.packets_received, r.duplicate_packets, r.late_packets, s.invalid_packets
    );
    println!(
        "  frames perdidos na rede:  {} ({} fragmentos reconstruídos pelo FEC; pedidos: {} de limpeza, {} de keyframe)",
        r.frames_lost, r.fec_recovered, s.refresh_requests, s.keyframe_requests
    );
    if r.frames_lost > 0 {
        println!(
            "  grupos de FEC que falharam: {} com 2 pacotes faltando, {} com 3 ou mais; {} frames sem paridade nenhuma",
            r.fec_groups_failed_2, r.fec_groups_failed_3plus, r.frames_abandoned_without_parity
        );
    }
    if !congelados.is_empty() {
        println!(
            "  recuperações:             {} ({} frames com estrago até limpar), média {:.1} ms, máx {:.1} ms",
            congelados.len(),
            sujos,
            congelados.iter().sum::<f64>() / congelados.len() as f64,
            congelados.iter().copied().fold(0.0, f64::max)
        );
    }
    if s.frames_dropped > 0 {
        println!(
            "  descartados na fila:      {} (este programa não leu a tempo)",
            s.frames_dropped
        );
    }
    match (s.clock_offset_ms, s.clock_error_ms) {
        (Some(dif), Some(erro)) => {
            println!(
                "  latência captura→chegada: p50 {:.1} ms, p99 {:.1} ms, máx {:.1} ms ({} frames)",
                pct(0.5),
                pct(0.99),
                pct(1.0),
                latencias.len()
            );
            println!(
                "  relógios:                 host {dif:+.1} ms em relação a este PC \
                 (sincronizados pelos pings, erro ±{erro:.1} ms)"
            );
        }
        _ => println!("  latência:                 — (relógios não sincronizados: nenhum ping respondido)"),
    }
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

/// Lê do terminal os códigos de resposta (numa thread: a medição segue) e
/// fura o NAT na direção de cada um.
fn ler_respostas(furos: Puncher) {
    std::thread::spawn(move || {
        for linha in std::io::stdin().lines() {
            let Ok(linha) = linha else {
                return; // sem terminal
            };
            if linha.trim().is_empty() {
                continue;
            }
            match ReplyCode::parse(&linha) {
                Ok(r) => {
                    furos.punch(&[std::net::SocketAddr::V4(r.addr)]);
                    println!("Abrindo caminho até {}…", r.addr);
                }
                Err(e) => println!("{e}. Cole de novo:"),
            }
        }
    });
}
