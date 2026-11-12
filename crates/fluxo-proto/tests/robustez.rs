//! Nada do que chega pela rede pode derrubar o programa: cada decodificador
//! recebe lixo aleatório e pacotes de verdade com bytes trocados, cortados ou
//! esticados, e tem que devolver erro (ou um valor), nunca entrar em pânico.
//!
//! Gerador aleatório próprio e semente fixa: o teste é sempre o mesmo e roda
//! em qualquer máquina, sem biblioteca de fuzzing.

use fluxo_proto::cursor::{shape_parts, CursorShape};
use fluxo_proto::{
    CursorMessage, Feedback, Fragmenter, PacketHeader, PacketKind, Reassembler, ReassemblerConfig,
    ShapeAssembler, SignalMessage,
};

/// xorshift64*: rápido e suficiente para embaralhar bytes.
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
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.proximo() as u8).collect()
    }
}

/// Pacotes de verdade de cada tipo, para corromper.
fn amostras() -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    let mut frag = Fragmenter::new(1180);
    let (_, pacotes) = frag.fragment(&vec![7u8; 5000], true, 1234).unwrap();
    v.extend(pacotes);
    let addr = |s: &str| s.parse().unwrap();
    for m in [
        SignalMessage::Register {
            room: 42,
            locals: vec![addr("192.168.0.5:9000"), addr("[2001:db8::1]:9000")],
        },
        SignalMessage::Found {
            public: addr("203.0.113.5:4000"),
            locals: vec![addr("10.0.0.2:5000")],
        },
        SignalMessage::Registered {
            public: addr("203.0.113.5:4000"),
        },
        SignalMessage::NotFound,
    ] {
        v.push(
            PacketKind::Signal
                .control(1, 0)
                .encode_with_payload(&m.to_bytes())
                .unwrap(),
        );
    }
    let forma = CursorShape {
        width: 32,
        height: 32,
        hot_x: 1,
        hot_y: 1,
        bgra: vec![200; 32 * 32 * 4],
    };
    for parte in shape_parts(3, &forma) {
        v.push(
            PacketKind::Cursor
                .control(0, 0)
                .encode_with_payload(&parte)
                .unwrap(),
        );
    }
    let pos = CursorMessage::Position {
        x: 100,
        y: 200,
        visible: true,
        shape: 3,
    };
    v.push(
        PacketKind::Cursor
            .control(0, 0)
            .encode_with_payload(&pos.to_bytes())
            .unwrap(),
    );
    v
}

/// Passa um datagrama por tudo o que o cliente e o host fazem com ele.
fn digerir(d: &[u8], remontagem: &mut Reassembler, formas: &mut ShapeAssembler) {
    let Ok((h, payload)) = PacketHeader::decode(d) else {
        return;
    };
    let _ = SignalMessage::from_bytes(payload);
    let _ = Feedback::from_bytes(payload);
    if let Ok(m) = CursorMessage::from_bytes(payload) {
        let _ = formas.push(&m);
    }
    let _ = fluxo_proto::stun::mapped_address(d, &[0; 12]);
    if h.kind == PacketKind::Video || h.kind == PacketKind::Parity {
        let _ = remontagem.push(&h, payload);
    }
}

#[test]
fn lixo_aleatorio_nao_derruba() {
    let mut r = Sorteio(0x5EED_F155);
    let mut remontagem = Reassembler::new(ReassemblerConfig::default());
    let mut formas = ShapeAssembler::default();
    for _ in 0..200_000 {
        let n = r.ate(1600);
        let d = r.bytes(n);
        digerir(&d, &mut remontagem, &mut formas);
    }
}

#[test]
fn pacotes_de_verdade_corrompidos_nao_derrubam() {
    let mut r = Sorteio(0xC0FF_EE11);
    let base = amostras();
    let mut remontagem = Reassembler::new(ReassemblerConfig::default());
    let mut formas = ShapeAssembler::default();
    for _ in 0..300_000 {
        let mut d = base[r.ate(base.len())].clone();
        match r.ate(4) {
            // Troca alguns bytes (inclusive do cabeçalho: tamanhos, índices).
            0 | 1 => {
                for _ in 0..1 + r.ate(4) {
                    let i = r.ate(d.len());
                    d[i] = r.proximo() as u8;
                }
            }
            // Corta.
            2 => d.truncate(r.ate(d.len() + 1)),
            // Estica com lixo.
            _ => {
                let mais = r.ate(200);
                d.extend(r.bytes(mais));
            }
        }
        digerir(&d, &mut remontagem, &mut formas);
    }
}
