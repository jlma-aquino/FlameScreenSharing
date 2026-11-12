//! O servidor de sinalização fica na internet: lixo e pedidos corrompidos não
//! podem derrubá-lo, com chaves ou aberto.

use std::time::{Duration, Instant};

use fluxo_net::{ServerKey, SignalServer};
use fluxo_proto::{PacketKind, SignalMessage};

struct Sorteio(u64);

impl Sorteio {
    fn proximo(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn ate(&mut self, n: usize) -> usize {
        (self.proximo() % n.max(1) as u64) as usize
    }
}

#[test]
fn servidor_aguenta_lixo_e_pedidos_corrompidos() {
    let chave = ServerKey::generate("203.0.113.5:3478").unwrap();
    let mut com_chave = SignalServer::with_keys(vec![chave.clone()]);
    let mut aberto = SignalServer::default();
    let pedido = PacketKind::Signal
        .control(1, 0)
        .encode_with_payload(
            &SignalMessage::Register {
                room: 7,
                locals: vec!["192.168.0.2:9000".parse().unwrap()],
            }
            .to_bytes(),
        )
        .unwrap();
    let assinado = chave.signed(&pedido);
    let mut r = Sorteio(0xABCD_1234);
    let t0 = Instant::now();
    for i in 0..200_000u64 {
        let mut d = if i % 2 == 0 {
            assinado.clone()
        } else {
            pedido.clone()
        };
        match r.ate(3) {
            0 => {
                let j = r.ate(d.len());
                d[j] = r.proximo() as u8;
            }
            1 => d.truncate(r.ate(d.len() + 1)),
            _ => d = (0..r.ate(300)).map(|_| r.proximo() as u8).collect(),
        }
        let de = format!("198.51.100.{}:{}", r.ate(255), 1000 + r.ate(60000))
            .parse()
            .unwrap();
        let agora = t0 + Duration::from_millis(i);
        let _ = com_chave.handle(&d, de, agora);
        let _ = aberto.handle(&d, de, agora);
    }
}
