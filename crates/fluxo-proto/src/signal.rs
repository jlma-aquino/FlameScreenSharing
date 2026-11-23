//! Mensagens do servidor de sinalização (payload de [`PacketKind::Signal`]).
//!
//! O servidor só apresenta um lado ao outro: o cliente se registra numa
//! **sala** (número tirado do código de pareamento), o host procura a sala, e
//! o servidor conta a cada um os endereços do outro — o público, que ele
//! mesmo vê (como um servidor STUN), e os locais, que cada lado informa.
//! Depois disso os dois falam direto (furo de NAT); o servidor não vê vídeo.
//!
//! ```text
//! cliente ── Register{sala} ──► servidor ◄── Lookup{sala} ── host
//!         ◄── Registered{público} ──┘
//!         ◄── Found{host} ──────────┴──────── Found{cliente} ──►
//! ```
//!
//! Endereço: família (1 byte: 4 ou 6), IP (4 ou 16 bytes), porta (u16 BE).
//!
//! **Grupos** (ADR-022): com o FSS aberto, cada membro manda `Presence` de
//! tempos em tempos, com a sua nota cifrada (apelido e, compartilhando, o
//! código da sala); o servidor responde `Members`, a lista do grupo da chave
//! que assinou o pedido, em páginas que nunca passam do tamanho do pedido.
//!
//! [`PacketKind::Signal`]: crate::PacketKind::Signal

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::ProtoError;

/// Endereços locais que cada lado pode informar (os excedentes são ignorados).
pub const MAX_LOCALS: usize = 4;

/// Tamanho mínimo do payload de um pedido (`Register`, `Lookup`), completado
/// com zeros. Toda resposta do servidor é menor: ele não serve para
/// amplificar tráfego contra um endereço forjado.
pub const REQUEST_LEN: usize = 100;

const REGISTER: u8 = 1;
const LOOKUP: u8 = 2;
const REGISTERED: u8 = 3;
const FOUND: u8 = 4;
const NOT_FOUND: u8 = 5;
const ROOM_TAKEN: u8 = 6;
const RELAY_BIND: u8 = 7;
const RELAY_READY: u8 = 8;
const PRESENCE: u8 = 9;
const MEMBERS: u8 = 10;

/// Tamanho do payload de `Presence` (completado com zeros). As páginas de
/// `Members` cabem nele: o servidor nunca responde mais do que recebe.
pub const PRESENCE_LEN: usize = 1200;

/// Maior nota de um membro (cifrada), em bytes.
pub const MAX_NOTE: usize = 120;

/// Maior nome de grupo, em bytes (UTF-8; o excedente é cortado).
pub const MAX_GROUP_NAME: usize = 32;

/// Membros por página de `Members`.
pub const MEMBERS_PER_PAGE: usize = 8;

/// Um membro na lista do grupo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberEntry {
    /// Número sorteado por cada FSS instalado (não identifica a pessoa).
    pub member: u64,
    /// Segundos desde o último `Presence` dele (até 65535).
    pub idle_s: u16,
    /// Nota cifrada com o segredo do grupo (o servidor não lê).
    pub note: Vec<u8>,
}

/// Uma mensagem de sinalização.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalMessage {
    /// Cliente → servidor: "espero um host nesta sala". Repetido de tempos
    /// em tempos, mantém a sala e o mapeamento do NAT vivos.
    Register { room: u32, locals: Vec<SocketAddr> },
    /// Host → servidor: "quero o cliente desta sala".
    Lookup { room: u32, locals: Vec<SocketAddr> },
    /// Servidor → cliente: registro aceito; `public` é o endereço do
    /// cliente visto da internet.
    Registered { public: SocketAddr },
    /// Servidor → host e cliente: o outro lado está nestes endereços.
    Found {
        public: SocketAddr,
        locals: Vec<SocketAddr>,
    },
    /// Servidor → host: nenhum cliente nessa sala (ainda).
    NotFound,
    /// Servidor → cliente: outro cliente já está nessa sala.
    RoomTaken,
    /// Host → servidor: o caminho direto não abriu; repasse os pacotes entre
    /// este socket e o cliente que você me entregou nesta sala.
    RelayBind { room: u32 },
    /// Servidor → host: repasse ligado; mande ao servidor o que iria ao cliente.
    RelayReady,
    /// Membro → servidor: "estou aqui", com a nota e a página da lista que
    /// quer de volta.
    Presence { member: u64, page: u8, note: Vec<u8> },
    /// Servidor → membro: uma página da lista do grupo.
    Members {
        name: String,
        page: u8,
        pages: u8,
        members: Vec<MemberEntry>,
    },
}

impl SignalMessage {
    /// Bytes do payload. Pedidos são completados até [`REQUEST_LEN`].
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(REQUEST_LEN);
        match self {
            Self::Register { room, locals } | Self::Lookup { room, locals } => {
                b.push(match self {
                    Self::Register { .. } => REGISTER,
                    _ => LOOKUP,
                });
                b.extend_from_slice(&room.to_be_bytes());
                write_list(&mut b, locals);
                b.resize(b.len().max(REQUEST_LEN), 0);
            }
            Self::Registered { public } => {
                b.push(REGISTERED);
                write_addr(&mut b, public);
            }
            Self::Found { public, locals } => {
                b.push(FOUND);
                write_addr(&mut b, public);
                write_list(&mut b, locals);
            }
            Self::NotFound => b.push(NOT_FOUND),
            Self::RoomTaken => b.push(ROOM_TAKEN),
            Self::RelayBind { room } => {
                b.push(RELAY_BIND);
                b.extend_from_slice(&room.to_be_bytes());
                b.resize(REQUEST_LEN, 0);
            }
            Self::RelayReady => b.push(RELAY_READY),
            Self::Presence { member, page, note } => {
                b.push(PRESENCE);
                b.extend_from_slice(&member.to_be_bytes());
                b.push(*page);
                write_note(&mut b, note);
                b.resize(b.len().max(PRESENCE_LEN), 0);
            }
            Self::Members {
                name,
                page,
                pages,
                members,
            } => {
                b.push(MEMBERS);
                b.push(*page);
                b.push(*pages);
                let nome = cortar(name, MAX_GROUP_NAME);
                b.push(nome.len() as u8);
                b.extend_from_slice(nome.as_bytes());
                let lista = &members[..members.len().min(MEMBERS_PER_PAGE)];
                b.push(lista.len() as u8);
                for m in lista {
                    b.extend_from_slice(&m.member.to_be_bytes());
                    b.extend_from_slice(&m.idle_s.to_be_bytes());
                    write_note(&mut b, &m.note);
                }
            }
        }
        b
    }

    /// Lê um payload. Pedidos menores que [`REQUEST_LEN`] são recusados.
    pub fn from_bytes(b: &[u8]) -> Result<Self, ProtoError> {
        let mut r = Reader(b);
        let msg = match r.u8()? {
            tag @ (REGISTER | LOOKUP) => {
                if b.len() < REQUEST_LEN {
                    return Err(ProtoError::BadSignal);
                }
                let room = u32::from_be_bytes(r.take(4)?.try_into().map_err(|_| ProtoError::BadSignal)?);
                let locals = r.list()?;
                match tag {
                    REGISTER => Self::Register { room, locals },
                    _ => Self::Lookup { room, locals },
                }
            }
            REGISTERED => Self::Registered { public: r.addr()? },
            FOUND => Self::Found {
                public: r.addr()?,
                locals: r.list()?,
            },
            NOT_FOUND => Self::NotFound,
            ROOM_TAKEN => Self::RoomTaken,
            RELAY_BIND => {
                if b.len() < REQUEST_LEN {
                    return Err(ProtoError::BadSignal);
                }
                Self::RelayBind {
                    room: u32::from_be_bytes(r.take(4)?.try_into().map_err(|_| ProtoError::BadSignal)?),
                }
            }
            RELAY_READY => Self::RelayReady,
            PRESENCE => {
                if b.len() < PRESENCE_LEN {
                    return Err(ProtoError::BadSignal);
                }
                Self::Presence {
                    member: r.u64()?,
                    page: r.u8()?,
                    note: r.note()?,
                }
            }
            MEMBERS => {
                let page = r.u8()?;
                let pages = r.u8()?;
                let n = r.u8()? as usize;
                let name = std::str::from_utf8(r.take(n)?)
                    .map_err(|_| ProtoError::BadSignal)?
                    .to_string();
                let quantos = r.u8()? as usize;
                if quantos > MEMBERS_PER_PAGE {
                    return Err(ProtoError::BadSignal);
                }
                let members = (0..quantos)
                    .map(|_| {
                        Ok(MemberEntry {
                            member: r.u64()?,
                            idle_s: r.u16()?,
                            note: r.note()?,
                        })
                    })
                    .collect::<Result<_, ProtoError>>()?;
                Self::Members {
                    name,
                    page,
                    pages,
                    members,
                }
            }
            _ => return Err(ProtoError::BadSignal),
        };
        Ok(msg)
    }
}

fn write_addr(b: &mut Vec<u8>, a: &SocketAddr) {
    match a.ip() {
        IpAddr::V4(ip) => {
            b.push(4);
            b.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            b.push(6);
            b.extend_from_slice(&ip.octets());
        }
    }
    b.extend_from_slice(&a.port().to_be_bytes());
}

fn write_note(b: &mut Vec<u8>, note: &[u8]) {
    let note = &note[..note.len().min(MAX_NOTE)];
    b.push(note.len() as u8);
    b.extend_from_slice(note);
}

/// `s` cortado em até `max` bytes, sem partir um caractere.
fn cortar(s: &str, max: usize) -> &str {
    let mut fim = s.len().min(max);
    while !s.is_char_boundary(fim) {
        fim -= 1;
    }
    &s[..fim]
}

fn write_list(b: &mut Vec<u8>, list: &[SocketAddr]) {
    let list = &list[..list.len().min(MAX_LOCALS)];
    b.push(list.len() as u8);
    for a in list {
        write_addr(b, a);
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], ProtoError> {
        if self.0.len() < n {
            return Err(ProtoError::BadSignal);
        }
        let (a, resto) = self.0.split_at(n);
        self.0 = resto;
        Ok(a)
    }

    fn u8(&mut self) -> Result<u8, ProtoError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ProtoError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u64(&mut self) -> Result<u64, ProtoError> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().map_err(|_| ProtoError::BadSignal)?,
        ))
    }

    fn note(&mut self) -> Result<Vec<u8>, ProtoError> {
        let n = self.u8()? as usize;
        if n > MAX_NOTE {
            return Err(ProtoError::BadSignal);
        }
        Ok(self.take(n)?.to_vec())
    }

    fn addr(&mut self) -> Result<SocketAddr, ProtoError> {
        let ip = match self.u8()? {
            4 => IpAddr::V4(Ipv4Addr::from(
                <[u8; 4]>::try_from(self.take(4)?).map_err(|_| ProtoError::BadSignal)?,
            )),
            6 => IpAddr::V6(Ipv6Addr::from(
                <[u8; 16]>::try_from(self.take(16)?).map_err(|_| ProtoError::BadSignal)?,
            )),
            _ => return Err(ProtoError::BadSignal),
        };
        let porta = self.take(2)?;
        Ok(SocketAddr::new(ip, u16::from_be_bytes([porta[0], porta[1]])))
    }

    fn list(&mut self) -> Result<Vec<SocketAddr>, ProtoError> {
        let n = self.u8()? as usize;
        if n > MAX_LOCALS {
            return Err(ProtoError::BadSignal);
        }
        (0..n).map(|_| self.addr()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn ida_e_volta_de_todas_as_mensagens() {
        let msgs = [
            SignalMessage::Register {
                room: 0xABCDE,
                locals: vec![a("192.168.1.2:9000"), a("[fe80::1]:9000")],
            },
            SignalMessage::Lookup {
                room: 7,
                locals: vec![],
            },
            SignalMessage::Registered {
                public: a("200.1.2.3:41000"),
            },
            SignalMessage::Found {
                public: a("[2001:db8::5]:5000"),
                locals: vec![a("10.0.0.9:5000")],
            },
            SignalMessage::NotFound,
            SignalMessage::RoomTaken,
            SignalMessage::RelayBind { room: 0x12345 },
            SignalMessage::RelayReady,
            SignalMessage::Presence {
                member: 0x0123_4567_89AB_CDEF,
                page: 2,
                note: vec![7; 40],
            },
            SignalMessage::Members {
                name: "Amigos da Flame".into(),
                page: 1,
                pages: 3,
                members: vec![
                    MemberEntry {
                        member: 1,
                        idle_s: 4,
                        note: vec![1, 2, 3],
                    },
                    MemberEntry {
                        member: u64::MAX,
                        idle_s: u16::MAX,
                        note: vec![],
                    },
                ],
            },
        ];
        for m in msgs {
            assert_eq!(SignalMessage::from_bytes(&m.to_bytes()).unwrap(), m);
        }
    }

    #[test]
    fn resposta_nunca_maior_que_pedido() {
        let quatro = vec![a("[2001:db8::1]:1"); MAX_LOCALS];
        let pedido = SignalMessage::Lookup {
            room: 1,
            locals: quatro.clone(),
        }
        .to_bytes();
        let resposta = SignalMessage::Found {
            public: a("[2001:db8::2]:2"),
            locals: quatro,
        }
        .to_bytes();
        assert_eq!(pedido.len(), REQUEST_LEN);
        assert!(resposta.len() < REQUEST_LEN);
    }

    #[test]
    fn pagina_cheia_de_membros_cabe_no_pedido_de_presenca() {
        let pedido = SignalMessage::Presence {
            member: 1,
            page: 0,
            note: vec![],
        }
        .to_bytes();
        let cheia = SignalMessage::Members {
            name: "x".repeat(MAX_GROUP_NAME + 10),
            page: 0,
            pages: 1,
            members: vec![
                MemberEntry {
                    member: 9,
                    idle_s: 9,
                    note: vec![9; MAX_NOTE + 5],
                };
                MEMBERS_PER_PAGE + 2
            ],
        }
        .to_bytes();
        assert_eq!(pedido.len(), PRESENCE_LEN);
        assert!(cheia.len() < PRESENCE_LEN, "{} bytes", cheia.len());
        match SignalMessage::from_bytes(&cheia).unwrap() {
            SignalMessage::Members { name, members, .. } => {
                assert_eq!(name.len(), MAX_GROUP_NAME);
                assert_eq!(members.len(), MEMBERS_PER_PAGE);
                assert!(members.iter().all(|m| m.note.len() == MAX_NOTE));
            }
            outra => panic!("{outra:?}"),
        }
        // Nome cortado sem partir um caractere de vários bytes.
        let acentos = SignalMessage::Members {
            name: "é".repeat(40),
            page: 0,
            pages: 1,
            members: vec![],
        };
        assert!(SignalMessage::from_bytes(&acentos.to_bytes()).is_ok());
    }

    #[test]
    fn presenca_curta_e_recusada() {
        let mut p = SignalMessage::Presence {
            member: 1,
            page: 0,
            note: vec![1; 10],
        }
        .to_bytes();
        p.truncate(PRESENCE_LEN - 1);
        assert_eq!(SignalMessage::from_bytes(&p), Err(ProtoError::BadSignal));
    }

    #[test]
    fn pedido_curto_ou_lixo_e_recusado() {
        let mut curto = SignalMessage::Register {
            room: 1,
            locals: vec![],
        }
        .to_bytes();
        curto.truncate(10);
        assert_eq!(SignalMessage::from_bytes(&curto), Err(ProtoError::BadSignal));
        assert_eq!(SignalMessage::from_bytes(&[]), Err(ProtoError::BadSignal));
        assert_eq!(SignalMessage::from_bytes(&[99]), Err(ProtoError::BadSignal));
        // Lista maior que o limite.
        let mut b = vec![FOUND, 4, 1, 2, 3, 4, 0, 1, MAX_LOCALS as u8 + 1];
        b.resize(200, 0);
        assert_eq!(SignalMessage::from_bytes(&b), Err(ProtoError::BadSignal));
    }

    #[test]
    fn excedentes_sao_cortados_ao_codificar() {
        let muitos = vec![a("10.0.0.1:1"); MAX_LOCALS + 3];
        let m = SignalMessage::Register {
            room: 2,
            locals: muitos,
        };
        match SignalMessage::from_bytes(&m.to_bytes()).unwrap() {
            SignalMessage::Register { locals, .. } => assert_eq!(locals.len(), MAX_LOCALS),
            outra => panic!("{outra:?}"),
        }
    }
}
