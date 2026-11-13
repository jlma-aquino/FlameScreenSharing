//! Vários espectadores da mesma transmissão: um encoder só e um
//! [`VideoSender`] por pessoa. Cada um tem socket, pareamento, cifra, pacing e
//! medição próprios, então o protocolo não muda e o cliente nem sabe que há
//! outros assistindo. Decisão: ADR-014 em `docs/DECISOES.md`.
//!
//! O que é um só para todos (o bitrate do encoder, os keyframes) segue o
//! espectador em pior situação.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fluxo_net::{
    BitrateChange, BitrateController, ConnectionCode, CursorShape, NetError, PairingCode, SenderConfig,
    SenderStats, VideoSender,
};

/// Cada espectador é uma cópia inteira do vídeo no upload de quem compartilha.
pub const MAX_ESPECTADORES: usize = 4;

/// Sem nenhum pacote por este tempo (o cliente manda pings a cada 500 ms), o
/// espectador saiu ou caiu: para de gastar upload com ele.
pub const SILENCIO_MAXIMO: Duration = Duration::from_secs(15);

/// Quanto esperar pelos codecs de quem acabou de parear.
const PRAZO_CODECS: Duration = Duration::from_millis(1500);

struct Espectador {
    numero: u32,
    tx: VideoSender,
    abr: Option<BitrateController>,
    /// Máscara de codecs que ele decodifica.
    codecs: u8,
}

/// Quem está assistindo. Tudo por `&self`: a thread de áudio usa junto com o
/// laço do vídeo.
#[derive(Default)]
pub struct Espectadores {
    lista: Mutex<Vec<Espectador>>,
    /// Última forma e posição do cursor, para quem chegar depois.
    forma: Mutex<Option<CursorShape>>,
    posicao: Mutex<Option<(i32, i32, bool)>>,
}

impl Espectadores {
    fn lista(&self) -> std::sync::MutexGuard<'_, Vec<Espectador>> {
        self.lista.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Entra um espectador; devolve quantos há agora. Ele já recebe o cursor.
    pub fn adicionar(
        &self,
        numero: u32,
        tx: VideoSender,
        codecs: u8,
        abr: Option<BitrateController>,
    ) -> usize {
        if let Some(f) = self.forma.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            tx.set_cursor_shape(f);
        }
        if let Some((x, y, v)) = *self.posicao.lock().unwrap_or_else(|e| e.into_inner()) {
            tx.send_cursor_position(x, y, v);
        }
        let mut l = self.lista();
        l.push(Espectador {
            numero,
            tx,
            abr,
            codecs,
        });
        l.len()
    }

    pub fn quantos(&self) -> usize {
        self.lista().len()
    }

    /// Enfileira o frame para todos. Uma cópia por espectador: cada um
    /// fragmenta, cifra e envia no seu ritmo, e um lento não segura os outros.
    pub fn send_frame_with_flags(&self, data: Vec<u8>, flags: u8, timestamp_us: u32) -> Result<(), NetError> {
        let l = self.lista();
        if let Some((ultimo, resto)) = l.split_last() {
            for e in resto {
                e.tx.send_frame_with_flags(data.clone(), flags, timestamp_us)?;
            }
            ultimo.tx.send_frame_with_flags(data, flags, timestamp_us)?;
        }
        Ok(())
    }

    pub fn send_audio(&self, seq: u32, timestamp_us: u32, opus: &[u8]) {
        for e in self.lista().iter() {
            e.tx.send_audio(seq, timestamp_us, opus);
        }
    }

    pub fn set_cursor_shape(&self, forma: CursorShape) {
        for e in self.lista().iter() {
            e.tx.set_cursor_shape(forma.clone());
        }
        *self.forma.lock().unwrap_or_else(|e| e.into_inner()) = Some(forma);
    }

    pub fn send_cursor_position(&self, x: i32, y: i32, visivel: bool) {
        for e in self.lista().iter() {
            e.tx.send_cursor_position(x, y, visivel);
        }
        *self.posicao.lock().unwrap_or_else(|e| e.into_inner()) = Some((x, y, visivel));
    }

    /// Alguém precisa de um keyframe. Pergunta a todos, para limpar o pedido
    /// de cada um.
    pub fn take_keyframe_request(&self) -> bool {
        self.lista()
            .iter()
            .fold(false, |pediu, e| e.tx.take_keyframe_request() | pediu)
    }

    pub fn take_refresh_request(&self) -> bool {
        self.lista()
            .iter()
            .fold(false, |pediu, e| e.tx.take_refresh_request() | pediu)
    }

    /// Passa os relatórios de rede a cada controlador. Devolve a descrição da
    /// queda mais recente, se alguém mandou baixar.
    pub fn processar_relatorios(&self, agora: Instant) -> Option<String> {
        let mut queda = None;
        for e in self.lista().iter_mut() {
            let Some(r) = e.tx.take_report() else { continue };
            let Some(abr) = e.abr.as_mut() else { continue };
            if let Some((_, motivo)) = abr.on_report(&r, agora) {
                if motivo != BitrateChange::Up {
                    let porque = match motivo {
                        BitrateChange::DownLoss => format!("perda {:.1}%", r.loss.unwrap_or(0.0) * 100.0),
                        _ => format!("fila {:.0} ms", r.queue_delay_ms),
                    };
                    queda = Some(format!("{porque}, espectador {}", e.numero));
                }
            }
        }
        queda
    }

    /// O bitrate que serve para todos: o do espectador em pior situação.
    /// `None` sem bitrate adaptativo (ou sem ninguém).
    pub fn alvo_kbps(&self) -> Option<u32> {
        self.lista()
            .iter()
            .filter_map(|e| e.abr.as_ref().map(BitrateController::current_kbps))
            .min()
    }

    /// O encoder recusou mudar o bitrate: daqui em diante, fixo.
    pub fn desligar_adaptativo(&self) {
        for e in self.lista().iter_mut() {
            e.abr = None;
        }
    }

    pub fn set_pacing_bps(&self, bps: u64) {
        for e in self.lista().iter() {
            e.tx.set_pacing_bps(bps);
        }
    }

    /// Codecs que todos decodificam, incluindo um candidato a entrar (H.264
    /// sempre: todo cliente decodifica).
    pub fn codecs_comuns(&self, novo: u8) -> u8 {
        self.lista().iter().fold(novo, |m, e| m & e.codecs) | fluxo_net::VideoCodec::H264.bit()
    }

    /// Tira quem está calado há mais de `limite`. Devolve o número de cada um
    /// e quantos sobraram depois dele.
    /// Tira o espectador `numero`; devolve o IP dele e quantos ficaram.
    pub fn expulsar(&self, numero: u32) -> Option<(std::net::IpAddr, usize)> {
        let mut l = self.lista();
        let i = l.iter().position(|e| e.numero == numero)?;
        let e = l.remove(i);
        let ip = e.tx.peer_addr().ip();
        let ficaram = l.len();
        std::thread::spawn(move || drop(e));
        Some((ip, ficaram))
    }

    pub fn remover_silenciosos(&self, limite: Duration) -> Vec<(u32, usize)> {
        let mut l = self.lista();
        let mut saiu = Vec::new();
        let mut i = 0;
        while i < l.len() {
            if l[i].tx.silent_for().is_some_and(|t| t > limite) {
                let e = l.remove(i);
                saiu.push((e.numero, l.len()));
                // Soltar um VideoSender espera a fila dele esvaziar: não pode
                // ser na thread do vídeo.
                std::thread::spawn(move || drop(e));
            } else {
                i += 1;
            }
        }
        saiu
    }

    /// Estatísticas de todos: contagens somadas e, no que é um retrato da
    /// rede (perda, fila, RTT), o pior espectador.
    pub fn stats(&self) -> SenderStats {
        let mut t = SenderStats::default();
        for s in self.lista().iter().map(|e| e.tx.stats()) {
            t.encrypted |= s.encrypted;
            t.frames_queued += s.frames_queued;
            t.frames_sent += s.frames_sent;
            t.queue_len = t.queue_len.max(s.queue_len);
            t.packets_sent += s.packets_sent;
            t.bytes_sent += s.bytes_sent;
            t.send_errors += s.send_errors;
            t.last_frame_send_ms = t.last_frame_send_ms.max(s.last_frame_send_ms);
            t.max_frame_send_ms = t.max_frame_send_ms.max(s.max_frame_send_ms);
            t.keyframe_requests += s.keyframe_requests;
            t.refresh_requests += s.refresh_requests;
            t.simulated_drops += s.simulated_drops;
            t.feedbacks_received += s.feedbacks_received;
            t.queue_delay_ms = t.queue_delay_ms.max(s.queue_delay_ms);
            t.last_loss = pior_perda(t.last_loss, s.last_loss);
            t.audio_packets += s.audio_packets;
            t.audio_bytes += s.audio_bytes;
            t.cursor_packets += s.cursor_packets;
            if t.rtt.samples == 0 || s.rtt.smoothed_ms > t.rtt.smoothed_ms {
                t.rtt = s.rtt;
            }
        }
        t
    }
}

fn pior_perda(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, y) => x.or(y),
    }
}

/// Um ponto em pixels da tela, em pixels do vídeo: com `--resolucao`, os dois
/// são diferentes, e o cliente desenha o cursor em pixels do vídeo.
pub fn ponto_no_video(p: (i32, i32), tela: (u32, u32), video: (u32, u32)) -> (i32, i32) {
    let conv = |v: i32, de: u32, para: u32| (v as i64 * para as i64 / de.max(1) as i64) as i32;
    (conv(p.0, tela.0, video.0), conv(p.1, tela.1, video.1))
}

/// A forma do cursor na escala do vídeo (o pixel mais próximo, que é o que
/// mantém um cursor nítido e a ponta no lugar).
pub fn forma_no_video(f: CursorShape, tela: (u32, u32), video: (u32, u32)) -> CursorShape {
    if tela == video {
        return f;
    }
    let (w, h) = (f.width as usize, f.height as usize);
    let (nw, _) = ponto_no_video((f.width as i32, f.height as i32), tela, video);
    let (_, nh) = ponto_no_video((f.width as i32, f.height as i32), tela, video);
    let (nw, nh) = (nw.max(1) as usize, nh.max(1) as usize);
    let mut bgra = Vec::with_capacity(nw * nh * 4);
    for y in 0..nh {
        for x in 0..nw {
            let origem = (y * h / nh) * w + x * w / nw;
            bgra.extend_from_slice(&f.bgra[origem * 4..origem * 4 + 4]);
        }
    }
    let (hx, hy) = ponto_no_video((f.hot_x as i32, f.hot_y as i32), tela, video);
    CursorShape {
        width: nw as u16,
        height: nh as u16,
        hot_x: hx as u16,
        hot_y: hy as u16,
        bgra,
    }
}

/// O que chega pela entrada padrão para pôr mais alguém na transmissão.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pedido {
    /// `conexao CÓDIGO`: pela internet, como `--conexao`.
    Conexao(ConnectionCode),
    /// `destino IP:PORTA CÓDIGO`: na mesma rede, como `--destino` + `--codigo`.
    Destino(String, PairingCode),
}

/// Interpreta uma linha da entrada padrão.
pub fn ler_pedido(linha: &str) -> Result<Pedido, String> {
    let mut p = linha.split_whitespace();
    match (p.next(), p.next(), p.next()) {
        (Some("conexao"), Some(_), _) => {
            let codigo = linha.trim_start().strip_prefix("conexao").unwrap_or_default();
            ConnectionCode::parse(codigo)
                .map(Pedido::Conexao)
                .map_err(|e| format!("código de conexão: {e}"))
        }
        (Some("destino"), Some(destino), Some(codigo)) => PairingCode::parse(codigo)
            .map(|c| Pedido::Destino(destino.to_string(), c))
            .map_err(|e| format!("código: {e}")),
        _ => Err("use: conexao CÓDIGO  ou  destino IP:PORTA CÓDIGO".into()),
    }
}

/// Mudança com a transmissão rodando, também pela entrada padrão (é por ela
/// que a janela do FSS manda os botões). Quem assiste não cai: no máximo
/// recebe um keyframe novo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Controle {
    /// `tela N`: passa a transmitir o monitor N (o índice de `--monitor`).
    Tela(usize),
    /// `som ligado` / `som desligado`: o som do PC vai ou não (desligado, vai silêncio).
    Som(bool),
    /// `pausar` / `continuar`: o vídeo para na última imagem e volta.
    Pausa(bool),
    /// `resolucao R`: como `--resolucao` (`nativa` volta à da tela).
    Resolucao(Option<u32>),
    /// `fps F`: como `--fps`.
    Fps(u32),
    /// `janela HWND`: passa a transmitir só essa janela (ver `fonte.rs`).
    Janela(isize),
    /// `expulsar N`: tira o espectador N e recusa o IP dele até o fim da
    /// transmissão.
    Expulsar(u32),
}

/// Se a linha é um [`Controle`]: `None` quando é outra coisa (um pedido).
pub fn ler_controle(linha: &str) -> Option<Result<Controle, String>> {
    let mut p = linha.split_whitespace();
    let (comando, valor) = (p.next()?, p.next());
    Some(match (comando, valor) {
        ("tela", Some(n)) => n
            .parse()
            .map(Controle::Tela)
            .map_err(|_| "tela: use o número do monitor (0, 1, …)".to_string()),
        ("som", Some("ligado")) => Ok(Controle::Som(true)),
        ("som", Some("desligado")) => Ok(Controle::Som(false)),
        ("som", _) => Err("som: use som ligado  ou  som desligado".to_string()),
        ("pausar", None) => Ok(Controle::Pausa(true)),
        ("continuar", None) => Ok(Controle::Pausa(false)),
        ("resolucao", Some(r)) => crate::parse_resolucao(r).map(Controle::Resolucao),
        ("janela", Some(h)) => h
            .parse()
            .map(Controle::Janela)
            .map_err(|_| "janela: use o número da janela (HWND)".to_string()),
        ("expulsar", Some(n)) => n
            .parse()
            .map(Controle::Expulsar)
            .map_err(|_| "expulsar: use o número do espectador".to_string()),
        ("fps", Some(f)) => match f.parse() {
            Ok(f) if (1..=240).contains(&f) => Ok(Controle::Fps(f)),
            _ => Err("fps: use um número de 1 a 240".to_string()),
        },
        ("tela" | "janela" | "resolucao" | "fps" | "expulsar", None) => {
            Err(format!("{comando}: falta o valor"))
        }
        _ => return None,
    })
}

/// Alguém que acabou de parear, a caminho do laço do vídeo.
pub struct Novo {
    pub numero: u32,
    pub tx: VideoSender,
    pub codecs: u8,
}

/// Lê a entrada padrão até ela fechar. Controles vão para o laço do vídeo
/// por `controles`; pedidos de espectador pareiam cada um em thread própria
/// (pode levar minutos). `ocupados` conta quem assiste e quem está pareando;
/// quem sair desconta lá no laço do vídeo.
pub fn atender_pedidos(
    cfg: SenderConfig,
    ttl_furo: u32,
    numeros: Arc<AtomicU32>,
    novos: Sender<Novo>,
    ocupados: Arc<AtomicUsize>,
    controles: Sender<Controle>,
) {
    for linha in std::io::stdin().lines() {
        let Ok(linha) = linha else { return };
        if linha.trim().is_empty() {
            continue;
        }
        match ler_controle(&linha) {
            Some(Ok(c)) => {
                if controles.send(c).is_err() {
                    return; // o vídeo acabou
                }
                continue;
            }
            Some(Err(e)) => {
                println!("({e})");
                continue;
            }
            None => {}
        }
        let pedido = match ler_pedido(&linha) {
            Ok(p) => p,
            Err(e) => {
                println!("(pedido de espectador recusado: {e})");
                continue;
            }
        };
        if ocupados.load(Ordering::Relaxed) >= MAX_ESPECTADORES {
            println!("(limite de {MAX_ESPECTADORES} espectadores: cada um é mais uma cópia no seu upload)");
            continue;
        }
        ocupados.fetch_add(1, Ordering::Relaxed);
        let n = numeros.fetch_add(1, Ordering::Relaxed) + 1;
        let (cfg, novos, ocupados) = (cfg.clone(), novos.clone(), ocupados.clone());
        std::thread::spawn(move || match conectar(n, pedido, cfg, ttl_furo) {
            Ok(tx) => {
                let codecs = tx
                    .wait_client_codecs(PRAZO_CODECS)
                    .unwrap_or(fluxo_net::VideoCodec::H264.bit());
                let _ = novos.send(Novo {
                    numero: n,
                    tx,
                    codecs,
                });
            }
            Err(e) => {
                ocupados.fetch_sub(1, Ordering::Relaxed);
                println!("Espectador {n} não conectou: {e}");
            }
        });
    }
}

/// Prazo de cada procura na sala: sem ninguém para entrar, termina e outra
/// começa (num socket novo).
pub const PRAZO_SALA: Duration = Duration::from_secs(30);

/// Sala: pede ao servidor, um de cada vez, quem entrou com o código, e pareia
/// cada um num socket só dele, até o limite. Roda até o laço do vídeo fechar
/// `novos`.
pub fn atender_sala(
    servidor: SocketAddr,
    cfg: SenderConfig,
    numeros: Arc<AtomicU32>,
    novos: Sender<Novo>,
    ocupados: Arc<AtomicUsize>,
) {
    let mut avisou_limite = false;
    loop {
        if ocupados.load(Ordering::Relaxed) >= MAX_ESPECTADORES {
            if !avisou_limite {
                println!("(sala cheia: {MAX_ESPECTADORES} espectadores; quem chegar espera uma vaga)");
                avisou_limite = true;
            }
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        avisou_limite = false;
        // A vaga fica reservada enquanto procura.
        ocupados.fetch_add(1, Ordering::Relaxed);
        match VideoSender::connect_via_signal(("0.0.0.0", 0), servidor, cfg.clone()) {
            Ok(tx) => {
                let codecs = tx
                    .wait_client_codecs(PRAZO_CODECS)
                    .unwrap_or(fluxo_net::VideoCodec::H264.bit());
                let numero = numeros.fetch_add(1, Ordering::Relaxed) + 1;
                if novos.send(Novo { numero, tx, codecs }).is_err() {
                    return;
                }
            }
            Err(e) => {
                ocupados.fetch_sub(1, Ordering::Relaxed);
                match e.to_string() {
                    // Ninguém entrou nesse prazo: procura de novo, quieto.
                    t if t.contains("nenhum cliente") => {}
                    t if t.contains("o servidor de sinalização") => {
                        println!("(o servidor da sala não respondeu; tentando de novo)");
                        std::thread::sleep(Duration::from_secs(5));
                    }
                    t => println!("Alguém tentou entrar e não conectou: {t}"),
                }
            }
        }
    }
}

/// Pareia com mais um espectador, em sockets só dele.
fn conectar(numero: u32, pedido: Pedido, cfg: SenderConfig, ttl_furo: u32) -> Result<VideoSender, NetError> {
    match pedido {
        Pedido::Conexao(c) => {
            let internet = fluxo_net::Internet::open(0, false)?;
            println!(
                "Código de resposta (espectador {numero}): {}",
                internet.reply_code(&c)?
            );
            let cfg = SenderConfig {
                pairing_code: Some(c.pairing.clone()),
                // Porta aberta por UPnP do outro lado: direto, sem esperar o furo.
                punch_ttl: (ttl_furo > 0 && !c.open).then_some(ttl_furo),
                ..cfg
            };
            VideoSender::connect_with_sockets(internet.v4, internet.v6, c.candidates(), cfg)
        }
        Pedido::Destino(destino, codigo) => {
            let cfg = SenderConfig {
                pairing_code: Some(codigo),
                ..cfg
            };
            VideoSender::connect(("0.0.0.0", 0), destino.as_str(), cfg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fluxo_net::{ReceiverConfig, VideoReceiver, FLAG_KEYFRAME as KEYFRAME};

    fn par() -> (VideoSender, VideoReceiver) {
        let rx = VideoReceiver::bind(("127.0.0.1", 0), ReceiverConfig::default()).unwrap();
        let tx = VideoSender::connect(
            ("127.0.0.1", 0),
            rx.local_addr(),
            SenderConfig::for_bitrate(20_000_000),
        )
        .unwrap();
        (tx, rx)
    }

    fn espera_frame(rx: &VideoReceiver) -> Vec<u8> {
        rx.recv_frame(Duration::from_secs(5))
            .unwrap()
            .expect("o frame não chegou")
            .frame
            .data
    }

    #[test]
    fn o_mesmo_frame_chega_a_todos() {
        let esp = Espectadores::default();
        let (t1, r1) = par();
        let (t2, r2) = par();
        assert_eq!(esp.adicionar(1, t1, 1, None), 1);
        assert_eq!(esp.adicionar(2, t2, 1, None), 2);
        esp.send_frame_with_flags(vec![7; 5000], KEYFRAME, 1).unwrap();
        assert_eq!(espera_frame(&r1), vec![7; 5000]);
        assert_eq!(espera_frame(&r2), vec![7; 5000]);
        assert_eq!(esp.stats().frames_queued, 2);
    }

    #[test]
    fn sem_ninguem_o_frame_nao_e_erro() {
        let esp = Espectadores::default();
        assert!(esp.send_frame_with_flags(vec![1], KEYFRAME, 1).is_ok());
        assert_eq!(esp.alvo_kbps(), None);
    }

    #[test]
    fn quem_para_de_responder_sai_e_quem_responde_fica() {
        let esp = Espectadores::default();
        let (t1, r1) = par();
        let (t2, r2) = par();
        esp.adicionar(1, t1, 1, None);
        esp.adicionar(2, t2, 1, None);
        // Os dois respondem (pings a cada 500 ms)...
        std::thread::sleep(Duration::from_millis(900));
        assert!(esp.remover_silenciosos(Duration::from_millis(700)).is_empty());
        // ...o primeiro fecha, e fica calado além do limite.
        drop(r1);
        std::thread::sleep(Duration::from_millis(1400));
        assert_eq!(esp.remover_silenciosos(Duration::from_millis(1000)), vec![(1, 1)]);
        assert_eq!(esp.quantos(), 1);
        drop(r2);
    }

    #[test]
    fn nunca_ouvido_nao_conta_como_calado() {
        // Sem cifra, o host pode começar antes do cliente abrir.
        let esp = Espectadores::default();
        let parado = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let cfg = SenderConfig::for_bitrate(20_000_000);
        let t = VideoSender::connect(("127.0.0.1", 0), parado.local_addr().unwrap(), cfg).unwrap();
        esp.adicionar(1, t, 1, None);
        std::thread::sleep(Duration::from_millis(50));
        assert!(esp.remover_silenciosos(Duration::from_millis(1)).is_empty());
    }

    #[test]
    fn bitrate_e_codec_seguem_o_pior() {
        let esp = Espectadores::default();
        let (t1, _r1) = par();
        let (t2, _r2) = par();
        let agora = Instant::now();
        esp.adicionar(1, t1, 0b111, Some(BitrateController::new(1000, 8000, agora)));
        esp.adicionar(2, t2, 0b101, Some(BitrateController::new(1000, 5000, agora)));
        assert_eq!(esp.alvo_kbps(), Some(5000));
        assert_eq!(esp.codecs_comuns(0b111), 0b101);
        assert_eq!(esp.codecs_comuns(0b011), 0b001, "H.264 sobra sempre");
        esp.desligar_adaptativo();
        assert_eq!(esp.alvo_kbps(), None);
    }

    #[test]
    fn cursor_acompanha_a_resolucao_do_video() {
        let (tela, video) = ((1920, 1080), (1280, 720));
        assert_eq!(ponto_no_video((960, 540), tela, video), (640, 360));
        assert_eq!(ponto_no_video((960, 540), tela, tela), (960, 540));
        // 4×4, com o pixel de cima à esquerda marcado.
        let mut bgra = vec![0u8; 4 * 4 * 4];
        bgra[0] = 9;
        let f = CursorShape {
            width: 4,
            height: 4,
            hot_x: 3,
            hot_y: 3,
            bgra,
        };
        let menor = forma_no_video(f.clone(), (4, 4), (2, 2));
        assert_eq!(
            (menor.width, menor.height, menor.hot_x, menor.hot_y),
            (2, 2, 1, 1)
        );
        assert_eq!(menor.bgra.len(), 2 * 2 * 4);
        assert_eq!(menor.bgra[0], 9);
        assert_eq!(forma_no_video(f.clone(), tela, tela), f);
    }

    #[test]
    fn pedidos_validos_e_invalidos() {
        let codigo = PairingCode::generate().unwrap().to_string();
        assert_eq!(
            ler_pedido(&format!("destino 10.0.0.2:9000 {codigo}")),
            Ok(Pedido::Destino(
                "10.0.0.2:9000".into(),
                PairingCode::parse(&codigo).unwrap()
            ))
        );
        assert!(ler_pedido("conexao AAAA")
            .unwrap_err()
            .starts_with("código de conexão"));
        assert!(ler_pedido("destino 10.0.0.2:9000").is_err());
        assert!(ler_pedido("oi").is_err());
    }

    #[test]
    fn controles_da_janela() {
        assert_eq!(ler_controle("tela 1"), Some(Ok(Controle::Tela(1))));
        assert_eq!(ler_controle(" som desligado "), Some(Ok(Controle::Som(false))));
        assert_eq!(ler_controle("som ligado"), Some(Ok(Controle::Som(true))));
        assert_eq!(ler_controle("pausar"), Some(Ok(Controle::Pausa(true))));
        assert_eq!(ler_controle("continuar"), Some(Ok(Controle::Pausa(false))));
        assert_eq!(
            ler_controle("resolucao 720p"),
            Some(Ok(Controle::Resolucao(Some(720))))
        );
        assert_eq!(
            ler_controle("resolucao nativa"),
            Some(Ok(Controle::Resolucao(None)))
        );
        assert_eq!(ler_controle("fps 30"), Some(Ok(Controle::Fps(30))));
        assert_eq!(ler_controle("expulsar 2"), Some(Ok(Controle::Expulsar(2))));
        assert_eq!(ler_controle("janela 132456"), Some(Ok(Controle::Janela(132456))));
        assert!(matches!(ler_controle("expulsar x"), Some(Err(_))));
        for errado in [
            "tela x",
            "tela",
            "som talvez",
            "fps 0",
            "fps 999",
            "resolucao abc",
        ] {
            assert!(matches!(ler_controle(errado), Some(Err(_))), "{errado}");
        }
        // Pedidos de espectador não são controles.
        assert_eq!(ler_controle("conexao ABCD"), None);
        assert_eq!(ler_controle("destino 10.0.0.2:9000 X"), None);
    }
}
