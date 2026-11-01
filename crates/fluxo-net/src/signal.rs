//! Servidor de sinalização: [`SignalServer`].
//!
//! Apresenta host e cliente que estão em redes diferentes. O cliente se
//! registra na sala do seu código ([`PairingCode::room`]); o host procura a
//! sala; o servidor conta a cada um os endereços do outro. O endereço
//! público é o de onde o pedido chegou — o servidor faz o papel de STUN pelo
//! mesmo socket que depois leva o vídeo, então é exatamente o mapeamento do
//! NAT que interessa. Depois disso os dois furam os NATs mandando pacotes um
//! ao outro (ver `sender.rs` e `receiver.rs`); vídeo nunca passa por aqui.
//!
//! [`PairingCode::room`]: fluxo_crypto::PairingCode::room

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fluxo_proto::{PacketHeader, PacketKind, SignalMessage};

use crate::{clock_us, is_transient};

/// Sem novo registro por este tempo, a sala do cliente some (o cliente
/// renova o registro a cada 5 s).
pub const ROOM_TTL: Duration = Duration::from_secs(30);

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
    /// Cliente tentou uma sala ocupada por outro.
    RoomTaken { room: u32, client: SocketAddr },
}

struct Sala {
    cliente: SocketAddr,
    locais: Vec<SocketAddr>,
    visto: Instant,
}

/// Estado do servidor: as salas com cliente esperando.
#[derive(Default)]
pub struct SignalServer {
    salas: HashMap<u32, Sala>,
}

impl SignalServer {
    /// Trata um datagrama: devolve o que mandar (destino, pacote) e o
    /// evento, se houve. Lixo e pacotes que não são pedidos são ignorados.
    pub fn handle(
        &mut self,
        datagrama: &[u8],
        de: SocketAddr,
        agora: Instant,
    ) -> (Vec<(SocketAddr, Vec<u8>)>, Option<SignalEvent>) {
        self.salas.retain(|_, s| agora.duration_since(s.visto) < ROOM_TTL);
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
                // Outro cliente na sala: só toma o lugar se for do mesmo IP
                // (o mesmo PC reabriu numa porta nova).
                if let Some(s) = self.salas.get(&room) {
                    if s.cliente != de && s.cliente.ip() != de.ip() {
                        let r = resposta(h.frame_id, &SignalMessage::RoomTaken);
                        return (vec![(de, r)], Some(SignalEvent::RoomTaken { room, client: de }));
                    }
                }
                let novo = self.salas.get(&room).is_none_or(|s| s.cliente != de);
                self.salas.insert(
                    room,
                    Sala {
                        cliente: de,
                        locais: locals,
                        visto: agora,
                    },
                );
                let r = resposta(h.frame_id, &SignalMessage::Registered { public: de });
                let evento = novo.then_some(SignalEvent::Registered { room, client: de });
                (vec![(de, r)], evento)
            }
            SignalMessage::Lookup { room, locals } => match self.salas.get(&room) {
                Some(s) => {
                    let para_host = SignalMessage::Found {
                        public: s.cliente,
                        locals: s.locais.clone(),
                    };
                    let para_cliente = SignalMessage::Found { public: de, locals };
                    let envios = vec![
                        (de, resposta(h.frame_id, &para_host)),
                        (s.cliente, resposta(0, &para_cliente)),
                    ];
                    let evento = SignalEvent::Found {
                        room,
                        host: de,
                        client: s.cliente,
                    };
                    (envios, Some(evento))
                }
                None => {
                    let r = resposta(h.frame_id, &SignalMessage::NotFound);
                    (vec![(de, r)], Some(SignalEvent::NotFound { room, host: de }))
                }
            },
            // Respostas não são pedidos: alguém mandando coisa errada.
            SignalMessage::Registered { .. }
            | SignalMessage::Found { .. }
            | SignalMessage::NotFound
            | SignalMessage::RoomTaken => (Vec::new(), None),
        }
    }

    /// Salas com cliente esperando.
    pub fn rooms(&self) -> usize {
        self.salas.len()
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

    #[test]
    fn sala_vazia_ocupada_e_expirada() {
        let mut s = SignalServer::default();
        let t = Instant::now();
        let host = a("201.0.0.2:52000");
        let (r, _) = s.handle(
            &pedido(
                1,
                SignalMessage::Lookup {
                    room: 7,
                    locals: vec![],
                },
            ),
            host,
            t,
        );
        assert_eq!(ler(&r[0].1).1, SignalMessage::NotFound);

        let registro = pedido(
            1,
            SignalMessage::Register {
                room: 7,
                locals: vec![],
            },
        );
        s.handle(&registro, a("200.0.0.1:1000"), t);
        // Renovar não gera evento; outro IP é recusado; o mesmo IP em outra
        // porta toma o lugar.
        assert_eq!(s.handle(&registro, a("200.0.0.1:1000"), t).1, None);
        let (r, ev) = s.handle(&registro, a("203.0.0.3:1000"), t);
        assert!(matches!(ev, Some(SignalEvent::RoomTaken { .. })));
        assert_eq!(ler(&r[0].1).1, SignalMessage::RoomTaken);
        let (_, ev) = s.handle(&registro, a("200.0.0.1:2000"), t);
        assert!(matches!(ev, Some(SignalEvent::Registered { .. })));

        // Sem renovar, a sala expira e fica livre para outro.
        let depois = t + ROOM_TTL + Duration::from_secs(1);
        let (_, ev) = s.handle(&registro, a("203.0.0.3:1000"), depois);
        assert!(matches!(ev, Some(SignalEvent::Registered { .. })));
        assert_eq!(s.rooms(), 1);
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
