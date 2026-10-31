//! Tudo o que uma ponta prepara para conectar sem servidor: [`Internet`].
//!
//! Abre o socket IPv4 (e um IPv6, se a máquina tiver), pergunta aos
//! servidores STUN os endereços públicos de cada um, tenta abrir a porta
//! IPv4 no roteador por UPnP e monta o código de conexão (cliente) ou de
//! resposta (host). Os sockets vão depois para
//! [`VideoReceiver::with_sockets`](crate::VideoReceiver::with_sockets) ou
//! [`VideoSender::connect_with_sockets`](crate::VideoSender::connect_with_sockets):
//! os endereços só valem para eles.

use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6, UdpSocket};

use fluxo_crypto::PairingCode;

use crate::{
    discover_public_addr, open_port, ConnectionCode, NetError, PortMapping, PublicAddr, ReplyCode,
    UpnpOutcome, STUN_SERVERS,
};

/// Sockets e endereços públicos de uma ponta.
#[derive(Debug)]
pub struct Internet {
    pub v4: UdpSocket,
    /// Socket IPv6, se a máquina tem IPv6 que chega à internet.
    pub v6: Option<UdpSocket>,
    /// O que o STUN viu pelo IPv4.
    pub public: PublicAddr,
    /// Endereço IPv6 público (sem NAT, costuma ser o próprio da máquina).
    pub public6: Option<SocketAddrV6>,
    /// Porta IPv4 aberta no roteador por UPnP (fechada ao destruir).
    pub mapping: Option<PortMapping>,
    /// Uma linha sobre o UPnP, para mostrar.
    pub upnp: String,
}

impl Internet {
    /// Abre os sockets (`porta` no IPv4; 0 = automática) e descobre os
    /// endereços. `upnp`: tenta abrir a porta no roteador (só quem recebe a
    /// conexão precisa). Leva de 1,5 a 3 s.
    pub fn open(porta: u16, upnp: bool) -> Result<Self, NetError> {
        let v4 = UdpSocket::bind(("0.0.0.0", porta)).map_err(NetError::io("abrir socket UDP"))?;
        let public = discover_public_addr(&v4, STUN_SERVERS)?;
        // IPv6 é bônus: sem ele (ou sem resposta do STUN por ele), segue só IPv4.
        let (v6, public6) = match UdpSocket::bind("[::]:0") {
            Ok(s) => match discover_public_addr(&s, STUN_SERVERS).map(|p| p.addr) {
                Ok(SocketAddr::V6(a)) => (Some(s), Some(a)),
                _ => (None, None),
            },
            Err(_) => (None, None),
        };
        let (mapping, upnp) = match (upnp, public.v4()) {
            (false, _) => (None, "UPnP desligado".to_string()),
            (true, Err(_)) => (None, "UPnP: sem IPv4 público".to_string()),
            (true, Ok(publico)) => {
                let local = SocketAddrV4::new(
                    match crate::local_ip() {
                        Some(std::net::IpAddr::V4(ip)) => ip,
                        _ => std::net::Ipv4Addr::UNSPECIFIED,
                    },
                    v4.local_addr().map_or(porta, |a| a.port()),
                );
                match open_port(local, publico) {
                    Ok(UpnpOutcome::Opened(m)) => {
                        let linha = format!("porta {} aberta no roteador por UPnP", m.public);
                        (Some(m), linha)
                    }
                    Ok(UpnpOutcome::BehindNat { external }) => (
                        None,
                        format!("roteador com UPnP, mas atrás de outro NAT (IP externo dele: {external})"),
                    ),
                    Ok(UpnpOutcome::NoGateway) => (None, "sem UPnP na rede local".to_string()),
                    Err(e) => (None, format!("UPnP falhou: {e}")),
                }
            }
        };
        Ok(Self {
            v4,
            v6,
            public,
            public6,
            mapping,
            upnp,
        })
    }

    /// Endereço IPv4 que vai nos códigos: o da porta aberta por UPnP, se
    /// houver; senão o que o STUN viu.
    fn v4_publico(&self) -> Result<SocketAddrV4, NetError> {
        match &self.mapping {
            Some(m) => Ok(m.public),
            None => self.public.v4(),
        }
    }

    /// Código de conexão (lado de quem assiste).
    pub fn connection_code(&self, pairing: PairingCode) -> Result<ConnectionCode, NetError> {
        Ok(ConnectionCode {
            pairing,
            addr: self.v4_publico()?,
            addr6: self.public6,
            open: self.mapping.is_some(),
        })
    }

    /// Código de resposta (lado de quem compartilha) para quem mandou
    /// `conexao`. Só leva o IPv6 se o outro lado também tem: sem ele, o
    /// endereço não serve e o código fica curto (11 símbolos em vez de 40).
    pub fn reply_code(&self, conexao: &ConnectionCode) -> Result<ReplyCode, NetError> {
        Ok(ReplyCode {
            addr: self.v4_publico()?,
            addr6: self.public6.filter(|_| conexao.addr6.is_some()),
        })
    }

    /// Linhas para mostrar: NAT, IPv6, UPnP e o aviso de NAT simétrico.
    pub fn summary(&self) -> Vec<String> {
        let mut linhas = vec![format!("NAT: {}", self.public.summary())];
        linhas.push(match self.public6 {
            Some(a) => format!("IPv6: {} (sem NAT; costuma passar onde o IPv4 trava)", a.ip()),
            None => "IPv6: não disponível nesta rede".to_string(),
        });
        linhas.push(self.upnp.clone());
        if let Some(aviso) = self.public.warning() {
            linhas.push(aviso.to_string());
        }
        linhas
    }
}
