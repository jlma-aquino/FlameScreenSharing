//! Lado do cliente: quando a imagem está boa para mostrar, e o que pedir ao
//! host para ela voltar a ficar ([`Recovery`]).
//!
//! Depois de uma perda, os P-frames seguintes apontam para uma imagem que o
//! decoder não tem. Há dois jeitos de sair disso:
//!
//! - **Keyframe**: o próximo keyframe limpa tudo. É o único jeito na
//!   largada (sem imagem nenhuma) e depois de um erro do decoder.
//! - **Onda de intra-refresh** (protocolo v7): o host redesenha a imagem em
//!   faixas ao longo de N frames. Um decoder que perdeu algo **antes** do
//!   início da onda está limpo de novo no fim dela (medido: idêntico, pixel a
//!   pixel, a um decoder que não perdeu nada). Enquanto isso o cliente
//!   continua decodificando, para o decoder acompanhar, mas não mostra.
//!
//! O host decide qual dos dois mandar: sem intra-refresh, responde a um
//! pedido de limpeza com keyframe — e o keyframe também resolve aqui.

use std::time::{Duration, Instant};

use fluxo_proto::CompletedFrame;

/// Esperando limpeza há mais que isto, pede um keyframe: a onda pedida se
/// perdeu no caminho ou o host parou de mandá-la.
const DESISTIR_DA_ONDA: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Estado {
    /// Sem imagem nenhuma (largada, ou o decoder falhou): só keyframe serve.
    SemImagem,
    /// Imagem boa: mostrar tudo.
    Limpo,
    /// Perdeu algo: espera o início de uma onda (ou um keyframe).
    EsperandoOnda,
    /// A onda começou depois da perda: espera o fim dela.
    NaOnda,
}

/// O que fazer com um frame que chegou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameAction {
    /// Não decodificar: o decoder ainda não tem por onde começar.
    Skip,
    /// Decodificar; mostrar só se `show` (senão a imagem ainda tem estrago
    /// da perda, e a última boa fica congelada na tela).
    Decode { show: bool },
}

/// O que pedir ao host agora.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryRequest {
    /// Nada (imagem boa, ou já no meio de uma onda).
    None,
    /// [`VideoReceiver::request_keyframe`](crate::VideoReceiver::request_keyframe).
    Keyframe,
    /// [`VideoReceiver::request_refresh`](crate::VideoReceiver::request_refresh).
    Refresh,
}

/// Estado de recuperação do cliente. Lógica pura: o instante atual vem como
/// parâmetro.
#[derive(Debug, Clone)]
pub struct Recovery {
    estado: Estado,
    /// Desde quando a imagem não está boa (`None` quando está).
    desde: Option<Instant>,
    perdas: u64,
}

impl Recovery {
    /// Começa sem imagem: o primeiro pedido é de keyframe.
    pub fn new(now: Instant) -> Self {
        Self {
            estado: Estado::SemImagem,
            desde: Some(now),
            perdas: 0,
        }
    }

    /// Decide o que fazer com `frame`, atualizando o estado.
    pub fn on_frame(&mut self, frame: &CompletedFrame, now: Instant) -> FrameAction {
        if frame.keyframe {
            self.limpar();
            return FrameAction::Decode { show: true };
        }
        if self.estado == Estado::SemImagem {
            return FrameAction::Skip;
        }
        if frame.preceded_by_loss {
            if self.estado == Estado::Limpo {
                self.desde = Some(now);
            }
            self.perdas += 1;
            self.estado = Estado::EsperandoOnda;
        }
        // Uma onda que começa neste frame ou depois da perda limpa a perda.
        if self.estado == Estado::EsperandoOnda && frame.refresh_start {
            self.estado = Estado::NaOnda;
        }
        if self.estado == Estado::NaOnda && frame.refresh_end {
            self.limpar();
        }
        FrameAction::Decode {
            show: self.estado == Estado::Limpo,
        }
    }

    /// O decoder falhou: recomeça do zero, num keyframe.
    pub fn on_decode_error(&mut self, now: Instant) {
        if self.desde.is_none() {
            self.desde = Some(now);
        }
        self.estado = Estado::SemImagem;
    }

    /// O que pedir ao host agora. Pode chamar a cada volta: o
    /// [`VideoReceiver`](crate::VideoReceiver) segura pedidos repetidos.
    pub fn request(&self, now: Instant) -> RecoveryRequest {
        let demorou = self
            .desde
            .is_some_and(|t| now.duration_since(t) >= DESISTIR_DA_ONDA);
        match self.estado {
            Estado::Limpo => RecoveryRequest::None,
            Estado::SemImagem => RecoveryRequest::Keyframe,
            _ if demorou => RecoveryRequest::Keyframe,
            Estado::EsperandoOnda => RecoveryRequest::Refresh,
            Estado::NaOnda => RecoveryRequest::None,
        }
    }

    /// `true` se a imagem está boa (os frames estão sendo mostrados).
    pub fn is_clean(&self) -> bool {
        self.estado == Estado::Limpo
    }

    /// Desde quando a imagem não está boa (`None` se está).
    pub fn dirty_since(&self) -> Option<Instant> {
        self.desde
    }

    /// Perdas notadas até agora.
    pub fn losses(&self) -> u64 {
        self.perdas
    }

    fn limpar(&mut self) {
        self.estado = Estado::Limpo;
        self.desde = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quadro(keyframe: bool, perda: bool, inicio: bool, fim: bool) -> CompletedFrame {
        CompletedFrame {
            frame_id: 0,
            keyframe,
            timestamp_us: 0,
            data: Vec::new(),
            preceded_by_loss: perda,
            refresh_start: inicio,
            refresh_end: fim,
            codec: Some(crate::VideoCodec::H264),
        }
    }

    const P: (bool, bool, bool, bool) = (false, false, false, false);
    const SHOW: FrameAction = FrameAction::Decode { show: true };
    const HIDE: FrameAction = FrameAction::Decode { show: false };

    fn com(f: (bool, bool, bool, bool)) -> CompletedFrame {
        quadro(f.0, f.1, f.2, f.3)
    }

    #[test]
    fn largada_espera_keyframe() {
        let t = Instant::now();
        let mut r = Recovery::new(t);
        assert_eq!(r.request(t), RecoveryRequest::Keyframe);
        assert_eq!(r.on_frame(&com(P), t), FrameAction::Skip);
        // Uma onda não serve sem imagem: o decoder não tem SPS/PPS.
        assert_eq!(
            r.on_frame(&quadro(false, false, true, false), t),
            FrameAction::Skip
        );
        assert_eq!(r.on_frame(&quadro(true, false, false, false), t), SHOW);
        assert_eq!(r.request(t), RecoveryRequest::None);
        assert_eq!(r.on_frame(&com(P), t), SHOW);
    }

    #[test]
    fn perda_espera_uma_onda_que_comeca_depois_dela() {
        let t = Instant::now();
        let mut r = Recovery::new(t);
        r.on_frame(&quadro(true, false, false, false), t);

        // Perda: decodifica sem mostrar e pede limpeza.
        assert_eq!(r.on_frame(&quadro(false, true, false, false), t), HIDE);
        assert_eq!(r.request(t), RecoveryRequest::Refresh);
        assert!(r.dirty_since().is_some());
        // O fim de uma onda que começou antes da perda não limpa.
        assert_eq!(r.on_frame(&quadro(false, false, false, true), t), HIDE);
        // A onda nova começa: para de pedir e espera o fim.
        assert_eq!(r.on_frame(&quadro(false, false, true, false), t), HIDE);
        assert_eq!(r.request(t), RecoveryRequest::None);
        assert_eq!(r.on_frame(&com(P), t), HIDE);
        // O fim limpa, e este frame já pode ser mostrado.
        assert_eq!(r.on_frame(&quadro(false, false, false, true), t), SHOW);
        assert!(r.is_clean() && r.dirty_since().is_none());
        assert_eq!(r.losses(), 1);
    }

    #[test]
    fn perda_no_meio_da_onda_espera_a_proxima() {
        let t = Instant::now();
        let mut r = Recovery::new(t);
        r.on_frame(&quadro(true, false, false, false), t);
        r.on_frame(&quadro(false, true, false, false), t);
        r.on_frame(&quadro(false, false, true, false), t);
        // Outra perda dentro da onda: ela não serve mais.
        assert_eq!(r.on_frame(&quadro(false, true, false, false), t), HIDE);
        assert_eq!(r.request(t), RecoveryRequest::Refresh);
        assert_eq!(r.on_frame(&quadro(false, false, false, true), t), HIDE);
        // A onda que começa no próprio frame depois da perda serve.
        assert_eq!(r.on_frame(&quadro(false, true, true, false), t), HIDE);
        assert_eq!(r.on_frame(&quadro(false, false, false, true), t), SHOW);
        assert_eq!(r.losses(), 3);
    }

    #[test]
    fn keyframe_tambem_limpa_uma_perda() {
        // Host sem intra-refresh responde ao pedido de limpeza com keyframe.
        let t = Instant::now();
        let mut r = Recovery::new(t);
        r.on_frame(&quadro(true, false, false, false), t);
        r.on_frame(&quadro(false, true, false, false), t);
        assert_eq!(r.on_frame(&quadro(true, false, false, false), t), SHOW);
        assert!(r.is_clean());
    }

    #[test]
    fn espera_longa_ou_erro_de_decode_pedem_keyframe() {
        let t = Instant::now();
        let mut r = Recovery::new(t);
        r.on_frame(&quadro(true, false, false, false), t);
        r.on_frame(&quadro(false, true, false, false), t);
        r.on_frame(&quadro(false, false, true, false), t);
        assert_eq!(r.request(t + Duration::from_millis(500)), RecoveryRequest::None);
        assert_eq!(r.request(t + Duration::from_secs(2)), RecoveryRequest::Keyframe);

        let mut r = Recovery::new(t);
        r.on_frame(&quadro(true, false, false, false), t);
        r.on_decode_error(t);
        assert_eq!(r.request(t), RecoveryRequest::Keyframe);
        assert_eq!(r.on_frame(&com(P), t), FrameAction::Skip);
    }
}
