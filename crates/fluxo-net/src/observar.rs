//! O que acontece enquanto host e cliente se procuram: [`PairingObserver`].
//!
//! Quando o furo de NAT não fecha, nenhum lado recebe nada útil, e sem
//! informação não dá para saber onde os pacotes param. Com um observador na
//! configuração ([`SenderConfig::on_pairing`](crate::SenderConfig::on_pairing),
//! [`ReceiverConfig::on_pairing`](crate::ReceiverConfig::on_pairing)), cada
//! datagrama que chega antes da sessão e, a cada ~5 s, um resumo do que saiu
//! viram [`PairingEvent`]s.

use std::collections::HashSet;
use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Intervalo dos resumos [`PairingEvent::Waiting`].
pub(crate) const RESUMO_A_CADA: Duration = Duration::from_secs(5);

/// [`PairingEvent::Received::kind`] de uma resposta STUN.
const STUN: &str = "resposta STUN";

/// Um acontecimento enquanto o outro lado não aparece.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingEvent {
    /// Chegou um datagrama de `from`; `kind` diz o que era ("furo",
    /// "pedido de pareamento", "resposta STUN", "lixo"...).
    Received { from: SocketAddr, kind: &'static str },
    /// Resumo periódico: `sent` pacotes de `what` (ex.: "pedidos de
    /// pareamento", "furos") saíram até agora para `to`.
    Waiting {
        elapsed: Duration,
        what: &'static str,
        sent: u64,
        to: Vec<SocketAddr>,
    },
}

/// Recebe os [`PairingEvent`]s (de outra thread).
#[derive(Clone)]
pub struct PairingObserver(Arc<dyn Fn(&PairingEvent) + Send + Sync>);

impl PairingObserver {
    pub fn new(f: impl Fn(&PairingEvent) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// Escreve no terminal: cada tipo de pacote de cada endereço na primeira
    /// vez, e os resumos dizendo quantos chegaram ao todo.
    pub fn print_to_terminal() -> Self {
        let vistos: Mutex<(HashSet<(SocketAddr, &'static str)>, u64)> = Mutex::new((HashSet::new(), 0));
        Self::new(move |e| {
            let mut v = vistos.lock().unwrap_or_else(|e| e.into_inner());
            match e {
                // Respostas STUN (keepalive do NAT) não vêm do outro lado.
                PairingEvent::Received { kind: STUN, .. } => {}
                PairingEvent::Received { from, kind } => {
                    v.1 += 1;
                    if v.0.insert((*from, kind)) {
                        println!("  [conexão] chegou de {from}: {kind}");
                    }
                }
                PairingEvent::Waiting {
                    elapsed,
                    what,
                    sent,
                    to,
                } => {
                    let destinos: Vec<String> = to.iter().map(|a| a.to_string()).collect();
                    let chegou = match v.1 {
                        0 => "nada chegou do outro lado ainda".to_string(),
                        1 => "1 pacote chegou".to_string(),
                        n => format!("{n} pacotes chegaram"),
                    };
                    println!(
                        "  [conexão] {} s: {sent} {what} enviados para {}; {chegou}",
                        elapsed.as_secs(),
                        match destinos.is_empty() {
                            true => "ninguém".to_string(),
                            false => destinos.join(", "),
                        }
                    );
                }
            }
        })
    }

    pub(crate) fn notify(&self, e: &PairingEvent) {
        (self.0)(e)
    }
}

impl fmt::Debug for PairingObserver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairingObserver")
    }
}

/// Dois observadores são iguais se são o mesmo (para `SenderConfig: PartialEq`).
impl PartialEq for PairingObserver {
    fn eq(&self, outro: &Self) -> bool {
        Arc::ptr_eq(&self.0, &outro.0)
    }
}

/// O que é um datagrama, em poucas palavras (para [`PairingEvent::Received`]).
pub(crate) fn kind_of(datagrama: &[u8]) -> &'static str {
    use fluxo_proto::{PacketHeader, PacketKind, ProtoError};
    if datagrama.len() >= 8 && datagrama[4..8] == fluxo_proto::stun::MAGIC_COOKIE.to_be_bytes() {
        return STUN;
    }
    match PacketHeader::decode(datagrama) {
        Ok((h, _)) => match h.kind {
            PacketKind::Punch => "furo",
            PacketKind::PairInit => "pedido de pareamento",
            PacketKind::PairReply => "resposta de pareamento",
            PacketKind::Signal => "sinalização",
            _ if h.is_encrypted() => "pacote cifrado do Fluxo (de outra sessão?)",
            _ => "pacote do Fluxo em claro",
        },
        Err(ProtoError::UnsupportedVersion(_)) => "pacote do Fluxo de outra versão",
        Err(_) => "lixo (não é do Fluxo)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fluxo_proto::PacketKind;

    #[test]
    fn classifica_o_que_chega() {
        let furo = PacketKind::Punch.control(0, 0).encode_with_payload(&[]).unwrap();
        assert_eq!(kind_of(&furo), "furo");
        let init = PacketKind::PairInit
            .control(1, 0)
            .encode_with_payload(&[0; 33])
            .unwrap();
        assert_eq!(kind_of(&init), "pedido de pareamento");
        assert_eq!(kind_of(&fluxo_proto::stun::binding_request(&[0; 12])), STUN);
        let mut velho = furo.clone();
        velho[2] = 1;
        assert_eq!(kind_of(&velho), "pacote do Fluxo de outra versão");
        assert_eq!(kind_of(b"oi"), "lixo (não é do Fluxo)");
    }
}
