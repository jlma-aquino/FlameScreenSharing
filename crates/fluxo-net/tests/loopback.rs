//! Testes com sockets de verdade em 127.0.0.1: host e cliente no mesmo processo.

use std::time::{Duration, Instant};

use fluxo_net::{clock_us, ReceiverConfig, SenderConfig, VideoReceiver, VideoSender};

/// Conteúdo reconhecível: cada frame tem bytes derivados do seu número.
fn frame(n: usize, len: usize) -> Vec<u8> {
    (0..len).map(|i| (n * 31 + i * 7) as u8).collect()
}

fn par(pacing_bps: u64, ping: Duration) -> (VideoReceiver, VideoSender) {
    let rx = VideoReceiver::bind(
        "127.0.0.1:0",
        ReceiverConfig {
            ping_interval: ping,
            queue_frames: 64,
            ..ReceiverConfig::default()
        },
    )
    .expect("bind");
    let tx = VideoSender::connect(
        "127.0.0.1:0",
        rx.local_addr(),
        SenderConfig {
            pacing_bps,
            ping_interval: ping,
            ..SenderConfig::for_bitrate(20_000_000)
        },
    )
    .expect("connect");
    (rx, tx)
}

#[test]
fn frames_chegam_inteiros_e_em_ordem() {
    let (rx, tx) = par(200_000_000, Duration::from_millis(500));
    let tamanhos: Vec<usize> = (0..30)
        .map(|n| if n % 10 == 0 { 120_000 } else { 30_000 + n * 100 })
        .collect();
    for (n, &len) in tamanhos.iter().enumerate() {
        tx.send_frame(frame(n, len), n % 10 == 0, clock_us()).unwrap();
    }

    let mut recebidos = Vec::new();
    let limite = Instant::now() + Duration::from_secs(5);
    while recebidos.len() < tamanhos.len() && Instant::now() < limite {
        if let Some(f) = rx.recv_frame(Duration::from_millis(100)).unwrap() {
            recebidos.push(f);
        }
    }

    assert_eq!(recebidos.len(), tamanhos.len(), "stats: {:?}", rx.stats());
    for (n, f) in recebidos.iter().enumerate() {
        assert_eq!(f.frame.data, frame(n, tamanhos[n]), "frame {n} corrompido");
        assert_eq!(f.frame.keyframe, n % 10 == 0);
        assert!(!f.frame.preceded_by_loss);
        // Mesma máquina: o trânsito é positivo e pequeno.
        assert!(
            (0..1_000_000).contains(&f.transit_us),
            "trânsito {} µs",
            f.transit_us
        );
    }
    let s = tx.stats();
    assert_eq!(s.frames_sent, 30);
    assert_eq!(s.send_errors, 0);
}

#[test]
fn pacing_espalha_o_frame_no_tempo() {
    // 10 Mbps = 1,25 MB/s. Um frame de 150 KB, menos a rajada inicial de
    // ~12 KB, precisa de ~110 ms para sair.
    let (rx, tx) = par(10_000_000, Duration::from_secs(10));
    tx.send_frame(frame(0, 150_000), true, clock_us()).unwrap();
    let f = rx.recv_frame(Duration::from_secs(2)).unwrap().expect("frame");
    assert_eq!(f.frame.data.len(), 150_000);

    // O frame pode completar no cliente antes de o host terminar de enviar
    // (a última paridade de FEC sai depois do último dado): espera o envio.
    let limite = Instant::now() + Duration::from_secs(2);
    while tx.stats().frames_sent == 0 && Instant::now() < limite {
        std::thread::sleep(Duration::from_millis(5));
    }
    let ms = tx.stats().last_frame_send_ms;
    assert!((90.0..400.0).contains(&ms), "frame levou {ms:.1} ms para sair");
}

#[test]
fn rtt_medido_nos_dois_lados() {
    let (rx, tx) = par(0, Duration::from_millis(20));
    // O cliente só conhece o host depois do primeiro pacote.
    tx.send_frame(frame(0, 1_000), true, clock_us()).unwrap();
    let limite = Instant::now() + Duration::from_secs(3);
    while (tx.stats().rtt.samples < 3 || rx.stats().rtt.samples < 3) && Instant::now() < limite {
        let _ = rx.recv_frame(Duration::from_millis(20));
    }
    let (h, c) = (tx.stats(), rx.stats());
    assert!(h.rtt.samples >= 3, "host: {h:?}");
    assert!(c.rtt.samples >= 3, "cliente: {c:?}");
    assert!(h.rtt.smoothed_ms < 50.0 && c.rtt.smoothed_ms < 50.0);
    assert!(c.pings_answered >= 3 && h.pings_answered >= 3);
}

#[test]
fn destruir_o_sender_envia_o_que_estava_na_fila() {
    let (rx, tx) = par(50_000_000, Duration::from_secs(10));
    for n in 0..5 {
        tx.send_frame(frame(n, 40_000), n == 0, clock_us()).unwrap();
    }
    drop(tx); // espera a fila esvaziar
    let mut n = 0;
    while rx.recv_frame(Duration::from_millis(300)).unwrap().is_some() {
        n += 1;
    }
    assert_eq!(n, 5);
}

#[test]
fn host_novo_e_aceito_depois_do_timeout() {
    let rx = VideoReceiver::bind(
        "127.0.0.1:0",
        ReceiverConfig {
            peer_timeout: Duration::from_millis(200),
            ..ReceiverConfig::default()
        },
    )
    .unwrap();
    let cfg = SenderConfig::for_bitrate(20_000_000);
    {
        let a = VideoSender::connect("127.0.0.1:0", rx.local_addr(), cfg.clone()).unwrap();
        for n in 0..3 {
            a.send_frame(frame(n, 5_000), n == 0, clock_us()).unwrap();
        }
    }
    for _ in 0..3 {
        rx.recv_frame(Duration::from_secs(1))
            .unwrap()
            .expect("frame do host A");
    }
    std::thread::sleep(Duration::from_millis(300));

    // Host "reiniciado": outra porta, numeração de frames do zero.
    let b = VideoSender::connect("127.0.0.1:0", rx.local_addr(), cfg).unwrap();
    b.send_frame(frame(0, 5_000), true, clock_us()).unwrap();
    let f = rx
        .recv_frame(Duration::from_secs(1))
        .unwrap()
        .expect("frame do host B");
    assert_eq!(f.frame.data, frame(0, 5_000));
    assert_eq!(rx.stats().peer_changes, 1);
}

#[test]
fn pedido_de_keyframe_chega_ao_host_uma_vez() {
    let (rx, tx) = par(0, Duration::from_secs(10));
    // Antes de conhecer o host, não há para quem pedir.
    assert!(!rx.request_keyframe());
    tx.send_frame(frame(0, 1_000), true, clock_us()).unwrap();
    rx.recv_frame(Duration::from_secs(1)).unwrap().expect("frame");

    assert!(rx.request_keyframe(), "primeiro pedido deve sair");
    assert!(
        !rx.request_keyframe(),
        "pedido repetido logo em seguida é segurado"
    );

    let limite = Instant::now() + Duration::from_secs(2);
    while !tx.take_keyframe_request() {
        assert!(Instant::now() < limite, "pedido não chegou: {:?}", tx.stats());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!tx.take_keyframe_request(), "o pedido é consumido uma vez só");
    assert_eq!(tx.stats().keyframe_requests, 1);
    assert_eq!(rx.stats().keyframe_requests, 1);

    // Passado o intervalo mínimo, pode pedir de novo.
    std::thread::sleep(Duration::from_millis(60));
    assert!(rx.request_keyframe());
}

#[test]
fn perda_simulada_descarta_pacotes_e_o_receptor_percebe() {
    let rx = VideoReceiver::bind("127.0.0.1:0", ReceiverConfig::default()).unwrap();
    let tx = VideoSender::connect(
        "127.0.0.1:0",
        rx.local_addr(),
        SenderConfig {
            pacing_bps: 0,
            simulated_loss: 0.2,
            ..SenderConfig::for_bitrate(20_000_000)
        },
    )
    .unwrap();
    for n in 0..40 {
        tx.send_frame(frame(n, 12_000), n == 0, clock_us()).unwrap();
    }
    let mut completos = 0;
    while let Some(f) = rx.recv_frame(Duration::from_millis(300)).unwrap() {
        assert_eq!(
            f.frame.data,
            frame(f.frame.frame_id as usize, 12_000),
            "frame remontado errado"
        );
        completos += 1;
    }
    let (s, r) = (tx.stats(), rx.stats());
    // 40 frames × 10 pacotes a 20% de perda: ~80 descartes; quase todo
    // frame perde algum pacote (só ~11% saem inteiros).
    assert!(s.simulated_drops > 30, "{s:?}");
    assert!(completos < 20, "{completos} frames completos");
    assert!(r.reassembly.frames_lost > 0);
}

/// Envia 60 frames de 36 KB (30 pacotes) com perda simulada; devolve
/// (frames completos, fragmentos reconstruídos pelo FEC).
fn completos_com_perda(fec_group: u8, perda: f64) -> (usize, u64) {
    let rx = VideoReceiver::bind(
        "127.0.0.1:0",
        ReceiverConfig {
            queue_frames: 128,
            ..ReceiverConfig::default()
        },
    )
    .unwrap();
    let tx = VideoSender::connect(
        "127.0.0.1:0",
        rx.local_addr(),
        SenderConfig {
            pacing_bps: 200_000_000,
            simulated_loss: perda,
            fec_group,
            ..SenderConfig::for_bitrate(20_000_000)
        },
    )
    .unwrap();
    for n in 0..60 {
        tx.send_frame(frame(n, 36_000), n == 0, clock_us()).unwrap();
    }
    let mut completos = 0;
    while let Some(f) = rx.recv_frame(Duration::from_millis(300)).unwrap() {
        assert_eq!(
            f.frame.data,
            frame(f.frame.frame_id as usize, 36_000),
            "frame errado"
        );
        completos += 1;
    }
    (completos, rx.stats().reassembly.fec_recovered)
}

#[test]
fn fec_salva_a_maioria_dos_frames_com_perda() {
    // 3% de perda, 30 pacotes por frame: sem FEC só ~40% saem inteiros;
    // com grupos de 10, ~88%.
    let (sem, rec_sem) = completos_com_perda(0, 0.03);
    let (com, rec_com) = completos_com_perda(10, 0.03);
    assert_eq!(rec_sem, 0);
    assert!(rec_com > 0, "o FEC não reconstruiu nada");
    assert!(com >= sem + 12, "sem FEC: {sem}/60, com FEC: {com}/60");
}
