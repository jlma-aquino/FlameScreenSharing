//! Servidor de sinalização: [`SignalServer`].
//!
//! Apresenta host e clientes que estão em redes diferentes. Cada cliente se
//! registra na sala do código ([`PairingCode::room`]); o host procura a sala
//! e recebe um cliente que ainda ninguém pegou; o servidor conta a cada um os
//! endereços do outro. Vários clientes cabem numa sala (todos com o mesmo
//! código, o da transmissão), e o host pede um de cada vez, cada um num
//! socket próprio. O endereço
//! público é o de onde o pedido chegou — o servidor faz o papel de STUN pelo
//! mesmo socket que depois leva o vídeo, então é exatamente o mapeamento do
//! NAT que interessa. Depois disso os dois furam os NATs mandando pacotes um
//! ao outro (ver `sender.rs` e `receiver.rs`); vídeo nunca passa por aqui.
//!
//! **Repasse** (quando o furo de NAT não abre): o host pede `RelayBind` da
//! sala, e o servidor passa a repetir os pacotes entre aquele socket do host e
//! o cliente que ele mesmo entregou a ele. Os dois mandam ao servidor, cujo
//! caminho já está aberto nos dois NATs. O conteúdo segue cifrado de ponta a
//! ponta (o servidor não tem a chave da sessão); ninguém consegue usar o
//! repasse para mandar pacotes a um endereço que o servidor não apresentou.
//!
//! Com chaves ([`SignalServer::with_keys`]), só atende pedidos assinados com
//! uma delas ([`ServerKey`]); o resto é ignorado em silêncio, sem resposta
//! (quem não tem a chave não usa o servidor nem o faz responder a terceiros).
//!
//! [`PairingCode::room`]: fluxo_crypto::PairingCode::room

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fluxo_crypto::ServerKey;
use fluxo_proto::{PacketHeader, PacketKind, SignalMessage};

use crate::{clock_us, is_transient};

/// Sem novo registro por este tempo, o cliente sai da sala (ele renova o
/// registro a cada 5 s enquanto espera, e para quando pareia).
pub const ROOM_TTL: Duration = Duration::from_secs(30);

/// Clientes esperando numa sala, no máximo.
pub const MAX_PER_ROOM: usize = 8;

/// Um cliente entregue a um host fica com ele enquanto o host procura e por
/// este tempo depois. É mais que [`ROOM_TTL`]: quem pareou para de se
/// registrar e some antes de ficar livre; quem continua se registrando sem
/// parear (pareamento que falhou) volta a ficar disponível.
const POSSE: Duration = Duration::from_secs(35);

/// Um par repassado sem nenhum pacote por este tempo é desfeito.
pub const RELAY_TTL: Duration = Duration::from_secs(30);

/// Endereços em repasse ao mesmo tempo (dois por par).
pub const MAX_RELAYED: usize = 128;

/// Um lado de um par repassado.
struct Repasse {
    outro: SocketAddr,
    visto: Instant,
}

/// O que aconteceu num pedido, para quem roda o servidor mostrar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalEvent {
    /// Cliente novo (ou que mudou de endereço) numa sala.
    Registered { room: u32, client: SocketAddr },
    /// Host achou o cliente: os dois foram avisados.
    Found {
        room: u32,
        host: SocketAddr,
        client: SocketAddr,
    },
    /// Host procurou uma sala vazia.
    NotFound { room: u32, host: SocketAddr },
    /// Cliente tentou uma sala cheia ([`MAX_PER_ROOM`]).
    RoomTaken { room: u32, client: SocketAddr },
    /// O host pediu, e o servidor passou a repassar entre os dois.
    Relay {
        room: u32,
        host: SocketAddr,
        client: SocketAddr,
    },
    /// Pacote sem assinatura de uma chave aceita (servidor com chaves): ignorado.
    Ignored { from: SocketAddr },
}

struct Cliente {
    endereco: SocketAddr,
    locais: Vec<SocketAddr>,
    visto: Instant,
    /// Host (socket) a quem foi entregue e quando ele procurou por último.
    host: Option<(SocketAddr, Instant)>,
}

/// Estado do servidor: as salas com clientes esperando.
#[derive(Default)]
pub struct SignalServer {
    salas: HashMap<u32, Vec<Cliente>>,
    /// Vazia: servidor aberto (atende qualquer um).
    chaves: Vec<ServerKey>,
    /// Pacotes ignorados por não virem assinados com uma das chaves.
    ignorados: u64,
    /// Pares repassados, pelos dois endereços.
    repasses: HashMap<SocketAddr, Repasse>,
    /// Bytes repassados desde que o servidor subiu.
    repassados: u64,
}

impl SignalServer {
    /// Servidor que só atende pedidos assinados com uma destas chaves.
    pub fn with_keys(chaves: Vec<ServerKey>) -> Self {
        Self {
            chaves,
            ..Self::default()
        }
    }

    /// Pacotes ignorados até agora por falta de chave válida.
    pub fn ignored(&self) -> u64 {
        self.ignorados
    }

    /// Bytes repassados até agora.
    pub fn relayed_bytes(&self) -> u64 {
        self.repassados
    }

    /// Pares em repasse agora.
    pub fn relays(&self) -> usize {
        self.repasses.len() / 2
    }

    /// Trata um datagrama: devolve o que mandar (destino, pacote) e o
    /// evento, se houve. Lixo e pacotes que não são pedidos são ignorados.
    pub fn handle(
        &mut self,
        datagrama: &[u8],
        de: SocketAddr,
        agora: Instant,
    ) -> (Vec<(SocketAddr, Vec<u8>)>, Option<SignalEvent>) {
        // Repasse primeiro: é o caminho de cada pacote de vídeo. Só o que não é
        // sinalização (um pedido assinado não passa no decode estrito) segue ao outro.
        if let Some(outro) = self.repasses.get(&de).map(|r| r.outro) {
            if PacketHeader::decode(datagrama).is_ok_and(|(h, _)| h.kind != PacketKind::Signal) {
                for a in [de, outro] {
                    if let Some(r) = self.repasses.get_mut(&a) {
                        r.visto = agora;
                    }
                }
                self.repassados += datagrama.len() as u64;
                return (vec![(outro, datagrama.to_vec())], None);
            }
        }
        self.repasses
            .retain(|_, r| agora.duration_since(r.visto) < RELAY_TTL);
        for sala in self.salas.values_mut() {
            sala.retain(|c| agora.duration_since(c.visto) < ROOM_TTL);
        }
        self.salas.retain(|_, sala| !sala.is_empty());
        let datagrama = match self.chaves.is_empty() {
            true => datagrama,
            false => match self.chaves.iter().find_map(|k| k.verify(datagrama)) {
                Some(pedido) => pedido,
                None => {
                    self.ignorados += 1;
                    return (Vec::new(), Some(SignalEvent::Ignored { from: de }));
                }
            },
        };
        let Ok((h, payload)) = PacketHeader::decode(datagrama) else {
            return (Vec::new(), None);
        };
        if h.kind != PacketKind::Signal || h.is_encrypted() {
            return (Vec::new(), None);
        }
        let Ok(msg) = SignalMessage::from_bytes(payload) else {
            return (Vec::new(), None);
        };
        let resposta = |id: u32, m: &SignalMessage| {
            PacketKind::Signal
                .control(id, clock_us())
                .encode_with_payload(&m.to_bytes())
                .expect("mensagem de sinalização cabe num pacote")
        };
        match msg {
            SignalMessage::Register { room, locals } => {
                let sala = self.salas.entry(room).or_default();
                let novo = match sala.iter().position(|c| c.endereco == de) {
                    Some(i) => {
                        sala[i].locais = locals;
                        sala[i].visto = agora;
                        false
                    }
                    None if sala.len() >= MAX_PER_ROOM => {
                        let r = resposta(h.frame_id, &SignalMessage::RoomTaken);
                        return (vec![(de, r)], Some(SignalEvent::RoomTaken { room, client: de }));
                    }
                    None => {
                        sala.push(Cliente {
                            endereco: de,
                            locais: locals,
                            visto: agora,
                            host: None,
                        });
                        true
                    }
                };
                let r = resposta(h.frame_id, &SignalMessage::Registered { public: de });
                let evento = novo.then_some(SignalEvent::Registered { room, client: de });
                (vec![(de, r)], evento)
            }
            SignalMessage::Lookup { room, locals } => {
                // O que já é deste host; senão, o mais antigo sem dono (ou
                // cujo dono sumiu).
                let livre = |c: &Cliente| c.host.is_none_or(|(_, t)| agora.duration_since(t) >= POSSE);
                let escolhido = self.salas.get_mut(&room).and_then(|sala| {
                    let i = sala
                        .iter()
                        .position(|c| c.host.is_some_and(|(h, _)| h == de))
                        .or_else(|| sala.iter().position(livre))?;
                    Some(&mut sala[i])
                });
                match escolhido {
                    Some(c) => {
                        c.host = Some((de, agora));
                        let para_host = SignalMessage::Found {
                            public: c.endereco,
                            locals: c.locais.clone(),
                        };
                        let para_cliente = SignalMessage::Found { public: de, locals };
                        let envios = vec![
                            (de, resposta(h.frame_id, &para_host)),
                            (c.endereco, resposta(0, &para_cliente)),
                        ];
                        let evento = SignalEvent::Found {
                            room,
                            host: de,
                            client: c.endereco,
                        };
                        (envios, Some(evento))
                    }
                    None => {
                        let r = resposta(h.frame_id, &SignalMessage::NotFound);
                        (vec![(de, r)], Some(SignalEvent::NotFound { room, host: de }))
                    }
                }
            }
            SignalMessage::RelayBind { room } => {
                // Só entre este socket do host e o cliente que o servidor
                // entregou a ele nesta sala.
                let cliente = self.salas.get(&room).and_then(|sala| {
                    sala.iter()
                        .find(|c| c.host.is_some_and(|(h, _)| h == de))
                        .map(|c| c.endereco)
                });
                let Some(cliente) = cliente else {
                    return (Vec::new(), None);
                };
                // Pares antigos destes endereços saem (o host tenta de novo num socket novo).
                for a in [de, cliente] {
                    if let Some(r) = self.repasses.remove(&a) {
                        self.repasses.remove(&r.outro);
                    }
                }
                if self.repasses.len() + 2 > MAX_RELAYED {
                    return (Vec::new(), None);
                }
                for (a, b) in [(de, cliente), (cliente, de)] {
                    self.repasses.insert(
                        a,
                        Repasse {
                            outro: b,
                            visto: agora,
                        },
                    );
                }
                let r = resposta(h.frame_id, &SignalMessage::RelayReady);
                (
                    vec![(de, r)],
                    Some(SignalEvent::Relay {
                        room,
                        host: de,
                        client: cliente,
                    }),
                )
            }
            // Respostas não são pedidos: alguém mandando coisa errada.
            SignalMessage::Registered { .. }
            | SignalMessage::Found { .. }
            | SignalMessage::NotFound
            | SignalMessage::RoomTaken
            | SignalMessage::RelayReady => (Vec::new(), None),
        }
    }

    /// Salas com cliente esperando.
    pub fn rooms(&self) -> usize {
        self.salas.len()
    }

    /// Clientes esperando, em todas as salas.
    pub fn clients(&self) -> usize {
        self.salas.values().map(Vec::len).sum()
    }

    /// Atende pedidos em `socket` até `stop` virar `true` (conferido a cada
    /// ~200 ms). `evento` é chamado a cada acontecimento.
    pub fn run(&mut self, socket: &UdpSocket, stop: &AtomicBool, mut evento: impl FnMut(&SignalEvent)) {
        let _ = socket.set_read_timeout(Some(Duration::from_millis(200)));
        let mut buf = [0u8; 2048];
        while !stop.load(Ordering::Relaxed) {
            let (n, de) = match socket.recv_from(&mut buf) {
                Ok(r) => r,
                Err(e) if is_transient(&e) => continue,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
            };
            let (envios, ev) = self.handle(&buf[..n], de, Instant::now());
            for (para, pacote) in envios {
                let _ = socket.send_to(&pacote, para);
            }
            if let Some(ev) = ev {
                evento(&ev);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn pedido(id: u32, m: SignalMessage) -> Vec<u8> {
        PacketKind::Signal
            .control(id, 0)
            .encode_with_payload(&m.to_bytes())
            .unwrap()
    }

    fn ler(p: &[u8]) -> (u32, SignalMessage) {
        let (h, payload) = PacketHeader::decode(p).unwrap();
        (h.frame_id, SignalMessage::from_bytes(payload).unwrap())
    }

    #[test]
    fn repasse_so_entre_os_dois_apresentados() {
        let mut s = SignalServer::default();
        let t = Instant::now();
        let (cliente, host, intruso) = (a("200.0.0.1:41000"), a("201.0.0.2:52000"), a("203.0.0.3:1000"));
        s.handle(
            &pedido(
                1,
                SignalMessage::Register {
                    room: 9,
                    locals: vec![],
                },
            ),
            cliente,
            t,
        );
        // Sem o servidor ter entregado o cliente a ele: nada.
        let (r, _) = s.handle(&pedido(2, SignalMessage::RelayBind { room: 9 }), host, t);
        assert!(r.is_empty());
        s.handle(
            &pedido(
                3,
                SignalMessage::Lookup {
                    room: 9,
                    locals: vec![],
                },
            ),
            host,
            t,
        );
        let (r, ev) = s.handle(&pedido(4, SignalMessage::RelayBind { room: 9 }), host, t);
        assert_eq!(ler(&r[0].1), (4, SignalMessage::RelayReady));
        assert_eq!(
            ev,
            Some(SignalEvent::Relay {
                room: 9,
                host,
                client: cliente
            })
        );
        assert_eq!(s.relays(), 1);
        // Vídeo do host vai ao cliente, e do cliente ao host, sem mudar nada.
        let video = PacketKind::Ping
            .control(77, 0)
            .encode_with_payload(&[1, 2, 3])
            .unwrap();
        let (r, _) = s.handle(&video, host, t);
        assert_eq!(r, vec![(cliente, video.clone())]);
        let (r, _) = s.handle(&video, cliente, t);
        assert_eq!(r, vec![(host, video.clone())]);
        // De quem não está no par, não.
        let (r, _) = s.handle(&video, intruso, t);
        assert!(r.is_empty());
        // Sinalização de quem está no par continua sendo sinalização.
        let (r, _) = s.handle(
            &pedido(
                5,
                SignalMessage::Register {
                    room: 9,
                    locals: vec![],
                },
            ),
            cliente,
            t,
        );
        assert_eq!(ler(&r[0].1).1, SignalMessage::Registered { public: cliente });
        assert_eq!(s.relayed_bytes(), 2 * video.len() as u64);
        // Parado por RELAY_TTL: desfeito.
        let (r, _) = s.handle(&video, intruso, t + RELAY_TTL);
        assert!(r.is_empty());
        assert_eq!(s.relays(), 0);
        let (r, _) = s.handle(&video, host, t + RELAY_TTL);
        assert!(r.is_empty());
    }

    #[test]
    fn com_chave_so_atende_pedido_assinado() {
        let chave = ServerKey::generate("203.0.113.5:3478").unwrap();
        let outra = ServerKey::generate("203.0.113.5:3478").unwrap();
        let mut s = SignalServer::with_keys(vec![chave.clone()]);
        let t = Instant::now();
        let cliente = a("200.0.0.1:41000");
        let registro = pedido(
            1,
            SignalMessage::Register {
                room: 42,
                locals: Vec::new(),
            },
        );
        // Sem assinatura, ou com a chave errada: nada, nem resposta.
        for p in [registro.clone(), outra.signed(&registro)] {
            let (r, ev) = s.handle(&p, cliente, t);
            assert!(r.is_empty());
            assert_eq!(ev, Some(SignalEvent::Ignored { from: cliente }));
        }
        assert_eq!((s.ignored(), s.clients()), (2, 0));
        let (r, ev) = s.handle(&chave.signed(&registro), cliente, t);
        assert_eq!(r.len(), 1);
        assert_eq!(ler(&r[0].1), (1, SignalMessage::Registered { public: cliente }));
        assert!(ev.is_some());
        assert_eq!(s.clients(), 1);
    }

    #[test]
    fn host_acha_cliente_e_os_dois_recebem_os_enderecos() {
        let mut s = SignalServer::default();
        let t = Instant::now();
        let (cliente, host) = (a("200.0.0.1:41000"), a("201.0.0.2:52000"));
        let local_cliente = vec![a("192.168.0.5:9000")];
        let (r, ev) = s.handle(
            &pedido(
                1,
                SignalMessage::Register {
                    room: 42,
                    locals: local_cliente.clone(),
                },
            ),
            cliente,
            t,
        );
        assert_eq!(
            ev,
            Some(SignalEvent::Registered {
                room: 42,
                client: cliente
            })
        );
        assert_eq!(r.len(), 1);
        assert_eq!(ler(&r[0].1), (1, SignalMessage::Registered { public: cliente }));

        let local_host = vec![a("10.0.0.7:5000")];
        let (r, ev) = s.handle(
            &pedido(
                9,
                SignalMessage::Lookup {
                    room: 42,
                    locals: local_host.clone(),
                },
            ),
            host,
            t,
        );
        assert_eq!(
            ev,
            Some(SignalEvent::Found {
                room: 42,
                host,
                client: cliente
            })
        );
        assert_eq!(r[0].0, host);
        assert_eq!(
            ler(&r[0].1),
            (
                9,
                SignalMessage::Found {
                    public: cliente,
                    locals: local_cliente
                }
            )
        );
        assert_eq!(r[1].0, cliente);
        assert_eq!(
            ler(&r[1].1).1,
            SignalMessage::Found {
                public: host,
                locals: local_host
            }
        );
    }

    fn registrar(s: &mut SignalServer, room: u32, de: SocketAddr, t: Instant) -> Option<SignalEvent> {
        s.handle(
            &pedido(1, SignalMessage::Register { room, locals: vec![] }),
            de,
            t,
        )
        .1
    }

    /// O cliente que o servidor entregou ao host `de`, se algum.
    fn procurar(s: &mut SignalServer, room: u32, de: SocketAddr, t: Instant) -> Option<SocketAddr> {
        let (r, _) = s.handle(&pedido(1, SignalMessage::Lookup { room, locals: vec![] }), de, t);
        match ler(&r[0].1).1 {
            SignalMessage::Found { public, .. } => Some(public),
            _ => None,
        }
    }

    #[test]
    fn sala_vazia_e_expirada() {
        let mut s = SignalServer::default();
        let t = Instant::now();
        let host = a("201.0.0.2:52000");
        assert_eq!(procurar(&mut s, 7, host, t), None);
        assert!(registrar(&mut s, 7, a("200.0.0.1:1000"), t).is_some());
        // Renovar não gera evento.
        assert_eq!(registrar(&mut s, 7, a("200.0.0.1:1000"), t), None);
        // Sem renovar, o cliente sai da sala.
        let depois = t + ROOM_TTL + Duration::from_secs(1);
        assert_eq!(procurar(&mut s, 7, host, depois), None);
        assert_eq!(s.rooms(), 0);
    }

    #[test]
    fn varios_clientes_cada_um_para_um_host() {
        let mut s = SignalServer::default();
        let t = Instant::now();
        // Dois da mesma casa (mesmo IP) e um de fora, todos com o mesmo código.
        let (c1, c2, c3) = (a("200.0.0.1:1000"), a("200.0.0.1:1001"), a("203.0.0.3:1000"));
        for c in [c1, c2, c3] {
            assert!(matches!(
                registrar(&mut s, 7, c, t),
                Some(SignalEvent::Registered { .. })
            ));
        }
        assert_eq!(s.clients(), 3);
        // Cada socket do host recebe um cliente diferente, na ordem de chegada,
        // e o mesmo socket continua recebendo o mesmo enquanto procura.
        let (h1, h2, h3, h4) = (
            a("201.0.0.2:5001"),
            a("201.0.0.2:5002"),
            a("201.0.0.2:5003"),
            a("201.0.0.2:5004"),
        );
        assert_eq!(procurar(&mut s, 7, h1, t), Some(c1));
        assert_eq!(procurar(&mut s, 7, h2, t), Some(c2));
        assert_eq!(procurar(&mut s, 7, h1, t), Some(c1));
        assert_eq!(procurar(&mut s, 7, h3, t), Some(c3));
        assert_eq!(procurar(&mut s, 7, h4, t), None);
    }

    #[test]
    fn quem_nao_pareou_volta_a_ficar_livre() {
        let mut s = SignalServer::default();
        let t = Instant::now();
        let (c1, h1, h2) = (a("200.0.0.1:1000"), a("201.0.0.2:5001"), a("201.0.0.2:5002"));
        registrar(&mut s, 7, c1, t);
        assert_eq!(procurar(&mut s, 7, h1, t), Some(c1));
        // O cliente continua se registrando (não pareou) e o host desistiu.
        let mut agora = t;
        while agora < t + POSSE {
            agora += Duration::from_secs(5);
            registrar(&mut s, 7, c1, agora);
        }
        assert_eq!(procurar(&mut s, 7, h2, agora), Some(c1));
    }

    #[test]
    fn sala_cheia_recusa() {
        let mut s = SignalServer::default();
        let t = Instant::now();
        for porta in 0..MAX_PER_ROOM as u16 {
            registrar(&mut s, 7, a(&format!("200.0.0.1:{}", 1000 + porta)), t);
        }
        let (r, ev) = s.handle(
            &pedido(
                1,
                SignalMessage::Register {
                    room: 7,
                    locals: vec![],
                },
            ),
            a("203.0.0.3:1000"),
            t,
        );
        assert!(matches!(ev, Some(SignalEvent::RoomTaken { .. })));
        assert_eq!(ler(&r[0].1).1, SignalMessage::RoomTaken);
    }

    #[test]
    fn lixo_respostas_e_pedidos_curtos_sao_ignorados() {
        let mut s = SignalServer::default();
        let t = Instant::now();
        let de = a("200.0.0.1:1000");
        assert_eq!(s.handle(b"lixo", de, t).0.len(), 0);
        let resposta = pedido(1, SignalMessage::NotFound);
        assert_eq!(s.handle(&resposta, de, t).0.len(), 0);
        let curto = PacketKind::Signal
            .control(1, 0)
            .encode_with_payload(&[2, 0, 0, 0, 7, 0])
            .unwrap();
        assert_eq!(s.handle(&curto, de, t).0.len(), 0);
        let ping = PacketKind::Ping.control(1, 0).encode_with_payload(&[]).unwrap();
        assert_eq!(s.handle(&ping, de, t).0.len(), 0);
    }
}
