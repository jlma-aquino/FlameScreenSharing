//! Testes com sockets de verdade em 127.0.0.1: host e cliente no mesmo processo.

use std::time::{Duration, Instant};

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use fluxo_net::{
    clock_us, NetError, PairingCode, PairingEvent, PairingObserver, ReceiverConfig, SenderConfig,
    SignalServer, VideoReceiver, VideoSender,
};

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
    // Antes de conhecer um host, não há para quem pedir. (Com o par criado,
    // o ping de largada do host pode já ter chegado.)
    let sozinho = VideoReceiver::bind("127.0.0.1:0", ReceiverConfig::default()).unwrap();
    assert!(!sozinho.request_keyframe());

    let (rx, tx) = par(0, Duration::from_secs(10));
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
            // Sem FEC: cada pacote perdido estraga o frame (com FEC, ~31%
            // se salvariam e o limite abaixo falharia de vez em quando).
            fec_group: 0,
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
    // 40 frames × 11 pacotes a 20% de perda: ~88 descartes; quase todo
    // frame perde algum pacote (só ~9% saem inteiros).
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

/// Manda frames num ritmo de 60 fps por `duracao` enquanto o cliente lê.
fn transmitir(tx: &VideoSender, rx: &VideoReceiver, bytes: usize, duracao: Duration) {
    let inicio = Instant::now();
    let mut n = 0;
    while inicio.elapsed() < duracao {
        tx.send_frame(frame(n, bytes), n == 0, clock_us()).unwrap();
        n += 1;
        let alvo = inicio + Duration::from_micros(16_667 * n as u64);
        while Instant::now() < alvo {
            let _ = rx.recv_frame(Duration::from_millis(1));
        }
    }
}

#[test]
fn relatorio_do_cliente_mede_a_perda() {
    let rx = VideoReceiver::bind("127.0.0.1:0", ReceiverConfig::default()).unwrap();
    let tx = VideoSender::connect(
        "127.0.0.1:0",
        rx.local_addr(),
        SenderConfig {
            pacing_bps: 0,
            simulated_loss: 0.10,
            fec_group: 0,
            ..SenderConfig::for_bitrate(20_000_000)
        },
    )
    .unwrap();
    transmitir(&tx, &rx, 12_000, Duration::from_millis(1500));
    let s = tx.stats();
    assert!(s.feedbacks_received >= 3, "{s:?}");
    let perda = s.last_loss.expect("perda calculada");
    assert!(
        (0.04..0.18).contains(&perda),
        "perda medida {perda:.3}, simulada 0,10"
    );
    assert!(tx.take_report().is_some());
    assert!(tx.take_report().is_none(), "o relatório é consumido uma vez");
}

#[test]
fn banda_simulada_gera_fila_e_descartes() {
    let rx = VideoReceiver::bind("127.0.0.1:0", ReceiverConfig::default()).unwrap();
    let tx = VideoSender::connect(
        "127.0.0.1:0",
        rx.local_addr(),
        SenderConfig {
            // 25 KB a 60 fps = 12 Mbps num link de 4 Mbps.
            simulated_bandwidth_bps: 4_000_000,
            ..SenderConfig::for_bitrate(12_000_000)
        },
    )
    .unwrap();
    transmitir(&tx, &rx, 25_000, Duration::from_millis(1500));
    let (h, c) = (tx.stats(), rx.stats());
    assert!(
        h.simulated_drops > 0,
        "a fila do roteador simulado deveria transbordar: {h:?}"
    );
    assert!(
        c.queue_delay_ms > 20.0,
        "atraso de fila {:.1} ms",
        c.queue_delay_ms
    );
}

#[test]
fn relogios_sincronizados_na_mesma_maquina_dao_diferenca_zero() {
    let (rx, tx) = par(0, Duration::from_millis(20));
    tx.send_frame(frame(0, 1_000), true, clock_us()).unwrap();
    let limite = Instant::now() + Duration::from_secs(3);
    while (rx.stats().clock_offset_ms.is_none() || tx.stats().clock_offset_ms.is_none())
        && Instant::now() < limite
    {
        let _ = rx.recv_frame(Duration::from_millis(20));
    }
    let (c, h) = (rx.stats(), tx.stats());
    // Mesmo relógio dos dois lados: a diferença medida é ~0.
    let oc = c.clock_offset_ms.expect("cliente mediu");
    let oh = h.clock_offset_ms.expect("host mediu");
    assert!(oc.abs() < 2.0 && oh.abs() < 2.0, "diferenças {oc} / {oh} ms");
    assert!(c.clock_error_ms.unwrap() < 5.0);

    // Com a diferença conhecida, cada frame traz a latência corrigida.
    tx.send_frame(frame(1, 1_000), false, clock_us()).unwrap();
    let f = rx.recv_frame(Duration::from_secs(1)).unwrap().expect("frame");
    let lat = f.latency_us.expect("latência corrigida");
    assert!((lat - f.transit_us).abs() < 2_000, "{lat} vs {}", f.transit_us);
}

#[test]
fn atraso_simulado_vale_nos_dois_sentidos() {
    let rx = VideoReceiver::bind(
        "127.0.0.1:0",
        ReceiverConfig {
            ping_interval: Duration::from_millis(50),
            ..ReceiverConfig::default()
        },
    )
    .unwrap();
    let tx = VideoSender::connect(
        "127.0.0.1:0",
        rx.local_addr(),
        SenderConfig {
            ping_interval: Duration::from_millis(50),
            simulated_delay: Duration::from_millis(25),
            ..SenderConfig::for_bitrate(20_000_000)
        },
    )
    .unwrap();
    let limite = Instant::now() + Duration::from_secs(3);
    let mut latencias = Vec::new();
    let mut n = 0;
    while latencias.len() < 10 && Instant::now() < limite {
        tx.send_frame(frame(n, 5_000), n == 0, clock_us()).unwrap();
        n += 1;
        // Os primeiros frames esperam a largada do host (até 50 ms pelo
        // primeiro pong), então ficam de fora.
        if let Some(f) = rx.recv_frame(Duration::from_millis(30)).unwrap() {
            if f.frame.frame_id >= 5 {
                latencias.extend(f.latency_us);
            }
        }
    }
    assert!(latencias.len() >= 10, "frames com latência: {}", latencias.len());
    // Ida: ~25 ms. O atraso é igual nos dois sentidos, então a
    // sincronização dos relógios continua certa.
    for l in &latencias {
        assert!((24_000..40_000).contains(l), "latência {l} µs");
    }
    // RTT: ~50 ms dos dois lados.
    let (rc, rh) = (rx.stats().rtt.min_ms, tx.stats().rtt.min_ms);
    assert!((49.0..65.0).contains(&rc), "RTT do cliente {rc} ms");
    assert!((49.0..65.0).contains(&rh), "RTT do host {rh} ms");
}

#[test]
fn jitter_simulado_aparece_como_atraso_de_fila() {
    let rx = VideoReceiver::bind("127.0.0.1:0", ReceiverConfig::default()).unwrap();
    let tx = VideoSender::connect(
        "127.0.0.1:0",
        rx.local_addr(),
        SenderConfig {
            simulated_jitter: Duration::from_millis(40),
            ..SenderConfig::for_bitrate(20_000_000)
        },
    )
    .unwrap();
    let mut transitos = Vec::new();
    let inicio = Instant::now();
    let mut n = 0;
    while inicio.elapsed() < Duration::from_millis(1500) {
        tx.send_frame(frame(n, 5_000), n == 0, clock_us()).unwrap();
        n += 1;
        let ate = Instant::now() + Duration::from_millis(16);
        while let Some(f) = rx
            .recv_frame(ate.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            // Fora a largada (ver o teste acima).
            if f.frame.frame_id >= 10 {
                transitos.push(f.transit_us);
            }
        }
    }
    transitos.sort_unstable();
    let (min, max) = (transitos[0], transitos[transitos.len() - 1]);
    assert!(transitos.len() > 50, "frames: {}", transitos.len());
    assert!(max - min > 20_000, "o trânsito deveria variar: {min}..{max} µs");
    assert!(max - min < 60_000, "variação acima do jitter: {min}..{max} µs");
}

#[test]
fn pedido_de_refresh_e_marcas_da_onda() {
    let (rx, tx) = par(0, Duration::from_millis(500));
    tx.send_frame(frame(0, 1_000), true, clock_us()).unwrap();
    rx.recv_frame(Duration::from_secs(1)).unwrap().expect("frame 0");

    // Perda: o cliente pede limpeza, e o host vê um pedido de refresh, não de keyframe.
    assert!(rx.request_refresh());
    let limite = Instant::now() + Duration::from_secs(2);
    while !tx.take_refresh_request() {
        assert!(Instant::now() < limite, "pedido de refresh não chegou");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!tx.take_keyframe_request());
    assert_eq!(
        (tx.stats().refresh_requests, tx.stats().keyframe_requests),
        (1, 0)
    );

    // A onda chega marcada no início e no fim.
    let marcas = [fluxo_proto::FLAG_REFRESH_START, 0, fluxo_proto::FLAG_REFRESH_END];
    for (i, flags) in marcas.into_iter().enumerate() {
        tx.send_frame_with_flags(frame(i + 1, 3_000), flags, clock_us())
            .unwrap();
    }
    let recebidos: Vec<_> = (0..3)
        .map(|_| {
            rx.recv_frame(Duration::from_secs(1))
                .unwrap()
                .expect("frame da onda")
                .frame
        })
        .collect();
    assert!(recebidos[0].refresh_start && !recebidos[0].refresh_end);
    assert!(!recebidos[1].refresh_start && !recebidos[1].refresh_end);
    assert!(recebidos[2].refresh_end && !recebidos[2].keyframe);
}

fn codigo(s: &str) -> PairingCode {
    PairingCode::parse(s).unwrap()
}

fn par_cifrado(host: &str, cliente: Option<&str>) -> (VideoReceiver, Result<VideoSender, NetError>) {
    let rx = VideoReceiver::bind(
        "127.0.0.1:0",
        ReceiverConfig {
            ping_interval: Duration::from_millis(50),
            pairing_code: cliente.map(codigo),
            ..ReceiverConfig::default()
        },
    )
    .unwrap();
    let tx = VideoSender::connect(
        "127.0.0.1:0",
        rx.local_addr(),
        SenderConfig {
            ping_interval: Duration::from_millis(50),
            pairing_code: Some(codigo(host)),
            pairing_timeout: Duration::from_secs(1),
            ..SenderConfig::for_bitrate(20_000_000)
        },
    );
    (rx, tx)
}

#[test]
fn com_o_mesmo_codigo_tudo_passa_cifrado() {
    let (rx, tx) = par_cifrado("K7M4-QX9P", Some("k7m4 qx9p"));
    let tx = tx.expect("pareamento");
    assert!(tx.stats().encrypted);
    for n in 0..5 {
        tx.send_frame(frame(n, 20_000), n == 0, clock_us()).unwrap();
        let f = rx.recv_frame(Duration::from_secs(1)).unwrap().expect("frame");
        assert_eq!(f.frame.data, frame(n, 20_000));
    }
    // Pings, pongs e relatórios também passam (cifrados) nos dois sentidos.
    let limite = Instant::now() + Duration::from_secs(2);
    while (rx.stats().rtt.samples == 0 || tx.stats().rtt.samples == 0 || tx.stats().feedbacks_received == 0)
        && Instant::now() < limite
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    let (c, h) = (rx.stats(), tx.stats());
    assert!(c.encrypted && c.pairings >= 1);
    assert!(
        c.rtt.samples > 0 && h.rtt.samples > 0 && h.feedbacks_received > 0,
        "{c:?} {h:?}"
    );
    assert_eq!(h.rejected_packets, 0);
}

#[test]
fn codigo_errado_e_recusado_na_hora() {
    let inicio = Instant::now();
    let (rx, tx) = par_cifrado("K7M4-QX9P", Some("K7M4-QX9Q"));
    let erro = tx.err().expect("não deveria parear");
    assert!(erro.to_string().contains("não confere"), "{erro}");
    assert!(
        inicio.elapsed() < Duration::from_millis(900),
        "devia falhar antes do prazo"
    );
    assert!(!rx.stats().encrypted);
}

#[test]
fn cliente_sem_codigo_nao_pareia() {
    let (rx, tx) = par_cifrado("K7M4-QX9P", None);
    assert!(matches!(tx, Err(NetError::Pairing(_))));
    assert!(rx.stats().invalid_packets > 0, "PairInit em claro é recusado");
}

#[test]
fn intruso_nao_atrapalha_a_sessao() {
    let (rx, tx) = par_cifrado("K7M4-QX9P", Some("K7M4-QX9P"));
    let tx = tx.unwrap();
    tx.send_frame(frame(0, 5_000), true, clock_us()).unwrap();
    rx.recv_frame(Duration::from_secs(1)).unwrap().expect("frame 0");

    // Um intruso sem o código: pede pareamento e manda vídeo em claro.
    let intruso = VideoSender::connect(
        "127.0.0.1:0",
        rx.local_addr(),
        SenderConfig {
            pairing_code: Some(codigo("ZZZZ-ZZZZ")),
            pairing_timeout: Duration::from_millis(500),
            ..SenderConfig::for_bitrate(20_000_000)
        },
    );
    assert!(intruso.is_err(), "o intruso não pode parear");
    let em_claro = VideoSender::connect(
        "127.0.0.1:0",
        rx.local_addr(),
        SenderConfig::for_bitrate(20_000_000),
    )
    .unwrap();
    em_claro.send_frame(frame(99, 5_000), true, clock_us()).unwrap();
    drop(em_claro);

    // O host de verdade continua sendo ouvido; nada do intruso aparece.
    for n in 1..4 {
        tx.send_frame(frame(n, 5_000), false, clock_us()).unwrap();
        let f = rx
            .recv_frame(Duration::from_secs(1))
            .unwrap()
            .expect("frame do host");
        assert_eq!(f.frame.data, frame(n, 5_000));
    }
    assert_eq!(rx.stats().peer_changes, 0);
    assert!(rx.stats().invalid_packets > 0);
}

/// Servidor de sinalização numa thread, em 127.0.0.1; para ao sair de escopo.
struct Servidor {
    endereco: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Servidor {
    fn novo() -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let endereco = socket.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::spawn(move || SignalServer::default().run(&socket, &s, |_| {}));
        Self {
            endereco,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Servidor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn cliente_no_servidor(servidor: SocketAddr, texto: &str) -> VideoReceiver {
    VideoReceiver::bind(
        "127.0.0.1:0",
        ReceiverConfig {
            pairing_code: Some(codigo(texto)),
            signal_server: Some(servidor),
            ..ReceiverConfig::default()
        },
    )
    .unwrap()
}

fn host_pelo_servidor(servidor: SocketAddr, texto: &str, prazo: Duration) -> Result<VideoSender, NetError> {
    VideoSender::connect_via_signal(
        "127.0.0.1:0",
        servidor,
        SenderConfig {
            pairing_code: Some(codigo(texto)),
            pairing_timeout: prazo,
            ..SenderConfig::for_bitrate(20_000_000)
        },
    )
}

#[test]
fn host_acha_o_cliente_pelo_servidor_de_sinalizacao() {
    let srv = Servidor::novo();
    let rx = cliente_no_servidor(srv.endereco, "K7M4-QX9P");
    let limite = Instant::now() + Duration::from_secs(2);
    while rx.stats().public_addr.is_none() && Instant::now() < limite {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        rx.stats().public_addr,
        Some(rx.local_addr()),
        "o servidor vê o nosso endereço"
    );

    let tx = host_pelo_servidor(srv.endereco, "K7M4-QX9P", Duration::from_secs(3)).expect("pareamento");
    assert_eq!(tx.peer_addr(), rx.local_addr());
    for n in 0..5 {
        tx.send_frame(frame(n, 20_000), n == 0, clock_us()).unwrap();
        let f = rx.recv_frame(Duration::from_secs(1)).unwrap().expect("frame");
        assert_eq!(f.frame.data, frame(n, 20_000));
    }
    let c = rx.stats();
    assert!(c.encrypted && c.punches_sent > 0, "{c:?}");
}

#[test]
fn sala_vazia_e_codigo_errado_pelo_servidor() {
    let srv = Servidor::novo();
    let erro = host_pelo_servidor(srv.endereco, "K7M4-QX9P", Duration::from_millis(600))
        .err()
        .expect("ninguém na sala");
    assert!(erro.to_string().contains("nenhum cliente"), "{erro}");

    // Outro código cai em outra sala: o host nem acha o cliente.
    let _rx = cliente_no_servidor(srv.endereco, "K7M4-QX9P");
    std::thread::sleep(Duration::from_millis(100));
    let erro = host_pelo_servidor(srv.endereco, "K7M4-QX9Q", Duration::from_millis(600))
        .err()
        .expect("sala de outro código");
    assert!(erro.to_string().contains("nenhum cliente"), "{erro}");
}

#[test]
fn servidor_mudo_e_sinalizacao_sem_codigo() {
    // Porta sem ninguém: o host desiste com um motivo claro.
    let mudo = UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let erro = host_pelo_servidor(mudo, "K7M4-QX9P", Duration::from_millis(600))
        .err()
        .expect("sem servidor");
    assert!(erro.to_string().contains("não respondeu"), "{erro}");

    let sem_codigo = ReceiverConfig {
        signal_server: Some(mudo),
        ..ReceiverConfig::default()
    };
    assert!(matches!(
        VideoReceiver::bind("127.0.0.1:0", sem_codigo),
        Err(NetError::Pairing(_))
    ));
}

#[test]
fn impostor_com_outro_codigo_nao_derruba_o_pareamento_pelo_servidor() {
    use fluxo_proto::{PacketHeader, PacketKind};

    let srv = Servidor::novo();
    // Host numa porta conhecida, para o impostor furar até ela.
    let host = UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let servidor = srv.endereco;
    let tx = std::thread::spawn(move || host_pelo_servidor_em(host, servidor, "K7M4-QX9P"));

    // Impostor: outro cliente Fluxo (outro código) num endereço que o host
    // conhece pelo furo. Responde ao PairInit com a chave dele.
    let impostor = UdpSocket::bind("127.0.0.1:0").unwrap();
    impostor
        .set_read_timeout(Some(Duration::from_millis(10)))
        .unwrap();
    let furo = PacketKind::Punch.control(0, 0).encode_with_payload(&[]).unwrap();
    let mut buf = [0u8; 2048];
    let limite = Instant::now() + Duration::from_secs(2);
    let mut respondeu = false;
    while !respondeu && Instant::now() < limite {
        impostor.send_to(&furo, host).unwrap();
        let Ok((n, de)) = impostor.recv_from(&mut buf) else {
            continue;
        };
        if let Ok((h, init)) = PacketHeader::decode(&buf[..n]) {
            if h.kind == PacketKind::PairInit {
                let (_, resposta) = fluxo_crypto::respond(&codigo("K7M4-QX9Q"), init).unwrap();
                let pacote = PacketKind::PairReply
                    .control(h.frame_id, 0)
                    .encode_with_payload(&resposta)
                    .unwrap();
                impostor.send_to(&pacote, de).unwrap();
                respondeu = true;
            }
        }
    }
    assert!(respondeu, "o host devia ter tentado o impostor");

    // Só agora o cliente de verdade aparece no servidor.
    let rx = cliente_no_servidor(servidor, "K7M4-QX9P");
    let tx = tx.join().unwrap().expect("pareia com o cliente certo");
    assert_eq!(tx.peer_addr(), rx.local_addr());
    tx.send_frame(frame(0, 5_000), true, clock_us()).unwrap();
    let f = rx.recv_frame(Duration::from_secs(1)).unwrap().expect("frame");
    assert_eq!(f.frame.data, frame(0, 5_000));
}

fn host_pelo_servidor_em(
    bind: SocketAddr,
    servidor: SocketAddr,
    texto: &str,
) -> Result<VideoSender, NetError> {
    VideoSender::connect_via_signal(
        bind,
        servidor,
        SenderConfig {
            pairing_code: Some(codigo(texto)),
            pairing_timeout: Duration::from_secs(4),
            ..SenderConfig::for_bitrate(20_000_000)
        },
    )
}

#[test]
fn codigo_de_conexao_host_aprende_o_endereco_pelo_furo() {
    let (no_cliente, no_host): (Eventos, Eventos) = Default::default();
    let rx = VideoReceiver::bind(
        "127.0.0.1:0",
        ReceiverConfig {
            pairing_code: Some(codigo("K7M4-QX9P")),
            on_pairing: Some(anotar(&no_cliente)),
            ..ReceiverConfig::default()
        },
    )
    .unwrap();
    // Como num NAT simétrico: o endereço que foi no código de conexão não é
    // o que o cliente usa para falar com o host.
    let errado = UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let host = socket.local_addr().unwrap();
    let observar_host = anotar(&no_host);
    let tx = std::thread::spawn(move || {
        VideoSender::connect_with_socket(
            socket,
            errado,
            SenderConfig {
                pairing_code: Some(codigo("K7M4-QX9P")),
                on_pairing: Some(observar_host),
                pairing_timeout: Duration::from_secs(4),
                ..SenderConfig::for_bitrate(20_000_000)
            },
        )
    });
    // O cliente recebe o código de resposta (o endereço do host) e fura.
    std::thread::sleep(Duration::from_millis(300));
    rx.punch(&[host]);
    let tx = tx
        .join()
        .unwrap()
        .expect("pareia pelo endereço de onde veio o furo");
    assert_eq!(tx.peer_addr(), rx.local_addr());
    tx.send_frame(frame(0, 5_000), true, clock_us()).unwrap();
    let f = rx.recv_frame(Duration::from_secs(1)).unwrap().expect("frame");
    assert_eq!(f.frame.data, frame(0, 5_000));
    let c = rx.stats();
    assert!(c.encrypted && c.punches_sent > 0, "{c:?}");
    // Com o host achado, os furos param.
    let antes = rx.stats().punches_sent;
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(rx.stats().punches_sent, antes);

    // O diagnóstico viu o furo chegar no host e o pedido chegar no cliente.
    let viu = |eventos: &Mutex<Vec<PairingEvent>>, de: SocketAddr, tipo: &str| {
        eventos
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, PairingEvent::Received { from, kind } if *from == de && *kind == tipo))
    };
    assert!(viu(&no_host, rx.local_addr(), "furo"));
    assert!(viu(&no_cliente, host, "pedido de pareamento"));
}

/// Acontecimentos guardados por [`anotar`].
type Eventos = Arc<Mutex<Vec<PairingEvent>>>;

/// Observador que guarda os acontecimentos numa lista.
fn anotar(lista: &Eventos) -> PairingObserver {
    let lista = lista.clone();
    PairingObserver::new(move |e| lista.lock().unwrap().push(e.clone()))
}
