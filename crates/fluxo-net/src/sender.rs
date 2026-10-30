//! Lado do host: [`VideoSender`].

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fluxo_proto::{Fragmenter, PacketHeader, PacketKind, DEFAULT_MAX_PAYLOAD};

use crate::pacer::Pacer;
use crate::rtt::{RttEstimator, RttStats};
use crate::{clock_diff_us, clock_us, is_transient, NetError};

/// Configuração do [`VideoSender`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderConfig {
    /// Payload máximo por pacote (ver ADR-005).
    pub max_payload: usize,
    /// Taxa de pacing em bit/s; 0 desliga o pacing.
    pub pacing_bps: u64,
    /// Quantos pacotes podem sair em rajada antes de o pacing agir.
    pub burst_packets: usize,
    /// Intervalo entre pings de medição de RTT.
    pub ping_interval: Duration,
}

impl SenderConfig {
    /// Configuração para um vídeo com este bitrate: pacing a 2,5× o
    /// bitrate (folga para keyframes e variações), rajadas de 10 pacotes,
    /// ping a cada 500 ms.
    pub fn for_bitrate(bitrate_bps: u64) -> Self {
        Self {
            max_payload: DEFAULT_MAX_PAYLOAD,
            pacing_bps: bitrate_bps.saturating_mul(5) / 2,
            burst_packets: 10,
            ping_interval: Duration::from_millis(500),
        }
    }
}

/// Estatísticas acumuladas do [`VideoSender`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SenderStats {
    /// Frames entregues a [`VideoSender::send_frame`].
    pub frames_queued: u64,
    /// Frames cujos pacotes já saíram todos.
    pub frames_sent: u64,
    /// Frames esperando na fila agora. Crescendo = a rede (ou o pacing) não
    /// dá conta do bitrate.
    pub queue_len: u64,
    pub packets_sent: u64,
    pub bytes_sent: u64,
    /// Envios que o sistema recusou (rede caiu, destino inalcançável...).
    pub send_errors: u64,
    /// Tempo do primeiro ao último pacote do último frame, em ms (efeito do pacing).
    pub last_frame_send_ms: f64,
    /// Maior tempo de envio de um frame, em ms.
    pub max_frame_send_ms: f64,
    /// Tempo que o último frame esperou na fila antes de começar a sair, em ms.
    pub last_queue_wait_ms: f64,
    pub pings_sent: u64,
    pub pongs_received: u64,
    /// Pings do cliente que respondemos.
    pub pings_answered: u64,
    pub rtt: RttStats,
}

struct Job {
    data: Vec<u8>,
    keyframe: bool,
    timestamp_us: u32,
    queued_at: Instant,
}

struct Shared {
    stats: Mutex<SenderStats>,
    rtt: Mutex<RttEstimator>,
    pacing_bps: AtomicU64,
    stop: AtomicBool,
}

impl Shared {
    fn with_stats(&self, f: impl FnOnce(&mut SenderStats)) {
        // Um panic em outra thread envenenaria o mutex; as estatísticas
        // continuam válidas, então seguimos com elas.
        f(&mut self.stats.lock().unwrap_or_else(|e| e.into_inner()));
    }
}

/// Envia frames de vídeo para um destino fixo, por UDP.
///
/// [`send_frame`](Self::send_frame) só enfileira e volta na hora; uma thread
/// fragmenta e envia com pacing. Outra thread lê o socket para responder
/// pings do cliente e medir o RTT com pongs. Ao destruir o objeto, os frames
/// ainda na fila são enviados antes de as threads terminarem.
pub struct VideoSender {
    queue: Option<mpsc::Sender<Job>>,
    shared: Arc<Shared>,
    send_thread: Option<JoinHandle<()>>,
    recv_thread: Option<JoinHandle<()>>,
    local: SocketAddr,
    peer: SocketAddr,
}

impl VideoSender {
    /// Abre um socket em `bind` (ex.: `"0.0.0.0:0"` para porta automática)
    /// e passa a enviar para `peer`.
    pub fn connect(
        bind: impl ToSocketAddrs,
        peer: impl ToSocketAddrs,
        cfg: SenderConfig,
    ) -> Result<Self, NetError> {
        let socket = UdpSocket::bind(bind).map_err(NetError::io("abrir socket UDP"))?;
        socket.connect(peer).map_err(NetError::io("definir destino"))?;
        // Timeout curto na leitura: a thread de recepção confere `stop` a cada volta.
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(NetError::io("configurar socket"))?;
        let local = socket.local_addr().map_err(NetError::io("endereço local"))?;
        let peer = socket.peer_addr().map_err(NetError::io("endereço do destino"))?;

        let shared = Arc::new(Shared {
            stats: Mutex::new(SenderStats::default()),
            rtt: Mutex::new(RttEstimator::default()),
            pacing_bps: AtomicU64::new(cfg.pacing_bps),
            stop: AtomicBool::new(false),
        });
        let (queue, jobs) = mpsc::channel();

        let send_socket = socket.try_clone().map_err(NetError::io("duplicar socket"))?;
        let s = shared.clone();
        let sender = thread::Builder::new()
            .name("fluxo-net-envio".into())
            .spawn(move || send_loop(send_socket, jobs, cfg, s))
            .map_err(NetError::io("criar thread de envio"))?;
        let s = shared.clone();
        let receiver = thread::Builder::new()
            .name("fluxo-net-pings".into())
            .spawn(move || recv_loop(socket, s))
            .map_err(NetError::io("criar thread de recepção"))?;

        Ok(Self {
            queue: Some(queue),
            shared,
            send_thread: Some(sender),
            recv_thread: Some(receiver),
            local,
            peer,
        })
    }

    /// Enfileira um frame codificado para envio. `timestamp_us` deve ser o
    /// [`clock_us`] do momento da captura.
    pub fn send_frame(&self, data: Vec<u8>, keyframe: bool, timestamp_us: u32) -> Result<(), NetError> {
        let job = Job {
            data,
            keyframe,
            timestamp_us,
            queued_at: Instant::now(),
        };
        let queue = self.queue.as_ref().ok_or(NetError::Closed)?;
        queue.send(job).map_err(|_| NetError::Closed)?;
        self.shared.with_stats(|s| {
            s.frames_queued += 1;
            s.queue_len += 1;
        });
        Ok(())
    }

    /// Muda a taxa de pacing (0 desliga). Vale para os próximos pacotes.
    pub fn set_pacing_bps(&self, bps: u64) {
        self.shared.pacing_bps.store(bps, Ordering::Relaxed);
    }

    /// Cópia das estatísticas atuais.
    pub fn stats(&self) -> SenderStats {
        let mut s = self
            .shared
            .stats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        s.rtt = self.shared.rtt.lock().unwrap_or_else(|e| e.into_inner()).stats();
        s
    }

    /// Endereço local do socket (útil quando a porta foi automática).
    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    /// Destino dos frames.
    pub fn peer_addr(&self) -> SocketAddr {
        self.peer
    }
}

impl Drop for VideoSender {
    fn drop(&mut self) {
        // Fechar a fila faz a thread de envio terminar depois de esvaziá-la.
        self.queue = None;
        if let Some(t) = self.send_thread.take() {
            let _ = t.join();
        }
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.recv_thread.take() {
            let _ = t.join();
        }
    }
}

fn send_loop(socket: UdpSocket, jobs: mpsc::Receiver<Job>, cfg: SenderConfig, shared: Arc<Shared>) {
    let mut frag = Fragmenter::new(cfg.max_payload);
    let packet_size = cfg.max_payload + fluxo_proto::HEADER_LEN;
    let mut pacer = Pacer::new(
        shared.pacing_bps.load(Ordering::Relaxed),
        cfg.burst_packets.max(1) * packet_size,
        Instant::now(),
    );
    let mut ping_id = 0u32;
    let mut next_ping = Instant::now();

    loop {
        let wait = next_ping.saturating_duration_since(Instant::now());
        match jobs.recv_timeout(wait) {
            Ok(job) => send_job(&socket, &mut frag, &mut pacer, &shared, job),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if Instant::now() >= next_ping {
            let ping = PacketHeader {
                kind: PacketKind::Ping,
                frame_id: ping_id,
                frag_index: 0,
                frag_count: 0,
                flags: 0,
                payload_len: 0,
                timestamp_us: clock_us(),
            };
            ping_id = ping_id.wrapping_add(1);
            let ok = ping
                .encode_with_payload(&[])
                .is_ok_and(|p| socket.send(&p).is_ok());
            shared.with_stats(|s| match ok {
                true => s.pings_sent += 1,
                false => s.send_errors += 1,
            });
            next_ping = Instant::now() + cfg.ping_interval;
        }
    }
}

fn send_job(socket: &UdpSocket, frag: &mut Fragmenter, pacer: &mut Pacer, shared: &Shared, job: Job) {
    let start = Instant::now();
    let queue_wait = start - job.queued_at;
    let packets = match frag.fragment(&job.data, job.keyframe, job.timestamp_us) {
        Ok((_, p)) => p,
        Err(_) => {
            // Frame acima de ~78 MB: impossível com vídeo real; descartamos.
            shared.with_stats(|s| {
                s.queue_len = s.queue_len.saturating_sub(1);
                s.send_errors += 1;
            });
            return;
        }
    };
    pacer.set_rate(shared.pacing_bps.load(Ordering::Relaxed), start);

    let (mut sent, mut bytes, mut errors) = (0u64, 0u64, 0u64);
    for p in &packets {
        let wait = pacer.delay(Instant::now());
        if !wait.is_zero() {
            thread::sleep(wait);
        }
        match socket.send(p) {
            Ok(n) => {
                sent += 1;
                bytes += n as u64;
            }
            Err(_) => errors += 1,
        }
        pacer.consume(p.len(), Instant::now());
    }

    let send_ms = start.elapsed().as_secs_f64() * 1000.0;
    shared.with_stats(|s| {
        s.queue_len = s.queue_len.saturating_sub(1);
        s.frames_sent += 1;
        s.packets_sent += sent;
        s.bytes_sent += bytes;
        s.send_errors += errors;
        s.last_frame_send_ms = send_ms;
        s.max_frame_send_ms = s.max_frame_send_ms.max(send_ms);
        s.last_queue_wait_ms = queue_wait.as_secs_f64() * 1000.0;
    });
}

fn recv_loop(socket: UdpSocket, shared: Arc<Shared>) {
    let mut buf = [0u8; 2048];
    while !shared.stop.load(Ordering::Relaxed) {
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(e) if is_transient(&e) => continue,
            Err(_) => {
                // Erro inesperado: evita girar em falso e tenta de novo.
                thread::sleep(Duration::from_millis(10));
                continue;
            }
        };
        let Ok((h, _)) = PacketHeader::decode(&buf[..n]) else {
            continue; // lixo na porta: ignorar
        };
        match h.kind {
            PacketKind::Pong => {
                shared
                    .rtt
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .add(clock_diff_us(clock_us(), h.timestamp_us));
                shared.with_stats(|s| s.pongs_received += 1);
            }
            PacketKind::Ping => {
                let pong = PacketHeader {
                    kind: PacketKind::Pong,
                    ..h
                };
                if pong
                    .encode_with_payload(&[])
                    .is_ok_and(|p| socket.send(&p).is_ok())
                {
                    shared.with_stats(|s| s.pings_answered += 1);
                }
            }
            PacketKind::Video => {} // o host não recebe vídeo
        }
    }
}
