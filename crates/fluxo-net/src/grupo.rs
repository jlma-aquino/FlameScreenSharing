//! Presença no grupo: [`GroupClient`].
//!
//! Com o FSS aberto, o cliente manda um `Presence` curto ao servidor da chave
//! (com a prova do convite do grupo, [`GroupKey`]) a cada [`PRESENCE_EVERY`]
//! (ou logo que o apelido ou a sala mudam), com a versão da lista que já tem.
//! A resposta é a versão atual; só quando ela muda o cliente pede as páginas
//! de `Members` (ADR-023). A nota de cada membro (apelido e, compartilhando,
//! o código da sala) vai cifrada com o segredo do convite: o servidor guarda
//! e repassa, mas não lê, e uma nota que não abre (forjada, ou de outro
//! grupo) é descartada.
//!
//! **Fotos** (ADR-023): cada membro sobe a sua foto (cifrada) uma vez, e de
//! novo só se o servidor disser que não tem (reiniciou, ou a foto mudou).
//! Quem vê a lista baixa a foto de um membro só quando o número dela muda, uma
//! foto de cada vez, e guarda a cifrada numa pasta ([`GroupClient::with_photo_cache`]):
//! ao abrir o FSS de novo, nada desce outra vez.
//!
//! Sem threads: quem usa chama [`GroupClient::poll`] de tempos em tempos
//! (o socket não bloqueia).

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use fluxo_crypto::{photo_id, GroupKey, ServerKey};
use fluxo_proto::{
    PacketHeader, PacketKind, SignalMessage, MAX_NOTE, MAX_PHOTO, MAX_PHOTO_PARTS, PHOTO_PART,
};

use crate::clock_us;
use crate::signal::{prova_foto, prova_pedido, MEMBER_ONLINE, PRESENCE_EVERY};

/// Sem resposta do servidor por este tempo, a lista é considerada velha.
pub const GROUP_STALE: Duration = Duration::from_secs(12);

/// Maior apelido, em bytes (o excedente é cortado).
pub const MAX_NICK: usize = 32;

/// Maior foto de perfil (o JPEG, antes de cifrar), em bytes.
pub const MAX_PHOTO_JPEG: usize = MAX_PHOTO - fluxo_crypto::NOTE_OVERHEAD;

/// Versão do formato da nota.
const NOTA_V1: u8 = 1;

/// Pediu páginas ou partes e nada voltou neste tempo: pede de novo.
const REPETIR: Duration = Duration::from_secs(2);

/// Subiu a foto e o servidor ainda não tem: espera isto antes de subir de novo.
const REENVIAR_FOTO: Duration = Duration::from_secs(20);

/// Uma foto que não deu para baixar só é tentada de novo depois disto.
const DESISTIR_FOTO: Duration = Duration::from_secs(60);

/// Tentativas de baixar uma foto antes de desistir por um tempo.
const TENTATIVAS_FOTO: u8 = 3;

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
    /// Número da foto dele (0: sem foto). A foto em si: [`GroupClient::photo`].
    pub photo: u32,
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

/// A foto de um membro chegando em partes.
struct Baixando {
    dono: u64,
    id: u32,
    partes: Vec<Option<Vec<u8>>>,
    pedido: Instant,
    tentativas: u8,
}

/// Presença deste FSS num grupo e a lista dos outros membros.
pub struct GroupClient {
    socket: UdpSocket,
    servidor: SocketAddr,
    chave: ServerKey,
    grupo: GroupKey,
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
    /// Versão da lista que está em `lista` (0: nenhuma ainda).
    versao: u32,
    /// Buscando a lista nesta versão, desde quando (último pedido).
    buscando: Option<(u32, Instant)>,
    /// A busca em andamento: membros e páginas que já chegaram.
    coleta: BTreeMap<u64, GroupMember>,
    paginas: Vec<bool>,
    /// A foto deste FSS: o JPEG, a cifrada e o número dela.
    minha_foto: Option<(Vec<u8>, Vec<u8>, u32)>,
    /// A foto deste FSS que o servidor tem (0: nenhuma).
    foto_no_servidor: u32,
    foto_enviada: Option<Instant>,
    /// Fotos dos membros, abertas: dono → (número, JPEG).
    fotos: HashMap<u64, (u32, Vec<u8>)>,
    baixando: Option<Baixando>,
    /// Fotos que não deram para baixar, e quando desistiu.
    desistiu: HashMap<(u64, u32), Instant>,
    pasta_fotos: Option<PathBuf>,
}

impl GroupClient {
    /// Liga ao servidor da chave (resolve o endereço; pode demorar se for
    /// um nome). `membro` é o número deste FSS (guardado nas configurações).
    pub fn connect(chave: ServerKey, grupo: GroupKey, membro: u64) -> io::Result<Self> {
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
            grupo,
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
            versao: 0,
            buscando: None,
            coleta: BTreeMap::new(),
            paginas: Vec::new(),
            minha_foto: None,
            foto_no_servidor: 0,
            foto_enviada: None,
            fotos: HashMap::new(),
            baixando: None,
            desistiu: HashMap::new(),
            pasta_fotos: None,
        })
    }

    /// Guarda as fotos baixadas (cifradas) nesta pasta, e procura nela antes
    /// de baixar: abrir o FSS de novo não baixa tudo outra vez.
    pub fn with_photo_cache(mut self, pasta: PathBuf) -> Self {
        self.pasta_fotos = Some(pasta);
        self
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

    /// A foto de perfil deste FSS (um JPEG de até [`MAX_PHOTO_JPEG`] bytes),
    /// ou nenhuma. Sobe ao servidor só se mudou.
    pub fn set_photo(&mut self, jpeg: Option<&[u8]>) {
        let jpeg = jpeg.filter(|j| !j.is_empty() && j.len() <= MAX_PHOTO_JPEG);
        if self.minha_foto.as_ref().map(|f| f.0.as_slice()) == jpeg {
            return;
        }
        self.minha_foto = jpeg.and_then(|j| {
            let cifrada = self.grupo.seal_photo(self.membro, j).ok()?;
            let id = photo_id(&cifrada);
            Some((j.to_vec(), cifrada, id))
        });
        match &self.minha_foto {
            Some((j, _, id)) => {
                self.fotos.insert(self.membro, (*id, j.clone()));
            }
            None => {
                self.fotos.remove(&self.membro);
            }
        }
        self.foto_enviada = None;
    }

    /// Nome do grupo (depois da primeira resposta).
    pub fn name(&self) -> Option<&str> {
        self.nome.as_deref()
    }

    /// Os membros: online primeiro, por apelido.
    pub fn members(&self) -> &[GroupMember] {
        &self.lista
    }

    /// A foto (JPEG) de um membro, se já chegou: (número, bytes).
    pub fn photo(&self, membro: u64) -> Option<(u32, &[u8])> {
        self.fotos.get(&membro).map(|(id, j)| (*id, j.as_slice()))
    }

    /// O servidor respondeu há pouco.
    pub fn connected(&self, agora: Instant) -> bool {
        self.respondido
            .is_some_and(|t| agora.duration_since(t) < GROUP_STALE)
    }

    /// Manda a presença quando é hora e lê as respostas. Devolve se a lista,
    /// o nome ou alguma foto mudaram.
    pub fn poll(&mut self, agora: Instant) -> bool {
        let hora = self
            .enviado
            .is_none_or(|t| agora.duration_since(t) >= PRESENCE_EVERY);
        if self.sujo || hora {
            if self.sujo {
                match self.grupo.seal_note(
                    self.membro,
                    &nota(&self.apelido, self.sala.as_deref(), self.assistindo.as_deref()),
                ) {
                    Ok(n) if n.len() <= MAX_NOTE => self.nota = n,
                    _ => self.nota.clear(),
                }
                self.sujo = false;
            }
            self.presenca(0, false);
            self.enviado = Some(agora);
        }
        // Páginas pedidas que não voltaram: pede as que faltam.
        if let Some((v, t)) = self.buscando {
            if agora.duration_since(t) >= REPETIR {
                self.buscando = Some((v, agora));
                self.pedir_faltando();
            }
        }
        let mut mudou = false;
        let mut buf = [0u8; 2048];
        while let Ok((n, de)) = self.socket.recv_from(&mut buf) {
            if de != self.servidor {
                continue;
            }
            mudou |= self.tratar(&buf[..n], agora);
        }
        mudou |= self.fotos_dos_outros(agora);
        self.subir_foto(agora);
        mudou
    }

    fn mandar(&self, id: u32, msg: &SignalMessage) {
        let Ok(pacote) = PacketKind::Signal
            .control(id, clock_us())
            .encode_with_payload(&msg.to_bytes())
        else {
            return;
        };
        let _ = self.socket.send_to(&self.chave.signed(&pacote), self.servidor);
    }

    /// `cheia`: pede a página `pagina` da lista; senão só a versão.
    fn presenca(&self, pagina: u8, cheia: bool) {
        let prova = [self.membro.to_be_bytes().as_slice(), &self.nota].concat();
        self.mandar(
            pagina as u32,
            &SignalMessage::Presence {
                member: self.membro,
                page: pagina,
                group: self.grupo.id(),
                known: self.versao,
                full: cheia,
                tag: self.grupo.tag(&prova),
                note: self.nota.clone(),
            },
        );
    }

    fn pedir_faltando(&self) {
        if self.paginas.is_empty() {
            self.presenca(0, true);
        }
        for (p, vista) in self.paginas.iter().enumerate() {
            if !vista {
                self.presenca(p as u8, true);
            }
        }
    }

    fn tratar(&mut self, datagrama: &[u8], agora: Instant) -> bool {
        let Ok((h, payload)) = PacketHeader::decode(datagrama) else {
            return false;
        };
        if h.kind != PacketKind::Signal {
            return false;
        }
        match SignalMessage::from_bytes(payload) {
            Ok(SignalMessage::GroupState { version, photo }) => {
                self.respondido = Some(agora);
                self.foto_no_servidor = photo;
                let ja_busca = self.buscando.is_some_and(|(v, _)| v == version);
                if version != self.versao && !ja_busca {
                    self.buscando = Some((version, agora));
                    self.coleta.clear();
                    self.paginas.clear();
                    self.presenca(0, true);
                }
                false
            }
            Ok(SignalMessage::Members {
                name,
                version,
                page,
                pages,
                members,
            }) => self.pagina(name, version, page, pages, members, agora),
            Ok(SignalMessage::Photo {
                owner,
                id,
                part,
                parts,
                data,
            }) => self.parte_de_foto(owner, id, part, parts, data, agora),
            _ => false,
        }
    }

    fn pagina(
        &mut self,
        name: String,
        version: u32,
        page: u8,
        pages: u8,
        members: Vec<fluxo_proto::MemberEntry>,
        agora: Instant,
    ) -> bool {
        self.respondido = Some(agora);
        if version == self.versao && self.buscando.is_none() {
            return false;
        }
        // A lista mudou de novo no meio da busca: recomeça nesta versão.
        if self.buscando.map(|(v, _)| v) != Some(version) {
            self.buscando = Some((version, agora));
            self.coleta.clear();
            self.paginas.clear();
        }
        let pages = pages.max(1) as usize;
        if self.paginas.len() != pages {
            // Primeira página da busca: pede o resto.
            self.paginas = vec![false; pages];
            for p in 1..pages {
                if p != page as usize {
                    self.presenca(p as u8, true);
                }
            }
        }
        if let Some(vista) = self.paginas.get_mut(page as usize) {
            *vista = true;
        }
        for m in members {
            let Some((nick, room, watching)) =
                self.grupo.open_note(m.member, &m.note).and_then(|n| ler_nota(&n))
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
                    photo: m.photo,
                },
            );
            if m.member == self.membro {
                self.foto_no_servidor = m.photo;
            }
        }
        let mut mudou = self.nome.as_deref() != Some(name.as_str());
        self.nome = Some(name);
        if self.paginas.iter().all(|v| *v) {
            let mut lista: Vec<GroupMember> = std::mem::take(&mut self.coleta).into_values().collect();
            lista.sort_by(|a, b| {
                b.online
                    .cmp(&a.online)
                    .then_with(|| a.nick.to_lowercase().cmp(&b.nick.to_lowercase()))
                    .then(a.member.cmp(&b.member))
            });
            // Quem tirou a foto: some daqui também.
            let antes = self.fotos.len();
            let eu = self.membro;
            self.fotos
                .retain(|dono, _| *dono == eu || lista.iter().any(|m| m.member == *dono && m.photo != 0));
            mudou |= lista != self.lista || antes != self.fotos.len();
            self.lista = lista;
            self.versao = version;
            self.buscando = None;
            self.paginas.clear();
        }
        mudou
    }

    /// Procura a próxima foto que falta: primeiro na pasta, senão pede ao
    /// servidor (uma de cada vez).
    fn fotos_dos_outros(&mut self, agora: Instant) -> bool {
        if let Some(b) = &mut self.baixando {
            if agora.duration_since(b.pedido) < REPETIR {
                return false;
            }
            if b.tentativas >= TENTATIVAS_FOTO {
                self.desistiu.insert((b.dono, b.id), agora);
                self.baixando = None;
                return false;
            }
            b.tentativas += 1;
            b.pedido = agora;
            let (dono, faltando): (u64, Vec<u8>) = (
                b.dono,
                match b.partes.is_empty() {
                    true => vec![0],
                    false => (0..b.partes.len() as u8)
                        .filter(|p| b.partes[*p as usize].is_none())
                        .collect(),
                },
            );
            for p in faltando {
                self.pedir_parte(dono, p);
            }
            return false;
        }
        self.desistiu
            .retain(|_, t| agora.duration_since(*t) < DESISTIR_FOTO);
        let falta = self.lista.iter().find(|m| {
            !m.me
                && m.photo != 0
                && self.fotos.get(&m.member).map(|f| f.0) != Some(m.photo)
                && !self.desistiu.contains_key(&(m.member, m.photo))
        });
        let Some((dono, id)) = falta.map(|m| (m.member, m.photo)) else {
            return false;
        };
        if let Some(jpeg) = self.da_pasta(dono, id) {
            self.fotos.insert(dono, (id, jpeg));
            return true;
        }
        self.baixando = Some(Baixando {
            dono,
            id,
            partes: Vec::new(),
            pedido: agora,
            tentativas: 1,
        });
        self.pedir_parte(dono, 0);
        false
    }

    fn pedir_parte(&self, dono: u64, parte: u8) {
        self.mandar(
            0,
            &SignalMessage::PhotoGet {
                member: self.membro,
                group: self.grupo.id(),
                tag: self.grupo.tag(&prova_pedido(self.membro, dono, parte)),
                owner: dono,
                part: parte,
            },
        );
    }

    fn parte_de_foto(
        &mut self,
        dono: u64,
        id: u32,
        parte: u8,
        partes: u8,
        dados: Vec<u8>,
        agora: Instant,
    ) -> bool {
        self.respondido = Some(agora);
        let Some(b) = self.baixando.as_mut().filter(|b| b.dono == dono) else {
            return false;
        };
        // O servidor não tem (ou já é outra foto): a lista nova vai dizer.
        if partes == 0 || id != b.id || partes as usize > MAX_PHOTO_PARTS {
            self.desistiu.insert((dono, b.id), agora);
            self.baixando = None;
            return false;
        }
        if b.partes.len() != partes as usize {
            b.partes = vec![None; partes as usize];
            for p in 1..partes {
                if p != parte {
                    self.pedir_parte(dono, p);
                }
            }
        }
        let Some(b) = self.baixando.as_mut() else {
            return false;
        };
        if let Some(lugar) = b.partes.get_mut(parte as usize) {
            *lugar = Some(dados);
        }
        if !b.partes.iter().all(Option::is_some) {
            return false;
        }
        let cifrada: Vec<u8> = b.partes.drain(..).flatten().flatten().collect();
        self.baixando = None;
        match (photo_id(&cifrada) == id)
            .then(|| self.grupo.open_photo(dono, &cifrada))
            .flatten()
        {
            Some(jpeg) => {
                self.para_pasta(dono, &cifrada);
                self.fotos.insert(dono, (id, jpeg));
                true
            }
            None => {
                self.desistiu.insert((dono, id), agora);
                false
            }
        }
    }

    /// Sobe a foto deste FSS se o servidor não tem a mesma (ou apaga lá).
    fn subir_foto(&mut self, agora: Instant) {
        if self.respondido.is_none() || self.versao == 0 {
            return; // ainda não está na lista do servidor
        }
        let minha = self.minha_foto.as_ref().map_or(0, |f| f.2);
        if minha == self.foto_no_servidor {
            return;
        }
        if self
            .foto_enviada
            .is_some_and(|t| agora.duration_since(t) < REENVIAR_FOTO)
        {
            return;
        }
        self.foto_enviada = Some(agora);
        let partes: Vec<&[u8]> = match &self.minha_foto {
            Some((_, cifrada, _)) => cifrada.chunks(PHOTO_PART).collect(),
            None => Vec::new(),
        };
        let total = partes.len() as u8;
        if total == 0 {
            self.mandar_parte(0, 0, &[]);
        }
        for (i, dados) in partes.iter().enumerate() {
            self.mandar_parte(i as u8, total, dados);
        }
    }

    fn mandar_parte(&self, parte: u8, partes: u8, dados: &[u8]) {
        self.mandar(
            0,
            &SignalMessage::PhotoPut {
                member: self.membro,
                group: self.grupo.id(),
                tag: self.grupo.tag(&prova_foto(self.membro, parte, partes, dados)),
                part: parte,
                parts: partes,
                data: dados.to_vec(),
            },
        );
    }

    fn arquivo(&self, dono: u64) -> Option<PathBuf> {
        Some(self.pasta_fotos.as_ref()?.join(format!("{dono:016x}.foto")))
    }

    fn da_pasta(&self, dono: u64, id: u32) -> Option<Vec<u8>> {
        let cifrada = std::fs::read(self.arquivo(dono)?).ok()?;
        (photo_id(&cifrada) == id)
            .then(|| self.grupo.open_photo(dono, &cifrada))
            .flatten()
    }

    fn para_pasta(&self, dono: u64, cifrada: &[u8]) {
        if let (Some(pasta), Some(arquivo)) = (&self.pasta_fotos, self.arquivo(dono)) {
            let _ = std::fs::create_dir_all(pasta);
            let _ = std::fs::write(arquivo, cifrada);
        }
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
        // Uma chave de servidor para todos; dois grupos com convites próprios.
        let chave = ServerKey::generate(&endereco).unwrap();
        let amigos = GroupKey::generate().unwrap();
        let outros = GroupKey::generate().unwrap();
        let parar = Arc::new(AtomicBool::new(false));
        let t = {
            let (parar, chaves, grupos) = (
                parar.clone(),
                vec![chave.clone()],
                vec![
                    (amigos.id(), amigos.auth_key(), "Amigos".to_string()),
                    (outros.id(), outros.auth_key(), "Outros".to_string()),
                ],
            );
            std::thread::spawn(move || {
                SignalServer::with_keys(chaves)
                    .with_groups(grupos)
                    .run(&servidor, &parar, |_| {})
            })
        };

        let mut ana = GroupClient::connect(chave.clone(), amigos.clone(), 1).unwrap();
        let mut bia = GroupClient::connect(chave.clone(), amigos.clone(), 2).unwrap();
        let mut caio = GroupClient::connect(chave, outros, 3).unwrap();
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
        let grupo = GroupKey::generate().unwrap();
        let parar = Arc::new(AtomicBool::new(false));
        let t = {
            let (parar, chaves, grupos) = (
                parar.clone(),
                vec![chave.clone()],
                vec![(grupo.id(), grupo.auth_key(), "Grande".to_string())],
            );
            std::thread::spawn(move || {
                SignalServer::with_keys(chaves)
                    .with_groups(grupos)
                    .run(&servidor, &parar, |_| {})
            })
        };
        let mut todos: Vec<GroupClient> = (0..20u64)
            .map(|i| {
                let mut c = GroupClient::connect(chave.clone(), grupo.clone(), 100 + i).unwrap();
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

    #[test]
    fn foto_sobe_desce_e_fica_na_pasta() {
        let servidor = UdpSocket::bind("127.0.0.1:0").unwrap();
        let endereco = servidor.local_addr().unwrap().to_string();
        let chave = ServerKey::generate(&endereco).unwrap();
        let grupo = GroupKey::generate().unwrap();
        let parar = Arc::new(AtomicBool::new(false));
        let t = {
            let (parar, chaves, grupos) = (
                parar.clone(),
                vec![chave.clone()],
                vec![(grupo.id(), grupo.auth_key(), "Fotos".to_string())],
            );
            std::thread::spawn(move || {
                SignalServer::with_keys(chaves)
                    .with_groups(grupos)
                    .run(&servidor, &parar, |_| {})
            })
        };
        let pasta = std::env::temp_dir().join(format!("fss-fotos-teste-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&pasta);
        // Uma foto de 3 partes (o JPEG de verdade é menor; aqui vale o tamanho).
        let jpeg: Vec<u8> = (0..2500u32).map(|i| (i % 251) as u8).collect();
        let mut ana = GroupClient::connect(chave.clone(), grupo.clone(), 1).unwrap();
        let mut bia = GroupClient::connect(chave.clone(), grupo.clone(), 2)
            .unwrap()
            .with_photo_cache(pasta.clone());
        ana.set_status("Ana", None, None);
        bia.set_status("Bia", None, None);
        ana.set_photo(Some(&jpeg));
        assert_eq!(
            ana.photo(1).map(|f| f.1),
            Some(jpeg.as_slice()),
            "a própria na hora"
        );
        let viu = ate(&mut [&mut ana, &mut bia], |c| c[1].photo(1).is_some());
        assert!(viu, "bia {:?}", bia.members());
        assert_eq!(bia.photo(1).map(|f| f.1), Some(jpeg.as_slice()));
        assert!(pasta.join(format!("{:016x}.foto", 1u64)).exists());

        // Ana tira a foto: some para Bia também.
        ana.set_photo(None);
        let sumiu = ate(&mut [&mut ana, &mut bia], |c| {
            c[1].photo(1).is_none() && c[1].members().iter().all(|m| m.photo == 0)
        });
        assert!(sumiu, "bia {:?}", bia.members());
        parar.store(true, Ordering::Relaxed);
        t.join().unwrap();
        let _ = std::fs::remove_dir_all(&pasta);
    }
}
