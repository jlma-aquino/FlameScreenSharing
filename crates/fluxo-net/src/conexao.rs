//! Códigos para conectar sem servidor: [`ConnectionCode`] e [`ReplyCode`].
//!
//! Sem servidor de sinalização, quem leva os endereços de um lado ao outro
//! são as pessoas (pelo chat que já usam). O cliente mostra o **código de
//! conexão** (código de pareamento + seu endereço público); o host digita e
//! mostra o **código de resposta** (o endereço público dele); o cliente
//! digita esse, e os dois furam os NATs um na direção do outro.
//!
//! ```text
//! código de conexão: K7M4-QX9P-ABCD-EFGH-JK   8 do pareamento + 10 do endereço + 1 de conferência
//! código de resposta: ABCD-EFGH-JKM            10 do endereço + 1 de conferência
//! ```
//!
//! Mesmo alfabeto do código de pareamento (31 símbolos, sem 0/O, 1/I/L). O
//! endereço (IPv4 + porta, 48 bits) vira 10 símbolos; o último símbolo é uma
//! soma ponderada módulo 31 de todos os anteriores, que pega qualquer
//! símbolo trocado e qualquer par de vizinhos invertido.

use std::fmt;
use std::net::{Ipv4Addr, SocketAddrV4};

use fluxo_crypto::{PairingCode, ALFABETO};

const BASE: u64 = 31;
const SIMBOLOS_PAREAMENTO: usize = 8;
const SIMBOLOS_ENDERECO: usize = 10;

/// Código que o cliente mostra: pareamento + endereço público do cliente.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionCode {
    pub pairing: PairingCode,
    pub addr: SocketAddrV4,
}

/// Código que o host mostra de volta: o endereço público do host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplyCode {
    pub addr: SocketAddrV4,
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
        let v = simbolos(texto, SIMBOLOS_PAREAMENTO + SIMBOLOS_ENDERECO + 1)?;
        let pareamento: String = v[..SIMBOLOS_PAREAMENTO]
            .iter()
            .map(|&s| ALFABETO[s as usize] as char)
            .collect();
        Ok(Self {
            pairing: PairingCode::parse(&pareamento).map_err(|_| CodeError::Check)?,
            addr: endereco(&v[SIMBOLOS_PAREAMENTO..SIMBOLOS_PAREAMENTO + SIMBOLOS_ENDERECO])?,
        })
    }
}

impl ReplyCode {
    /// Lê o que foi digitado: ignora hífens e espaços, aceita minúsculas.
    pub fn parse(texto: &str) -> Result<Self, CodeError> {
        let v = simbolos(texto, SIMBOLOS_ENDERECO + 1)?;
        Ok(Self {
            addr: endereco(&v[..SIMBOLOS_ENDERECO])?,
        })
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
        v.extend(simbolos_do_endereco(self.addr));
        escrever(f, v)
    }
}

impl fmt::Display for ReplyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        escrever(f, simbolos_do_endereco(self.addr))
    }
}

/// Não mostra o código de pareamento em logs de depuração.
impl fmt::Debug for ConnectionCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ConnectionCode(****-****, {})", self.addr)
    }
}

/// Soma ponderada módulo 31: com base prima, pega qualquer símbolo trocado
/// e qualquer par de vizinhos invertido.
fn conferencia(v: &[u8]) -> u8 {
    (v.iter()
        .enumerate()
        .map(|(i, &s)| (i as u64 + 1) * s as u64)
        .sum::<u64>()
        % BASE) as u8
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

/// Símbolos (0..31) do que foi digitado, com a conferência verificada e removida.
fn simbolos(texto: &str, esperado: usize) -> Result<Vec<u8>, CodeError> {
    let mut v = Vec::with_capacity(esperado);
    for c in texto.chars().filter(|c| !matches!(c, '-' | ' ')) {
        let maiuscula = c.to_ascii_uppercase();
        let s = ALFABETO
            .iter()
            .position(|&a| a as char == maiuscula)
            .ok_or(CodeError::Symbol(c))?;
        v.push(s as u8);
    }
    if v.len() != esperado {
        return Err(CodeError::Length {
            got: v.len(),
            expected: esperado,
        });
    }
    let conf = v.pop().expect("tamanho conferido");
    if conferencia(&v) != conf {
        return Err(CodeError::Check);
    }
    Ok(v)
}

fn simbolos_do_endereco(a: SocketAddrV4) -> Vec<u8> {
    let mut n = (u32::from(*a.ip()) as u64) << 16 | a.port() as u64;
    let mut v = vec![0u8; SIMBOLOS_ENDERECO];
    for s in v.iter_mut().rev() {
        *s = (n % BASE) as u8;
        n /= BASE;
    }
    v
}

fn endereco(v: &[u8]) -> Result<SocketAddrV4, CodeError> {
    let n = v.iter().fold(0u64, |n, &s| n * BASE + s as u64);
    // 31¹⁰ passa um pouco de 2⁴⁸: o que sobra não é endereço.
    if n >> 48 != 0 {
        return Err(CodeError::Check);
    }
    Ok(SocketAddrV4::new(Ipv4Addr::from((n >> 16) as u32), n as u16))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exemplo() -> ConnectionCode {
        ConnectionCode {
            pairing: PairingCode::parse("K7M4-QX9P").unwrap(),
            addr: "203.0.113.45:51234".parse().unwrap(),
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

        for a in ["0.0.0.0:0", "255.255.255.255:65535", "192.168.1.27:9000"] {
            let r = ReplyCode {
                addr: a.parse().unwrap(),
            };
            let texto = r.to_string();
            assert_eq!(texto.len(), 11 + 2, "{texto}");
            assert_eq!(ReplyCode::parse(&texto).unwrap(), r);
        }
    }

    #[test]
    fn erros_de_digitacao_sao_pegos() {
        let texto: Vec<char> = exemplo().to_string().chars().filter(|&c| c != '-').collect();
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
    }

    #[test]
    fn debug_nao_mostra_o_pareamento() {
        assert!(!format!("{:?}", exemplo()).contains("K7M4"));
    }
}
