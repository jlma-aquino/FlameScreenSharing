//! Lado do cliente: [`VideoReceiver`].

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fluxo_proto::{
    CompletedFrame, Feedback, PacketHeader, PacketKind, Reassembler, ReassemblerConfig, ReassemblerStats,
};

use crate::delay::QueueDelay;
use crate::rtt::{RttEstimator, RttStats};
use crate::sync::ClockSync;
use crate::{clock_diff_us, clock_us, is_transient, pong_clock, pong_for, NetError};

/// Configuração do [`VideoReceiver`].
#[derive(Debug, Clone)]
pub struct ReceiverConfig {
    /// Limites da remontagem (ver `fluxo-proto`).
    pub reassembler: ReassemblerConfig,
    /// Intervalo entre pings ao host, depois que ele for conhecido.
    pub ping_interval: Duration,
    /// Frames prontos que podem esperar o consumidor. Passou disso, os
    /// novos são descartados (e contados) — sinal de decoder lento.
    ///
    /// Generoso de propósito: descartar um frame quebra a cadeia de
    /// P-frames (artefatos até o próximo keyframe), enquanto um consumidor
    /// que decodifica tudo e mostra só o mais novo recupera um atraso
    /// rapidamente. 120 frames ≈ 2 s a 60 fps, ~5 MB a 20 Mbps.
    pub queue_frames: usize,
    /// Sem pacotes do host por este tempo, aceitamos um host novo (ex.:
    /// o host reiniciou e veio de outra porta).
    pub peer_timeout: Duration,
    /// Intervalo entre relatórios ([`Feedback`]) ao host, para o bitrate
    /// adaptativo.
    pub feedback_interval: Duration,
}

impl Default for ReceiverConfig {
    fn default() -> Self {
        Self {
            reassembler: ReassemblerConfig::default(),
            ping_interval: Duration::from_millis(500),
            queue_frames: 120,
            peer_timeout: Duration::from_secs(2),
            feedback_interval: Duration::from_millis(250),
        }
    }
}

/// Um frame completo, com dados de tempo.
#[derive(Debug, Clone)]
pub struct ReceivedFrame {
    pub frame: CompletedFrame,
    /// [`clock_us`] no momento em que o último fragmento chegou.
    pub received_us: u32,
    /// Captura no host → frame completo aqui, em µs, comparando os dois
    /// relógios como estão. Entre PCs, inclui a diferença entre eles
    /// (prefira [`Self::latency_us`]).
    pub transit_us: i32,
    /// Latência real captura → frame completo, em µs, com a diferença entre
    /// os relógios descontada (sincronização pelos Ping/Pong). `None` até a
    /// primeira medição (~500 ms depois de o host aparecer).
    pub latency_us: Option<i32>,
}

/// Estatísticas acumuladas do [`VideoReceiver`].
#[derive(Debug, Clone, Default)]
pub struct ReceiverStats {
    /// Contadores da remontagem (pacotes, frames completos e perdidos...).
    pub reassembly: ReassemblerStats,
    /// Bytes de vídeo recebidos (com cabeçalhos).
    pub bytes_received: u64,
    /// Datagramas descartados: formato inválido ou vindos de outro endereço.
    pub invalid_packets: u64,
    /// Frames completos descartados porque o consumidor não os retirou a tempo.
    pub frames_dropped: u64,
    /// Endereço do host atual, aprendido pelo primeiro pacote válido.
    pub peer: Option<SocketAddr>,
    /// Quantas vezes o host mudou (reinício do host).
    pub peer_changes: u64,
    pub pings_sent: u64,
    pub pongs_received: u64,
    pub pings_answered: u64,
    /// Pedidos de keyframe enviados ao host.
    pub keyframe_requests: u64,
    /// Relatórios de rede enviados ao host.
    pub feedbacks_sent: u64,
    /// Atraso de fila do último relatório, em ms: quanto os frames chegam
    /// mais tarde que o normal (0 = rede sem fila).
    pub queue_delay_ms: f64,
    /// Relógio do host menos o nosso, em ms (`None` antes da 1ª medição).
    pub clock_offset_ms: Option<f64>,
    /// Erro máximo de [`Self::clock_offset_ms`] (metade do melhor RTT), em ms.
    pub clock_error_ms: Option<f64>,
    pub rtt: RttStats,
}

struct Shared {
    stats: Mutex<ReceiverStats>,
    rtt: Mutex<RttEstimator>,
    stop: AtomicBool,
}

impl Shared {
    fn with_stats(&self, f: impl FnOnce(&mut ReceiverStats)) {
        f(&mut self.stats.lock().unwrap_or_else(|e| e.into_inner()));
    }
}

/// Recebe frames de vídeo numa porta UDP.
///
/// Uma thread lê o socket sem parar, remonta os frames e os deixa numa fila
/// para [`recv_frame`](Self::recv_frame). Ela também responde aos pings do
/// host e manda os seus, para medir o RTT.
///
/// Aceita pacotes de um host só (o primeiro que mandar algo válido); outro
/// endereço só é aceito depois de [`ReceiverConfig::peer_timeout`] sem
/// notícias do atual.
pub struct VideoReceiver {
    frames: mpsc::Receiver<ReceivedFrame>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    local: SocketAddr,
    /// Cópia do socket para mandar pedidos de keyframe desta thread.
    socket: UdpSocket,
    /// Último pedido de keyframe e quantos já foram (número do próximo).
    last_request: Mutex<(Option<Instant>, u32)>,
}

/// Intervalo mínimo entre pedidos de keyframe (ou 2 RTTs, o que for maior):
/// tempo para o keyframe pedido chegar antes de pedir de novo.
const KEYFRAME_REQUEST_INTERVAL: Duration = Duration::from_millis(50);

impl VideoReceiver {
    /// Escuta em `bind` (ex.: `"0.0.0.0:9000"`).
    pub fn bind(bind: impl ToSocketAddrs, cfg: ReceiverConfig) -> Result<Self, NetError> {
        let socket = UdpSocket::bind(bind).map_err(NetError::io("abrir socket UDP"))?;
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .map_err(NetError::io("configurar socket"))?;
        let local = socket.local_addr().map_err(NetError::io("endereço local"))?;
        let shared = Arc::new(Shared {
            stats: Mutex::new(ReceiverStats::default()),
            rtt: Mutex::new(RttEstimator::default()),
            stop: AtomicBool::new(false),
        });
        let (tx, frames) = mpsc::sync_channel(cfg.queue_frames.max(1));
        let s = shared.clone();
        let copia = socket.try_clone().map_err(NetError::io("duplicar socket"))?;
        let thread = thread::Builder::new()
            .name("fluxo-net-recepcao".into())
            .spawn(move || recv_loop(socket, cfg, tx, s))
            .map_err(NetError::io("criar thread de recepção"))?;
        Ok(Self {
            frames,
            shared,
            thread: Some(thread),
            local,
            socket: copia,
            last_request: Mutex::new((None, 0)),
        })
    }

    /// Pede ao host um keyframe — chame quando um frame se perdeu
    /// ([`CompletedFrame::preceded_by_loss`](fluxo_proto::CompletedFrame)) ou
    /// enquanto estiver sem imagem decodificável.
    ///
    /// Seguro de chamar a cada frame: só envia se o último pedido tiver mais
    /// de ~50 ms (ou 2 RTTs). Devolve `true` se um pedido saiu agora; `false`
    /// se foi segurado ou se o host ainda é desconhecido.
    pub fn request_keyframe(&self) -> bool {
        let s = self.stats();
        let Some(peer) = s.peer else {
            return false;
        };
        let espera = KEYFRAME_REQUEST_INTERVAL.max(Duration::from_secs_f64(s.rtt.smoothed_ms * 2.0 / 1000.0));
        let mut ultimo = self.last_request.lock().unwrap_or_else(|e| e.into_inner());
        if ultimo.0.is_some_and(|t| t.elapsed() < espera) {
            return false;
        }
        let pedido = PacketKind::KeyframeRequest.control(ultimo.1, clock_us());
        let ok = pedido
            .encode_with_payload(&[])
            .is_ok_and(|p| self.socket.send_to(&p, peer).is_ok());
        if ok {
            *ultimo = (Some(Instant::now()), ultimo.1.wrapping_add(1));
            self.shared.with_stats(|s| s.keyframe_requests += 1);
        }
        ok
    }

    /// Espera até `timeout` pelo próximo frame completo. `Ok(None)` se nada
    /// chegou no prazo.
    pub fn recv_frame(&self, timeout: Duration) -> Result<Option<ReceivedFrame>, NetError> {
        match self.frames.recv_timeout(timeout) {
            Ok(f) => Ok(Some(f)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(NetError::Closed),
        }
    }

    /// Cópia das estatísticas atuais.
    pub fn stats(&self) -> ReceiverStats {
        let mut s = self
            .shared
            .stats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        s.rtt = self.shared.rtt.lock().unwrap_or_else(|e| e.into_inner()).stats();
        s
    }

    /// Endereço em que estamos escutando.
    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }
}

impl Drop for VideoReceiver {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn recv_loop(
    socket: UdpSocket,
    cfg: ReceiverConfig,
    frames: mpsc::SyncSender<ReceivedFrame>,
    shared: Arc<Shared>,
) {
    let mut reasm = Reassembler::new(cfg.reassembler);
    let mut buf = [0u8; 2048];
    let mut peer: Option<SocketAddr> = None;
    let mut last_from_peer = Instant::now();
    let mut next_ping = Instant::now();
    let mut ping_id = 0u32;
    // Bitrate adaptativo: pacotes de vídeo/paridade que chegaram (todos,
    // inclusive os que a remontagem descarta como sobra) e atraso de fila.
    let mut pacotes_video = 0u32;
    let mut fila = QueueDelay::default();
    // Diferença entre o relógio do host e o nosso, pelos Ping/Pong.
    let mut relogio = ClockSync::default();
    let mut next_feedback = Instant::now() + cfg.feedback_interval;

    while !shared.stop.load(Ordering::Relaxed) {
        if let Some(p) = peer {
            if Instant::now() >= next_ping {
                if ping_to(&socket, p, ping_id) {
                    shared.with_stats(|s| s.pings_sent += 1);
                }
                ping_id = ping_id.wrapping_add(1);
                next_ping = Instant::now() + cfg.ping_interval;
            }
            if Instant::now() >= next_feedback {
                let r = reasm.stats();
                let f = Feedback {
                    packets_received: pacotes_video,
                    queue_delay_us: fila.current_us(),
                    frames_lost: r.frames_lost as u32,
                    fec_recovered: r.fec_recovered as u32,
                };
                let ok = PacketKind::Feedback
                    .control(0, clock_us())
                    .encode_with_payload(&f.to_bytes())
                    .is_ok_and(|pkt| socket.send_to(&pkt, p).is_ok());
                if ok {
                    shared.with_stats(|s| {
                        s.feedbacks_sent += 1;
                        s.queue_delay_ms = f.queue_delay_us as f64 / 1000.0;
                    });
                }
                next_feedback = Instant::now() + cfg.feedback_interval;
            }
        }

        let (n, from) = match socket.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e) if is_transient(&e) => continue,
            Err(_) => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
        };
        let Ok((h, payload)) = PacketHeader::decode(&buf[..n]) else {
            shared.with_stats(|s| s.invalid_packets += 1);
            continue;
        };

        // Um host por vez. Host novo: remontagem do zero (a numeração dos
        // frames dele recomeça e pareceria "atrasada").
        match peer {
            Some(p) if p == from => {}
            Some(_) if last_from_peer.elapsed() < cfg.peer_timeout => {
                shared.with_stats(|s| s.invalid_packets += 1);
                continue;
            }
            anterior => {
                if anterior.is_some() {
                    reasm = Reassembler::new(cfg.reassembler);
                    fila = QueueDelay::default();
                    relogio = ClockSync::default();
                    shared.with_stats(|s| s.peer_changes += 1);
                }
                peer = Some(from);
                next_ping = Instant::now();
                shared.with_stats(|s| s.peer = Some(from));
            }
        }
        last_from_peer = Instant::now();

        match h.kind {
            // Só o host recebe pedidos de keyframe e relatórios.
            PacketKind::KeyframeRequest | PacketKind::Feedback => {}
            PacketKind::Ping => {
                if pong_for(&h).is_some_and(|p| socket.send_to(&p, from).is_ok()) {
                    shared.with_stats(|s| s.pings_answered += 1);
                }
            }
            PacketKind::Pong => {
                let agora = clock_us();
                shared
                    .rtt
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .add(clock_diff_us(agora, h.timestamp_us));
                if let Some(t1) = pong_clock(payload) {
                    relogio.add(h.timestamp_us, t1, agora);
                }
                let (offset, erro) = (relogio.offset_us(), relogio.best_rtt_us());
                shared.with_stats(|s| {
                    s.pongs_received += 1;
                    s.clock_offset_ms = offset.map(|o| o as f64 / 1000.0);
                    s.clock_error_ms = erro.map(|r| r as f64 / 2000.0);
                });
            }
            PacketKind::Video | PacketKind::Parity => {
                pacotes_video = pacotes_video.wrapping_add(1);
                let completed = reasm.push(&h, payload);
                let mut dropped = false;
                if let Some(frame) = completed {
                    let received_us = clock_us();
                    fila.add(clock_diff_us(received_us, frame.timestamp_us), Instant::now());
                    let transit_us = clock_diff_us(received_us, frame.timestamp_us);
                    let rf = ReceivedFrame {
                        // Captura no relógio do host + diferença dos relógios
                        // = captura no nosso relógio.
                        latency_us: relogio.offset_us().map(|o| transit_us.wrapping_add(o)),
                        transit_us,
                        received_us,
                        frame,
                    };
                    match frames.try_send(rf) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => dropped = true,
                        // Ninguém mais vai ler: o VideoReceiver foi destruído.
                        Err(TrySendError::Disconnected(_)) => return,
                    }
                }
                let r = reasm.stats();
                shared.with_stats(|s| {
                    s.reassembly = r;
                    s.bytes_received += n as u64;
                    s.frames_dropped += dropped as u64;
                });
            }
        }
    }
}

fn ping_to(socket: &UdpSocket, to: SocketAddr, id: u32) -> bool {
    PacketKind::Ping
        .control(id, clock_us())
        .encode_with_payload(&[])
        .is_ok_and(|p| socket.send_to(&p, to).is_ok())
}
