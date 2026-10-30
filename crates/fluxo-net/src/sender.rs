//! Lado do host: [`VideoSender`].

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fluxo_proto::{Feedback, Fragmenter, PacketHeader, PacketKind, DEFAULT_MAX_PAYLOAD};

use crate::bitrate::NetworkReport;
use crate::history::{LossSample, LossWindow, SentHistory};
use crate::pacer::Pacer;
use crate::rtt::{RttEstimator, RttStats};
use crate::{clock_diff_us, clock_us, is_transient, NetError};

/// Configuração do [`VideoSender`].
#[derive(Debug, Clone, PartialEq)]
pub struct SenderConfig {
    /// Payload máximo por pacote (ver ADR-005).
    pub max_payload: usize,
    /// Taxa de pacing em bit/s; 0 desliga o pacing.
    pub pacing_bps: u64,
    /// Quantos pacotes podem sair em rajada antes de o pacing agir.
    pub burst_packets: usize,
    /// Intervalo entre pings de medição de RTT.
    pub ping_interval: Duration,
    /// **Só para testes:** fração dos pacotes de vídeo descartados de
    /// propósito antes do envio (0.01 = 1%), simulando uma rede ruim.
    pub simulated_loss: f64,
    /// FEC: fragmentos por grupo de paridade (0 = sem FEC). Com 10, ~10% a
    /// mais de pacotes, e uma perda por grupo é reconstruída no cliente sem
    /// pedir nada ao host. Ver `fluxo_proto::fec`.
    pub fec_group: u8,
    /// **Só para testes:** simula um link com esta banda (bit/s; 0 = sem
    /// limite). Os pacotes saem no máximo nesse ritmo, os frames esperam na
    /// fila (o cliente vê o atraso crescer) e, passando de
    /// [`SIMULATED_QUEUE_FRAMES`] na fila, são descartados — como num
    /// roteador congestionado.
    pub simulated_bandwidth_bps: u64,
}

/// Tamanho da "fila do roteador" da banda simulada, em frames (~66 ms a 60 fps).
pub const SIMULATED_QUEUE_FRAMES: u64 = 4;

impl SenderConfig {
    /// Configuração para um vídeo com este bitrate: pacing a 2,5× o
    /// bitrate (folga para keyframes e variações), rajadas de 10 pacotes,
    /// ping a cada 500 ms, FEC com grupos de 10, sem simulações.
    pub fn for_bitrate(bitrate_bps: u64) -> Self {
        Self {
            max_payload: DEFAULT_MAX_PAYLOAD,
            pacing_bps: bitrate_bps.saturating_mul(5) / 2,
            burst_packets: 10,
            ping_interval: Duration::from_millis(500),
            simulated_loss: 0.0,
            fec_group: 10,
            simulated_bandwidth_bps: 0,
        }
    }
}

/// Espera máxima, na largada, pela resposta do primeiro ping antes de mandar
/// o primeiro frame. Na primeira rajada para um IP novo, o sistema ainda
/// está descobrindo o endereço físico do destino (ARP) e descarta parte dos
/// pacotes — no teste Windows → Linux, o primeiro keyframe chegou quebrado.
const WARMUP: Duration = Duration::from_millis(50);

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
    /// Pedidos de keyframe recebidos do cliente.
    pub keyframe_requests: u64,
    /// Pacotes descartados de propósito por [`SenderConfig::simulated_loss`]
    /// e pela fila cheia de [`SenderConfig::simulated_bandwidth_bps`].
    pub simulated_drops: u64,
    /// Relatórios de rede recebidos do cliente.
    pub feedbacks_received: u64,
    /// Perda calculada no último relatório (fração), se houve pacotes suficientes.
    pub last_loss: Option<f64>,
    /// Atraso de fila do último relatório do cliente, em ms.
    pub queue_delay_ms: f64,
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
    keyframe_requested: AtomicBool,
    /// Pacotes de vídeo/paridade que tentamos mandar (inclui os descartados
    /// pelas simulações), contados um a um na hora em que saem: base da
    /// conta de perda.
    sent_history: Mutex<SentHistory>,
    /// Relatórios da última ~1 s, para a conta de perda (ver `history.rs`).
    loss_window: Mutex<LossWindow>,
    /// Último relatório ainda não consumido por [`VideoSender::take_report`].
    report: Mutex<Option<NetworkReport>>,
}

impl Shared {
    fn with_stats(&self, f: impl FnOnce(&mut SenderStats)) {
        // Um panic em outra thread envenenaria o mutex; as estatísticas
        // continuam válidas, então seguimos com elas.
        f(&mut self.stats.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// Registra `n` pacotes de vídeo/paridade saindo agora (ou descartados
    /// pelas simulações) no histórico usado na conta de perda.
    fn record_sent(&self, n: u64) {
        self.sent_history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record(n, Instant::now());
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
            keyframe_requested: AtomicBool::new(false),
            sent_history: Mutex::new(SentHistory::default()),
            loss_window: Mutex::new(LossWindow::default()),
            report: Mutex::new(None),
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

    /// `true` (uma vez) se o cliente pediu um keyframe desde a última
    /// chamada. Vários pedidos seguidos contam como um só.
    pub fn take_keyframe_request(&self) -> bool {
        self.shared.keyframe_requested.swap(false, Ordering::Relaxed)
    }

    /// O relatório de rede mais recente do cliente, uma vez (`None` se não
    /// chegou nenhum desde a última chamada). Alimente um
    /// [`BitrateController`](crate::BitrateController) com ele.
    pub fn take_report(&self) -> Option<NetworkReport> {
        self.shared
            .report
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
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

/// Gerador pseudoaleatório mínimo (xorshift) para a perda simulada.
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let semente = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self(semente | 1)
    }

    /// Número em [0, 1).
    fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn send_loop(socket: UdpSocket, jobs: mpsc::Receiver<Job>, cfg: SenderConfig, shared: Arc<Shared>) {
    let mut frag = Fragmenter::new(cfg.max_payload).with_fec_group(cfg.fec_group);
    let packet_size = cfg.max_payload + fluxo_proto::HEADER_LEN;
    let mut pacer = Pacer::new(
        shared.pacing_bps.load(Ordering::Relaxed),
        cfg.burst_packets.max(1) * packet_size,
        Instant::now(),
    );
    let mut rng = Rng::new();
    let mut ping_id = 0u32;

    // Largada: um ping antes de qualquer frame, e uma espera curta pela
    // resposta (ver WARMUP).
    send_ping(&socket, &shared, &mut ping_id);
    let limite = Instant::now() + WARMUP;
    while Instant::now() < limite && shared.stats.lock().map(|s| s.pongs_received == 0).unwrap_or(true) {
        thread::sleep(Duration::from_millis(2));
    }
    let mut next_ping = Instant::now() + cfg.ping_interval;

    loop {
        let wait = next_ping.saturating_duration_since(Instant::now());
        match jobs.recv_timeout(wait) {
            Ok(job) => send_job(&socket, &mut frag, &mut pacer, &shared, &cfg, &mut rng, job),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if Instant::now() >= next_ping {
            send_ping(&socket, &shared, &mut ping_id);
            next_ping = Instant::now() + cfg.ping_interval;
        }
    }
}

fn send_ping(socket: &UdpSocket, shared: &Shared, ping_id: &mut u32) {
    let ping = PacketKind::Ping.control(*ping_id, clock_us());
    *ping_id = ping_id.wrapping_add(1);
    let ok = ping
        .encode_with_payload(&[])
        .is_ok_and(|p| socket.send(&p).is_ok());
    shared.with_stats(|s| match ok {
        true => s.pings_sent += 1,
        false => s.send_errors += 1,
    });
}

fn send_job(
    socket: &UdpSocket,
    frag: &mut Fragmenter,
    pacer: &mut Pacer,
    shared: &Shared,
    cfg: &SenderConfig,
    rng: &mut Rng,
    job: Job,
) {
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
    // Banda simulada: fila do "roteador" cheia → o frame inteiro se perde
    // (o fragmentador já numerou, então o cliente vê o buraco).
    let fila = shared.stats.lock().map(|s| s.queue_len).unwrap_or(0);
    if cfg.simulated_bandwidth_bps > 0 && fila > SIMULATED_QUEUE_FRAMES {
        shared.record_sent(packets.len() as u64);
        shared.with_stats(|s| {
            s.queue_len = s.queue_len.saturating_sub(1);
            s.simulated_drops += packets.len() as u64;
        });
        return;
    }
    let pacing = shared.pacing_bps.load(Ordering::Relaxed);
    let pacing = match cfg.simulated_bandwidth_bps {
        0 => pacing,
        banda if pacing == 0 => banda,
        banda => pacing.min(banda),
    };
    pacer.set_rate(pacing, start);

    let (mut sent, mut bytes, mut errors, mut dropped) = (0u64, 0u64, 0u64, 0u64);
    for p in &packets {
        let wait = pacer.delay(Instant::now());
        if !wait.is_zero() {
            thread::sleep(wait);
        }
        // O pacote "perdido" ocupa seu tempo no pacing, como numa rede real.
        if cfg.simulated_loss > 0.0 && rng.next_f64() < cfg.simulated_loss {
            dropped += 1;
        } else {
            match socket.send(p) {
                Ok(n) => {
                    sent += 1;
                    bytes += n as u64;
                }
                Err(_) => errors += 1,
            }
        }
        // Conta na hora em que sai (não no início do frame): pacotes ainda
        // no pacing não podem parecer perdidos para o relatório do cliente.
        shared.record_sent(1);
        pacer.consume(p.len(), Instant::now());
    }

    let send_ms = start.elapsed().as_secs_f64() * 1000.0;
    shared.with_stats(|s| {
        s.queue_len = s.queue_len.saturating_sub(1);
        s.frames_sent += 1;
        s.packets_sent += sent;
        s.bytes_sent += bytes;
        s.send_errors += errors;
        s.simulated_drops += dropped;
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
        let Ok((h, payload)) = PacketHeader::decode(&buf[..n]) else {
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
            PacketKind::KeyframeRequest => {
                shared.keyframe_requested.store(true, Ordering::Relaxed);
                shared.with_stats(|s| s.keyframe_requests += 1);
            }
            PacketKind::Feedback => {
                if let Ok(f) = Feedback::from_bytes(payload) {
                    on_feedback(&shared, &f);
                }
            }
            PacketKind::Video | PacketKind::Parity => {} // o host não recebe vídeo
        }
    }
}

/// Transforma um relatório do cliente em [`NetworkReport`].
///
/// A perda é `1 − recebidos/tentados` na última ~1 s, onde "tentados" é o
/// total que tinha saído **um RTT antes** de o relatório chegar (o que saiu
/// depois ainda estava no caminho quando o cliente contou), e só conta se o
/// cliente confirmou perda (FEC ou frame perdido). Ver `history.rs`.
fn on_feedback(shared: &Shared, f: &Feedback) {
    let rtt = shared.rtt.lock().unwrap_or_else(|e| e.into_inner()).stats();
    let um_rtt = Duration::from_secs_f64(rtt.smoothed_ms.max(0.0) / 1000.0);
    let tentados = {
        let h = shared.sent_history.lock().unwrap_or_else(|e| e.into_inner());
        match Instant::now().checked_sub(um_rtt) {
            Some(quando) => h.at(quando),
            None => h.total(),
        }
    };
    let loss = shared
        .loss_window
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .add(LossSample {
            quando: Instant::now(),
            recebidos: f.packets_received,
            tentados,
            frames_perdidos: f.frames_lost,
            fec_reconstruidos: f.fec_recovered,
        });
    let report = NetworkReport {
        loss,
        queue_delay_ms: f.queue_delay_us as f64 / 1000.0,
    };
    *shared.report.lock().unwrap_or_else(|e| e.into_inner()) = Some(report);
    shared.with_stats(|s| {
        s.feedbacks_received += 1;
        s.queue_delay_ms = report.queue_delay_ms;
        if loss.is_some() {
            s.last_loss = loss;
        }
    });
}
