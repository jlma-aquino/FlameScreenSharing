//! Relatório periódico do cliente para o host (pacote `Feedback`).
//!
//! O host usa estes números para ajustar o bitrate (controle adaptativo):
//!
//! - **Perda:** o host sabe quantos pacotes mandou; comparando com quantos o
//!   cliente diz ter recebido entre dois relatórios, calcula a taxa de perda
//!   do intervalo. Os contadores são **acumulados** (não "desde o último
//!   relatório"), então um relatório perdido não estraga a conta.
//! - **Atraso de fila:** quanto os frames estão chegando mais atrasados que o
//!   normal — sinal de que algum roteador ou o Wi-Fi está enfileirando
//!   pacotes, antes mesmo de começar a perder.
//!
//! Formato do payload (16 bytes, big-endian):
//!
//! | Offset | Campo |
//! |---:|---|
//! | 0 | `packets_received` (u32, acumulado, dá a volta) |
//! | 4 | `queue_delay_us` (u32) |
//! | 8 | `frames_lost` (u32, acumulado) |
//! | 12 | `fec_recovered` (u32, acumulado) |

use crate::packet::ProtoError;

/// Tamanho do payload de um `Feedback`.
pub const FEEDBACK_LEN: usize = 16;

/// Conteúdo de um pacote `Feedback`.
///
/// ```
/// use fluxo_proto::Feedback;
///
/// let f = Feedback { packets_received: 1234, queue_delay_us: 4_500, frames_lost: 2, fec_recovered: 17 };
/// assert_eq!(Feedback::from_bytes(&f.to_bytes()), Ok(f));
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Feedback {
    /// Pacotes de vídeo e paridade recebidos desde o início (dá a volta).
    pub packets_received: u32,
    /// Atraso de fila estimado agora, em µs (0 = rede sem fila).
    pub queue_delay_us: u32,
    /// Frames que nunca chegaram inteiros, desde o início.
    pub frames_lost: u32,
    /// Fragmentos reconstruídos pelo FEC, desde o início.
    pub fec_recovered: u32,
}

impl Feedback {
    /// Serializa no formato do protocolo.
    pub fn to_bytes(&self) -> [u8; FEEDBACK_LEN] {
        let mut b = [0u8; FEEDBACK_LEN];
        b[0..4].copy_from_slice(&self.packets_received.to_be_bytes());
        b[4..8].copy_from_slice(&self.queue_delay_us.to_be_bytes());
        b[8..12].copy_from_slice(&self.frames_lost.to_be_bytes());
        b[12..16].copy_from_slice(&self.fec_recovered.to_be_bytes());
        b
    }

    /// Lê um payload de `Feedback`.
    pub fn from_bytes(b: &[u8]) -> Result<Self, ProtoError> {
        if b.len() != FEEDBACK_LEN {
            return Err(ProtoError::LengthMismatch {
                declared: FEEDBACK_LEN,
                actual: b.len(),
            });
        }
        let u = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Ok(Self {
            packets_received: u(0),
            queue_delay_us: u(4),
            frames_lost: u(8),
            fec_recovered: u(12),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ida_e_volta_com_valores_extremos() {
        let f = Feedback {
            packets_received: u32::MAX,
            queue_delay_us: 0,
            frames_lost: 1,
            fec_recovered: 0x0102_0304,
        };
        let b = f.to_bytes();
        assert_eq!(&b[12..16], &[1, 2, 3, 4]); // big-endian
        assert_eq!(Feedback::from_bytes(&b), Ok(f));
    }

    #[test]
    fn tamanho_errado_e_recusado() {
        assert!(Feedback::from_bytes(&[0; 15]).is_err());
        assert!(Feedback::from_bytes(&[0; 17]).is_err());
    }
}
