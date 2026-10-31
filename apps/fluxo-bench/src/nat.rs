//! Comando `nat`: como o NAT desta rede trata um socket UDP.
//!
//! Pergunta a vários servidores STUN públicos, pelo mesmo socket, de onde ele
//! aparece na internet, e compara: mesma porta para todos = o furo de NAT
//! costuma passar; porta diferente por destino = NAT simétrico, que só
//! conecta se o outro lado for mais aberto.

use std::error::Error;
use std::net::UdpSocket;

use fluxo_net::{probe_nat, STUN_SERVERS};

use crate::Opts;

pub fn cmd_nat(_: &Opts) -> Result<(), Box<dyn Error>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let local = socket.local_addr()?;
    match fluxo_net::local_ip() {
        Some(ip) => println!("Socket local: porta {} (IP na rede local {ip})", local.port()),
        None => println!("Socket local: porta {} (sem rota para a internet?)", local.port()),
    }
    let respostas = probe_nat(&socket, STUN_SERVERS)?;
    for r in &respostas {
        let visto = match (r.server_addr, r.mapped) {
            (None, _) => "nome não resolveu".to_string(),
            (Some(_), None) => "sem resposta".to_string(),
            (Some(_), Some(a)) => a.to_string(),
        };
        println!("  {:<28} → {visto}", r.server);
    }
    let vistos: Vec<_> = respostas.iter().filter_map(|r| r.mapped).collect();
    // UPnP: só pergunta ao roteador (não abre porta nenhuma).
    match fluxo_net::find_gateway() {
        Ok(Some(g)) => match (g.external_ip(), vistos.first()) {
            (Ok(ip), _) if fluxo_net::is_private_v4(ip) => {
                println!("  roteador com UPnP, mas o IP externo dele é {ip}: há CGNAT na frente, abrir porta não adianta")
            }
            (Ok(ip), Some(publico)) if std::net::IpAddr::V4(ip) == publico.ip() => {
                println!("  roteador com UPnP e IP público ({ip}): o cliente abre a porta e basta um código")
            }
            (Ok(ip), _) => println!("  roteador com UPnP, IP externo {ip} (diferente do que o STUN viu)"),
            (Err(e), _) => println!("  roteador com UPnP, mas não informou o IP externo ({e})"),
        },
        Ok(None) => println!("  sem UPnP na rede local"),
        Err(e) => println!("  UPnP: {e}"),
    }
    println!();
    let Some(primeiro) = vistos.first() else {
        println!("Nenhum servidor respondeu: sem internet, ou a rede bloqueia UDP.");
        return Ok(());
    };
    if vistos.len() == 1 {
        println!("Só 1 servidor respondeu ({primeiro}): não deu para comparar portas.");
        return Ok(());
    }
    if vistos.iter().any(|a| a.ip() != primeiro.ip()) {
        println!("IPs públicos diferentes conforme o destino: o furo direto dificilmente passa.");
        return Ok(());
    }
    let portas: Vec<u16> = vistos.iter().map(|a| a.port()).collect();
    if portas.iter().all(|&p| p == portas[0]) {
        print!("Mesma porta para todos os destinos: bom para o furo de NAT");
        if portas[0] == local.port() {
            print!(" (e a porta local foi preservada)");
        }
        println!(".");
    } else {
        let (min, max) = (
            portas.iter().min().unwrap_or(&0),
            portas.iter().max().unwrap_or(&0),
        );
        println!("NAT simétrico: cada destino ganha uma porta nova ({portas:?}).");
        if max - min <= 10 {
            println!(
                "As portas parecem sequenciais (diferença de até {}): dá para tentar prever.",
                max - min
            );
        } else {
            println!("As portas não seguem uma ordem clara: prever a próxima é improvável.");
        }
        println!("A conexão direta só passa se o NAT do outro lado não for simétrico.");
    }
    Ok(())
}
