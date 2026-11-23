//! Convite de um grupo: [`GroupKey`] (ADR-022).
//!
//! Separado da chave do servidor ([`ServerKey`](crate::ServerKey)): a chave do
//! servidor dá acesso ao servidor (salas por código, repasse); o convite dá
//! acesso a um grupo dele. Para tirar alguém do grupo, quem roda o servidor
//! troca só o convite (mesmo número de grupo, segredo novo); a chave do
//! servidor continua a mesma para todos.
//!
//! Texto: `FSSG-` + base32 de número do grupo (u32) ‖ segredo (16 bytes) ‖
//! 2 bytes de conferência. Do segredo saem três chaves independentes:
//!
//! - **autenticação** ([`GroupKey::auth_key`]): é o que o servidor guarda. Com
//!   ela, ele confere que um `Presence` veio de quem tem o convite, mas não
//!   consegue abrir as notas nem calcular as salas;
//! - **notas** ([`GroupKey::seal_note`]): apelido e salas de cada membro,
//!   cifrados para os outros membros;
//! - **salas** ([`GroupKey::room`]): o número da sala no servidor depende do
//!   segredo, então quem não é do grupo, mesmo com o código, procura outra sala.

use std::fmt;

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::{CryptoError, PairingCode, BITS_DA_SALA};

/// Começo do texto do convite (e versão do formato).
const PREFIXO: &str = "FSSG-";

/// Bytes do segredo.
const SEGREDO: usize = 16;

/// Bytes da prova de grupo num `Presence`.
pub const GROUP_TAG_LEN: usize = 16;

/// Bytes que a cifra acrescenta a uma nota: nonce (12) e tag (16).
pub const NOTE_OVERHEAD: usize = 12 + 16;

/// Convite de um grupo: número e segredo.
#[derive(Clone, PartialEq, Eq)]
pub struct GroupKey {
    id: u32,
    segredo: [u8; SEGREDO],
}

impl GroupKey {
    /// Convite novo, com número de grupo sorteado.
    pub fn generate() -> Result<Self, CryptoError> {
        let mut b = [0u8; 4];
        crate::random_bytes(&mut b)?;
        Self::with_id(u32::from_be_bytes(b))
    }

    /// Convite novo para o grupo `id` (troca o convite sem mudar o grupo:
    /// o antigo para de valer quando o servidor passa a usar este).
    pub fn with_id(id: u32) -> Result<Self, CryptoError> {
        let mut segredo = [0u8; SEGREDO];
        crate::random_bytes(&mut segredo)?;
        Ok(Self { id, segredo })
    }

    /// Lê o texto de um convite (espaços, quebras de linha e minúsculas valem).
    pub fn parse(texto: &str) -> Result<Self, CryptoError> {
        let limpo: String = texto
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_uppercase();
        let corpo = limpo.strip_prefix(PREFIXO).ok_or(CryptoError::InvalidGroup)?;
        let simbolos: String = corpo.chars().filter(|c| *c != '-').collect();
        let bytes = crate::chave::de_base32(&simbolos).ok_or(CryptoError::InvalidGroup)?;
        if bytes.len() != 4 + SEGREDO + 2 {
            return Err(CryptoError::InvalidGroup);
        }
        let (dados, conferencia) = bytes.split_at(4 + SEGREDO);
        if Sha256::digest(dados)[..2] != *conferencia {
            return Err(CryptoError::InvalidGroup);
        }
        let id = u32::from_be_bytes(dados[..4].try_into().map_err(|_| CryptoError::InvalidGroup)?);
        let segredo = dados[4..].try_into().map_err(|_| CryptoError::InvalidGroup)?;
        Ok(Self { id, segredo })
    }

    /// Número do grupo (o mesmo depois de trocar o convite).
    pub fn id(&self) -> u32 {
        self.id
    }

    /// O que o servidor guarda para conferir os `Presence` deste convite.
    pub fn auth_key(&self) -> [u8; 32] {
        self.derivar(b"fluxo grupo auth v1")
    }

    /// Prova de que um pedido veio de quem tem o convite.
    pub fn tag(&self, dados: &[u8]) -> [u8; GROUP_TAG_LEN] {
        tag_com(&self.auth_key(), dados)
    }

    /// Confere uma prova com a chave guardada pelo servidor (tempo constante).
    pub fn verify_with(auth_key: &[u8; 32], dados: &[u8], tag: &[u8]) -> bool {
        let mut m = <Hmac<Sha256> as Mac>::new_from_slice(auth_key).expect("HMAC aceita qualquer chave");
        m.update(dados);
        m.verify_truncated_left(tag).is_ok()
    }

    /// Número da sala deste código no servidor, dentro do grupo: quem não tem
    /// o convite calcula outro número e não acha a sala.
    pub fn room(&self, codigo: &PairingCode) -> u32 {
        let mut m = <Hmac<Sha256> as Mac>::new_from_slice(&self.derivar(b"fluxo grupo sala v1"))
            .expect("HMAC aceita qualquer chave");
        m.update(codigo.as_bytes());
        let h = m.finalize().into_bytes();
        u32::from_be_bytes([h[0], h[1], h[2], h[3]]) >> (32 - BITS_DA_SALA)
    }

    /// Cifra uma nota do membro `membro` para os outros membros do grupo
    /// (ChaCha20-Poly1305, nonce sorteado). O número do membro vai como dado
    /// autenticado: o servidor não consegue pôr a nota de um em outro.
    pub fn seal_note(&self, membro: u64, nota: &[u8]) -> Result<Vec<u8>, CryptoError> {
        use chacha20poly1305::aead::{Aead, KeyInit, Payload};
        let mut nonce = [0u8; 12];
        crate::random_bytes(&mut nonce)?;
        let cifra = chacha20poly1305::ChaCha20Poly1305::new(&self.derivar(b"fluxo grupo nota v1").into());
        let corpo = cifra
            .encrypt(
                &nonce.into(),
                Payload {
                    msg: nota,
                    aad: &membro.to_be_bytes(),
                },
            )
            .map_err(|_| CryptoError::Seal)?;
        Ok([nonce.as_slice(), &corpo].concat())
    }

    /// Abre a nota do membro `membro`; `None` se ela não foi cifrada com este
    /// convite para ele (forjada, de outro grupo, convite antigo ou alterada).
    pub fn open_note(&self, membro: u64, nota: &[u8]) -> Option<Vec<u8>> {
        use chacha20poly1305::aead::{Aead, KeyInit, Payload};
        if nota.len() < NOTE_OVERHEAD {
            return None;
        }
        let (nonce, corpo) = nota.split_at(12);
        let nonce: [u8; 12] = nonce.try_into().ok()?;
        let cifra = chacha20poly1305::ChaCha20Poly1305::new(&self.derivar(b"fluxo grupo nota v1").into());
        cifra
            .decrypt(
                &nonce.into(),
                Payload {
                    msg: corpo,
                    aad: &membro.to_be_bytes(),
                },
            )
            .ok()
    }

    fn derivar(&self, contexto: &[u8]) -> [u8; 32] {
        let mut m = <Hmac<Sha256> as Mac>::new_from_slice(&self.segredo).expect("HMAC aceita qualquer chave");
        m.update(contexto);
        m.finalize().into_bytes().into()
    }
}

fn tag_com(auth: &[u8; 32], dados: &[u8]) -> [u8; GROUP_TAG_LEN] {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(auth).expect("HMAC aceita qualquer chave");
    m.update(dados);
    let mut out = [0u8; GROUP_TAG_LEN];
    out.copy_from_slice(&m.finalize().into_bytes()[..GROUP_TAG_LEN]);
    out
}

impl fmt::Display for GroupKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut b = self.id.to_be_bytes().to_vec();
        b.extend_from_slice(&self.segredo);
        let conferencia = Sha256::digest(&b);
        b.extend_from_slice(&conferencia[..2]);
        let simbolos = crate::chave::base32(&b);
        let grupos: Vec<&str> = simbolos
            .as_bytes()
            .chunks(5)
            .map(|c| std::str::from_utf8(c).unwrap_or_default())
            .collect();
        write!(f, "{PREFIXO}{}", grupos.join("-"))
    }
}

/// Sem o segredo: o convite não vaza num `{:?}` de registro.
impl fmt::Debug for GroupKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GroupKey({:08x})", self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texto_de_ida_e_volta() {
        let g = GroupKey::generate().unwrap();
        let t = g.to_string();
        assert!(t.starts_with("FSSG-"), "{t}");
        assert_eq!(GroupKey::parse(&t).unwrap(), g);
        assert_eq!(GroupKey::parse(&format!(" {}\n", t.to_lowercase())).unwrap(), g);
        let mut c: Vec<char> = t.chars().collect();
        let i = c.len() / 2;
        c[i] = if c[i] == '2' { '3' } else { '2' };
        assert_eq!(
            GroupKey::parse(&c.into_iter().collect::<String>()),
            Err(CryptoError::InvalidGroup)
        );
        assert_eq!(GroupKey::parse("FSS1-ABCDE"), Err(CryptoError::InvalidGroup));
        assert!(!format!("{g:?}").contains(&t[5..15]));
    }

    #[test]
    fn convite_novo_mesmo_grupo_antigo_para_de_valer() {
        let velho = GroupKey::generate().unwrap();
        let novo = GroupKey::with_id(velho.id()).unwrap();
        assert_eq!(novo.id(), velho.id());
        let pedido = b"presenca do membro 7";
        // O servidor guarda a auth do convite novo: o velho não passa.
        assert!(GroupKey::verify_with(&novo.auth_key(), pedido, &novo.tag(pedido)));
        assert!(!GroupKey::verify_with(
            &novo.auth_key(),
            pedido,
            &velho.tag(pedido)
        ));
        // Nem abre as notas nem acha as salas do convite novo.
        let nota = novo.seal_note(7, b"Ana").unwrap();
        assert_eq!(velho.open_note(7, &nota), None);
        let codigo = PairingCode::generate().unwrap();
        assert_ne!(velho.room(&codigo), novo.room(&codigo));
    }

    #[test]
    fn nota_e_sala() {
        let g = GroupKey::generate().unwrap();
        let nota = g.seal_note(7, b"Ana|K7M4-QX9P").unwrap();
        assert_eq!(nota.len(), 13 + NOTE_OVERHEAD);
        assert_eq!(
            g.open_note(7, &nota).as_deref(),
            Some(b"Ana|K7M4-QX9P".as_slice())
        );
        assert_eq!(g.open_note(8, &nota), None);
        let codigo = PairingCode::generate().unwrap();
        assert_eq!(g.room(&codigo), g.room(&codigo));
        assert!(g.room(&codigo) < 1 << BITS_DA_SALA);
        assert_ne!(g.room(&codigo), codigo.room(), "sala do grupo não é a sala comum");
        // A auth não abre notas: o servidor não lê.
        assert_ne!(g.auth_key(), g.derivar(b"fluxo grupo nota v1"));
    }
}
