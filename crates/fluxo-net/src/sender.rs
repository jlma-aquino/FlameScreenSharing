//! Lado do host: [`VideoSender`].

use std::collections::VecDeque;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fluxo_crypto::{CryptoError, HostHandshake, Opener, PairingCode, Sealer, Session, TAG_LEN};
use fluxo_proto::{
    prepare_seal, stamp_send_time, CursorMessage, CursorShape, Feedback, Fragmenter, PacketHeader,
    PacketKind, SignalMessage, DEFAULT_MAX_PAYLOAD, HEADER_LEN,
};

use crate::bitrate::NetworkReport;
use crate::history::{LossSample, LossWindow, SentHistory};
use crate::observar::{kind_of, PairingEvent, PairingObserver, RESUMO_A_CADA};
use crate::pacer::Pacer;
use crate::portas::Portas;
use crate::rtt::{RttEstimator, RttStats};
use crate::sim::{DelayLine, DelayModel, Rng};
use crate::sync::ClockSync;
use crate::{clock_diff_us, clock_us, is_transient, pong_clock, pong_for, NetError};

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
    /// **Só para testes:** atraso fixo somado em **cada sentido** (o RTT
    /// sobe o dobro). Vale para tudo o que o host manda e recebe.
    pub simulated_delay: Duration,
    /// **Só para testes:** atraso extra sorteado em `[0, jitter]` a cada
    /// 20 ms, em cada sentido; os pacotes nunca se ultrapassam.
    pub simulated_jitter: Duration,
    /// Código de pareamento mostrado pelo cliente. Com ele, [`VideoSender::connect`]
    /// faz o handshake antes de tudo e cifra todos os pacotes. `None`: sem
    /// cifra (rede de teste; o cliente também precisa estar sem código).
    pub pairing_code: Option<PairingCode>,
    /// Quanto [`VideoSender::connect`] espera o cliente responder ao pareamento.
    pub pairing_timeout: Duration,
    /// Recebe o que chega e um resumo do que sai enquanto o cliente não
    /// responde ao pareamento (diagnóstico de furo de NAT).
    pub on_pairing: Option<PairingObserver>,
    /// TTL dos pedidos de pareamento até chegar o primeiro furo do cliente
    /// (`None`: normal). Baixo o bastante para morrer antes do NAT do
    /// cliente: um pedido que chega lá antes do furo dele pode ocupar a porta
    /// (NATs Linux, como o do celular roteando), e o furo sai por outra. Ver
    /// [`PUNCH_TTL`].
    pub punch_ttl: Option<u32>,
    /// Chave do servidor de sinalização: a procura sai assinada com ela
    /// (servidor com chaves ignora pedido sem assinatura). `None`: servidor aberto.
    pub signal_key: Option<fluxo_crypto::ServerKey>,
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
            simulated_delay: Duration::ZERO,
            simulated_jitter: Duration::ZERO,
            pairing_code: None,
            pairing_timeout: Duration::from_secs(10),
            on_pairing: None,
            punch_ttl: None,
            signal_key: None,
        }
    }
}

/// TTL sugerido para [`SenderConfig::punch_ttl`]: tem que passar pelos NATs
/// de quem transmite e morrer antes do NAT de quem recebe. Medido entre uma
/// rede de casa com CGNAT (IP público no 7º salto) e uma de celular com
/// 464XLAT (o tradutor NAT64 fica escondido na rede IPv6): 8 basta de casa,
/// mas não do celular; 12 funciona nos dois sentidos (o caminho todo tem mais
/// de 12 saltos).
pub const PUNCH_TTL: u32 = 12;

/// Intervalo entre reenvios do `PairInit` enquanto o cliente não responde.
const REENVIO_PAREAMENTO: Duration = Duration::from_millis(250);

/// Endereços do cliente tentados ao mesmo tempo, no máximo (o público e os
/// locais que o servidor informou, mais os de onde chegou furo).
const MAX_CANDIDATOS: usize = 8;

/// Como achar o cliente.
enum Rota {
    /// Endereços conhecidos (mesma rede, código de conexão, porta aberta no
    /// roteador dele).
    Direta(Vec<SocketAddr>),
    /// Pelo servidor de sinalização, na sala do código.
    Sinal(SocketAddr),
}

/// Uma tentativa de handshake: o `PairInit` e o estado para conferir a resposta.
struct Tentativa {
    hs: HostHandshake,
    id: u32,
    init: Vec<u8>,
}

impl Tentativa {
    fn nova(codigo: &PairingCode) -> Result<Self, NetError> {
        let hs = HostHandshake::start(codigo);
        let id = clock_us();
        let init = PacketKind::PairInit
            .control(id, clock_us())
            .encode_with_payload(hs.message())
            .map_err(NetError::Proto)?;
        Ok(Self { hs, id, init })
    }
}

/// Handshake com o cliente: manda `PairInit` a cada candidato até chegar o
/// `PairReply` correspondente (ou acabar o prazo). Devolve a sessão e o
/// endereço que respondeu, que vira o destino do vídeo.
///
/// Pela sinalização, a cada volta também pede a sala ao servidor, que
/// devolve os endereços do cliente e avisa o cliente para furar o NAT dele
/// na nossa direção; os nossos `PairInit` furam o nosso. Um furo do cliente
/// vindo de um endereço desconhecido (NAT que troca a porta por destino)
/// vira candidato também. Um candidato que responde com outra chave é outro
/// cliente Fluxo que calhou de estar naquele endereço (um endereço local
/// que, na nossa rede, é de outro PC): sai da lista, e o handshake recomeça
/// com os outros.
fn parear(
    socket: &Portas,
    codigo: &PairingCode,
    prazo: Duration,
    rota: &Rota,
    observar: Option<&PairingObserver>,
    mut ttl_normal: Option<u32>,
    chave: Option<&fluxo_crypto::ServerKey>,
) -> Result<(Session, SocketAddr), NetError> {
    let mut t = Tentativa::nova(codigo)?;
    let (mut candidatos, servidor) = match *rota {
        Rota::Direta(ref peers) => (peers.clone(), None),
        Rota::Sinal(servidor) => (Vec::new(), Some(servidor)),
    };
    let procura = match servidor {
        Some(_) => Some(
            PacketKind::Signal
                .control(t.id, clock_us())
                .encode_with_payload(
                    &SignalMessage::Lookup {
                        room: codigo.room(),
                        locals: crate::local_candidates(socket.v4(), socket.v6()),
                    }
                    .to_bytes(),
                )
                .map_err(NetError::Proto)?,
        ),
        None => None,
    };
    // Servidor com chaves: a procura vai assinada.
    let procura = match (procura, chave) {
        (Some(p), Some(k)) => Some(k.signed(&p)),
        (p, _) => p,
    };
    let mut servidor_respondeu = false;
    let mut recusados: Vec<SocketAddr> = Vec::new();
    // Diagnóstico (com observador): quantos PairInit saíram e quando resumir.
    let inicio = Instant::now();
    let mut enviados = 0u64;
    let mut proximo_resumo = inicio + RESUMO_A_CADA;
    socket
        .set_read_timeout(Duration::from_millis(50))
        .map_err(NetError::io("configurar socket"))?;
    let limite = Instant::now() + prazo;
    let mut buf = [0u8; 2048];
    while Instant::now() < limite {
        // Erro de envio (porta do outro lado ainda fechada, por exemplo):
        // tenta de novo na próxima volta.
        if let (Some(s), Some(p)) = (servidor, &procura) {
            // O pedido ao servidor vai com o TTL normal: o baixo, que serve
            // para furar o nosso NAT sem chegar ao do cliente, morreria antes
            // do servidor.
            let baixo = ttl_normal.and_then(|normal| {
                let v4 = socket.v4()?;
                let baixo = v4.ttl().ok()?;
                v4.set_ttl(normal).ok()?;
                Some(baixo)
            });
            let _ = socket.send_to(p, s);
            if let (Some(b), Some(v4)) = (baixo, socket.v4()) {
                let _ = v4.set_ttl(b);
            }
        }
        for c in &candidatos {
            enviados += socket.send_to(&t.init, *c).is_ok() as u64;
        }
        if let Some(o) = observar.filter(|_| Instant::now() >= proximo_resumo) {
            o.notify(&PairingEvent::Waiting {
                elapsed: inicio.elapsed(),
                what: match ttl_normal {
                    Some(_) => "pedidos de pareamento com TTL baixo",
                    None => "pedidos de pareamento",
                },
                sent: enviados,
                to: candidatos.clone(),
            });
            proximo_resumo = Instant::now() + RESUMO_A_CADA;
        }
        let reenvio = Instant::now() + REENVIO_PAREAMENTO;
        while Instant::now() < reenvio {
            let Ok((n, de)) = socket.recv_from(&mut buf) else {
                continue;
            };
            if let Some(o) = observar {
                o.notify(&PairingEvent::Received {
                    from: de,
                    kind: kind_of(&buf[..n]),
                });
            }
            let Ok((h, payload)) = PacketHeader::decode(&buf[..n]) else {
                continue; // lixo na porta
            };
            match h.kind {
                PacketKind::Signal if Some(de) == servidor => {
                    servidor_respondeu = true;
                    if let Ok(SignalMessage::Found { public, locals }) = SignalMessage::from_bytes(payload) {
                        for c in std::iter::once(public).chain(locals) {
                            novo_candidato(&mut candidatos, &recusados, c);
                        }
                    }
                }
                PacketKind::Punch => {
                    novo_candidato(&mut candidatos, &recusados, de);
                    // O furo do cliente chegou: o caminho está aberto dos dois
                    // lados, e os pedidos já podem ir até ele.
                    if let Some(n) = ttl_normal.take() {
                        if let Some(v4) = socket.v4() {
                            let _ = v4.set_ttl(n);
                        }
                    }
                }
                // Só de um candidato: quem não sabe por onde o cliente anda
                // não derruba o pareamento com uma resposta falsa.
                PacketKind::PairReply if h.frame_id == t.id && candidatos.contains(&de) => {
                    let feita = std::mem::replace(&mut t, Tentativa::nova(codigo)?);
                    match feita.hs.finish(payload) {
                        Ok(sessao) => return Ok((sessao, de)),
                        Err(CryptoError::WrongCode) if servidor.is_some() => {
                            candidatos.retain(|c| *c != de);
                            recusados.push(de);
                        }
                        Err(e) => return Err(NetError::Pairing(e.to_string())),
                    }
                }
                _ => {}
            }
        }
    }
    let motivo = match (servidor, servidor_respondeu, candidatos.is_empty()) {
        (Some(s), false, _) => format!("o servidor de sinalização {s} não respondeu"),
        (Some(_), true, true) if !recusados.is_empty() => {
            "achei um cliente, mas o código de pareamento não confere".to_string()
        }
        (Some(_), true, true) => "nenhum cliente com esse código no servidor de sinalização".to_string(),
        (Some(_), true, false) => {
            "o cliente foi achado, mas os pacotes não passaram (NAT dos dois lados restritivo?)".to_string()
        }
        (None, ..) => "endereço, firewall ou cliente sem código?".to_string(),
    };
    Err(NetError::Pairing(format!(
        "o cliente não respondeu em {} s ({motivo})",
        prazo.as_secs()
    )))
}

fn novo_candidato(candidatos: &mut Vec<SocketAddr>, recusados: &[SocketAddr], c: SocketAddr) {
    if !candidatos.contains(&c) && !recusados.contains(&c) && candidatos.len() < MAX_CANDIDATOS {
        candidatos.push(c);
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
    /// Pedidos de limpeza depois de perda recebidos (uma onda de intra-refresh
    /// serve; ver [`VideoSender::take_refresh_request`]).
    pub refresh_requests: u64,
    /// Pacotes descartados de propósito por [`SenderConfig::simulated_loss`]
    /// e pela fila cheia de [`SenderConfig::simulated_bandwidth_bps`].
    pub simulated_drops: u64,
    /// Relatórios de rede recebidos do cliente.
    pub feedbacks_received: u64,
    /// Perda calculada no último relatório (fração), se houve pacotes suficientes.
    pub last_loss: Option<f64>,
    /// Atraso de fila do último relatório do cliente, em ms.
    pub queue_delay_ms: f64,
    /// Relógio do cliente menos o nosso, em ms (`None` antes da 1ª medição).
    pub clock_offset_ms: Option<f64>,
    pub rtt: RttStats,
    /// A sessão é cifrada (pareamento com código).
    pub encrypted: bool,
    /// Pacotes recebidos que não decifraram (forjados, de outra sessão) ou
    /// em claro numa sessão cifrada.
    pub rejected_packets: u64,
    /// Pacotes de cursor enviados (posição e partes de forma).
    pub cursor_packets: u64,
    /// Quadros de áudio enviados, e os bytes de Opus neles.
    pub audio_packets: u64,
    pub audio_bytes: u64,
}

struct Job {
    data: Vec<u8>,
    flags: u8,
    timestamp_us: u32,
    queued_at: Instant,
}

pub(crate) struct Shared {
    stats: Mutex<SenderStats>,
    rtt: Mutex<RttEstimator>,
    pacing_bps: AtomicU64,
    pub(crate) stop: AtomicBool,
    keyframe_requested: AtomicBool,
    refresh_requested: AtomicBool,
    /// Codecs que o cliente decodifica (máscara de `VideoCodec::bit`; 0 =
    /// ainda não disse).
    codecs_cliente: std::sync::atomic::AtomicU8,
    /// Pacotes de vídeo/paridade que tentamos mandar (inclui os descartados
    /// pelas simulações), contados um a um na hora em que saem: base da
    /// conta de perda.
    sent_history: Mutex<SentHistory>,
    /// Relatórios da última ~1 s, para a conta de perda (ver `history.rs`).
    loss_window: Mutex<LossWindow>,
    /// Último relatório ainda não consumido por [`VideoSender::take_report`].
    report: Mutex<Option<NetworkReport>>,
    /// Quando chegou o relatório anterior e quantos bytes tinham saído até
    /// ali, para medir a taxa de envio real ([`NetworkReport::sent_kbps`]).
    envio_anterior: Mutex<Option<(Instant, u64)>>,
    /// Estado do cursor, para repetir a posição e reenviar a forma pedida.
    cursor: Mutex<CursorHost>,
    /// Quando chegou o último pacote do cliente (`None`: ainda nenhum).
    ultimo_contato: Mutex<Option<Instant>>,
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
    /// Por onde saem o cursor e o áudio (da thread de quem chama).
    saida_cursor: Saida,
    /// Thread da linha de atraso simulada, se houver.
    delay_thread: Option<JoinHandle<()>>,
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
        let peer = peer
            .to_socket_addrs()
            .map_err(NetError::io("resolver destino"))?
            .next()
            .ok_or_else(|| NetError::Io {
                context: "resolver destino",
                message: "nenhum endereço".into(),
            })?;
        let socket = UdpSocket::bind(bind).map_err(NetError::io("abrir socket UDP"))?;
        let portas = Portas::one(socket).map_err(NetError::io("abrir socket UDP"))?;
        Self::start(portas, Rota::Direta(vec![peer]), cfg)
    }

    /// Como [`connect`](Self::connect), mas acha o cliente pelo servidor de
    /// sinalização em `server` (redes diferentes, atrás de NAT), na sala do
    /// [`SenderConfig::pairing_code`] — obrigatório aqui.
    pub fn connect_via_signal(
        bind: impl ToSocketAddrs,
        server: SocketAddr,
        cfg: SenderConfig,
    ) -> Result<Self, NetError> {
        if cfg.pairing_code.is_none() {
            return Err(NetError::Pairing(
                "a sinalização precisa do código de pareamento".into(),
            ));
        }
        let socket = UdpSocket::bind(bind).map_err(NetError::io("abrir socket UDP"))?;
        // IPv6 também, se houver: o endereço dele vai ao cliente pelo servidor.
        let portas = Portas::new(Some(socket), crate::ipv6_socket_for_signal())
            .map_err(NetError::io("abrir socket UDP"))?;
        Self::start(portas, Rota::Sinal(server), cfg)
    }

    /// Como [`connect`](Self::connect), num socket já aberto — o mesmo em que
    /// [`discover_public_addr`](crate::discover_public_addr) descobriu o
    /// endereço público que foi no código de resposta. Os `PairInit` para
    /// `peer` (do código de conexão) furam o nosso NAT; um furo do cliente
    /// vindo de outra porta vira candidato também.
    pub fn connect_with_socket(
        socket: UdpSocket,
        peer: SocketAddr,
        cfg: SenderConfig,
    ) -> Result<Self, NetError> {
        let portas = Portas::one(socket).map_err(NetError::io("abrir socket UDP"))?;
        Self::start(portas, Rota::Direta(vec![peer]), cfg)
    }

    /// Como [`connect_with_socket`](Self::connect_with_socket), com um
    /// socket IPv6 também, tentando todos os `peers` (do código de conexão)
    /// ao mesmo tempo. O vídeo vai pelo que parear primeiro; o TTL baixo
    /// ([`SenderConfig::punch_ttl`]) vale só no IPv4 (IPv6 não tem NAT para
    /// ocupar a porta).
    pub fn connect_with_sockets(
        v4: UdpSocket,
        v6: Option<UdpSocket>,
        peers: Vec<SocketAddr>,
        cfg: SenderConfig,
    ) -> Result<Self, NetError> {
        let portas = Portas::new(Some(v4), v6).map_err(NetError::io("abrir socket UDP"))?;
        Self::start(portas, Rota::Direta(peers), cfg)
    }

    fn start(portas: Portas, rota: Rota, cfg: SenderConfig) -> Result<Self, NetError> {
        // Pareamento antes de qualquer outro pacote: com código, nada sai em
        // claro. O socket só se prende ao cliente depois, no endereço que
        // respondeu.
        let (cifra, opener, peer) = match (&cfg.pairing_code, &rota) {
            (Some(codigo), _) => {
                // TTL baixo até o furo do cliente; o normal volta depois, dê
                // certo ou não.
                let ttl_normal = match (cfg.punch_ttl, portas.v4()) {
                    (Some(baixo), Some(v4)) => {
                        let normal = v4.ttl().map_err(NetError::io("ler TTL"))?;
                        v4.set_ttl(baixo).map_err(NetError::io("configurar TTL"))?;
                        Some(normal)
                    }
                    _ => None,
                };
                let pareado = parear(
                    &portas,
                    codigo,
                    cfg.pairing_timeout,
                    &rota,
                    cfg.on_pairing.as_ref(),
                    ttl_normal,
                    cfg.signal_key.as_ref(),
                );
                if let (Some(normal), Some(v4)) = (ttl_normal, portas.v4()) {
                    v4.set_ttl(normal).map_err(NetError::io("configurar TTL"))?;
                }
                let (s, peer) = pareado?;
                (Some(Arc::new(Mutex::new(s.sealer))), Some(s.opener), peer)
            }
            (None, Rota::Direta(peers)) => {
                let peer = *peers
                    .first()
                    .ok_or_else(|| NetError::Pairing("nenhum destino".into()))?;
                (None, None, peer)
            }
            (None, Rota::Sinal(_)) => unreachable!("conferido em connect_via_signal"),
        };
        // Só o socket da família que pareou continua.
        let socket = portas
            .into_socket(peer)
            .map_err(NetError::io("definir destino"))?;
        socket.connect(peer).map_err(NetError::io("definir destino"))?;
        // Timeout curto na leitura: a thread de recepção confere `stop` a cada volta.
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(NetError::io("configurar socket"))?;
        let local = socket.local_addr().map_err(NetError::io("endereço local"))?;
        let peer = socket.peer_addr().map_err(NetError::io("endereço do destino"))?;

        let shared = Arc::new(Shared {
            stats: Mutex::new(SenderStats {
                encrypted: cifra.is_some(),
                ..SenderStats::default()
            }),
            rtt: Mutex::new(RttEstimator::default()),
            pacing_bps: AtomicU64::new(cfg.pacing_bps),
            stop: AtomicBool::new(false),
            keyframe_requested: AtomicBool::new(false),
            refresh_requested: AtomicBool::new(false),
            codecs_cliente: std::sync::atomic::AtomicU8::new(0),
            sent_history: Mutex::new(SentHistory::default()),
            loss_window: Mutex::new(LossWindow::default()),
            report: Mutex::new(None),
            envio_anterior: Mutex::new(None),
            cursor: Mutex::new(CursorHost::default()),
            // Pareado é contato; sem cifra, o cliente pode nem ter aberto ainda.
            ultimo_contato: Mutex::new(cifra.is_some().then(Instant::now)),
        });
        let (queue, jobs) = mpsc::channel();

        let duplicar = || socket.try_clone().map_err(NetError::io("duplicar socket"));
        let simula_atraso = !cfg.simulated_delay.is_zero() || !cfg.simulated_jitter.is_zero();
        let (atraso, delay_thread) = match simula_atraso {
            true => {
                let modelo = DelayModel::new(cfg.simulated_delay, cfg.simulated_jitter);
                let (linha, t) = DelayLine::spawn(duplicar()?, modelo, shared.clone())
                    .map_err(NetError::io("criar thread de atraso"))?;
                (Some(linha), Some(t))
            }
            false => (None, None),
        };
        // O que chega também atrasa (o outro sentido do link simulado).
        let atraso_chegada =
            simula_atraso.then(|| DelayModel::new(cfg.simulated_delay, cfg.simulated_jitter));

        // O cursor sai na hora, da thread de quem chama (pacotes pequenos,
        // sem pacing).
        let saida_cursor = Saida {
            socket: duplicar()?,
            atraso: atraso.clone(),
            cifra: cifra.clone(),
        };
        let saida = Saida {
            socket: duplicar()?,
            atraso: atraso.clone(),
            cifra: cifra.clone(),
        };
        let s = shared.clone();
        let sender = thread::Builder::new()
            .name("fluxo-net-envio".into())
            .spawn(move || send_loop(saida, jobs, cfg, s))
            .map_err(NetError::io("criar thread de envio"))?;
        let saida = Saida {
            socket: duplicar()?,
            atraso,
            cifra,
        };
        let s = shared.clone();
        let receiver = thread::Builder::new()
            .name("fluxo-net-pings".into())
            .spawn(move || recv_loop(socket, saida, atraso_chegada, opener, s))
            .map_err(NetError::io("criar thread de recepção"))?;

        Ok(Self {
            queue: Some(queue),
            shared,
            send_thread: Some(sender),
            recv_thread: Some(receiver),
            delay_thread,
            saida_cursor,
            local,
            peer,
        })
    }

    /// Enfileira um frame codificado para envio. `timestamp_us` deve ser o
    /// [`clock_us`] do momento da captura.
    pub fn send_frame(&self, data: Vec<u8>, keyframe: bool, timestamp_us: u32) -> Result<(), NetError> {
        let flags = if keyframe { fluxo_proto::FLAG_KEYFRAME } else { 0 };
        self.send_frame_with_flags(data, flags, timestamp_us)
    }

    /// Como [`send_frame`](Self::send_frame), com o campo `flags` completo:
    /// [`FLAG_KEYFRAME`](fluxo_proto::FLAG_KEYFRAME) e as marcas de onda de
    /// intra-refresh ([`FLAG_REFRESH_START`](fluxo_proto::FLAG_REFRESH_START),
    /// [`FLAG_REFRESH_END`](fluxo_proto::FLAG_REFRESH_END)).
    pub fn send_frame_with_flags(&self, data: Vec<u8>, flags: u8, timestamp_us: u32) -> Result<(), NetError> {
        let job = Job {
            data,
            flags,
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
    /// chamada — e **só** um keyframe serve (ele não tem imagem nenhuma, ou o
    /// decoder falhou). Vários pedidos seguidos contam como um só.
    pub fn take_keyframe_request(&self) -> bool {
        self.shared.keyframe_requested.swap(false, Ordering::Relaxed)
    }

    /// Codecs que o cliente disse decodificar (máscara de
    /// [`VideoCodec::bit`](fluxo_proto::VideoCodec::bit)), esperando até
    /// `prazo` pela primeira notícia. `None`: nada chegou no prazo (os Caps
    /// se perderam) — use H.264, que todo cliente decodifica.
    pub fn wait_client_codecs(&self, prazo: Duration) -> Option<u8> {
        let limite = Instant::now() + prazo;
        loop {
            match self.shared.codecs_cliente.load(Ordering::Relaxed) {
                0 if Instant::now() < limite => thread::sleep(Duration::from_millis(5)),
                0 => return None,
                m => return Some(m),
            }
        }
    }

    /// `true` (uma vez) se o cliente perdeu algo e pediu para limpar a
    /// imagem: uma onda de intra-refresh serve; sem ela, mande um keyframe.
    pub fn take_refresh_request(&self) -> bool {
        self.shared.refresh_requested.swap(false, Ordering::Relaxed)
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

    /// Há quanto tempo não chega nada do cliente, que manda pings a cada 500
    /// ms. `None`: nunca chegou nada (sem cifra, ele pode nem ter aberto).
    pub fn silent_for(&self) -> Option<Duration> {
        let contato = self
            .shared
            .ultimo_contato
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        contato.map(|t| t.elapsed())
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

    /// Posição do cursor (canto superior esquerdo da imagem dele, em pixels
    /// do monitor capturado). Sai na hora; a última também é repetida junto
    /// com os pings, para um pacote perdido não deixar o cursor parado.
    pub fn send_cursor_position(&self, x: i32, y: i32, visible: bool) {
        let mut c = self.shared.cursor.lock().unwrap_or_else(|e| e.into_inner());
        c.pos = Some((clamp_i16(x), clamp_i16(y), visible));
        let pacote = c.position_packet();
        drop(c);
        if let Some(p) = pacote {
            enviar_cursor(&self.saida_cursor, &self.shared, &p);
        }
    }

    /// Um quadro de áudio Opus: `seq` sobe de 1 em 1 a cada quadro (o
    /// cliente percebe perdas e ordena), `timestamp_us` é o [`clock_us`] da
    /// captura. Sai na hora, sem passar pela fila do vídeo.
    pub fn send_audio(&self, seq: u32, timestamp_us: u32, opus: &[u8]) {
        let ok = PacketKind::Audio
            .control(seq, timestamp_us)
            .encode_with_payload(opus)
            .is_ok_and(|p| self.saida_cursor.send(&p).is_ok());
        self.shared.with_stats(|s| match ok {
            true => {
                s.audio_packets += 1;
                s.audio_bytes += opus.len() as u64;
            }
            false => s.send_errors += 1,
        });
    }

    /// Forma nova do cursor: ganha um número e sai na hora, em partes. O
    /// cliente que perder alguma pede de novo (ver `on_packet`).
    pub fn set_cursor_shape(&self, shape: CursorShape) {
        let mut c = self.shared.cursor.lock().unwrap_or_else(|e| e.into_inner());
        // 0 = "sem forma" para o cliente.
        c.shape_id = c.shape_id.wrapping_add(1).max(1);
        let partes = fluxo_proto::cursor::shape_parts(c.shape_id, &shape);
        c.shape = Some(Arc::new(shape));
        // Envia ainda com o estado travado: a posição repetida pelos pings
        // não pode anunciar a forma nova antes de ela sair.
        for p in partes.iter().filter_map(|p| cursor_packet(p)) {
            enviar_cursor(&self.saida_cursor, &self.shared, &p);
        }
        if let Some(p) = c.position_packet() {
            enviar_cursor(&self.saida_cursor, &self.shared, &p);
        }
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
        // A linha de atraso entrega o que ainda segura e termina.
        if let Some(t) = self.delay_thread.take() {
            let _ = t.join();
        }
    }
}

/// Cursor do lado do host: o último estado, para repetir e reenviar.
#[derive(Default)]
struct CursorHost {
    pos: Option<(i16, i16, bool)>,
    shape_id: u16,
    shape: Option<Arc<CursorShape>>,
    /// Último reenvio da forma a pedido (no máximo um a cada 100 ms).
    ultimo_reenvio: Option<Instant>,
}

impl CursorHost {
    fn position_packet(&self) -> Option<Vec<u8>> {
        let (x, y, visible) = self.pos?;
        let m = CursorMessage::Position {
            x,
            y,
            visible,
            shape: self.shape_id,
        };
        cursor_packet(&m.to_bytes())
    }
}

fn cursor_packet(payload: &[u8]) -> Option<Vec<u8>> {
    PacketKind::Cursor
        .control(0, clock_us())
        .encode_with_payload(payload)
        .ok()
}

fn clamp_i16(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

fn enviar_cursor(saida: &Saida, shared: &Shared, pacote: &[u8]) {
    let ok = saida.send(pacote).is_ok();
    shared.with_stats(|s| match ok {
        true => s.cursor_packets += 1,
        false => s.send_errors += 1,
    });
}

/// O cliente pediu a forma `id`: reenvia, se for a atual (e não acabou de
/// reenviar).
fn reenviar_forma(id: u16, saida: &Saida, shared: &Shared) {
    let mut c = shared.cursor.lock().unwrap_or_else(|e| e.into_inner());
    let recente = c
        .ultimo_reenvio
        .is_some_and(|t| t.elapsed() < Duration::from_millis(100));
    let Some(forma) = c.shape.clone().filter(|_| c.shape_id == id && !recente) else {
        return;
    };
    c.ultimo_reenvio = Some(Instant::now());
    drop(c);
    for p in fluxo_proto::cursor::shape_parts(id, &forma) {
        if let Some(pacote) = cursor_packet(&p) {
            enviar_cursor(saida, shared, &pacote);
        }
    }
}

/// Por onde os pacotes saem: cifrados (com pareamento) e direto no socket ou
/// pela linha de atraso simulada.
struct Saida {
    socket: UdpSocket,
    atraso: Option<Arc<DelayLine>>,
    /// Chave do sentido host → cliente, dividida entre as duas threads (o
    /// número de sequência é um só).
    cifra: Option<Arc<Mutex<Sealer>>>,
}

impl Saida {
    /// Manda um pacote montado. Cifra por último: `send_us` já está gravado
    /// e o cabeçalho inteiro vira dado autenticado.
    fn send(&self, pacote: &[u8]) -> std::io::Result<usize> {
        let cifrado;
        let pacote = match &self.cifra {
            Some(c) => {
                let mut sealer = c.lock().unwrap_or_else(|e| e.into_inner());
                let seq = sealer.next_seq().ok_or_else(|| {
                    std::io::Error::other("sessão esgotou os números de sequência; pareie de novo")
                })?;
                let mut p = pacote.to_vec();
                prepare_seal(&mut p, seq, TAG_LEN);
                sealer
                    .seal(seq, &mut p, HEADER_LEN)
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                cifrado = p;
                &cifrado[..]
            }
            None => pacote,
        };
        match &self.atraso {
            Some(linha) => Ok(linha.send(pacote)),
            None => self.socket.send(pacote),
        }
    }
}

fn send_loop(socket: Saida, jobs: mpsc::Receiver<Job>, cfg: SenderConfig, shared: Arc<Shared>) {
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
            // A última posição do cursor também, caso uma tenha se perdido.
            let posicao = shared.cursor.lock().map(|c| c.position_packet()).unwrap_or(None);
            if let Some(p) = posicao {
                enviar_cursor(&socket, &shared, &p);
            }
            next_ping = Instant::now() + cfg.ping_interval;
        }
    }
}

fn send_ping(socket: &Saida, shared: &Shared, ping_id: &mut u32) {
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
    socket: &Saida,
    frag: &mut Fragmenter,
    pacer: &mut Pacer,
    shared: &Shared,
    cfg: &SenderConfig,
    rng: &mut Rng,
    job: Job,
) {
    let start = Instant::now();
    let queue_wait = start - job.queued_at;
    let mut packets = match frag.fragment_with_flags(&job.data, job.flags, job.timestamp_us) {
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
    // Na banda simulada, a fila de envio faz o papel da fila do roteador,
    // que numa rede real fica **depois** da saída: os pacotes levam a hora em
    // que o frame entrou na fila, para o cliente enxergar esse atraso.
    let entrou_no_roteador = (cfg.simulated_bandwidth_bps > 0)
        .then(|| clock_us().wrapping_sub(queue_wait.as_micros().min(u32::MAX as u128) as u32));

    let (mut sent, mut bytes, mut errors, mut dropped) = (0u64, 0u64, 0u64, 0u64);
    for p in &mut packets {
        let wait = pacer.delay(Instant::now());
        if !wait.is_zero() {
            thread::sleep(wait);
        }
        stamp_send_time(p, entrou_no_roteador.unwrap_or_else(clock_us));
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

fn recv_loop(
    socket: UdpSocket,
    saida: Saida,
    mut atraso: Option<DelayModel>,
    mut opener: Option<Opener>,
    shared: Arc<Shared>,
) {
    let mut buf = [0u8; 2048];
    // Diferença entre o relógio do cliente e o nosso, pelos Ping/Pong.
    let mut relogio = ClockSync::default();
    // Atraso simulado na chegada: pacotes recebidos esperando a sua hora.
    let mut segurados: VecDeque<(Instant, Vec<u8>)> = VecDeque::new();
    while !shared.stop.load(Ordering::Relaxed) {
        while segurados
            .front()
            .is_some_and(|(quando, _)| *quando <= Instant::now())
        {
            if let Some((_, pacote)) = segurados.pop_front() {
                on_packet(&pacote, &saida, &mut relogio, &mut opener, &shared);
            }
        }
        if atraso.is_some() {
            // Acorda a tempo do próximo pacote segurado (mínimo 1 ms: zero
            // significaria "sem timeout").
            let espera = segurados
                .front()
                .map_or(Duration::from_millis(100), |(quando, _)| {
                    quando.saturating_duration_since(Instant::now())
                });
            let _ = socket.set_read_timeout(Some(
                espera.clamp(Duration::from_millis(1), Duration::from_millis(100)),
            ));
        }
        let n = match socket.recv(&mut buf) {
            Ok(n) => {
                *shared.ultimo_contato.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                n
            }
            Err(e) if is_transient(&e) => continue,
            Err(_) => {
                // Erro inesperado: evita girar em falso e tenta de novo.
                thread::sleep(Duration::from_millis(10));
                continue;
            }
        };
        match atraso.as_mut() {
            Some(modelo) => segurados.push_back((modelo.due(Instant::now()), buf[..n].to_vec())),
            None => on_packet(&buf[..n], &saida, &mut relogio, &mut opener, &shared),
        }
    }
}

/// Trata um pacote que chegou do cliente. Com pareamento, só aceita o que
/// decifrar com a chave da sessão.
fn on_packet(
    pacote: &[u8],
    socket: &Saida,
    relogio: &mut ClockSync,
    opener: &mut Option<Opener>,
    shared: &Shared,
) {
    let Ok((h, payload)) = PacketHeader::decode(pacote) else {
        return; // lixo na porta: ignorar
    };
    let claro;
    let (h, payload) = match opener {
        Some(o) => {
            let aberto = match h.is_encrypted() {
                true => o.open(h.seq, pacote, HEADER_LEN).ok(),
                false => None,
            };
            let Some(c) = aberto else {
                shared.with_stats(|s| s.rejected_packets += 1);
                return;
            };
            let h = h.decrypted(c.len());
            if h.check_payload(&c).is_err() {
                return;
            }
            claro = c;
            (h, claro.as_slice())
        }
        None if h.is_encrypted() => return,
        None => (h, payload),
    };
    match h.kind {
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
            let offset = relogio.offset_us();
            shared.with_stats(|s| {
                s.pongs_received += 1;
                s.clock_offset_ms = offset.map(|o| o as f64 / 1000.0);
            });
        }
        PacketKind::Ping => {
            if pong_for(&h).is_some_and(|p| socket.send(&p).is_ok()) {
                shared.with_stats(|s| s.pings_answered += 1);
            }
        }
        PacketKind::KeyframeRequest => {
            if h.flags & fluxo_proto::FLAG_KEYFRAME != 0 {
                shared.keyframe_requested.store(true, Ordering::Relaxed);
                shared.with_stats(|s| s.keyframe_requests += 1);
            } else {
                shared.refresh_requested.store(true, Ordering::Relaxed);
                shared.with_stats(|s| s.refresh_requests += 1);
            }
        }
        PacketKind::Cursor => {
            if let Ok(CursorMessage::ShapeRequest { shape }) = CursorMessage::from_bytes(payload) {
                reenviar_forma(shape, socket, shared);
            }
        }
        PacketKind::Feedback => {
            if let Ok(f) = Feedback::from_bytes(payload) {
                on_feedback(shared, &f);
            }
        }
        // O host não recebe vídeo; pareamento, sinalização e furo depois da
        // sessão pronta são sobra
        // (reenvios de quem ainda não sabia que deu certo).
        PacketKind::Video
        | PacketKind::Parity
        | PacketKind::PairInit
        | PacketKind::PairReply
        | PacketKind::Signal
        | PacketKind::Punch
        | PacketKind::Audio => {}
        PacketKind::Caps => {
            if let Some(&m) = payload.first() {
                // Bit 0 (H.264) sempre: um cliente sem nenhum codec não existe.
                shared.codecs_cliente.store(m | 1, Ordering::Relaxed);
            }
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
    // Quanto mandamos de fato desde o relatório anterior.
    let agora = Instant::now();
    let bytes = shared.stats.lock().map(|s| s.bytes_sent).unwrap_or(0);
    let sent_kbps = {
        let mut anterior = shared.envio_anterior.lock().unwrap_or_else(|e| e.into_inner());
        let taxa = anterior.and_then(|(quando, b)| {
            let ms = agora.duration_since(quando).as_millis() as u64;
            (ms > 0).then(|| (bytes.saturating_sub(b) * 8 / ms) as u32)
        });
        *anterior = Some((agora, bytes));
        taxa
    };
    let report = NetworkReport {
        loss,
        queue_delay_ms: f.queue_delay_us as f64 / 1000.0,
        sent_kbps,
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
