//! Presença no grupo: [`GroupClient`].
//!
//! Com o FSS aberto, o cliente manda `Presence` ao servidor da chave a cada
//! [`PRESENCE_EVERY`] (ou logo que o apelido ou a sala mudam) e monta a lista
//! do grupo com as páginas de `Members` que voltam. A nota de cada membro
//! (apelido e, compartilhando, o código da sala) vai cifrada com o segredo
//! da chave: o servidor guarda e repassa, mas não lê, e uma nota que não abre
//! (forjada, ou de outro grupo) é descartada.
//!
//! Sem threads: quem usa chama [`GroupClient::poll`] de tempos em tempos
//! (o socket não bloqueia).

use std::collections::BTreeMap;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use fluxo_crypto::ServerKey;
use fluxo_proto::{PacketHeader, PacketKind, SignalMessage, MAX_NOTE};

use crate::clock_us;
use crate::signal::{MEMBER_ONLINE, PRESENCE_EVERY};

/// Sem resposta do servidor por este tempo, a lista é considerada velha.
pub const GROUP_STALE: Duration = Duration::from_secs(12);

/// Maior apelido, em bytes (o excedente é cortado).
pub const MAX_NICK: usize = 32;

/// Versão do formato da nota.
const NOTA_V1: u8 = 1;

/// Um membro do grupo, como este FSS vê.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupMember {
    /// Número sorteado pelo FSS dele.
    pub member: u64,
    pub nick: String,
    /// Compartilhando agora: o código da sala para entrar.
    pub room: Option<String>,
    /// Assistindo agora: o código da sala de quem.
    pub watching: Option<String>,
    /// Mandou presença há menos de [`MEMBER_ONLINE`].
    pub online: bool,
    /// É este FSS.
    pub me: bool,
}

/// O que este FSS conta ao grupo: apelido, a sala que compartilha e a que
/// assiste. A sala assistida vai no fim: notas sem ela (de versões antigas)
/// continuam valendo, e versões antigas ignoram o que sobra.
fn nota(apelido: &str, sala: Option<&str>, assistindo: Option<&str>) -> Vec<u8> {
    let mut n = vec![NOTA_V1];
    for (texto, max) in [
        (apelido.trim(), MAX_NICK),
        (sala.unwrap_or_default(), 16),
        (assistindo.unwrap_or_default(), 16),
    ] {
        let texto = cortar(texto, max);
        n.push(texto.len() as u8);
        n.extend_from_slice(texto.as_bytes());
    }
    n
}

/// Lê uma nota aberta: (apelido, sala, sala assistida).
fn ler_nota(n: &[u8]) -> Option<(String, Option<String>, Option<String>)> {
    let (&versao, mut resto) = n.split_first()?;
    if versao != NOTA_V1 {
        return None;
    }
    let mut campo = || -> Option<Option<String>> {
        let (&tam, depois) = resto.split_first()?;
        let texto = std::str::from_utf8(depois.get(..tam as usize)?).ok()?;
        resto = &depois[tam as usize..];
        Some((!texto.is_empty()).then(|| texto.to_string()))
    };
    let apelido = campo()?.unwrap_or_default();
    let sala = campo()?;
    let assistindo = campo().flatten();
    Some((apelido, sala, assistindo))
}

fn cortar(s: &str, max: usize) -> &str {
    let mut fim = s.len().min(max);
    while !s.is_char_boundary(fim) {
        fim -= 1;
    }
    &s[..fim]
}

/// Presença deste FSS num grupo e a lista dos outros membros.
pub struct GroupClient {
    socket: UdpSocket,
    servidor: SocketAddr,
    chave: ServerKey,
    membro: u64,
    apelido: String,
    sala: Option<String>,
    assistindo: Option<String>,
    nota: Vec<u8>,
    /// Mudou o apelido ou a sala: avisar já.
    sujo: bool,
    enviado: Option<Instant>,
    respondido: Option<Instant>,
    nome: Option<String>,
    lista: Vec<GroupMember>,
    /// A rodada em montagem: membros e páginas que já chegaram.
    coleta: BTreeMap<u64, GroupMember>,
    paginas: Vec<bool>,
}

impl GroupClient {
    /// Liga ao servidor da chave (resolve o endereço; pode demorar se for
    /// um nome). `membro` é o número deste FSS (guardado nas configurações).
    pub fn connect(chave: ServerKey, membro: u64) -> io::Result<Self> {
        let servidor = chave
            .server()
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "servidor da chave sem endereço"))?;
        let local = match servidor {
            SocketAddr::V4(_) => "0.0.0.0:0",
            SocketAddr::V6(_) => "[::]:0",
        };
        let socket = UdpSocket::bind(local)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            servidor,
            chave,
            membro,
            apelido: String::new(),
            sala: None,
            assistindo: None,
            nota: Vec::new(),
            sujo: true,
            enviado: None,
            respondido: None,
            nome: None,
            lista: Vec::new(),
            coleta: BTreeMap::new(),
            paginas: Vec::new(),
        })
    }

    /// O que contar ao grupo: o apelido, o código da sala que compartilha e o
    /// da que assiste.
    pub fn set_status(&mut self, apelido: &str, sala: Option<&str>, assistindo: Option<&str>) {
        if self.apelido != apelido
            || self.sala.as_deref() != sala
            || self.assistindo.as_deref() != assistindo
            || self.nota.is_empty()
        {
            self.apelido = apelido.to_string();
            self.sala = sala.map(str::to_string);
            self.assistindo = assistindo.map(str::to_string);
            self.sujo = true;
        }
    }

    /// Nome do grupo (depois da primeira resposta).
    pub fn name(&self) -> Option<&str> {
        self.nome.as_deref()
    }

    /// Os membros: online primeiro, por apelido.
    pub fn members(&self) -> &[GroupMember] {
        &self.lista
    }

    /// O servidor respondeu há pouco.
    pub fn connected(&self, agora: Instant) -> bool {
        self.respondido
            .is_some_and(|t| agora.duration_since(t) < GROUP_STALE)
    }

    /// Manda a presença quando é hora e lê as respostas. Devolve se a lista
    /// ou o nome mudaram.
    pub fn poll(&mut self, agora: Instant) -> bool {
        let hora = self
            .enviado
            .is_none_or(|t| agora.duration_since(t) >= PRESENCE_EVERY);
        if self.sujo || hora {
            if self.sujo {
                match self.chave.seal_note(
                    self.membro,
                    &nota(&self.apelido, self.sala.as_deref(), self.assistindo.as_deref()),
                ) {
                    Ok(n) if n.len() <= MAX_NOTE => self.nota = n,
                    _ => self.nota.clear(),
                }
                self.sujo = false;
            }
            self.coleta.clear();
            self.paginas.clear();
            self.pedir(0);
            self.enviado = Some(agora);
        }
        let mut mudou = false;
        let mut buf = [0u8; 2048];
        while let Ok((n, de)) = self.socket.recv_from(&mut buf) {
            if de != self.servidor {
                continue;
            }
            mudou |= self.tratar(&buf[..n], agora);
        }
        mudou
    }

    fn pedir(&self, pagina: u8) {
        let msg = SignalMessage::Presence {
            member: self.membro,
            page: pagina,
            note: self.nota.clone(),
        };
        let Ok(pacote) = PacketKind::Signal
            .control(pagina as u32, clock_us())
            .encode_with_payload(&msg.to_bytes())
        else {
            return;
        };
        let _ = self.socket.send_to(&self.chave.signed(&pacote), self.servidor);
    }

    fn tratar(&mut self, datagrama: &[u8], agora: Instant) -> bool {
        let Ok((h, payload)) = PacketHeader::decode(datagrama) else {
            return false;
        };
        if h.kind != PacketKind::Signal {
            return false;
        }
        let Ok(SignalMessage::Members {
            name,
            page,
            pages,
            members,
        }) = SignalMessage::from_bytes(payload)
        else {
            return false;
        };
        let pages = pages.max(1) as usize;
        if self.paginas.len() != pages {
            // Primeira página da rodada (ou o grupo mudou de tamanho): pede o resto.
            self.paginas = vec![false; pages];
            for p in 1..pages {
                if p != page as usize {
                    self.pedir(p as u8);
                }
            }
        }
        if let Some(vista) = self.paginas.get_mut(page as usize) {
            *vista = true;
        }
        for m in members {
            let Some((nick, room, watching)) =
                self.chave.open_note(m.member, &m.note).and_then(|n| ler_nota(&n))
            else {
                continue;
            };
            let online = Duration::from_secs(m.idle_s as u64) < MEMBER_ONLINE;
            self.coleta.insert(
                m.member,
                GroupMember {
                    member: m.member,
                    nick,
                    room: room.filter(|_| online),
                    watching: watching.filter(|_| online),
                    online,
                    me: m.member == self.membro,
                },
            );
        }
        self.respondido = Some(agora);
        let mut mudou = self.nome.as_deref() != Some(name.as_str());
        self.nome = Some(name);
        if self.paginas.iter().all(|v| *v) {
            let mut lista: Vec<GroupMember> = self.coleta.values().cloned().collect();
            lista.sort_by(|a, b| {
                b.online
                    .cmp(&a.online)
                    .then_with(|| a.nick.to_lowercase().cmp(&b.nick.to_lowercase()))
                    .then(a.member.cmp(&b.member))
            });
            mudou |= lista != self.lista;
            self.lista = lista;
        }
        mudou
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SignalServer;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn nota_de_ida_e_volta() {
        assert_eq!(
            ler_nota(&nota("Ana", Some("K7M4-QX9P"), None)),
            Some(("Ana".into(), Some("K7M4-QX9P".into()), None))
        );
        assert_eq!(
            ler_nota(&nota("  Bia ", None, Some("K7M4-QX9P"))),
            Some(("Bia".into(), None, Some("K7M4-QX9P".into())))
        );
        assert_eq!(ler_nota(&nota(&"é".repeat(40), None, None)).unwrap().0.len(), 32);
        // Nota da primeira versão (sem a sala assistida) continua valendo.
        assert_eq!(
            ler_nota(&[NOTA_V1, 3, b'C', b'a', b'i', 0]),
            Some(("Cai".into(), None, None))
        );
        assert_eq!(ler_nota(&[]), None);
        assert_eq!(ler_nota(&[2, 0, 0]), None);
    }

    /// Espera até `ok` valer, chamando `poll` nos clientes.
    fn ate(clientes: &mut [&mut GroupClient], ok: impl Fn(&[&mut GroupClient]) -> bool) -> bool {
        let fim = Instant::now() + Duration::from_secs(5);
        while Instant::now() < fim {
            for c in clientes.iter_mut() {
                c.poll(Instant::now());
            }
            if ok(clientes) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn membros_do_grupo_se_veem_e_outro_grupo_nao() {
        let servidor = UdpSocket::bind("127.0.0.1:0").unwrap();
        let endereco = servidor.local_addr().unwrap().to_string();
        let amigos = ServerKey::generate(&endereco).unwrap();
        let outros = ServerKey::generate(&endereco).unwrap();
        let parar = Arc::new(AtomicBool::new(false));
        let t = {
            let (parar, grupos) = (
                parar.clone(),
                vec![
                    (amigos.clone(), "Amigos".to_string()),
                    (outros.clone(), "Outros".to_string()),
                ],
            );
            std::thread::spawn(move || SignalServer::with_groups(grupos).run(&servidor, &parar, |_| {}))
        };

        let mut ana = GroupClient::connect(amigos.clone(), 1).unwrap();
        let mut bia = GroupClient::connect(amigos.clone(), 2).unwrap();
        let mut caio = GroupClient::connect(outros, 3).unwrap();
        ana.set_status("Ana", Some("K7M4-QX9P"), None);
        bia.set_status("Bia", None, Some("K7M4-QX9P"));
        caio.set_status("Caio", None, None);
        let viram = ate(&mut [&mut ana, &mut bia, &mut caio], |c| {
            c[0].members().len() == 2 && c[1].members().len() == 2 && c[2].members().len() == 1
        });
        assert!(viram, "ana {:?} bia {:?}", ana.members(), bia.members());
        assert_eq!(bia.name(), Some("Amigos"));
        assert_eq!(caio.name(), Some("Outros"));
        let ana_vista = bia.members().iter().find(|m| m.nick == "Ana").unwrap();
        assert_eq!(ana_vista.room.as_deref(), Some("K7M4-QX9P"));
        assert!(ana_vista.online && !ana_vista.me);
        assert!(bia.members().iter().any(|m| m.me && m.nick == "Bia"));
        // Ana vê que Bia está assistindo a sala dela.
        let bia_vista = ana.members().iter().find(|m| m.nick == "Bia").unwrap();
        assert_eq!(bia_vista.watching.as_deref(), Some("K7M4-QX9P"));
        assert!(caio.members().iter().all(|m| m.nick == "Caio"));

        // Ana parou de compartilhar: ela vê na hora, e Bia na próxima rodada dela (até 4 s).
        ana.set_status("Ana", None, None);
        let parou = ate(&mut [&mut ana, &mut bia], |c| {
            c.iter()
                .all(|x| x.members().len() == 2 && x.members().iter().all(|m| m.room.is_none()))
        });
        assert!(parou, "bia {:?}", bia.members());
        parar.store(true, Ordering::Relaxed);
        t.join().unwrap();
    }

    #[test]
    fn grupo_grande_vem_em_varias_paginas() {
        let servidor = UdpSocket::bind("127.0.0.1:0").unwrap();
        let endereco = servidor.local_addr().unwrap().to_string();
        let chave = ServerKey::generate(&endereco).unwrap();
        let parar = Arc::new(AtomicBool::new(false));
        let t = {
            let (parar, grupos) = (parar.clone(), vec![(chave.clone(), "Grande".to_string())]);
            std::thread::spawn(move || SignalServer::with_groups(grupos).run(&servidor, &parar, |_| {}))
        };
        let mut todos: Vec<GroupClient> = (0..20u64)
            .map(|i| {
                let mut c = GroupClient::connect(chave.clone(), 100 + i).unwrap();
                c.set_status(&format!("Membro {i}"), None, None);
                c
            })
            .collect();
        let mut refs: Vec<&mut GroupClient> = todos.iter_mut().collect();
        assert!(ate(&mut refs, |c| c[0].members().len() == 20
            && c[19].members().len() == 20));
        parar.store(true, Ordering::Relaxed);
        t.join().unwrap();
    }
}
