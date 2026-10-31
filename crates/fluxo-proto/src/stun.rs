//! STUN mínimo (RFC 5389): só o *Binding Request* e a leitura do endereço
//! na resposta.
//!
//! Um servidor STUN público (Google, Cloudflare) responde "de onde este
//! pacote veio": o endereço público do nosso socket, como o NAT o mapeou.
//! É o que vai no código de conexão, sem servidor nosso. Não faz parte do
//! protocolo do Fluxo: é outro formato, no mesmo socket.
//!
//! ```text
//! Binding Request (20 bytes):  tipo 0x0001 | tamanho 0 | cookie 0x2112A442 | id (12 bytes)
//! Resposta:                    tipo 0x0101 | tamanho | cookie | id | atributos...
//!   XOR-MAPPED-ADDRESS (0x0020): família, porta ^ cookie, IP ^ cookie (^ id no IPv6)
//!   MAPPED-ADDRESS (0x0001):     o mesmo sem XOR (servidores antigos)
//! ```

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// Valor fixo que identifica STUN moderno (RFC 5389).
pub const MAGIC_COOKIE: u32 = 0x2112_A442;
/// Tamanho do pedido: só o cabeçalho.
pub const REQUEST_LEN: usize = 20;

const BINDING_REQUEST: u16 = 0x0001;
const BINDING_RESPONSE: u16 = 0x0101;
const MAPPED_ADDRESS: u16 = 0x0001;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;

/// Monta um *Binding Request* com este identificador de transação.
pub fn binding_request(id: &[u8; 12]) -> [u8; REQUEST_LEN] {
    let mut p = [0u8; REQUEST_LEN];
    p[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    // Tamanho dos atributos: 0.
    p[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    p[8..20].copy_from_slice(id);
    p
}

/// Lê o endereço público de uma resposta ao pedido `id`. `None` se não é
/// STUN, não é a resposta desse pedido ou não traz endereço.
pub fn mapped_address(resposta: &[u8], id: &[u8; 12]) -> Option<SocketAddr> {
    if resposta.len() < 20
        || u16::from_be_bytes([resposta[0], resposta[1]]) != BINDING_RESPONSE
        || resposta[4..8] != MAGIC_COOKIE.to_be_bytes()
        || &resposta[8..20] != id
    {
        return None;
    }
    let tamanho = u16::from_be_bytes([resposta[2], resposta[3]]) as usize;
    let mut attrs = resposta.get(20..20 + tamanho)?;
    let mut sem_xor = None;
    while attrs.len() >= 4 {
        let tipo = u16::from_be_bytes([attrs[0], attrs[1]]);
        let len = u16::from_be_bytes([attrs[2], attrs[3]]) as usize;
        let valor = attrs.get(4..4 + len)?;
        match tipo {
            XOR_MAPPED_ADDRESS => return endereco(valor, Some(id)),
            MAPPED_ADDRESS => sem_xor = endereco(valor, None),
            _ => {}
        }
        // Atributos ocupam múltiplos de 4 bytes.
        let pulo = (4 + len + 3) & !3;
        attrs = attrs.get(pulo..).unwrap_or(&[]);
    }
    sem_xor
}

/// Lê um atributo de endereço; com `xor`, desfaz o XOR com o cookie (e o id).
fn endereco(v: &[u8], xor: Option<&[u8; 12]>) -> Option<SocketAddr> {
    let cookie = MAGIC_COOKIE.to_be_bytes();
    let mut porta = u16::from_be_bytes([*v.get(2)?, *v.get(3)?]);
    if xor.is_some() {
        porta ^= (MAGIC_COOKIE >> 16) as u16;
    }
    let ip = match v.get(1)? {
        0x01 => {
            let mut b: [u8; 4] = v.get(4..8)?.try_into().ok()?;
            if xor.is_some() {
                for (x, c) in b.iter_mut().zip(cookie) {
                    *x ^= c;
                }
            }
            IpAddr::V4(Ipv4Addr::from(b))
        }
        0x02 => {
            let mut b: [u8; 16] = v.get(4..20)?.try_into().ok()?;
            if let Some(id) = xor {
                let chave = cookie.iter().chain(id.iter());
                for (x, c) in b.iter_mut().zip(chave) {
                    *x ^= c;
                }
            }
            IpAddr::V6(Ipv6Addr::from(b))
        }
        _ => return None,
    };
    Some(SocketAddr::new(ip, porta))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resposta de exemplo da RFC 5769 (seção 2.2), com XOR-MAPPED-ADDRESS
    /// 192.0.2.1:32853. Os atributos de software e integridade foram tirados
    /// (não os lemos) e o tamanho ajustado.
    #[test]
    fn le_o_exemplo_da_rfc_5769() {
        let id = [
            0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6, 0x86, 0xfa, 0x87, 0xdf, 0xae,
        ];
        let mut r = vec![0x01, 0x01, 0x00, 0x0c, 0x21, 0x12, 0xa4, 0x42];
        r.extend_from_slice(&id);
        r.extend_from_slice(&[
            0x00, 0x20, 0x00, 0x08, 0x00, 0x01, 0xa1, 0x47, 0xe1, 0x12, 0xa6, 0x43,
        ]);
        assert_eq!(mapped_address(&r, &id), Some("192.0.2.1:32853".parse().unwrap()));
        // Outro pedido, ou um pacote que não é resposta: nada.
        assert_eq!(mapped_address(&r, &[0; 12]), None);
        assert_eq!(mapped_address(&binding_request(&id), &id), None);
    }

    #[test]
    fn pedido_tem_o_formato_certo() {
        let p = binding_request(&[7; 12]);
        assert_eq!(&p[..8], &[0x00, 0x01, 0x00, 0x00, 0x21, 0x12, 0xa4, 0x42]);
        assert_eq!(&p[8..], &[7; 12]);
    }

    #[test]
    fn aceita_mapped_address_sem_xor_e_ignora_lixo() {
        let id = [1; 12];
        let mut r = vec![0x01, 0x01, 0x00, 0x14, 0x21, 0x12, 0xa4, 0x42];
        r.extend_from_slice(&id);
        // Atributo desconhecido de 5 bytes (ocupa 8) e depois MAPPED-ADDRESS.
        r.extend_from_slice(&[0x80, 0x22, 0x00, 0x05, b'a', b'b', b'c', b'd', b'e', 0, 0, 0]);
        r.extend_from_slice(&[0x00, 0x01, 0x00, 0x08, 0x00, 0x01, 0x1f, 0x90, 203, 0, 113, 9]);
        let tamanho = (r.len() - 20) as u16;
        r[2..4].copy_from_slice(&tamanho.to_be_bytes());
        assert_eq!(mapped_address(&r, &id), Some("203.0.113.9:8080".parse().unwrap()));
        assert_eq!(mapped_address(&r[..25], &id), None, "cortado no meio");
        assert_eq!(mapped_address(b"lixo", &id), None);
    }
}
