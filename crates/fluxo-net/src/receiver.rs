//! Lado do cliente: [`VideoReceiver`].

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fluxo_crypto::{Opener, PairingCode, Sealer, Session, TAG_LEN};
use fluxo_proto::{
    prepare_seal, CompletedFrame, CursorMessage, CursorShape, Feedback, PacketHeader, PacketKind,
    Reassembler, ReassemblerConfig, ReassemblerStats, ShapeAssembler, SignalMessage, FLAG_KEYFRAME,
    HEADER_LEN,
};

use crate::delay::QueueDelay;
use crate::observar::{kind_of, PairingEvent, PairingObserver, RESUMO_A_CADA};
use crate::portas::Portas;
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
    /// Código de pareamento. Com ele, só se aceita o host que fizer o
    /// handshake com o mesmo código, e tudo trafega cifrado; pacotes em
    /// claro são descartados. `None`: sem cifra (rede de teste).
    pub pairing_code: Option<PairingCode>,
    /// Servidor de sinalização: registra a sala do código nele, para um host
    /// em outra rede achar este cliente ([`VideoSender::connect_via_signal`]).
    /// Exige [`pairing_code`](Self::pairing_code).
    ///
    /// [`VideoSender::connect_via_signal`]: crate::VideoSender::connect_via_signal
    pub signal_server: Option<SocketAddr>,
    /// Servidor STUN que viu o nosso endereço público
    /// ([`PublicAddr::server`](crate::PublicAddr::server)). Enquanto nenhum
    /// host chegar, um pedido STUN a cada 15 s mantém vivo o mapeamento do
    /// NAT que está no código de conexão (alguns NATs o esquecem em 30 s).
    pub keepalive_to: Option<SocketAddr>,
    /// Recebe o que chega enquanto nenhum host apareceu e, depois de
    /// [`VideoReceiver::punch`], um resumo dos furos (diagnóstico).
    pub on_pairing: Option<PairingObserver>,
}

/// Intervalo dos pedidos de [`ReceiverConfig::keepalive_to`].
const MANTER_NAT: Duration = Duration::from_secs(15);
/// Furo na direção do host ([`VideoReceiver::punch`]): a cada...
const FURO_A_CADA: Duration = Duration::from_millis(250);
/// ...por no máximo este tempo (ou até o host aparecer).
const FURO_DURA: Duration = Duration::from_secs(60);

/// Registro na sala renovado a cada... (mantém a sala e o mapeamento do NAT
/// vivos; os NATs costumam esquecer um mapeamento UDP parado em 30 s ou mais).
const REGISTRO_VIVO: Duration = Duration::from_secs(5);
/// ...ou, enquanto o servidor não confirmou, a cada 1 s.
const REGISTRO_PENDENTE: Duration = Duration::from_secs(1);

impl Default for ReceiverConfig {
    fn default() -> Self {
        Self {
            reassembler: ReassemblerConfig::default(),
            ping_interval: Duration::from_millis(500),
            queue_frames: 120,
            peer_timeout: Duration::from_secs(2),
            feedback_interval: Duration::from_millis(250),
            pairing_code: None,
            signal_server: None,
            keepalive_to: None,
            on_pairing: None,
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

/// Um quadro de áudio Opus do host ([`VideoReceiver::recv_audio`]).
#[derive(Debug, Clone)]
pub struct AudioPacket {
    /// Número do quadro (sobe de 1 em 1 no host).
    pub seq: u32,
    /// Captura no host, no relógio dele.
    pub timestamp_us: u32,
    /// [`clock_us`] na chegada.
    pub received_us: u32,
    /// Captura → chegada, com a diferença dos relógios descontada.
    pub latency_us: Option<i32>,
    /// O pacote Opus.
    pub data: Vec<u8>,
}

/// Fila de áudio de um [`VideoReceiver`], para tocar em outra thread
/// ([`VideoReceiver::take_audio`]).
pub struct AudioStream(mpsc::Receiver<AudioPacket>);

impl AudioStream {
    /// Espera até `timeout` pelo próximo quadro. `Ok(None)` se nada chegou.
    pub fn recv(&self, timeout: Duration) -> Result<Option<AudioPacket>, NetError> {
        match self.0.recv_timeout(timeout) {
            Ok(a) => Ok(Some(a)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(NetError::Closed),
        }
    }
}

/// Quadros de áudio esperando quem os toque (1 s): mais que isso, ninguém
/// está tocando, e os novos se perdem.
const FILA_AUDIO: usize = 100;

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
    /// Pedidos de limpeza depois de perda enviados ao host (onda de
    /// intra-refresh ou keyframe, o host decide).
    pub refresh_requests: u64,
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
    /// Pedidos de pareamento respondidos (com código configurado).
    pub pairings: u64,
    /// Há uma sessão cifrada com o host (o handshake deu certo).
    pub encrypted: bool,
    /// Com sinalização: nosso endereço visto da internet, depois que o
    /// servidor confirmou o registro.
    pub public_addr: Option<SocketAddr>,
    /// Com sinalização: o servidor recusou a sala (outro cliente com um
    /// código da mesma sala). Sorteie outro código.
    pub room_taken: bool,
    /// Pacotes de furo mandados ao host (avisado pelo servidor de sinalização
    /// ou pedido por [`VideoReceiver::punch`]).
    pub punches_sent: u64,
    /// Pacotes de cursor recebidos do host.
    pub cursor_packets: u64,
    /// Formas de cursor pedidas de novo ao host (alguma parte se perdeu).
    pub cursor_requests: u64,
    /// Quadros de áudio recebidos, e os perdidos porque ninguém os tirou da
    /// fila a tempo (sem consumidor de áudio).
    pub audio_packets: u64,
    pub audio_dropped: u64,
}

struct Shared {
    stats: Mutex<ReceiverStats>,
    rtt: Mutex<RttEstimator>,
    stop: AtomicBool,
    /// Com código de pareamento: tudo o que sai vai cifrado.
    cifrado: bool,
    /// Chave do sentido cliente → host, depois do pareamento.
    sealer: Mutex<Option<Sealer>>,
    /// Endereços do host para furar o nosso NAT, e até quando ([`VideoReceiver::punch`]).
    furos: Mutex<Option<(Vec<SocketAddr>, Instant)>>,
    /// Cursor do host: posição, forma e a montagem da forma em partes.
    cursor: Mutex<CursorClient>,
}

impl Shared {
    fn with_stats(&self, f: impl FnOnce(&mut ReceiverStats)) {
        f(&mut self.stats.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// Manda um pacote montado ao host: cifrado se há código de pareamento
    /// (e nada, se o pareamento ainda não aconteceu), em claro se não há.
    fn enviar(&self, socket: &Portas, mut pacote: Vec<u8>, para: SocketAddr) -> bool {
        if self.cifrado {
            let mut sealer = self.sealer.lock().unwrap_or_else(|e| e.into_inner());
            let Some(s) = sealer.as_mut() else {
                return false;
            };
            let Some(seq) = s.next_seq() else {
                return false;
            };
            prepare_seal(&mut pacote, seq, TAG_LEN);
            if s.seal(seq, &mut pacote, HEADER_LEN).is_err() {
                return false;
            }
        }
        socket.send_to(&pacote, para).is_ok()
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
    /// Fila de áudio (`None` depois de [`VideoReceiver::take_audio`]).
    audio: Mutex<Option<mpsc::Receiver<AudioPacket>>>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    local: SocketAddr,
    /// Cópia do socket para mandar pedidos de keyframe desta thread.
    socket: Portas,
    /// Último pedido de keyframe e quantos já foram (número do próximo).
    last_request: Mutex<(Option<Instant>, u32)>,
}

/// Pede furos ao [`VideoReceiver`] de outra thread ([`VideoReceiver::puncher`]).
#[derive(Clone)]
pub struct Puncher(Arc<Shared>);

impl Puncher {
    /// O mesmo que [`VideoReceiver::punch`].
    pub fn punch(&self, peers: &[SocketAddr]) {
        *self.0.furos.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((peers.to_vec(), Instant::now() + FURO_DURA));
    }
}

/// Intervalo mínimo entre pedidos de keyframe (ou 2 RTTs, o que for maior):
/// tempo para o keyframe pedido chegar antes de pedir de novo.
const KEYFRAME_REQUEST_INTERVAL: Duration = Duration::from_millis(50);

/// Timeout de leitura do socket (com IPv4 e IPv6, metade em cada).
const LEITURA: Duration = Duration::from_millis(50);

impl VideoReceiver {
    /// Escuta em `bind` (ex.: `"0.0.0.0:9000"`).
    pub fn bind(bind: impl ToSocketAddrs, cfg: ReceiverConfig) -> Result<Self, NetError> {
        let socket = UdpSocket::bind(bind).map_err(NetError::io("abrir socket UDP"))?;
        Self::with_socket(socket, cfg)
    }

    /// Como [`bind`](Self::bind), num socket já aberto — o mesmo em que
    /// [`discover_public_addr`](crate::discover_public_addr) descobriu o
    /// endereço público, que só vale para ele.
    pub fn with_socket(socket: UdpSocket, cfg: ReceiverConfig) -> Result<Self, NetError> {
        Self::with_sockets(socket, None, cfg)
    }

    /// Como [`with_socket`](Self::with_socket), escutando também num socket
    /// IPv6: o host pode parear por qualquer um dos dois; depois só o da
    /// família que pareou continua.
    pub fn with_sockets(v4: UdpSocket, v6: Option<UdpSocket>, cfg: ReceiverConfig) -> Result<Self, NetError> {
        if cfg.signal_server.is_some() && cfg.pairing_code.is_none() {
            return Err(NetError::Pairing(
                "a sinalização precisa do código de pareamento".into(),
            ));
        }
        let socket = Portas::new(Some(v4), v6).map_err(NetError::io("abrir socket UDP"))?;
        socket
            .set_read_timeout(LEITURA)
            .map_err(NetError::io("configurar socket"))?;
        let local = socket.local_addr().map_err(NetError::io("endereço local"))?;
        let shared = Arc::new(Shared {
            stats: Mutex::new(ReceiverStats::default()),
            rtt: Mutex::new(RttEstimator::default()),
            stop: AtomicBool::new(false),
            cifrado: cfg.pairing_code.is_some(),
            sealer: Mutex::new(None),
            furos: Mutex::new(None),
            cursor: Mutex::new(CursorClient::default()),
        });
        let (tx, frames) = mpsc::sync_channel(cfg.queue_frames.max(1));
        let (atx, audio) = mpsc::sync_channel(FILA_AUDIO);
        let s = shared.clone();
        let copia = socket.try_clone().map_err(NetError::io("duplicar socket"))?;
        let thread = thread::Builder::new()
            .name("fluxo-net-recepcao".into())
            .spawn(move || recv_loop(socket, cfg, tx, atx, s))
            .map_err(NetError::io("criar thread de recepção"))?;
        Ok(Self {
            frames,
            audio: Mutex::new(Some(audio)),
            shared,
            thread: Some(thread),
            local,
            socket: copia,
            last_request: Mutex::new((None, 0)),
        })
    }

    /// Fura o nosso NAT na direção do host (endereços do código de resposta):
    /// manda um `Punch` a cada endereço a cada 250 ms, até o host aparecer
    /// ou passar 1 minuto. Os `PairInit` que ele já está mandando passam a
    /// chegar.
    pub fn punch(&self, peers: &[SocketAddr]) {
        self.puncher().punch(peers);
    }

    /// Alça para pedir [`punch`](Self::punch) de outra thread (ex.: a que lê
    /// o código de resposta no terminal enquanto outra mostra o vídeo).
    pub fn puncher(&self) -> Puncher {
        Puncher(self.shared.clone())
    }

    /// Pede ao host um keyframe — chame enquanto estiver sem imagem
    /// decodificável (largada) ou quando o decoder falhar.
    ///
    /// Seguro de chamar a cada frame: só envia se o último pedido tiver mais
    /// de ~50 ms (ou 2 RTTs). Devolve `true` se um pedido saiu agora; `false`
    /// se foi segurado ou se o host ainda é desconhecido.
    pub fn request_keyframe(&self) -> bool {
        self.request(true)
    }

    /// Pede ao host para limpar a imagem depois de uma perda
    /// ([`CompletedFrame::preceded_by_loss`](fluxo_proto::CompletedFrame)):
    /// uma onda de intra-refresh se o encoder dele tiver, senão um keyframe.
    /// A imagem está limpa de novo depois de um frame com `refresh_end` cuja
    /// onda (`refresh_start`) começou depois da perda, ou de um keyframe.
    ///
    /// Mesmo controle de ritmo de [`request_keyframe`](Self::request_keyframe).
    pub fn request_refresh(&self) -> bool {
        self.request(false)
    }

    fn request(&self, precisa_keyframe: bool) -> bool {
        let s = self.stats();
        let Some(peer) = s.peer else {
            return false;
        };
        let espera = KEYFRAME_REQUEST_INTERVAL.max(Duration::from_secs_f64(s.rtt.smoothed_ms * 2.0 / 1000.0));
        let mut ultimo = self.last_request.lock().unwrap_or_else(|e| e.into_inner());
        if ultimo.0.is_some_and(|t| t.elapsed() < espera) {
            return false;
        }
        let mut pedido = PacketKind::KeyframeRequest.control(ultimo.1, clock_us());
        if precisa_keyframe {
            pedido.flags = FLAG_KEYFRAME;
        }
        let ok = pedido
            .encode_with_payload(&[])
            .is_ok_and(|p| self.shared.enviar(&self.socket, p, peer));
        if ok {
            *ultimo = (Some(Instant::now()), ultimo.1.wrapping_add(1));
            self.shared.with_stats(|s| match precisa_keyframe {
                true => s.keyframe_requests += 1,
                false => s.refresh_requests += 1,
            });
        }
        ok
    }

    /// Espera até `timeout` pelo próximo quadro de áudio. `Ok(None)` se nada
    /// chegou no prazo (ou se a fila já foi entregue a [`take_audio`](Self::take_audio)).
    /// Quem não toca áudio pode ignorar: a fila só descarta.
    pub fn recv_audio(&self, timeout: Duration) -> Result<Option<AudioPacket>, NetError> {
        let fila = self.audio.lock().unwrap_or_else(|e| e.into_inner());
        match fila.as_ref().map(|f| f.recv_timeout(timeout)) {
            Some(Ok(a)) => Ok(Some(a)),
            Some(Err(RecvTimeoutError::Timeout)) | None => Ok(None),
            Some(Err(RecvTimeoutError::Disconnected)) => Err(NetError::Closed),
        }
    }

    /// Entrega a fila de áudio para outra thread (a que toca o som); depois
    /// disso, [`recv_audio`](Self::recv_audio) não recebe mais nada. Uma vez só.
    pub fn take_audio(&self) -> Option<AudioStream> {
        self.audio
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .map(AudioStream)
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

    /// O cursor do host, para desenhar por cima do vídeo (`None` até chegar
    /// a primeira posição). Compare [`CursorView::version`] para saber se
    /// mudou desde a última vez.
    pub fn cursor(&self) -> Option<CursorView> {
        let c = self.shared.cursor.lock().unwrap_or_else(|e| e.into_inner());
        c.visto.then(|| CursorView {
            version: c.versao,
            x: c.x as i32,
            y: c.y as i32,
            visible: c.visivel,
            shape: c
                .forma
                .as_ref()
                .filter(|(id, _)| *id == c.forma_id)
                .map(|(_, f)| f.clone()),
        })
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

/// Sessão nova, respondida mas ainda não confirmada: vira a ativa no
/// primeiro pacote do host que ela decifrar. Até lá a sessão antiga continua
/// valendo — um `PairInit` forjado, sem o código, não derruba a conexão.
struct Candidata {
    sessao: Session,
}

/// Resposta já mandada a um `PairInit`: reenviada se o mesmo `PairInit`
/// chegar de novo — reenvio do host, ou o mesmo pedido por outro caminho (o
/// host tenta todos os endereços nossos de uma vez). Gerar outra sessão
/// para ele desencontraria as chaves.
struct Respondido {
    init: Vec<u8>,
    pacote: Vec<u8>,
}

/// Registro no servidor de sinalização.
struct Sinalizacao {
    servidor: SocketAddr,
    /// Pedido `Register` pronto (sempre o mesmo).
    registro: Vec<u8>,
    proximo: Instant,
    confirmado: bool,
}

impl Sinalizacao {
    fn new(servidor: SocketAddr, codigo: &PairingCode, socket: &Portas) -> Self {
        let msg = SignalMessage::Register {
            room: codigo.room(),
            locals: crate::local_candidates(socket.v4()),
        };
        let registro = PacketKind::Signal
            .control(0, clock_us())
            .encode_with_payload(&msg.to_bytes())
            .expect("mensagem de sinalização cabe num pacote");
        Self {
            servidor,
            registro,
            proximo: Instant::now(),
            confirmado: false,
        }
    }

    /// Renova o registro, se está na hora.
    fn renovar(&mut self, socket: &Portas) {
        if Instant::now() >= self.proximo {
            let _ = socket.send_to(&self.registro, self.servidor);
            let espera = if self.confirmado {
                REGISTRO_VIVO
            } else {
                REGISTRO_PENDENTE
            };
            self.proximo = Instant::now() + espera;
        }
    }

    /// Trata uma mensagem do servidor. Um host procurando por nós: manda um
    /// furo a cada endereço dele, para os `PairInit` passarem pelo nosso NAT.
    fn tratar(&mut self, payload: &[u8], socket: &Portas, shared: &Shared) {
        match SignalMessage::from_bytes(payload) {
            Ok(SignalMessage::Registered { public }) => {
                self.confirmado = true;
                shared.with_stats(|s| {
                    s.public_addr = Some(public);
                    s.room_taken = false;
                });
            }
            Ok(SignalMessage::Found { public, locals }) => {
                let Ok(furo) = PacketKind::Punch.control(0, clock_us()).encode_with_payload(&[]) else {
                    return;
                };
                let mut n = 0;
                for destino in std::iter::once(public).chain(locals) {
                    n += socket.send_to(&furo, destino).is_ok() as u64;
                }
                shared.with_stats(|s| s.punches_sent += n);
            }
            Ok(SignalMessage::RoomTaken) => shared.with_stats(|s| s.room_taken = true),
            _ => shared.with_stats(|s| s.invalid_packets += 1),
        }
    }
}

fn recv_loop(
    mut socket: Portas,
    cfg: ReceiverConfig,
    frames: mpsc::SyncSender<ReceivedFrame>,
    audio: mpsc::SyncSender<AudioPacket>,
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
    // Pareamento (só com código): chave de abrir o que o host manda.
    let mut ativa: Option<Opener> = None;
    let mut candidata: Option<Candidata> = None;
    let mut respondido: Option<Respondido> = None;
    let mut sinal = match (cfg.signal_server, &cfg.pairing_code) {
        (Some(servidor), Some(codigo)) => Some(Sinalizacao::new(servidor, codigo, &socket)),
        _ => None,
    };

    let mut proximo_furo = Instant::now();
    let mut proximo_stun = Instant::now() + MANTER_NAT;
    // Diagnóstico: resumo dos furos a cada ~5 s, a partir do primeiro.
    let mut primeiro_furo: Option<Instant> = None;
    let mut proximo_resumo = Instant::now();

    while !shared.stop.load(Ordering::Relaxed) {
        if let Some(s) = sinal.as_mut() {
            s.renovar(&socket);
        }
        // Sem host ainda: furos pedidos e o mapeamento do NAT vivo.
        if peer.is_none() && Instant::now() >= proximo_furo {
            let destinos = furar(&socket, &shared);
            proximo_furo = Instant::now() + FURO_A_CADA;
            if let (Some(o), Some(destinos)) = (&cfg.on_pairing, destinos) {
                let desde = *primeiro_furo.get_or_insert_with(|| {
                    proximo_resumo = Instant::now() + RESUMO_A_CADA;
                    Instant::now()
                });
                if Instant::now() >= proximo_resumo {
                    o.notify(&PairingEvent::Waiting {
                        elapsed: desde.elapsed(),
                        what: "furos",
                        sent: shared.stats.lock().map(|s| s.punches_sent).unwrap_or(0),
                        to: destinos,
                    });
                    proximo_resumo = Instant::now() + RESUMO_A_CADA;
                }
            }
        }
        if let Some(stun) = cfg.keepalive_to.filter(|_| peer.is_none()) {
            if Instant::now() >= proximo_stun {
                let _ = socket.send_to(&fluxo_proto::stun::binding_request(&[0; 12]), stun);
                proximo_stun = Instant::now() + MANTER_NAT;
            }
        }
        if let Some(p) = peer {
            if Instant::now() >= next_ping {
                let ping = PacketKind::Ping
                    .control(ping_id, clock_us())
                    .encode_with_payload(&[]);
                if ping.is_ok_and(|pkt| shared.enviar(&socket, pkt, p)) {
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
                    .is_ok_and(|pkt| shared.enviar(&socket, pkt, p));
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
        let datagrama = &buf[..n];
        if let Some(o) = cfg.on_pairing.as_ref().filter(|_| peer.is_none()) {
            o.notify(&PairingEvent::Received {
                from,
                kind: kind_of(datagrama),
            });
        }
        if Some(from) == cfg.keepalive_to {
            continue; // resposta do STUN ao keepalive
        }
        let Ok((h, payload)) = PacketHeader::decode(datagrama) else {
            shared.with_stats(|s| s.invalid_packets += 1);
            continue;
        };
        // Sinalização (só do nosso servidor) e furos (só abrem caminho).
        match h.kind {
            PacketKind::Signal => {
                match sinal.as_mut().filter(|s| s.servidor == from) {
                    Some(s) => s.tratar(payload, &socket, &shared),
                    None => shared.with_stats(|s| s.invalid_packets += 1),
                }
                continue;
            }
            PacketKind::Punch => continue,
            _ => {}
        }

        // Daqui sai o pacote em claro, e se ele inaugura uma sessão (host novo).
        let claro: Vec<u8>;
        let (h, payload, host_novo) = match &cfg.pairing_code {
            Some(codigo) => {
                if h.kind == PacketKind::PairInit {
                    responder_pareamento(
                        &socket,
                        &shared,
                        codigo,
                        &h,
                        payload,
                        from,
                        &mut candidata,
                        &mut respondido,
                    );
                    continue;
                }
                if !h.is_encrypted() || h.kind.is_pairing() {
                    shared.with_stats(|s| s.invalid_packets += 1);
                    continue;
                }
                // A sessão ativa primeiro; senão a candidata, que vira a ativa.
                let mut host_novo = false;
                let aberto = match ativa.as_mut().map(|o| o.open(h.seq, datagrama, HEADER_LEN)) {
                    Some(Ok(c)) => Some(c),
                    _ => match candidata
                        .as_mut()
                        .map(|c| c.sessao.opener.open(h.seq, datagrama, HEADER_LEN))
                    {
                        Some(Ok(c)) => {
                            let Candidata { sessao } = candidata.take().expect("conferida acima");
                            ativa = Some(sessao.opener);
                            *shared.sealer.lock().unwrap_or_else(|e| e.into_inner()) = Some(sessao.sealer);
                            shared.with_stats(|s| s.encrypted = true);
                            host_novo = true;
                            Some(c)
                        }
                        _ => None,
                    },
                };
                let Some(c) = aberto else {
                    shared.with_stats(|s| s.invalid_packets += 1);
                    continue;
                };
                let h = h.decrypted(c.len());
                if h.check_payload(&c).is_err() {
                    shared.with_stats(|s| s.invalid_packets += 1);
                    continue;
                }
                claro = c;
                // Autenticado: o host pode ter mudado de endereço (NAT).
                if !host_novo && peer != Some(from) {
                    peer = Some(from);
                    shared.with_stats(|s| s.peer = Some(from));
                }
                (h, claro.as_slice(), host_novo)
            }
            None => {
                if h.is_encrypted() || h.kind.is_pairing() {
                    shared.with_stats(|s| s.invalid_packets += 1);
                    continue;
                }
                // Sem cifra: um host por vez; outro endereço só depois de
                // `peer_timeout` sem notícias do atual.
                let host_novo = match peer {
                    Some(p) if p == from => false,
                    Some(_) if last_from_peer.elapsed() < cfg.peer_timeout => {
                        shared.with_stats(|s| s.invalid_packets += 1);
                        continue;
                    }
                    _ => true,
                };
                (h, payload, host_novo)
            }
        };

        // Host novo: remontagem do zero (a numeração dos frames dele
        // recomeça e pareceria "atrasada").
        if host_novo {
            if peer.is_some() {
                reasm = Reassembler::new(cfg.reassembler);
                fila = QueueDelay::default();
                relogio = ClockSync::default();
                shared.with_stats(|s| s.peer_changes += 1);
            }
            peer = Some(from);
            // Pareou por uma família: a outra não precisa mais ser escutada.
            if socket.has_v6() && socket.v4().is_some() {
                let _ = socket.keep_only(from);
                let _ = socket.set_read_timeout(LEITURA);
            }
            next_ping = Instant::now();
            shared.with_stats(|s| s.peer = Some(from));
        }
        last_from_peer = Instant::now();

        match h.kind {
            // Só o host recebe pedidos de keyframe e relatórios; pareamento,
            // sinalização e furo já foram tratados acima.
            PacketKind::KeyframeRequest
            | PacketKind::Feedback
            | PacketKind::PairInit
            | PacketKind::PairReply
            | PacketKind::Signal
            | PacketKind::Punch => {}
            PacketKind::Cursor => on_cursor(payload, &socket, from, &shared),
            PacketKind::Audio => {
                let received_us = clock_us();
                let transit_us = clock_diff_us(received_us, h.timestamp_us);
                let pacote = AudioPacket {
                    seq: h.frame_id,
                    timestamp_us: h.timestamp_us,
                    received_us,
                    latency_us: relogio.offset_us().map(|o| transit_us.wrapping_add(o)),
                    data: payload.to_vec(),
                };
                // Fila cheia: ninguém está tocando o áudio; o pacote se perde.
                let cheio = matches!(audio.try_send(pacote), Err(TrySendError::Full(_)));
                shared.with_stats(|s| {
                    s.audio_packets += 1;
                    s.audio_dropped += cheio as u64;
                });
            }
            PacketKind::Ping => {
                if pong_for(&h).is_some_and(|p| shared.enviar(&socket, p, from)) {
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
                    // Atraso de fila pela hora em que o pacote que completou
                    // o frame **saiu** do host: só a rede conta, sem encode,
                    // espera do limite de fps nem pacing (que crescem com o
                    // tamanho do frame e pareceriam fila).
                    fila.add(clock_diff_us(received_us, h.send_us), Instant::now());
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

/// O cursor do host, como [`VideoReceiver::cursor`] o entrega.
#[derive(Debug, Clone)]
pub struct CursorView {
    /// Muda a cada posição ou forma nova.
    pub version: u64,
    /// Canto superior esquerdo da imagem do cursor, em pixels do vídeo.
    pub x: i32,
    pub y: i32,
    /// `false`: escondido ou em outro monitor do host.
    pub visible: bool,
    /// Imagem atual (`None` enquanto ela não chegou inteira).
    pub shape: Option<Arc<CursorShape>>,
}

/// Cursor do lado do cliente.
#[derive(Default)]
struct CursorClient {
    visto: bool,
    versao: u64,
    x: i16,
    y: i16,
    visivel: bool,
    /// Forma que a última posição usa.
    forma_id: u16,
    /// Última forma montada.
    forma: Option<(u16, Arc<CursorShape>)>,
    montagem: ShapeAssembler,
    ultimo_pedido: Option<Instant>,
}

/// Intervalo mínimo entre pedidos da mesma forma (tempo de ela chegar).
const PEDIDO_DE_FORMA: Duration = Duration::from_millis(200);

/// Trata um pacote de cursor do host (já decifrado).
fn on_cursor(payload: &[u8], socket: &Portas, de: SocketAddr, shared: &Shared) {
    let Ok(m) = CursorMessage::from_bytes(payload) else {
        return;
    };
    let mut c = shared.cursor.lock().unwrap_or_else(|e| e.into_inner());
    match m {
        CursorMessage::Position { x, y, visible, shape } => {
            let mudou = !c.visto || (c.x, c.y, c.visivel, c.forma_id) != (x, y, visible, shape);
            (c.visto, c.x, c.y, c.visivel, c.forma_id) = (true, x, y, visible, shape);
            if mudou {
                c.versao += 1;
            }
            // Forma que não temos (alguma parte se perdeu): pede de novo.
            let falta = shape != 0 && c.forma.as_ref().map_or(true, |(id, _)| *id != shape);
            let pode = c.ultimo_pedido.map_or(true, |t| t.elapsed() >= PEDIDO_DE_FORMA);
            if falta && pode {
                c.ultimo_pedido = Some(Instant::now());
                drop(c);
                let pedido = CursorMessage::ShapeRequest { shape }.to_bytes();
                let ok = PacketKind::Cursor
                    .control(0, clock_us())
                    .encode_with_payload(&pedido)
                    .is_ok_and(|p| shared.enviar(socket, p, de));
                shared.with_stats(|s| {
                    s.cursor_packets += 1;
                    s.cursor_requests += ok as u64;
                });
                return;
            }
        }
        m @ CursorMessage::ShapePart { .. } => {
            if let Some((id, forma)) = c.montagem.push(&m) {
                c.forma = Some((id, Arc::new(forma)));
                c.versao += 1;
            }
        }
        CursorMessage::ShapeRequest { .. } => {} // só o host recebe pedidos
    }
    drop(c);
    shared.with_stats(|s| s.cursor_packets += 1);
}

/// Manda os furos pedidos por [`VideoReceiver::punch`], se ainda no prazo, e
/// devolve para onde foram.
fn furar(socket: &Portas, shared: &Shared) -> Option<Vec<SocketAddr>> {
    let mut furos = shared.furos.lock().unwrap_or_else(|e| e.into_inner());
    let (destinos, ate) = furos.as_ref()?;
    if Instant::now() > *ate {
        *furos = None;
        return None;
    }
    let Ok(furo) = PacketKind::Punch.control(0, clock_us()).encode_with_payload(&[]) else {
        return None;
    };
    let mut n = 0;
    for d in destinos {
        n += socket.send_to(&furo, *d).is_ok() as u64;
    }
    let destinos = destinos.clone();
    drop(furos);
    shared.with_stats(|s| s.punches_sent += n);
    Some(destinos)
}

/// Responde a um `PairInit`: o mesmo de antes (reenvio do host) recebe a
/// mesma resposta; um novo gera uma sessão candidata.
#[allow(clippy::too_many_arguments)]
fn responder_pareamento(
    socket: &Portas,
    shared: &Shared,
    codigo: &PairingCode,
    h: &PacketHeader,
    payload: &[u8],
    from: SocketAddr,
    candidata: &mut Option<Candidata>,
    respondido: &mut Option<Respondido>,
) {
    if let Some(r) = respondido.as_ref().filter(|r| r.init == payload) {
        let _ = socket.send_to(&r.pacote, from);
        return;
    }
    let Ok((sessao, resposta)) = fluxo_crypto::respond(codigo, payload) else {
        shared.with_stats(|s| s.invalid_packets += 1);
        return;
    };
    let Ok(pacote) = PacketKind::PairReply
        .control(h.frame_id, clock_us())
        .encode_with_payload(&resposta)
    else {
        return;
    };
    let _ = socket.send_to(&pacote, from);
    *respondido = Some(Respondido {
        init: payload.to_vec(),
        pacote,
    });
    *candidata = Some(Candidata { sessao });
    shared.with_stats(|s| s.pairings += 1);
}
