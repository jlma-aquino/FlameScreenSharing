//! Chave de acesso ao servidor de sinalização: [`ServerKey`].
//!
//! Um texto que o dono do servidor manda aos amigos junto com o programa
//! (`FSS1-…`). Leva o endereço do servidor e um segredo aleatório de 128 bits.
//! Cada pedido ao servidor sai assinado com o segredo (HMAC-SHA256, 16 bytes
//! no fim do datagrama), e o servidor ignora o que não estiver assinado com
//! uma chave que ele conhece. Saber o endereço não basta para usar o
//! servidor; perdeu a chave, o dono gera outra e apaga a antiga.
//!
//! O endereço não é segredo (aparece na rede de quem usa); quem protege o
//! servidor é a assinatura.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::CryptoError;

/// Bytes da assinatura no fim de cada pedido.
pub const KEY_TAG_LEN: usize = 16;

/// Bytes do segredo.
const SEGREDO: usize = 16;

/// Começo do texto da chave (e versão do formato).
const PREFIXO: &str = "FSS1-";

/// Base32 sem 0, 1, I e O (não se confundem ao copiar à mão).
const ALFABETO: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

/// Separa a assinatura dos pedidos de qualquer outro uso do segredo.
const CONTEXTO: &[u8] = b"fluxo sinal v1";

/// Endereço do servidor dentro da chave: IP (compacto) ou nome.
const IPV4: u8 = 4;
const IPV6: u8 = 6;
const NOME: u8 = 1;

/// Chave de acesso a um servidor de sinalização.
#[derive(Clone, PartialEq, Eq)]
pub struct ServerKey {
    /// Como achar o servidor: `IP:porta` ou `nome:porta`.
    servidor: String,
    segredo: [u8; SEGREDO],
}

impl ServerKey {
    /// Chave nova para `servidor` (`IP:porta` ou `nome:porta`), com segredo
    /// sorteado pelo sistema.
    pub fn generate(servidor: &str) -> Result<Self, CryptoError> {
        let mut segredo = [0u8; SEGREDO];
        crate::random_bytes(&mut segredo)?;
        let k = Self {
            servidor: servidor.trim().to_string(),
            segredo,
        };
        // Confere que o endereço cabe no formato.
        k.bytes()?;
        Ok(k)
    }

    /// Lê o texto de uma chave (espaços, quebras de linha e minúsculas valem).
    pub fn parse(texto: &str) -> Result<Self, CryptoError> {
        let limpo: String = texto
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_uppercase();
        let corpo = limpo.strip_prefix(PREFIXO).ok_or(CryptoError::InvalidKey)?;
        let simbolos: String = corpo.chars().filter(|c| *c != '-').collect();
        let bytes = de_base32(&simbolos).ok_or(CryptoError::InvalidKey)?;
        // Os 2 últimos bytes conferem o resto: erro de cópia não passa.
        if bytes.len() < 3 {
            return Err(CryptoError::InvalidKey);
        }
        let (dados, conferencia) = bytes.split_at(bytes.len() - 2);
        if Sha256::digest(dados)[..2] != *conferencia {
            return Err(CryptoError::InvalidKey);
        }
        let (tipo, resto) = dados.split_first().ok_or(CryptoError::InvalidKey)?;
        let (servidor, resto) = match *tipo {
            IPV4 if resto.len() >= 6 => {
                let ip = Ipv4Addr::new(resto[0], resto[1], resto[2], resto[3]);
                let porta = u16::from_be_bytes([resto[4], resto[5]]);
                (SocketAddr::from((ip, porta)).to_string(), &resto[6..])
            }
            IPV6 if resto.len() >= 18 => {
                let ip: [u8; 16] = resto[..16].try_into().map_err(|_| CryptoError::InvalidKey)?;
                let porta = u16::from_be_bytes([resto[16], resto[17]]);
                (
                    SocketAddr::from((Ipv6Addr::from(ip), porta)).to_string(),
                    &resto[18..],
                )
            }
            NOME => {
                let (tamanho, resto) = resto.split_first().ok_or(CryptoError::InvalidKey)?;
                let nome = resto.get(..*tamanho as usize).ok_or(CryptoError::InvalidKey)?;
                let nome = std::str::from_utf8(nome).map_err(|_| CryptoError::InvalidKey)?;
                (nome.to_string(), &resto[*tamanho as usize..])
            }
            _ => return Err(CryptoError::InvalidKey),
        };
        let segredo: [u8; SEGREDO] = resto.try_into().map_err(|_| CryptoError::InvalidKey)?;
        Ok(Self { servidor, segredo })
    }

    /// Onde fica o servidor (`IP:porta` ou `nome:porta`).
    pub fn server(&self) -> &str {
        &self.servidor
    }

    /// Assinatura de um pedido.
    pub fn sign(&self, pedido: &[u8]) -> [u8; KEY_TAG_LEN] {
        let mut out = [0u8; KEY_TAG_LEN];
        out.copy_from_slice(&self.mac(pedido).finalize().into_bytes()[..KEY_TAG_LEN]);
        out
    }

    /// Pedido + assinatura, pronto para mandar.
    pub fn signed(&self, pedido: &[u8]) -> Vec<u8> {
        let mut v = pedido.to_vec();
        v.extend_from_slice(&self.sign(pedido));
        v
    }

    /// Se `datagrama` (pedido + assinatura) foi assinado com esta chave,
    /// devolve o pedido. Comparação em tempo constante.
    pub fn verify<'a>(&self, datagrama: &'a [u8]) -> Option<&'a [u8]> {
        let corte = datagrama.len().checked_sub(KEY_TAG_LEN)?;
        let (pedido, tag) = datagrama.split_at(corte);
        self.mac(pedido).verify_truncated_left(tag).ok().map(|_| pedido)
    }

    /// Identificação curta para registros (não revela o segredo).
    pub fn id(&self) -> String {
        let h = Sha256::digest([CONTEXTO, b" id".as_slice(), &self.segredo].concat());
        h[..4].iter().map(|b| format!("{b:02x}")).collect()
    }

    fn mac(&self, pedido: &[u8]) -> Hmac<Sha256> {
        let mut m = <Hmac<Sha256> as Mac>::new_from_slice(&self.segredo)
            .expect("HMAC aceita qualquer tamanho de chave");
        m.update(CONTEXTO);
        m.update(pedido);
        m
    }

    fn bytes(&self) -> Result<Vec<u8>, CryptoError> {
        let mut b = Vec::new();
        match self.servidor.parse::<SocketAddr>() {
            Ok(SocketAddr::V4(a)) => {
                b.push(IPV4);
                b.extend_from_slice(&a.ip().octets());
                b.extend_from_slice(&a.port().to_be_bytes());
            }
            Ok(SocketAddr::V6(a)) => {
                b.push(IPV6);
                b.extend_from_slice(&a.ip().octets());
                b.extend_from_slice(&a.port().to_be_bytes());
            }
            Err(_) => {
                let nome = self.servidor.as_bytes();
                if nome.is_empty() || nome.len() > 200 {
                    return Err(CryptoError::InvalidKey);
                }
                b.push(NOME);
                b.push(nome.len() as u8);
                b.extend_from_slice(nome);
            }
        }
        b.extend_from_slice(&self.segredo);
        let conferencia = Sha256::digest(&b);
        b.extend_from_slice(&conferencia[..2]);
        Ok(b)
    }
}

impl fmt::Display for ServerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let simbolos = base32(&self.bytes().map_err(|_| fmt::Error)?);
        let grupos: Vec<&str> = simbolos
            .as_bytes()
            .chunks(5)
            .map(|c| std::str::from_utf8(c).unwrap_or_default())
            .collect();
        write!(f, "{PREFIXO}{}", grupos.join("-"))
    }
}

/// Sem o segredo: a chave não vaza num `{:?}` de registro.
impl fmt::Debug for ServerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ServerKey({} · {})", self.servidor, self.id())
    }
}

fn base32(bytes: &[u8]) -> String {
    let mut s = String::new();
    let (mut acumulado, mut bits) = (0u32, 0u32);
    for &b in bytes {
        acumulado = (acumulado << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            s.push(ALFABETO[((acumulado >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        s.push(ALFABETO[((acumulado << (5 - bits)) & 31) as usize] as char);
    }
    s
}

fn de_base32(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acumulado, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = ALFABETO.iter().position(|a| *a == c)? as u32;
        acumulado = (acumulado << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acumulado >> bits) as u8);
        }
        acumulado &= (1 << bits) - 1;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texto_de_ida_e_volta() {
        for servidor in [
            "137.131.152.250:3478",
            "[2001:db8::7]:3478",
            "sinal.exemplo.com:7000",
        ] {
            let k = ServerKey::generate(servidor).unwrap();
            let texto = k.to_string();
            assert!(texto.starts_with("FSS1-"), "{texto}");
            let lido = ServerKey::parse(&texto).unwrap();
            assert_eq!(lido, k);
            assert_eq!(lido.server(), servidor);
        }
    }

    #[test]
    fn colado_com_espacos_e_minusculas() {
        let k = ServerKey::generate("137.131.152.250:3478").unwrap();
        let texto = k.to_string();
        let baguncado = format!("  {}\n", texto.to_lowercase().replace('-', " - "));
        assert_eq!(ServerKey::parse(&baguncado).unwrap(), k);
    }

    #[test]
    fn erro_de_copia_nao_passa() {
        let texto = ServerKey::generate("137.131.152.250:3478").unwrap().to_string();
        // Troca um símbolo do meio.
        let mut c: Vec<char> = texto.chars().collect();
        let i = c.len() / 2;
        c[i] = if c[i] == '2' { '3' } else { '2' };
        let trocado: String = c.into_iter().collect();
        assert_eq!(ServerKey::parse(&trocado), Err(CryptoError::InvalidKey));
        assert_eq!(
            ServerKey::parse(&texto[..texto.len() - 3]),
            Err(CryptoError::InvalidKey)
        );
        assert_eq!(ServerKey::parse("FSS1-"), Err(CryptoError::InvalidKey));
        assert_eq!(ServerKey::parse("K7M4-QX9P"), Err(CryptoError::InvalidKey));
    }

    #[test]
    fn assinatura_so_vale_com_a_mesma_chave() {
        let a = ServerKey::generate("137.131.152.250:3478").unwrap();
        let b = ServerKey::generate("137.131.152.250:3478").unwrap();
        let pedido = b"registro na sala 42";
        let assinado = a.signed(pedido);
        assert_eq!(assinado.len(), pedido.len() + KEY_TAG_LEN);
        assert_eq!(a.verify(&assinado), Some(pedido.as_slice()));
        assert_eq!(b.verify(&assinado), None);
        let mut alterado = assinado.clone();
        alterado[0] ^= 1;
        assert_eq!(a.verify(&alterado), None);
        assert_eq!(a.verify(&[0u8; 5]), None);
        assert_eq!(a.verify(pedido), None);
    }

    #[test]
    fn debug_nao_mostra_o_segredo() {
        let k = ServerKey::generate("137.131.152.250:3478").unwrap();
        let d = format!("{k:?}");
        assert!(d.contains("137.131.152.250:3478") && d.contains(&k.id()));
        assert!(!d.contains(&k.to_string()[5..15]));
    }
}
