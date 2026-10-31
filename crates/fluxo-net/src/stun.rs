//! Endereço público pelo STUN: [`discover_public_addr`].
//!
//! Pergunta a servidores STUN públicos de onde o nosso socket aparece na
//! internet. Tem que ser **o mesmo socket** que depois leva o vídeo: o NAT
//! mapeia cada socket numa porta própria. Dois servidores dão de bônus um
//! diagnóstico: se cada um vê uma porta diferente, o NAT é simétrico (muda a
//! porta conforme o destino) e o furo direto provavelmente não passa.

use std::net::{SocketAddr, SocketAddrV4, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use fluxo_proto::stun::{binding_request, mapped_address};

use crate::NetError;

/// Servidores STUN públicos e gratuitos usados por padrão (sem conta; só
/// respondem o endereço de quem perguntou). Três, para comparar as portas
/// mesmo se um não responder.
pub const STUN_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun.cloudflare.com:3478",
    "stun1.l.google.com:19302",
];

/// Quanto esperar pelas respostas, ao todo.
const PRAZO: Duration = Duration::from_millis(1500);
/// Reenvio para quem ainda não respondeu (UDP pode perder o pedido).
const REENVIO: Duration = Duration::from_millis(300);

/// O que os servidores STUN viram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAddr {
    /// Nosso endereço visto da internet (o do primeiro servidor que respondeu).
    pub addr: SocketAddr,
    /// `Some(true)` se servidores diferentes viram portas diferentes (NAT
    /// simétrico); `None` se só um respondeu.
    pub symmetric: Option<bool>,
    /// Servidor que respondeu primeiro (para manter o mapeamento do NAT vivo
    /// com novos pedidos: [`ReceiverConfig::keepalive_to`](crate::ReceiverConfig::keepalive_to)).
    pub server: SocketAddr,
    /// Quantos servidores responderam.
    pub answered: usize,
}

impl PublicAddr {
    /// O endereço como vai nos códigos (só IPv4 por enquanto).
    pub fn v4(&self) -> Result<SocketAddrV4, NetError> {
        match self.addr {
            SocketAddr::V4(a) => Ok(a),
            SocketAddr::V6(_) => Err(NetError::Io {
                context: "STUN",
                message: "endereço público IPv6: os códigos ainda só levam IPv4".into(),
            }),
        }
    }

    /// Aviso para mostrar a quem conecta, se a rede atrapalha o furo.
    pub fn warning(&self) -> Option<&'static str> {
        match self.symmetric {
            Some(true) => Some(
                "aviso: NAT simétrico nesta rede (cada destino ganha uma porta nova); \
                 a conexão direta só passa se o NAT do outro lado for mais aberto",
            ),
            _ => None,
        }
    }

    /// Uma linha sobre o NAT, para diagnóstico.
    pub fn summary(&self) -> String {
        match self.symmetric {
            Some(true) => format!(
                "NAT simétrico (portas diferentes em {} servidores STUN)",
                self.answered
            ),
            Some(false) => format!(
                "mesma porta nos {} servidores STUN (bom para o furo)",
                self.answered
            ),
            None => "só 1 servidor STUN respondeu: não deu para comparar portas".to_string(),
        }
    }
}

/// Resposta de um servidor a [`probe_nat`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StunAnswer {
    /// Como foi pedido (`host:porta`).
    pub server: String,
    /// Endereço do servidor (`None`: o nome não resolveu).
    pub server_addr: Option<SocketAddr>,
    /// O que ele viu (`None`: não respondeu a tempo).
    pub mapped: Option<SocketAddr>,
}

/// Pergunta aos `servers` (`host:porta`; ver [`STUN_SERVERS`]) o endereço
/// público de `socket`, todos ao mesmo tempo, e devolve o que cada um viu.
/// Leva até ~1,5 s. Muda o timeout de leitura do socket. Comparar as portas
/// diz como o NAT se comporta (`fluxo-bench nat`).
pub fn probe_nat(socket: &UdpSocket, servers: &[&str]) -> Result<Vec<StunAnswer>, NetError> {
    let ipv6 = socket.local_addr().map(|a| a.is_ipv6()).unwrap_or(false);
    // Um pedido por servidor, cada um com seu identificador aleatório.
    let mut pedidos: Vec<(StunAnswer, [u8; 12])> = Vec::new();
    for s in servers {
        let server_addr = s
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.find(|a| a.is_ipv6() == ipv6));
        let mut id = [0u8; 12];
        fluxo_crypto::random_bytes(&mut id).map_err(|e| NetError::Io {
            context: "STUN",
            message: e.to_string(),
        })?;
        let resposta = StunAnswer {
            server: s.to_string(),
            server_addr,
            mapped: None,
        };
        pedidos.push((resposta, id));
    }
    if pedidos.iter().all(|(r, _)| r.server_addr.is_none()) {
        return Err(NetError::Io {
            context: "STUN",
            message: "nenhum servidor STUN resolveu (sem internet ou sem DNS?)".into(),
        });
    }
    socket
        .set_read_timeout(Some(Duration::from_millis(50)))
        .map_err(NetError::io("configurar socket"))?;
    let falta = |p: &[(StunAnswer, [u8; 12])]| {
        p.iter()
            .any(|(r, _)| r.server_addr.is_some() && r.mapped.is_none())
    };
    let limite = Instant::now() + PRAZO;
    let mut proximo_envio = Instant::now();
    let mut buf = [0u8; 1500];
    while Instant::now() < limite && falta(&pedidos) {
        if Instant::now() >= proximo_envio {
            for (r, id) in &pedidos {
                if let (Some(servidor), None) = (r.server_addr, r.mapped) {
                    let _ = socket.send_to(&binding_request(id), servidor);
                }
            }
            proximo_envio = Instant::now() + REENVIO;
        }
        let Ok((n, de)) = socket.recv_from(&mut buf) else {
            continue;
        };
        for (r, id) in &mut pedidos {
            if r.server_addr == Some(de) && r.mapped.is_none() {
                r.mapped = mapped_address(&buf[..n], id);
            }
        }
    }
    Ok(pedidos.into_iter().map(|(r, _)| r).collect())
}

/// Pergunta aos `servers` (`host:porta`; ver [`STUN_SERVERS`]) o endereço
/// público de `socket`. Leva até ~1,5 s. Muda o timeout de leitura do socket.
pub fn discover_public_addr(socket: &UdpSocket, servers: &[&str]) -> Result<PublicAddr, NetError> {
    let vistos: Vec<(SocketAddr, SocketAddr)> = probe_nat(socket, servers)?
        .into_iter()
        .filter_map(|r| Some((r.server_addr?, r.mapped?)))
        .collect();
    let Some(&(server, addr)) = vistos.first() else {
        return Err(NetError::Io {
            context: "STUN",
            message: "nenhum servidor STUN respondeu (sem internet, ou UDP bloqueado?)".into(),
        });
    };
    let symmetric = (vistos.len() > 1).then(|| vistos.iter().any(|(_, a)| *a != addr));
    Ok(PublicAddr {
        addr,
        symmetric,
        server,
        answered: vistos.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Servidor STUN de mentira em 127.0.0.1 que responde `visto` (ou o
    /// endereço real de quem perguntou, com `None`).
    fn servidor_falso(visto: Option<SocketAddr>) -> (String, std::thread::JoinHandle<()>) {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let endereco = s.local_addr().unwrap().to_string();
        let t = std::thread::spawn(move || {
            let mut buf = [0u8; 100];
            s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            if let Ok((n, de)) = s.recv_from(&mut buf) {
                if n != 20 {
                    return;
                }
                let a = visto.unwrap_or(de);
                let ip = match a.ip() {
                    std::net::IpAddr::V4(ip) => ip.octets(),
                    _ => unreachable!(),
                };
                let mut r = vec![0x01, 0x01, 0x00, 0x0c, 0x21, 0x12, 0xa4, 0x42];
                r.extend_from_slice(&buf[8..20]);
                r.extend_from_slice(&[0x00, 0x01, 0x00, 0x08, 0x00, 0x01]);
                r.extend_from_slice(&a.port().to_be_bytes());
                r.extend_from_slice(&ip);
                let _ = s.send_to(&r, de);
            }
        });
        (endereco, t)
    }

    #[test]
    fn descobre_o_endereco_e_detecta_nat_simetrico() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let (a, ta) = servidor_falso(None);
        let (b, tb) = servidor_falso(None);
        let r = discover_public_addr(&socket, &[&a, &b]).unwrap();
        assert_eq!(r.addr, socket.local_addr().unwrap());
        assert_eq!(r.symmetric, Some(false));
        ta.join().unwrap();
        tb.join().unwrap();

        // Cada servidor vê uma porta: NAT simétrico.
        let (a, ta) = servidor_falso(Some("203.0.113.5:40000".parse().unwrap()));
        let (b, tb) = servidor_falso(Some("203.0.113.5:40001".parse().unwrap()));
        let r = discover_public_addr(&socket, &[&a, &b]).unwrap();
        assert_eq!(r.symmetric, Some(true));
        ta.join().unwrap();
        tb.join().unwrap();
    }

    #[test]
    fn sem_resposta_da_erro_claro() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mudo = UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .to_string();
        let erro = discover_public_addr(&socket, &[&mudo]).unwrap_err();
        assert!(
            erro.to_string().contains("nenhum servidor STUN respondeu"),
            "{erro}"
        );
    }
}
