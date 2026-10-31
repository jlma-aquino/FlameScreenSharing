//! Cursor do mouse (payload de [`PacketKind::Cursor`]).
//!
//! O ponteiro não vem na imagem capturada: vai à parte, em pacotes pequenos,
//! e o cliente o desenha por cima do vídeo. Assim ele anda na hora, sem
//! esperar o próximo frame (e sem custar bitrate).
//!
//! ```text
//! Posição (host → cliente):  [1][x i16][y i16][visível u8][forma u16]            8 bytes
//! Forma   (host → cliente):  [2][forma u16][parte u8][partes u8]
//!                            [largura u16][altura u16][hot x u16][hot y u16][BGRA...]
//! Pedido  (cliente → host):  [3][forma u16]
//! ```
//!
//! `x`/`y` são o canto superior esquerdo da imagem do cursor, em pixels do
//! monitor capturado. A forma (BGRA com alfa não pré-multiplicado, linha a
//! linha) é dividida em partes de até [`SHAPE_PART_MAX`] bytes; o cliente
//! que vê na posição uma forma que não tem pede de novo.
//!
//! [`PacketKind::Cursor`]: crate::PacketKind::Cursor

use crate::ProtoError;

const POSICAO: u8 = 1;
const FORMA: u8 = 2;
const PEDIDO: u8 = 3;

/// Cabeçalho de uma parte da forma.
const CABECALHO_FORMA: usize = 1 + 2 + 1 + 1 + 8;
/// Bytes de imagem por parte (cabe no payload máximo com o cabeçalho).
pub const SHAPE_PART_MAX: usize = crate::DEFAULT_MAX_PAYLOAD - CABECALHO_FORMA;
/// Maior lado aceito de um cursor (o Windows vai até 256 com acessibilidade).
pub const MAX_CURSOR_SIDE: u16 = 256;

/// Imagem de um cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorShape {
    pub width: u16,
    pub height: u16,
    /// Ponto que "clica", a partir do canto superior esquerdo.
    pub hot_x: u16,
    pub hot_y: u16,
    /// `width × height × 4` bytes BGRA, alfa não pré-multiplicado.
    pub bgra: Vec<u8>,
}

/// Uma mensagem de cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorMessage<'a> {
    Position {
        x: i16,
        y: i16,
        visible: bool,
        shape: u16,
    },
    ShapePart {
        shape: u16,
        index: u8,
        count: u8,
        width: u16,
        height: u16,
        hot_x: u16,
        hot_y: u16,
        data: &'a [u8],
    },
    ShapeRequest {
        shape: u16,
    },
}

impl CursorMessage<'_> {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        match self {
            Self::Position { x, y, visible, shape } => {
                b.push(POSICAO);
                b.extend_from_slice(&x.to_be_bytes());
                b.extend_from_slice(&y.to_be_bytes());
                b.push(*visible as u8);
                b.extend_from_slice(&shape.to_be_bytes());
            }
            Self::ShapePart {
                shape,
                index,
                count,
                width,
                height,
                hot_x,
                hot_y,
                data,
            } => {
                b.push(FORMA);
                b.extend_from_slice(&shape.to_be_bytes());
                b.push(*index);
                b.push(*count);
                for v in [width, height, hot_x, hot_y] {
                    b.extend_from_slice(&v.to_be_bytes());
                }
                b.extend_from_slice(data);
            }
            Self::ShapeRequest { shape } => {
                b.push(PEDIDO);
                b.extend_from_slice(&shape.to_be_bytes());
            }
        }
        b
    }
}

impl<'a> CursorMessage<'a> {
    pub fn from_bytes(b: &'a [u8]) -> Result<Self, ProtoError> {
        let u16_em = |i: usize| -> Result<u16, ProtoError> {
            Ok(u16::from_be_bytes([
                *b.get(i).ok_or(ProtoError::BadCursor)?,
                *b.get(i + 1).ok_or(ProtoError::BadCursor)?,
            ]))
        };
        match b.first() {
            Some(&POSICAO) if b.len() == 8 => Ok(Self::Position {
                x: u16_em(1)? as i16,
                y: u16_em(3)? as i16,
                visible: b[5] != 0,
                shape: u16_em(6)?,
            }),
            Some(&FORMA) if b.len() > CABECALHO_FORMA => {
                let (index, count) = (b[3], b[4]);
                let (width, height) = (u16_em(5)?, u16_em(7)?);
                if count == 0
                    || index >= count
                    || width == 0
                    || height == 0
                    || width > MAX_CURSOR_SIDE
                    || height > MAX_CURSOR_SIDE
                {
                    return Err(ProtoError::BadCursor);
                }
                Ok(Self::ShapePart {
                    shape: u16_em(1)?,
                    index,
                    count,
                    width,
                    height,
                    hot_x: u16_em(9)?,
                    hot_y: u16_em(11)?,
                    data: &b[CABECALHO_FORMA..],
                })
            }
            Some(&PEDIDO) if b.len() == 3 => Ok(Self::ShapeRequest { shape: u16_em(1)? }),
            _ => Err(ProtoError::BadCursor),
        }
    }
}

/// Payloads das partes da forma `id`.
pub fn shape_parts(id: u16, s: &CursorShape) -> Vec<Vec<u8>> {
    let pedacos: Vec<&[u8]> = s.bgra.chunks(SHAPE_PART_MAX).collect();
    let count = pedacos.len().min(u8::MAX as usize) as u8;
    pedacos
        .iter()
        .take(count as usize)
        .enumerate()
        .map(|(i, data)| {
            CursorMessage::ShapePart {
                shape: id,
                index: i as u8,
                count,
                width: s.width,
                height: s.height,
                hot_x: s.hot_x,
                hot_y: s.hot_y,
                data,
            }
            .to_bytes()
        })
        .collect()
}

/// Forma sendo montada: o número, as medidas e as partes que já chegaram.
#[derive(Debug)]
struct Montagem {
    id: u16,
    base: CursorShape,
    partes: Vec<Option<Vec<u8>>>,
}

/// Junta as partes de uma forma (só da mais recente: partes de uma forma
/// anterior que chegarem atrasadas são ignoradas).
#[derive(Debug, Default)]
pub struct ShapeAssembler {
    atual: Option<Montagem>,
}

impl ShapeAssembler {
    /// Recebe uma parte; devolve a forma quando ela fica completa.
    pub fn push(&mut self, m: &CursorMessage) -> Option<(u16, CursorShape)> {
        let CursorMessage::ShapePart {
            shape,
            index,
            count,
            width,
            height,
            hot_x,
            hot_y,
            data,
        } = *m
        else {
            return None;
        };
        let nova = !matches!(&self.atual, Some(Montagem { id, base: s, partes })
            if *id == shape && s.width == width && s.height == height && partes.len() == count as usize);
        if nova {
            let base = CursorShape {
                width,
                height,
                hot_x,
                hot_y,
                bgra: Vec::new(),
            };
            self.atual = Some(Montagem {
                id: shape,
                base,
                partes: vec![None; count as usize],
            });
        }
        let Montagem { id, base, partes } = self.atual.as_mut().expect("criada acima");
        partes[index as usize] = Some(data.to_vec());
        if partes.iter().any(Option::is_none) {
            return None;
        }
        let bgra: Vec<u8> = partes.iter().flatten().flatten().copied().collect();
        let esperado = base.width as usize * base.height as usize * 4;
        let pronta = (bgra.len() == esperado).then(|| (*id, CursorShape { bgra, ..base.clone() }));
        self.atual = None;
        pronta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forma(w: u16, h: u16) -> CursorShape {
        CursorShape {
            width: w,
            height: h,
            hot_x: 3,
            hot_y: 4,
            bgra: (0..w as usize * h as usize * 4).map(|i| i as u8).collect(),
        }
    }

    #[test]
    fn posicao_e_pedido_ida_e_volta() {
        for m in [
            CursorMessage::Position {
                x: -12,
                y: 2100,
                visible: true,
                shape: 7,
            },
            CursorMessage::ShapeRequest { shape: 65535 },
        ] {
            assert_eq!(CursorMessage::from_bytes(&m.to_bytes()).unwrap(), m);
        }
    }

    #[test]
    fn forma_grande_vai_em_partes_e_volta_inteira_em_qualquer_ordem() {
        let s = forma(64, 64); // 16 KB → 15 partes
        let partes = shape_parts(9, &s);
        assert_eq!(partes.len(), (16384usize).div_ceil(SHAPE_PART_MAX));
        assert!(partes.iter().all(|p| p.len() <= crate::DEFAULT_MAX_PAYLOAD));
        let mut a = ShapeAssembler::default();
        // De trás para frente, com uma repetida.
        let mut pronta = None;
        for p in partes.iter().rev().chain(std::iter::once(&partes[3])) {
            if let Some(r) = a.push(&CursorMessage::from_bytes(p).unwrap()) {
                pronta = Some(r);
            }
        }
        assert_eq!(pronta, Some((9, s)));
    }

    #[test]
    fn forma_nova_descarta_a_incompleta() {
        let (velha, nova) = (forma(32, 32), forma(16, 16));
        let mut a = ShapeAssembler::default();
        let pv = shape_parts(1, &velha);
        assert!(a.push(&CursorMessage::from_bytes(&pv[0]).unwrap()).is_none());
        let pn = shape_parts(2, &nova);
        assert_eq!(
            a.push(&CursorMessage::from_bytes(&pn[0]).unwrap()),
            Some((2, nova))
        );
        // Parte atrasada da velha começa outra montagem, que não completa.
        assert!(a.push(&CursorMessage::from_bytes(&pv[1]).unwrap()).is_none());
    }

    #[test]
    fn lixo_e_recusado() {
        for b in [&[][..], &[1, 0, 0], &[2, 0, 1, 0, 0], &[9, 9, 9], &[3, 0]] {
            assert_eq!(CursorMessage::from_bytes(b), Err(ProtoError::BadCursor), "{b:?}");
        }
        // Lado maior que o permitido.
        let mut grande = CursorMessage::ShapePart {
            shape: 1,
            index: 0,
            count: 1,
            width: MAX_CURSOR_SIDE + 1,
            height: 1,
            hot_x: 0,
            hot_y: 0,
            data: &[0; 4],
        }
        .to_bytes();
        assert_eq!(CursorMessage::from_bytes(&grande), Err(ProtoError::BadCursor));
        grande[5] = 0; // largura 257 → 1
        grande[6] = 1;
        assert!(CursorMessage::from_bytes(&grande).is_ok());
    }
}
