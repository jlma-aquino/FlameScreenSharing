//! Som do host no Linux: buffer de jitter e Opus como no Windows, e o PCM vai
//! por pipe para um tocador que já vem no sistema. Sem biblioteca nova para
//! compilar nem depender (ADR-015).

use std::io::Write;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use fluxo_audio::{Decoder, JitterBuffer, Next, FRAME_SAMPLES, SAMPLE_RATE};

use super::Compartilhado;

/// Quanto som deixar adiantado no tocador, como `DEVICE_QUEUE_MS` no Windows.
const FILA_MS: f64 = 20.0;

/// Os tocadores que sabemos usar, do de menor atraso ao mais comum.
const TOCADORES: [(&str, &[&str]); 3] = [
    (
        "pacat",
        &[
            "--playback",
            "--raw",
            "--format=float32le",
            "--rate=48000",
            "--channels=2",
            "--latency-msec=30",
        ],
    ),
    (
        "pw-cat",
        &[
            "--playback",
            "--raw",
            "--format=f32",
            "--rate=48000",
            "--channels=2",
            "--latency=30ms",
            "-",
        ],
    ),
    (
        "aplay",
        &[
            "-q",
            "-t",
            "raw",
            "-f",
            "FLOAT_LE",
            "-r",
            "48000",
            "-c",
            "2",
            "--buffer-time=60000",
            "-",
        ],
    ),
];

/// Abre o primeiro tocador que existir e continuar de pé.
fn abrir_tocador() -> Result<(&'static str, Child, ChildStdin), String> {
    for (nome, args) in TOCADORES {
        let Ok(mut filho) = Command::new(nome)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        // Um tocador que não gostou do formato ou não achou servidor de som
        // sai logo.
        std::thread::sleep(Duration::from_millis(150));
        if matches!(filho.try_wait(), Ok(None)) {
            if let Some(entrada) = filho.stdin.take() {
                return Ok((nome, filho, entrada));
            }
        }
        let _ = filho.kill();
        let _ = filho.wait();
    }
    Err("nenhum tocador de som funcionou (pacat, pw-cat ou aplay)".into())
}

/// Quanto som ainda está na frente do relógio. O tocador é um pipe, sem como
/// perguntar: conta o que foi mandado contra o tempo que passou.
#[derive(Default)]
struct Adiantado {
    inicio: Option<Instant>,
    enviado_ms: f64,
}

impl Adiantado {
    fn ms(&self, agora: Instant) -> f64 {
        self.inicio.map_or(0.0, |t| {
            (self.enviado_ms - agora.duration_since(t).as_secs_f64() * 1000.0).max(0.0)
        })
    }

    /// Mandou `ms` de som. Se a fila tinha acabado, recomeça a conta dali.
    fn enviou(&mut self, ms: f64, agora: Instant) {
        if self.ms(agora) <= 0.0 {
            self.inicio = Some(agora);
            self.enviado_ms = 0.0;
        }
        self.enviado_ms += ms;
    }
}

/// Amostras em f32, com volume, nos bytes que o tocador lê.
fn para_bytes(pcm: &[f32], volume: f32, saida: &mut Vec<u8>) {
    saida.clear();
    saida.reserve(pcm.len() * 4);
    for a in pcm {
        saida.extend_from_slice(&(a * volume).to_le_bytes());
    }
}

/// Thread do som, até a janela fechar ou a rede terminar. Devolve o resumo.
pub fn tocar(
    fila: fluxo_net::AudioStream,
    volume: f32,
    c: &Compartilhado,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let (nome, mut filho, mut entrada) = abrir_tocador()?;
    println!("  som: {nome}");
    let quadro_ms = FRAME_SAMPLES as f64 * 1000.0 / SAMPLE_RATE as f64;
    let mut jitter = JitterBuffer::default();
    let mut dec = Decoder::new()?;
    let mut adiantado = Adiantado::default();
    let mut bytes = Vec::new();
    let mut fec = 0u64;
    'tocar: while !c.fechar.load(Ordering::Relaxed) {
        let mut espera = Duration::from_millis(3);
        loop {
            match fila.recv(espera) {
                Ok(Some(a)) => jitter.push(a.seq, a.data),
                Ok(None) => break,
                Err(fluxo_net::NetError::Closed) => break 'tocar,
                Err(e) => return Err(e.into()),
            }
            espera = Duration::ZERO;
        }
        while adiantado.ms(Instant::now()) < FILA_MS {
            let pcm = match jitter.next(Instant::now()) {
                Next::Packet(p) => dec.decode(Some(&p))?,
                Next::Lost { following: Some(p) } => {
                    fec += 1;
                    dec.decode_fec(&p)?
                }
                Next::Lost { following: None } => dec.decode(None)?,
                Next::Empty => break,
            };
            para_bytes(&pcm, volume, &mut bytes);
            entrada.write_all(&bytes)?;
            adiantado.enviou(quadro_ms, Instant::now());
        }
    }
    drop(entrada);
    let _ = filho.kill();
    let _ = filho.wait();
    let s = jitter.stats();
    Ok(format!(
        "áudio: {} quadros tocados, {} perdidos ({fec} refeitos pelo FEC), {} vezes sem som para tocar, \
         buffer final {} quadros de 10 ms",
        s.played, s.lost, s.underruns, s.target
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adiantado_desconta_o_tempo_e_recomeca_depois_de_esvaziar() {
        let t = Instant::now();
        let mut a = Adiantado::default();
        assert_eq!(a.ms(t), 0.0);
        a.enviou(10.0, t);
        a.enviou(10.0, t);
        assert_eq!(a.ms(t), 20.0);
        assert_eq!(a.ms(t + Duration::from_millis(5)), 15.0);
        // Esvaziou: o som novo conta a partir de agora, sem dívida antiga.
        let depois = t + Duration::from_millis(100);
        assert_eq!(a.ms(depois), 0.0);
        a.enviou(10.0, depois);
        assert_eq!(a.ms(depois), 10.0);
    }

    #[test]
    fn bytes_em_f32_com_volume() {
        let mut b = Vec::new();
        para_bytes(&[1.0, -0.5], 0.5, &mut b);
        assert_eq!(b.len(), 8);
        assert_eq!(f32::from_le_bytes([b[0], b[1], b[2], b[3]]), 0.5);
        assert_eq!(f32::from_le_bytes([b[4], b[5], b[6], b[7]]), -0.25);
    }
}
