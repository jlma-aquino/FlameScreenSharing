//! Limite de tentativas de pareamento: cada `PairInit` novo é um palpite do
//! código (o SPAKE2 aceita um por pareamento), então um atacante só tem os
//! palpites que este limite deixar. Um host de verdade manda o mesmo
//! `PairInit` repetido (reenvio, não conta) e, no máximo, alguns novos por
//! minuto (um por tentativa de 30 s).

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Janela de contagem.
pub const JANELA: Duration = Duration::from_secs(10);
/// `PairInit` novos aceitos por IP na janela.
pub const POR_IP: u32 = 5;
/// `PairInit` novos aceitos de todos os IPs juntos na janela (muitos IPs
/// forjados ou uma rede inteira tentando).
pub const NO_TOTAL: u32 = 40;

#[derive(Debug, Default)]
pub struct LimitePareamento {
    por_ip: HashMap<IpAddr, (Instant, u32)>,
    total: Option<(Instant, u32)>,
}

impl LimitePareamento {
    /// Conta uma tentativa de `ip`; `false`: passou do limite, descarte.
    pub fn permitir(&mut self, ip: IpAddr, agora: Instant) -> bool {
        let conta = |c: &mut (Instant, u32), max: u32| {
            if agora.duration_since(c.0) >= JANELA {
                *c = (agora, 0);
            }
            c.1 += 1;
            c.1 <= max
        };
        // Sem crescer para sempre: quem não tenta há uma janela sai.
        if self.por_ip.len() > 1024 {
            self.por_ip.retain(|_, c| agora.duration_since(c.0) < JANELA);
        }
        let total = self.total.get_or_insert((agora, 0));
        if !conta(total, NO_TOTAL) {
            return false;
        }
        let c = self.por_ip.entry(ip).or_insert((agora, 0));
        conta(c, POR_IP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limite_por_ip_e_total() {
        let mut l = LimitePareamento::default();
        let t = Instant::now();
        let a: IpAddr = "203.0.113.5".parse().unwrap();
        for _ in 0..POR_IP {
            assert!(l.permitir(a, t));
        }
        assert!(!l.permitir(a, t));
        // Outro IP ainda passa; depois da janela, o primeiro também.
        assert!(l.permitir("198.51.100.7".parse().unwrap(), t));
        assert!(l.permitir(a, t + JANELA));

        let mut l = LimitePareamento::default();
        let mut aceitos = 0;
        for i in 0..100u32 {
            let ip = IpAddr::from([10, 0, (i / 256) as u8, i as u8]);
            aceitos += l.permitir(ip, t) as u32;
        }
        assert_eq!(aceitos, NO_TOTAL);
    }
}
