//! Códigos para conectar sem servidor: [`ConnectionCode`] e [`ReplyCode`].
//!
//! Sem servidor de sinalização, quem leva os endereços de um lado ao outro
//! são as pessoas (pelo chat que já usam). O cliente mostra o **código de
//! conexão** (código de pareamento + seus endereços públicos); o host digita
//! e mostra o **código de resposta** (os endereços dele); o cliente digita
//! esse, e os dois furam os NATs um na direção do outro.
//!
//! ```text
//! só IPv4:   K7M4-QX9P-ABCD-EFGH-JK   8 do pareamento + 10 de endereços + 1 de conferência
//! com IPv6:  K7M4-QX9P-…  (48 símbolos)  8 + 39 + 1
//! resposta:  ABCD-EFGH-JKM  (ou 40 símbolos com IPv6)
//! ```
//!
//! Mesmo alfabeto do código de pareamento (31 símbolos, sem 0/O, 1/I/L). Os
//! endereços viram um número em base 31: `[marca][IPv4 4][porta 2]` e, com
//! IPv6, mais `[IPv6 16][porta 2]` (bytes big-endian; a marca é 0 ou 1 e diz
//! que a porta IPv4 foi aberta no roteador por UPnP). O último símbolo é
//! uma soma ponderada módulo 31 de todos os anteriores (pesos 1 a 30 em
//! ciclo), que pega qualquer
//! símbolo trocado e qualquer par de vizinhos invertido.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use fluxo_crypto::{PairingCode, ALFABETO};

const BASE: u32 = 31;
const SIMBOLOS_PAREAMENTO: usize = 8;
/// Endereços só IPv4 (7 bytes, < 2⁴⁹ ≤ 31¹⁰).
const SIMBOLOS_V4: usize = 10;
/// Endereços IPv4 + IPv6 (25 bytes, < 2¹⁹³ ≤ 31³⁹).
const SIMBOLOS_V6: usize = 39;

/// Código que o cliente mostra: pareamento + endereços públicos do cliente.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionCode {
    pub pairing: PairingCode,
    pub addr: SocketAddrV4,
    /// Endereço IPv6, se o cliente tem (IPv6 costuma não ter NAT).
    pub addr6: Option<SocketAddrV6>,
    /// A porta IPv4 foi aberta no roteador por UPnP: o host chega direto,
    /// sem esperar furo nem código de resposta.
    pub open: bool,
}

/// Código que o host mostra de volta: os endereços públicos do host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplyCode {
    pub addr: SocketAddrV4,
    pub addr6: Option<SocketAddrV6>,
}

/// Endereços a tentar, IPv6 primeiro (sem NAT, passa mais fácil).
fn candidatos(addr: SocketAddrV4, addr6: Option<SocketAddrV6>) -> Vec<SocketAddr> {
    addr6
        .map(SocketAddr::V6)
        .into_iter()
        .chain(Some(SocketAddr::V4(addr)).filter(|a| !a.ip().is_unspecified()))
        .collect()
}

impl ConnectionCode {
    /// Endereços do cliente, para o host tentar.
    pub fn candidates(&self) -> Vec<SocketAddr> {
        candidatos(self.addr, self.addr6)
    }
}

impl ReplyCode {
    /// Endereços do host, para o cliente furar na direção deles.
    pub fn candidates(&self) -> Vec<SocketAddr> {
        candidatos(self.addr, self.addr6)
    }
}

/// Código digitado com erro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeError {
    /// Quantidade de símbolos errada.
    Length { got: usize, expected: usize },
    /// Símbolo fora do alfabeto (ex.: 0, O, 1, I, L).
    Symbol(char),
    /// O símbolo de conferência não bate: algo foi digitado errado.
    Check,
}

impl fmt::Display for CodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length { got, expected } => {
                write!(f, "o código tem {got} letras/números; deveria ter {expected}")
            }
            Self::Symbol(c) => write!(f, "'{c}' não existe nos códigos (não há 0, O, 1, I nem L)"),
            Self::Check => write!(f, "código digitado errado (a conferência não bate)"),
        }
    }
}

impl std::error::Error for CodeError {}

impl ConnectionCode {
    /// Lê o que foi digitado: ignora hífens e espaços, aceita minúsculas.
    pub fn parse(texto: &str) -> Result<Self, CodeError> {
        let v = simbolos(
            texto,
            &[
                SIMBOLOS_PAREAMENTO + SIMBOLOS_V4 + 1,
                SIMBOLOS_PAREAMENTO + SIMBOLOS_V6 + 1,
            ],
        )?;
        let pareamento: String = v[..SIMBOLOS_PAREAMENTO]
            .iter()
            .map(|&s| ALFABETO[s as usize] as char)
            .collect();
        let (addr, addr6, open) = enderecos(&v[SIMBOLOS_PAREAMENTO..])?;
        Ok(Self {
            pairing: PairingCode::parse(&pareamento).map_err(|_| CodeError::Check)?,
            addr,
            addr6,
            open,
        })
    }
}

impl ReplyCode {
    /// Lê o que foi digitado: ignora hífens e espaços, aceita minúsculas.
    pub fn parse(texto: &str) -> Result<Self, CodeError> {
        let v = simbolos(texto, &[SIMBOLOS_V4 + 1, SIMBOLOS_V6 + 1])?;
        let (addr, addr6, _) = enderecos(&v)?;
        Ok(Self { addr, addr6 })
    }
}

impl fmt::Display for ConnectionCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut v: Vec<u8> = self
            .pairing
            .as_bytes()
            .iter()
            .map(|c| ALFABETO.iter().position(|a| a == c).unwrap_or(0) as u8)
            .collect();
        v.extend(simbolos_dos_enderecos(self.addr, self.addr6, self.open));
        escrever(f, v)
    }
}

impl fmt::Display for ReplyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        escrever(f, simbolos_dos_enderecos(self.addr, self.addr6, false))
    }
}

/// Não mostra o código de pareamento em logs de depuração.
impl fmt::Debug for ConnectionCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ConnectionCode(****-****, {}, {:?}, aberto: {})",
            self.addr, self.addr6, self.open
        )
    }
}

/// Soma ponderada módulo 31, com pesos 1, 2, …, 30, 1, 2… (nunca 31, que
/// seria 0 e deixaria passar um erro naquela posição): com base prima, pega
/// qualquer símbolo trocado e qualquer par de vizinhos invertido.
fn conferencia(v: &[u8]) -> u8 {
    (v.iter()
        .enumerate()
        .map(|(i, &s)| (i as u64 % 30 + 1) * s as u64)
        .sum::<u64>()
        % BASE as u64) as u8
}

/// Acrescenta a conferência e escreve em grupos de 4.
fn escrever(f: &mut fmt::Formatter<'_>, mut v: Vec<u8>) -> fmt::Result {
    v.push(conferencia(&v));
    for (i, s) in v.iter().enumerate() {
        if i > 0 && i % 4 == 0 {
            f.write_str("-")?;
        }
        write!(f, "{}", ALFABETO[*s as usize] as char)?;
    }
    Ok(())
}

/// Símbolos (0..31) do que foi digitado, com a conferência verificada e
/// removida. `tamanhos`: as quantidades aceitas (com a conferência).
fn simbolos(texto: &str, tamanhos: &[usize]) -> Result<Vec<u8>, CodeError> {
    let mut v = Vec::new();
    for c in texto.chars().filter(|c| !matches!(c, '-' | ' ')) {
        let maiuscula = c.to_ascii_uppercase();
        let s = ALFABETO
            .iter()
            .position(|&a| a as char == maiuscula)
            .ok_or(CodeError::Symbol(c))?;
        v.push(s as u8);
    }
    if !tamanhos.contains(&v.len()) {
        return Err(CodeError::Length {
            got: v.len(),
            expected: tamanhos[0],
        });
    }
    let conf = v.pop().expect("tamanho conferido");
    if conferencia(&v) != conf {
        return Err(CodeError::Check);
    }
    Ok(v)
}

/// Número big-endian em `bytes` → `n` algarismos em base 31.
fn para_base31(bytes: &[u8], n: usize) -> Vec<u8> {
    let mut num = bytes.to_vec();
    let mut algarismos = vec![0u8; n];
    for a in algarismos.iter_mut().rev() {
        let mut resto = 0u32;
        for b in num.iter_mut() {
            let atual = resto * 256 + *b as u32;
            *b = (atual / BASE) as u8;
            resto = atual % BASE;
        }
        *a = resto as u8;
    }
    algarismos
}

/// Algarismos em base 31 → número big-endian de `n` bytes (`None` se não cabe).
fn de_base31(algarismos: &[u8], n: usize) -> Option<Vec<u8>> {
    let mut num = vec![0u8; n];
    for &d in algarismos {
        let mut vai = d as u32;
        for b in num.iter_mut().rev() {
            let atual = *b as u32 * BASE + vai;
            *b = atual as u8;
            vai = atual >> 8;
        }
        if vai != 0 {
            return None;
        }
    }
    Some(num)
}

fn simbolos_dos_enderecos(a: SocketAddrV4, a6: Option<SocketAddrV6>, aberto: bool) -> Vec<u8> {
    let mut b = vec![aberto as u8];
    b.extend_from_slice(&a.ip().octets());
    b.extend_from_slice(&a.port().to_be_bytes());
    match a6 {
        Some(a6) => {
            b.extend_from_slice(&a6.ip().octets());
            b.extend_from_slice(&a6.port().to_be_bytes());
            para_base31(&b, SIMBOLOS_V6)
        }
        None => para_base31(&b, SIMBOLOS_V4),
    }
}

fn enderecos(v: &[u8]) -> Result<(SocketAddrV4, Option<SocketAddrV6>, bool), CodeError> {
    let bytes = if v.len() == SIMBOLOS_V6 { 25 } else { 7 };
    // Base 31 tem folga: o que passa da marca (0 ou 1) não é endereço.
    let b = de_base31(v, bytes)
        .filter(|b| b[0] <= 1)
        .ok_or(CodeError::Check)?;
    let v4 = SocketAddrV4::new(
        Ipv4Addr::new(b[1], b[2], b[3], b[4]),
        u16::from_be_bytes([b[5], b[6]]),
    );
    let v6 = (bytes == 25).then(|| {
        let ip: [u8; 16] = b[7..23].try_into().expect("tamanho fixo");
        SocketAddrV6::new(Ipv6Addr::from(ip), u16::from_be_bytes([b[23], b[24]]), 0, 0)
    });
    Ok((v4, v6, b[0] == 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exemplo() -> ConnectionCode {
        ConnectionCode {
            pairing: PairingCode::parse("K7M4-QX9P").unwrap(),
            addr: "203.0.113.45:51234".parse().unwrap(),
            addr6: None,
            open: false,
        }
    }

    fn com_ipv6() -> ConnectionCode {
        ConnectionCode {
            addr6: Some("[2001:db8:4a7:2600:79bb:1b75:6722:b2b9]:40123".parse().unwrap()),
            open: true,
            ..exemplo()
        }
    }

    #[test]
    fn ida_e_volta() {
        let c = exemplo();
        let texto = c.to_string();
        assert_eq!(texto.len(), 19 + 4, "{texto}");
        assert!(texto.starts_with("K7M4-QX9P-"), "{texto}");
        assert_eq!(ConnectionCode::parse(&texto).unwrap(), c);
        assert_eq!(
            ConnectionCode::parse(&texto.to_lowercase().replace('-', " ")).unwrap(),
            c
        );
        let c6 = com_ipv6();
        let texto = c6.to_string();
        assert_eq!(texto.chars().filter(|&c| c != '-').count(), 48, "{texto}");
        assert_eq!(ConnectionCode::parse(&texto).unwrap(), c6);
        assert_eq!(c6.candidates().len(), 2);
        assert!(c6.candidates()[0].is_ipv6(), "IPv6 primeiro");

        for a in ["0.0.0.0:0", "255.255.255.255:65535", "192.168.1.27:9000"] {
            let r = ReplyCode {
                addr: a.parse().unwrap(),
                addr6: None,
            };
            let texto = r.to_string();
            assert_eq!(texto.len(), 11 + 2, "{texto}");
            assert_eq!(ReplyCode::parse(&texto).unwrap(), r);
        }
        let r = ReplyCode {
            addr: "0.0.0.0:0".parse().unwrap(),
            addr6: Some("[ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff]:65535".parse().unwrap()),
        };
        assert_eq!(ReplyCode::parse(&r.to_string()).unwrap(), r);
        assert_eq!(r.candidates().len(), 1, "IPv4 vazio não é candidato");
    }

    #[test]
    fn erros_de_digitacao_sao_pegos() {
        for codigo in [exemplo(), com_ipv6()] {
            let texto: Vec<char> = codigo.to_string().chars().filter(|&c| c != '-').collect();
            // Cada símbolo trocado por outro do alfabeto.
            for i in 0..texto.len() {
                for &outro in ALFABETO {
                    if outro as char == texto[i] {
                        continue;
                    }
                    let mut t = texto.clone();
                    t[i] = outro as char;
                    let t: String = t.into_iter().collect();
                    assert!(ConnectionCode::parse(&t).is_err(), "troca em {i}: {t}");
                }
            }
            // Vizinhos invertidos.
            for i in 0..texto.len() - 1 {
                if texto[i] == texto[i + 1] {
                    continue;
                }
                let mut t = texto.clone();
                t.swap(i, i + 1);
                let t: String = t.into_iter().collect();
                assert!(ConnectionCode::parse(&t).is_err(), "inversão em {i}: {t}");
            }
        }
    }

    #[test]
    fn tamanho_e_simbolos_invalidos() {
        assert_eq!(
            ReplyCode::parse("ABCD-EFGH-JK"),
            Err(CodeError::Length {
                got: 10,
                expected: 11
            })
        );
        assert_eq!(ReplyCode::parse("ABCD-EFGH-JK0"), Err(CodeError::Symbol('0')));
        // O código de conexão não serve como resposta, e vice-versa.
        assert!(ReplyCode::parse(&exemplo().to_string()).is_err());
        // Folga da base 31: um número grande demais não é endereço.
        let mut zs = vec![30u8; SIMBOLOS_V4];
        zs.push(conferencia(&zs));
        let texto: String = zs.iter().map(|&s| ALFABETO[s as usize] as char).collect();
        assert_eq!(ReplyCode::parse(&texto), Err(CodeError::Check));
    }

    #[test]
    fn debug_nao_mostra_o_pareamento() {
        assert!(!format!("{:?}", exemplo()).contains("K7M4"));
    }
}
