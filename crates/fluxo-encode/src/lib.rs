//! # fluxo-encode
//!
//! Comprime em **H.264**, **HEVC** ou **AV1** ([`VideoCodec`]), por hardware, as texturas que o `fluxo-capture`
//! entrega — sem o frame sair da GPU.
//!
//! ## Backends
//!
//! | [`Backend`] | GPUs | Observação |
//! |---|---|---|
//! | [`Backend::Nvenc`] (`nvenc`) | NVIDIA | API do driver, menor latência. Padrão em NVIDIA. |
//! | [`Backend::MediaFoundation`] (`mf`) | NVIDIA, AMD, Intel | Encoder de hardware do fabricante via Windows. |
//!
//! A escolha é feita por [`BackendChoice`] (`--encoder auto|nvenc|mf` nos
//! executáveis). Em `auto`, placas NVIDIA usam NVENC e caem para Media
//! Foundation se ele falhar; as demais usam Media Foundation. Motivo do
//! desenho: ADR-006 em `docs/DECISOES.md`.
//!
//! ## Uso
//!
//! ```no_run
//! # #[cfg(windows)] {
//! use fluxo_capture::DesktopCapturer;
//! use fluxo_encode::{open_encoder, EncoderConfig};
//!
//! let mut cap = DesktopCapturer::new(0)?;
//! let m = cap.monitor().clone();
//! let cfg = EncoderConfig::new(m.width, m.height); // 60 fps, 20 Mbps, auto
//! let aberto = open_encoder(cap.d3d_device(), &cfg)?;
//! let mut enc = aberto.encoder;
//! println!("usando {}", enc.name());
//!
//! if let Some(frame) = cap.next_frame(100)? {
//!     for pacote in enc.encode(frame.texture(), false)? {
//!         // pacote.data: H.264 Annex B, pronto para fragmentar e enviar
//!     }
//! };
//! # }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Organização
//!
//! - `lib.rs` (este arquivo): configuração, erros e escolha de backend —
//!   código puro, testado em qualquer sistema.
//! - `convert.rs`: conversão BGRA → NV12 na GPU (comum aos dois backends).
//! - `nvenc/`: backend NVENC (FFI própria; layout gerado do cabeçalho C).
//! - `mf.rs`: backend Media Foundation.
//! - `platform.rs`: trait `Encoder`, `open_encoder` e detecção da GPU
//!   (só Windows).

use std::fmt;
use std::str::FromStr;

pub use fluxo_proto::VideoCodec;

#[cfg(windows)]
mod convert;
#[cfg(windows)]
mod mf;
#[cfg(windows)]
mod nvenc;
#[cfg(windows)]
mod platform;

#[cfg(windows)]
pub use platform::{gpu_vendor, open_encoder, probe_backend, Encoder, OpenedEncoder};

/// Uma implementação de encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    /// NVIDIA NVENC, direto pela API do driver.
    Nvenc,
    /// Media Foundation do Windows (encoder de hardware de qualquer fabricante).
    MediaFoundation,
}

impl Backend {
    /// Todos os backends, na ordem de preferência.
    pub const ALL: [Backend; 2] = [Backend::Nvenc, Backend::MediaFoundation];

    /// Nome curto usado na linha de comando (`nvenc`, `mf`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Nvenc => "nvenc",
            Self::MediaFoundation => "mf",
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Nvenc => "NVENC",
            Self::MediaFoundation => "Media Foundation",
        })
    }
}

/// Qual backend usar: automático ou um específico.
///
/// Converte de texto com [`str::parse`]:
///
/// ```
/// use fluxo_encode::{Backend, BackendChoice};
///
/// assert_eq!("auto".parse(), Ok(BackendChoice::Auto));
/// assert_eq!("nvenc".parse(), Ok(BackendChoice::Only(Backend::Nvenc)));
/// assert_eq!("MF".parse(), Ok(BackendChoice::Only(Backend::MediaFoundation)));
/// assert!("x264".parse::<BackendChoice>().is_err());
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BackendChoice {
    /// NVENC em placas NVIDIA (com Media Foundation de reserva); Media
    /// Foundation nas demais.
    #[default]
    Auto,
    /// Só este backend; erro se ele não funcionar.
    Only(Backend),
}

impl FromStr for BackendChoice {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "nvenc" => Ok(Self::Only(Backend::Nvenc)),
            "mf" => Ok(Self::Only(Backend::MediaFoundation)),
            outro => Err(format!("encoder desconhecido '{outro}' (use auto, nvenc ou mf)")),
        }
    }
}

impl fmt::Display for BackendChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Only(b) => f.write_str(b.id()),
        }
    }
}

/// Fabricante de GPU, para decidir o backend no modo automático.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuVendor {
    /// NVIDIA (PCI vendor id 0x10DE).
    Nvidia,
    /// AMD (0x1002).
    Amd,
    /// Intel (0x8086).
    Intel,
    /// Qualquer outro (ex.: adaptador de software, GPU virtual).
    Other(u32),
}

impl GpuVendor {
    /// Identifica o fabricante pelo PCI vendor id.
    pub fn from_pci_id(id: u32) -> Self {
        match id {
            0x10DE => Self::Nvidia,
            0x1002 => Self::Amd,
            0x8086 => Self::Intel,
            outro => Self::Other(outro),
        }
    }
}

/// Ordem em que os backends serão tentados para esta escolha e GPU.
///
/// ```
/// use fluxo_encode::{backend_order, Backend, BackendChoice, GpuVendor};
///
/// assert_eq!(
///     backend_order(BackendChoice::Auto, GpuVendor::Nvidia),
///     vec![Backend::Nvenc, Backend::MediaFoundation]
/// );
/// assert_eq!(
///     backend_order(BackendChoice::Auto, GpuVendor::Amd),
///     vec![Backend::MediaFoundation]
/// );
/// ```
pub fn backend_order(choice: BackendChoice, vendor: GpuVendor) -> Vec<Backend> {
    match (choice, vendor) {
        (BackendChoice::Only(b), _) => vec![b],
        (BackendChoice::Auto, GpuVendor::Nvidia) => vec![Backend::Nvenc, Backend::MediaFoundation],
        (BackendChoice::Auto, _) => vec![Backend::MediaFoundation],
    }
}

/// Parâmetros do encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderConfig {
    /// Largura do vídeo codificado, em pixels (par).
    pub width: u32,
    /// Altura do vídeo codificado, em pixels (par).
    pub height: u32,
    /// Tamanho das texturas que [`Encoder::encode`] recebe (a captura), se
    /// diferente de `width` × `height`: a conversão para NV12 redimensiona na
    /// mesma passada, na GPU. `None` = o mesmo tamanho.
    pub input_size: Option<(u32, u32)>,
    /// Quadros por segundo esperados. Usado pelo controle de bitrate.
    pub fps: u32,
    /// Bitrate alvo em kbit/s (CBR).
    pub bitrate_kbps: u32,
    /// Codec do bitstream. HEVC e AV1 dependem da GPU (AV1: NVIDIA RTX 40+,
    /// AMD RX 7000+, Intel Arc); sem suporte, a abertura falha.
    pub codec: VideoCodec,
    /// Backend a usar.
    pub backend: BackendChoice,
    /// Intra-refresh: duração de cada onda, em frames (0 = desligado). Com
    /// ele ligado, [`Encoder::start_intra_refresh`] recupera o decoder de uma
    /// perda redesenhando a imagem em faixas ao longo desses frames, em vez
    /// de um keyframe inteiro. Só no NVENC; nos outros backends é ignorado.
    pub intra_refresh_frames: u32,
}

impl EncoderConfig {
    /// Configuração padrão para este tamanho: H.264, 60 fps, 20 Mbps,
    /// backend automático, sem intra-refresh.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            input_size: None,
            fps: 60,
            bitrate_kbps: 20_000,
            codec: VideoCodec::H264,
            backend: BackendChoice::Auto,
            intra_refresh_frames: 0,
        }
    }

    /// Confere se os valores fazem sentido antes de falar com o driver.
    ///
    /// ```
    /// use fluxo_encode::EncoderConfig;
    ///
    /// assert!(EncoderConfig::new(1920, 1080).validate().is_ok());
    /// assert!(EncoderConfig::new(1921, 1080).validate().is_err()); // ímpar
    /// ```
    pub fn validate(&self) -> Result<(), EncodeError> {
        let erro = |m: &str| Err(EncodeError::InvalidConfig(m.into()));
        if self.width < 64 || self.height < 64 || self.width > 8192 || self.height > 8192 {
            return erro("resolução fora de 64..=8192");
        }
        // NV12 guarda cor a cada 2x2 pixels: largura e altura precisam ser pares.
        if self.width % 2 != 0 || self.height % 2 != 0 {
            return erro("largura e altura precisam ser pares");
        }
        let (iw, ih) = self.input();
        if !(1..=16384).contains(&iw) || !(1..=16384).contains(&ih) {
            return erro("tamanho de entrada fora de 1..=16384");
        }
        if self.fps == 0 || self.fps > 240 {
            return erro("fps fora de 1..=240");
        }
        if self.bitrate_kbps < 100 || self.bitrate_kbps > 500_000 {
            return erro("bitrate fora de 100..=500000 kbps");
        }
        if self.intra_refresh_frames == 1 || self.intra_refresh_frames > 600 {
            return erro("intra-refresh fora de 2..=600 frames (0 desliga)");
        }
        Ok(())
    }

    /// Tamanho das texturas de entrada ([`input_size`](Self::input_size) ou o do vídeo).
    pub fn input(&self) -> (u32, u32) {
        self.input_size.unwrap_or((self.width, self.height))
    }

    /// Bitrate em bit/s.
    pub fn bitrate_bps(&self) -> u32 {
        self.bitrate_kbps.saturating_mul(1000)
    }
}

/// Tamanho do vídeo com `altura` linhas e a proporção da captura `origem`
/// (largura arredondada para par). Para o host transmitir em outra resolução.
///
/// ```
/// use fluxo_encode::scaled_size;
///
/// assert_eq!(scaled_size((1920, 1080), 1440), (2560, 1440));
/// assert_eq!(scaled_size((1920, 1080), 2160), (3840, 2160));
/// assert_eq!(scaled_size((2560, 1080), 720), (1706, 720)); // ultrawide
/// ```
pub fn scaled_size(origem: (u32, u32), altura: u32) -> (u32, u32) {
    let altura = altura & !1;
    let largura = (origem.0 as u64 * altura as u64 / origem.1.max(1) as u64) as u32;
    (largura & !1, altura)
}

/// Bitrate sugerido (kbit/s) para um tamanho e fps: 20 Mbps em 1080p60,
/// proporcional aos pixels por segundo, entre 20 e 100 Mbps.
///
/// ```
/// use fluxo_encode::suggested_bitrate_kbps;
///
/// assert_eq!(suggested_bitrate_kbps(1920, 1080, 60), 20_000);
/// assert_eq!(suggested_bitrate_kbps(1920, 1080, 120), 40_000);
/// assert_eq!(suggested_bitrate_kbps(3840, 2160, 60), 80_000);
/// assert_eq!(suggested_bitrate_kbps(3840, 2160, 120), 100_000); // teto
/// assert_eq!(suggested_bitrate_kbps(1280, 720, 60), 20_000); // piso
/// ```
pub fn suggested_bitrate_kbps(width: u32, height: u32, fps: u32) -> u32 {
    let base = 1920u64 * 1080 * 60;
    let pixels = width as u64 * height as u64 * fps as u64;
    (20_000 * pixels / base).clamp(20_000, 100_000) as u32
}

/// Um frame comprimido.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    /// Bitstream do codec configurado. H.264 e HEVC: Annex B (NALs com
    /// prefixo `00 00 00 01`), keyframes com os parâmetros (SPS/PPS, e VPS no
    /// HEVC). AV1: OBUs no formato de baixo overhead, keyframes com o
    /// cabeçalho de sequência. Em todos, o decoder pode começar por um keyframe.
    pub data: Vec<u8>,
    /// `true` se é um keyframe (IDR): decodificável sem frames anteriores.
    pub keyframe: bool,
    /// Número sequencial do frame de entrada que gerou este (0, 1, 2...).
    pub index: u64,
    /// Primeiro frame de uma onda de intra-refresh pedida por
    /// [`Encoder::start_intra_refresh`].
    pub refresh_start: bool,
    /// Último frame da onda: depois dele, a imagem não depende mais de nada
    /// anterior ao início da onda (um decoder que perdeu algo antes do início
    /// está limpo de novo).
    pub refresh_end: bool,
}

/// Erros do encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    /// Plataforma sem suporte (qualquer coisa que não seja Windows).
    Unsupported,
    /// Configuração inválida (ver [`EncoderConfig::validate`]).
    InvalidConfig(String),
    /// O backend pedido não pôde ser criado nesta máquina.
    BackendUnavailable { backend: Backend, reason: String },
    /// No modo automático, nenhum backend funcionou. Lista o motivo de cada um.
    NoBackend(Vec<(Backend, String)>),
    /// A textura recebida não tem o tamanho configurado (resolução mudou:
    /// recrie o encoder).
    SizeMismatch { expected: (u32, u32), got: (u32, u32) },
    /// Uma chamada NVENC falhou.
    Nvenc {
        call: &'static str,
        status: String,
        detail: String,
    },
    /// Erro do Windows (D3D11 / Media Foundation), com HRESULT e mensagem.
    Windows {
        context: &'static str,
        hresult: i32,
        message: String,
    },
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => write!(f, "encoder de hardware só é suportado no Windows"),
            Self::InvalidConfig(m) => write!(f, "configuração do encoder inválida: {m}"),
            Self::BackendUnavailable { backend, reason } => {
                write!(f, "{backend} indisponível: {reason}")
            }
            Self::NoBackend(motivos) => {
                write!(f, "nenhum encoder de hardware funcionou")?;
                for (b, m) in motivos {
                    write!(f, "; {b}: {m}")?;
                }
                Ok(())
            }
            Self::SizeMismatch { expected, got } => write!(
                f,
                "textura {}x{} diferente do encoder {}x{} (a resolução mudou?)",
                got.0, got.1, expected.0, expected.1
            ),
            Self::Nvenc { call, status, detail } => {
                write!(f, "NVENC {call} falhou: {status}")?;
                if !detail.is_empty() {
                    write!(f, " ({detail})")?;
                }
                Ok(())
            }
            Self::Windows {
                context,
                hresult,
                message,
            } => write!(
                f,
                "{context}: erro do Windows 0x{:08X}: {message}",
                *hresult as u32
            ),
        }
    }
}

impl std::error::Error for EncodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escolha_aceita_maiusculas_e_rejeita_desconhecido() {
        assert_eq!("NVENC".parse(), Ok(BackendChoice::Only(Backend::Nvenc)));
        assert_eq!("Auto".parse(), Ok(BackendChoice::Auto));
        assert!("amf".parse::<BackendChoice>().is_err());
    }

    #[test]
    fn escolha_volta_ao_mesmo_texto() {
        for texto in ["auto", "nvenc", "mf"] {
            let c: BackendChoice = texto.parse().unwrap();
            assert_eq!(c.to_string(), texto);
        }
    }

    #[test]
    fn auto_em_nvidia_tenta_nvenc_depois_mf() {
        assert_eq!(
            backend_order(BackendChoice::Auto, GpuVendor::Nvidia),
            vec![Backend::Nvenc, Backend::MediaFoundation]
        );
    }

    #[test]
    fn auto_fora_da_nvidia_usa_so_mf() {
        for v in [GpuVendor::Amd, GpuVendor::Intel, GpuVendor::Other(0x1414)] {
            assert_eq!(
                backend_order(BackendChoice::Auto, v),
                vec![Backend::MediaFoundation]
            );
        }
    }

    #[test]
    fn escolha_explicita_ignora_fabricante() {
        let so_nvenc = BackendChoice::Only(Backend::Nvenc);
        assert_eq!(backend_order(so_nvenc, GpuVendor::Amd), vec![Backend::Nvenc]);
    }

    #[test]
    fn fabricante_pelo_pci_id() {
        assert_eq!(GpuVendor::from_pci_id(0x10DE), GpuVendor::Nvidia);
        assert_eq!(GpuVendor::from_pci_id(0x1002), GpuVendor::Amd);
        assert_eq!(GpuVendor::from_pci_id(0x8086), GpuVendor::Intel);
        assert_eq!(GpuVendor::from_pci_id(0x1414), GpuVendor::Other(0x1414));
    }

    #[test]
    fn validacao_recusa_valores_absurdos() {
        let base = EncoderConfig::new(1920, 1080);
        assert!(base.validate().is_ok());
        assert!(EncoderConfig {
            height: 1081,
            ..base.clone()
        }
        .validate()
        .is_err());
        assert!(EncoderConfig {
            fps: 0,
            ..base.clone()
        }
        .validate()
        .is_err());
        assert!(EncoderConfig {
            bitrate_kbps: 10,
            ..base.clone()
        }
        .validate()
        .is_err());
        assert!(EncoderConfig { width: 32, ..base }.validate().is_err());
    }

    #[test]
    fn mensagem_de_nenhum_backend_lista_todos_os_motivos() {
        let e = EncodeError::NoBackend(vec![
            (Backend::Nvenc, "driver antigo".into()),
            (Backend::MediaFoundation, "sem MFT".into()),
        ]);
        let m = e.to_string();
        assert!(m.contains("NVENC: driver antigo"));
        assert!(m.contains("Media Foundation: sem MFT"));
    }
}
