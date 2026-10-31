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

use fluxo_net::{clock_us, ConnectionCode, PairingCode, SenderConfig, VideoSender};

use crate::Opts;

pub fn cmd_enviar(o: &Opts) -> Result<(), Box<dyn Error>> {
    let servidor = o
        .sinal
        .as_deref()
        .map(fluxo_net::resolve_signal_server)
        .transpose()?;
    let conexao = o.conexao.as_deref().map(ConnectionCode::parse).transpose()?;
    let modos = o.destino.is_some() as u8 + servidor.is_some() as u8 + conexao.is_some() as u8;
    if modos != 1 {
        return Err(
            "enviar precisa de um destino: --destino IP:PORTA, --sinal SERVIDOR ou --conexao CÓDIGO".into(),
        );
    }
    let bitrate_bps = (o.mbps * 1e6).round() as u64;
    let fps = o.fps.max(1);
    let tamanho = (bitrate_bps / 8 / fps as u64).max(1) as usize;
    let pairing_code = match (&o.codigo, o.sem_cifra, &conexao) {
        (None, false, Some(cc)) => Some(cc.pairing.clone()),
        (_, _, Some(_)) => return Err("--conexao já traz o código: sem --codigo nem --sem-cifra".into()),
        (_, true, None) => None,
        (Some(c), false, None) => Some(PairingCode::parse(c)?),
        (None, false, None) => {
            return Err("enviar precisa de --codigo (mostrado pelo receber) ou --sem-cifra".into())
        }
    };
    let cfg = SenderConfig {
        fec_group: o.fec,
        pairing_code,
        // Com código de conexão, o de resposta ainda vai e volta pelo chat.
        pairing_timeout: Duration::from_secs(if conexao.is_some() { 300 } else { 10 }),
        ..SenderConfig::for_bitrate(bitrate_bps)
    };
    let pacing = cfg.pacing_bps;
    let tx = match (o.destino.as_deref(), servidor, conexao) {
        (Some(d), ..) => VideoSender::connect("0.0.0.0:0", d, cfg)?,
        (None, Some(s), _) => {
            println!("Procurando o receber pelo servidor de sinalização {s}…");
            VideoSender::connect_via_signal("0.0.0.0:0", s, cfg)?
        }
        (None, None, Some(cc)) => {
            let internet = fluxo_net::Internet::open(0, false)?;
            println!("Código de resposta: {}", internet.reply_code(&cc)?);
            println!("  Mande para quem roda o receber (ele cola no terminal). Esperando até 5 min…");
            for linha in internet.summary() {
                println!("  {linha}");
            }
            // Porta aberta por UPnP do outro lado: dá para ir direto, sem
            // esperar o furo (nada para ocupar a porta lá).
            let ttl = (o.ttl_furo > 0 && !cc.open).then_some(o.ttl_furo);
            match (ttl, cc.open) {
                (_, true) => println!("  O receber abriu a porta no roteador (UPnP): conectando direto"),
                (Some(t), _) => {
                    println!("  Pedidos IPv4 com TTL {t} até o furo do outro lado chegar (--ttl-furo)")
                }
                (None, _) => {}
            }
            let cfg = SenderConfig {
                on_pairing: Some(fluxo_net::PairingObserver::print_to_terminal()),
                punch_ttl: ttl,
                ..cfg
            };
            VideoSender::connect_with_sockets(internet.v4, internet.v6, cc.candidates(), cfg)?
        }
        (None, None, None) => unreachable!("conferido acima"),
    };
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
