//! Porta aberta no roteador por UPnP: [`open_port`].
//!
//! Se o roteador de quem assiste aceita UPnP (IGD) **e** tem IP público, uma
//! porta aberta nele deixa o host chegar direto, sem furo: basta o código de
//! conexão. Atrás de CGNAT (o IP externo do roteador é privado, ex.
//! `100.64.x.x`, ou diferente do que o STUN viu) a porta não serve para nada
//! e nem é pedida. Só biblioteca padrão: a descoberta é um pacote SSDP
//! (multicast na rede local) e os comandos são SOAP por HTTP simples.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use crate::NetError;

/// Quanto esperar o roteador responder à descoberta.
const PRAZO_DESCOBERTA: Duration = Duration::from_millis(1500);
/// Prazo de cada pedido HTTP ao roteador.
const PRAZO_HTTP: Duration = Duration::from_secs(3);
/// Validade da porta aberta. Enquanto o [`PortMapping`] existe, ela é
/// renovada na metade do prazo; o Fluxo apaga ao sair, e se ele cair (ou faltar
/// luz), o roteador apaga sozinho quando o prazo acabar.
pub const UPNP_LEASE: Duration = Duration::from_secs(3600);

fn erro(message: impl Into<String>) -> NetError {
    NetError::Io {
        context: "UPnP",
        message: message.into(),
    }
}

/// Roteador com UPnP (o serviço WANIPConnection ou WANPPPConnection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gateway {
    /// Endereço do servidor HTTP do roteador.
    addr: SocketAddr,
    /// Caminho de controle do serviço.
    control: String,
    /// Tipo do serviço (vai no SOAPAction).
    service: String,
}

/// O que o UPnP conseguiu ([`open_port`]).
#[derive(Debug)]
pub enum UpnpOutcome {
    /// Nenhum roteador UPnP respondeu na rede local.
    NoGateway,
    /// O roteador tem UPnP, mas está atrás de outro NAT (CGNAT): `external`
    /// é o IP que ele próprio vê, que não é o da internet.
    BehindNat { external: Ipv4Addr },
    /// Porta aberta: o host chega em `public` direto.
    Opened(PortMapping),
}

/// Porta aberta no roteador; renovada enquanto existe, fechada ao destruir.
#[derive(Debug)]
pub struct PortMapping {
    gateway: Gateway,
    /// Endereço público que leva a este PC.
    pub public: SocketAddrV4,
    /// Para a thread que renova.
    parar: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl PortMapping {
    fn novo(gateway: Gateway, public: SocketAddrV4, local: SocketAddrV4) -> Self {
        use std::sync::atomic::Ordering;
        let parar = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (g, p) = (gateway.clone(), parar.clone());
        let _ = std::thread::Builder::new()
            .name("fluxo-upnp".into())
            .spawn(move || {
                let mut desde = std::time::Instant::now();
                while !p.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(500));
                    if desde.elapsed() >= UPNP_LEASE / 2 {
                        // Falhou? Tenta de novo na próxima metade; a conexão já aberta segue.
                        let _ = g.add_udp_mapping(public.port(), local, UPNP_LEASE);
                        desde = std::time::Instant::now();
                    }
                }
            });
        Self {
            gateway,
            public,
            parar,
        }
    }
}

impl Drop for PortMapping {
    fn drop(&mut self) {
        self.parar.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = self.gateway.remove_udp_mapping(self.public.port());
    }
}

/// Procura um roteador UPnP na rede local.
pub fn find_gateway() -> Result<Option<Gateway>, NetError> {
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| erro(e.to_string()))?;
    let pedido = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\n\
                  ST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\r\n";
    socket
        .send_to(pedido.as_bytes(), "239.255.255.250:1900")
        .map_err(|e| erro(e.to_string()))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|e| erro(e.to_string()))?;
    let limite = Instant::now() + PRAZO_DESCOBERTA;
    let mut buf = [0u8; 2048];
    while Instant::now() < limite {
        let Ok((n, _)) = socket.recv_from(&mut buf) else {
            continue;
        };
        let resposta = String::from_utf8_lossy(&buf[..n]);
        let Some(local) = cabecalho(&resposta, "location") else {
            continue;
        };
        // Um roteador que responde mas não serve (descrição estranha): tenta
        // o próximo que responder.
        if let Ok(Some(g)) = gateway_de(local) {
            return Ok(Some(g));
        }
    }
    Ok(None)
}

/// Lê a descrição do roteador e acha o serviço de conexão WAN.
fn gateway_de(location: &str) -> Result<Option<Gateway>, NetError> {
    let (addr, caminho) = url(location).ok_or_else(|| erro(format!("endereço estranho: {location}")))?;
    let xml = http(
        addr,
        &format!("GET {caminho} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
    )?;
    Ok(servico_wan(&xml).map(|(service, control)| {
        // O caminho de controle pode vir completo ou relativo ao roteador.
        let (addr, control) = match url(&control) {
            Some((a, c)) => (a, c),
            None if control.starts_with('/') => (addr, control),
            None => (addr, format!("/{control}")),
        };
        Gateway {
            addr,
            control,
            service,
        }
    }))
}

impl Gateway {
    /// IP externo que o roteador vê (o da internet, se não houver outro NAT).
    pub fn external_ip(&self) -> Result<Ipv4Addr, NetError> {
        let r = self.soap("GetExternalIPAddress", "")?;
        tag(&r, "NewExternalIPAddress")
            .and_then(|ip| ip.trim().parse().ok())
            .ok_or_else(|| erro("o roteador não informou o IP externo"))
    }

    /// Abre `externa` (UDP) no roteador, levando a `interno`.
    pub fn add_udp_mapping(
        &self,
        externa: u16,
        interno: SocketAddrV4,
        validade: Duration,
    ) -> Result<(), NetError> {
        let args = format!(
            "<NewRemoteHost></NewRemoteHost><NewExternalPort>{externa}</NewExternalPort>\
             <NewProtocol>UDP</NewProtocol><NewInternalPort>{}</NewInternalPort>\
             <NewInternalClient>{}</NewInternalClient><NewEnabled>1</NewEnabled>\
             <NewPortMappingDescription>FlameScreenSharing</NewPortMappingDescription>\
             <NewLeaseDuration>{}</NewLeaseDuration>",
            interno.port(),
            interno.ip(),
            validade.as_secs()
        );
        self.soap("AddPortMapping", &args).map(|_| ())
    }

    /// Fecha `externa` (UDP).
    pub fn remove_udp_mapping(&self, externa: u16) -> Result<(), NetError> {
        let args = format!(
            "<NewRemoteHost></NewRemoteHost><NewExternalPort>{externa}</NewExternalPort><NewProtocol>UDP</NewProtocol>"
        );
        self.soap("DeletePortMapping", &args).map(|_| ())
    }

    fn soap(&self, acao: &str, args: &str) -> Result<String, NetError> {
        let corpo = format!(
            "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
             s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body>\
             <u:{acao} xmlns:u=\"{}\">{args}</u:{acao}></s:Body></s:Envelope>",
            self.service
        );
        let pedido = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: text/xml; charset=\"utf-8\"\r\n\
             SOAPAction: \"{}#{acao}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{corpo}",
            self.control,
            self.addr,
            self.service,
            corpo.len()
        );
        let resposta = http(self.addr, &pedido)?;
        if resposta.contains("Fault>") {
            let motivo = tag(&resposta, "errorDescription").unwrap_or("recusado");
            return Err(erro(format!("o roteador recusou {acao}: {motivo}")));
        }
        Ok(resposta)
    }
}

/// Tenta abrir no roteador a porta do socket `local`, para o host chegar
/// direto. `stun` é o endereço público visto pelo STUN: só abre se o IP
/// externo do roteador for esse (sem CGNAT na frente). Usa a mesma porta
/// externa que o STUN viu, se o roteador aceitar; senão, a porta local.
pub fn open_port(local: SocketAddrV4, stun: SocketAddrV4) -> Result<UpnpOutcome, NetError> {
    let Some(g) = find_gateway()? else {
        return Ok(UpnpOutcome::NoGateway);
    };
    let externo = g.external_ip()?;
    if externo != *stun.ip() {
        return Ok(UpnpOutcome::BehindNat { external: externo });
    }
    let mut ultimo_erro = None;
    for porta in [stun.port(), local.port()] {
        match g.add_udp_mapping(porta, local, UPNP_LEASE) {
            Ok(()) => {
                return Ok(UpnpOutcome::Opened(PortMapping::novo(
                    g,
                    SocketAddrV4::new(externo, porta),
                    local,
                )))
            }
            Err(e) => ultimo_erro = Some(e),
        }
    }
    Err(ultimo_erro.unwrap_or_else(|| erro("o roteador recusou abrir a porta")))
}

/// Um pedido HTTP/1.1 simples ao roteador; devolve o corpo da resposta.
fn http(addr: SocketAddr, pedido: &str) -> Result<String, NetError> {
    let falha = |e: std::io::Error| erro(format!("{addr}: {e}"));
    let mut s = TcpStream::connect_timeout(&addr, PRAZO_HTTP).map_err(falha)?;
    s.set_read_timeout(Some(PRAZO_HTTP)).map_err(falha)?;
    s.write_all(pedido.as_bytes()).map_err(falha)?;
    let mut bruto = Vec::new();
    // Alguns roteadores não fecham a conexão: o que chegou até o prazo vale.
    let _ = s.read_to_end(&mut bruto);
    let texto = String::from_utf8_lossy(&bruto).into_owned();
    let (cabecalhos, corpo) = texto
        .split_once("\r\n\r\n")
        .ok_or_else(|| erro(format!("{addr}: resposta HTTP incompleta")))?;
    let pedacos =
        cabecalho(cabecalhos, "transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked"));
    Ok(if pedacos {
        juntar_pedacos(corpo)
    } else {
        corpo.to_string()
    })
}

/// Desfaz `Transfer-Encoding: chunked`.
fn juntar_pedacos(mut s: &str) -> String {
    let mut corpo = String::new();
    while let Some((tamanho, resto)) = s.split_once("\r\n") {
        let Ok(n) = usize::from_str_radix(tamanho.split(';').next().unwrap_or("").trim(), 16) else {
            break;
        };
        if n == 0 || resto.len() < n {
            break;
        }
        corpo.push_str(&resto[..n]);
        s = resto[n..].trim_start_matches("\r\n");
    }
    corpo
}

/// Valor de um cabeçalho (nome sem diferenciar maiúsculas).
fn cabecalho<'a>(texto: &'a str, nome: &str) -> Option<&'a str> {
    texto.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(nome).then(|| v.trim())
    })
}

/// `http://host:porta/caminho` → (endereço, caminho).
fn url(u: &str) -> Option<(SocketAddr, String)> {
    let resto = u.strip_prefix("http://")?;
    let (host, caminho) = match resto.find('/') {
        Some(i) => (&resto[..i], resto[i..].to_string()),
        None => (resto, "/".to_string()),
    };
    let host = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:80")
    };
    let addr = host.to_socket_addrs().ok()?.find(|a| a.is_ipv4())?;
    Some((addr, caminho))
}

/// Conteúdo da primeira `<nome>...</nome>` (ignora prefixos de namespace).
fn tag<'a>(xml: &'a str, nome: &str) -> Option<&'a str> {
    let abre = xml.find(&format!("{nome}>"))? + nome.len() + 1;
    let fim = xml[abre..].find("</")?;
    Some(&xml[abre..abre + fim])
}

/// Primeiro serviço WANIPConnection/WANPPPConnection: (tipo, controlURL).
fn servico_wan(xml: &str) -> Option<(String, String)> {
    xml.split("<service>").skip(1).find_map(|bloco| {
        let tipo = tag(bloco, "serviceType")?.trim();
        let wan = tipo.contains(":WANIPConnection:") || tipo.contains(":WANPPPConnection:");
        wan.then(|| Some((tipo.to_string(), tag(bloco, "controlURL")?.trim().to_string())))?
    })
}

/// IPv4 que não é da internet: privado, CGNAT (100.64/10), local.
pub fn is_private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || (a == 100 && (64..128).contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESCRICAO: &str = r#"<?xml version="1.0"?><root><device><serviceList>
        <service><serviceType>urn:schemas-upnp-org:service:Layer3Forwarding:1</serviceType>
        <controlURL>/upnp/control/L3F</controlURL></service></serviceList>
        <deviceList><device><serviceList>
        <service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
        <serviceId>urn:upnp-org:serviceId:WANIPConn1</serviceId>
        <controlURL>/upnp/control/WANIPConn1</controlURL></service>
        </serviceList></device></deviceList></device></root>"#;

    #[test]
    fn acha_o_servico_wan_na_descricao() {
        assert_eq!(
            servico_wan(DESCRICAO),
            Some((
                "urn:schemas-upnp-org:service:WANIPConnection:1".into(),
                "/upnp/control/WANIPConn1".into()
            ))
        );
        assert_eq!(servico_wan("<root></root>"), None);
    }

    #[test]
    fn le_respostas_do_roteador() {
        let ssdp = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nLocation: http://192.168.1.1:49152/gatedesc.xml\r\n\r\n";
        assert_eq!(
            cabecalho(ssdp, "LOCATION"),
            Some("http://192.168.1.1:49152/gatedesc.xml")
        );
        let (addr, caminho) = url("http://192.168.1.1:49152/gatedesc.xml").unwrap();
        assert_eq!(
            (addr.to_string(), caminho.as_str()),
            ("192.168.1.1:49152".into(), "/gatedesc.xml")
        );
        let soap = "<s:Envelope><s:Body><u:GetExternalIPAddressResponse>\
                    <NewExternalIPAddress>100.66.54.143</NewExternalIPAddress></u:GetExternalIPAddressResponse></s:Body></s:Envelope>";
        assert_eq!(tag(soap, "NewExternalIPAddress"), Some("100.66.54.143"));
        assert_eq!(
            juntar_pedacos("5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"),
            "hello world"
        );
    }

    #[test]
    fn ips_que_nao_sao_da_internet() {
        for ip in [
            "100.66.54.143",
            "10.1.2.3",
            "192.168.1.1",
            "172.16.0.1",
            "127.0.0.1",
        ] {
            assert!(is_private_v4(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["177.73.1.2", "100.128.0.1", "8.8.8.8"] {
            assert!(!is_private_v4(ip.parse().unwrap()), "{ip}");
        }
    }
}
