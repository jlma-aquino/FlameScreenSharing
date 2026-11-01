//! Comando `receber`: recebe o vídeo do `fluxo-host`, mede e grava em
//! `.h264`, sem janela.
//!
//! É o único comando que roda **fora do Windows** (só usa o `fluxo-net`):
//! serve para testar a rede com um segundo PC Linux/macOS como receptor.
//! Lá, `--verificar` não existe (o decoder é do Windows); confira o arquivo
//! gravado com `ffplay` ou VLC.

use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::net::UdpSocket;
use std::time::{Duration, Instant};

use fluxo_net::{
    FrameAction, PairingCode, Puncher, ReceiverConfig, Recovery, RecoveryRequest, ReplyCode, VideoReceiver,
};

use crate::Opts;

/// `--verificar` no Windows: o mesmo decoder do cliente (na GPU), trocado
/// quando o codec dos frames muda. Só conta os frames que saem.
#[cfg(windows)]
struct Decoder {
    device: windows::Win32::Graphics::Direct3D11::ID3D11Device,
    dec: fluxo_decode::VideoDecoder,
    size: (u32, u32),
}

#[cfg(windows)]
impl Decoder {
    /// Decodifica um frame; devolve quantos saíram (0 ou 1).
    fn decode(&mut self, codec: fluxo_net::VideoCodec, data: &[u8]) -> Result<u64, Box<dyn Error>> {
        if codec != self.dec.codec() {
            self.dec = fluxo_decode::VideoDecoder::with_codec(&self.device, codec)?;
            println!("  vídeo em {} | decoder: {}", codec.name(), self.dec.name());
        }
        Ok(match self.dec.decode(data)? {
            Some(f) => {
                self.size = (f.width(), f.height());
                1
            }
            None => 0,
        })
    }
    fn size(&self) -> (u32, u32) {
        self.size
    }
}

/// Fora do Windows não há decoder: `--verificar` vira um aviso.
#[cfg(not(windows))]
struct Decoder;

#[cfg(not(windows))]
impl Decoder {
    fn decode(&mut self, _: fluxo_net::VideoCodec, _: &[u8]) -> Result<u64, Box<dyn Error>> {
        Ok(0)
    }
    fn size(&self) -> (u32, u32) {
        (0, 0)
    }
}

#[cfg(windows)]
fn abrir_decoder() -> Result<Option<Decoder>, Box<dyn Error>> {
    let device = fluxo_decode::create_device()?;
    let dec = fluxo_decode::VideoDecoder::new(&device)?;
    Ok(Some(Decoder {
        device,
        dec,
        size: (0, 0),
    }))
}

#[cfg(not(windows))]
fn abrir_decoder() -> Result<Option<Decoder>, Box<dyn Error>> {
    println!("(--verificar só existe no Windows; confira o arquivo gravado com ffplay ou VLC)");
    Ok(None)
}

/// Codecs que o `receber` anuncia ao host: os que este Windows decodifica
/// (com ou sem `--verificar`, o arquivo gravado abre no player). Fora do
/// Windows, só H.264, o que qualquer player toca.
fn codecs_anunciados() -> u8 {
    #[cfg(windows)]
    return fluxo_decode::decodable_codecs();
    #[cfg(not(windows))]
    return fluxo_net::VideoCodec::H264.bit();
}

pub fn cmd_receber(o: &Opts) -> Result<(), Box<dyn Error>> {
    let codigo = match (&o.codigo, o.sem_cifra) {
        (_, true) => None,
        (Some(c), false) => Some(PairingCode::parse(c)?),
        (None, false) => Some(PairingCode::generate()?),
    };
    let servidor = o
        .sinal
        .as_deref()
        .map(fluxo_net::resolve_signal_server)
        .transpose()?;
    if (servidor.is_some() || o.internet) && codigo.is_none() {
        return Err("--sinal e --internet precisam do código (sem --sem-cifra)".into());
    }
    if servidor.is_some() && o.internet {
        return Err("use --sinal ou --internet, não os dois".into());
    }
    // Sem servidor: endereços públicos pelo STUN (IPv4 e IPv6) e porta aberta
    // por UPnP, se der; só valem para estes sockets.
    let internet = match o.internet {
        true => Some(fluxo_net::Internet::open(o.porta, !o.sem_upnp)?),
        false => None,
    };
    let (v4, v6, keepalive) = match &internet {
        Some(i) => (
            i.v4.try_clone()?,
            i.v6.as_ref().map(UdpSocket::try_clone).transpose()?,
            Some(i.public.server),
        ),
        None => (UdpSocket::bind(("0.0.0.0", o.porta))?, None, None),
    };
    let rx = VideoReceiver::with_sockets(
        v4,
        v6,
        ReceiverConfig {
            pairing_code: codigo.clone(),
            signal_server: servidor,
            keepalive_to: keepalive,
            on_pairing: internet
                .as_ref()
                .map(|_| fluxo_net::PairingObserver::print_to_terminal()),
            codecs: match o.so_h264 {
                true => fluxo_net::VideoCodec::H264.bit(),
                false => codecs_anunciados(),
            },
            ..ReceiverConfig::default()
        },
    )?;
    // Sem --saida, o nome sai do codec do primeiro frame (recebido.h264/.h265/.obu).
    let mut saida = o.saida.clone();
    match (&codigo, &o.sinal, &internet) {
        (Some(c), _, Some(i)) => {
            let cc = i.connection_code(c.clone())?;
            println!("Escutando em {} (público {}).", rx.local_addr(), i.public.addr);
            for linha in i.summary() {
                println!("  {linha}");
            }
            println!("Código de conexão: {cc}");
            println!("  No PC que envia: fluxo-bench enviar --conexao {cc}");
            println!("Depois cole aqui o código de resposta que ele mostrar:");
            ler_respostas(rx.puncher());
        }
        (Some(c), Some(s), None) => println!(
            "Escutando em {}, código de pareamento {c}. No host: --sinal {s} --codigo {c}",
            rx.local_addr(),
        ),
        (Some(c), None, None) => println!(
            "Escutando em {}, código de pareamento {c}. No host: --destino <IP deste PC>:{} --codigo {c}",
            rx.local_addr(),
            o.porta
        ),
        (None, ..) => println!(
            "Escutando em {} SEM cifra. No host: --destino <IP deste PC>:{} --sem-cifra",
            rx.local_addr(),
            o.porta
        ),
    }
    if o.segundos > 0.0 {
        println!(
            "(mede {} s a partir do primeiro frame; Ctrl+C encerra antes)",
            o.segundos
        );
    }

    let mut decoder = match o.verificar {
        true => abrir_decoder()?,
        false => None,
    };
    let mut arquivo: Option<BufWriter<File>> = None;
    let duracao = (o.segundos > 0.0).then(|| Duration::from_secs_f64(o.segundos));

    let mut inicio: Option<Instant> = None;
    // Largada e perdas: o que gravar e o que pedir ao host (ver fluxo_net::Recovery).
    let mut recuperacao = Recovery::new(Instant::now());
    let mut congelados: Vec<f64> = Vec::new();
    let mut ja_limpo = false;
    let (mut frames, mut bytes, mut keyframes, mut pulados, mut decodificados, mut sujos) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    // Latência captura → chegada, com os relógios sincronizados pelos
    // Ping/Pong. Os frames de antes da 1ª sincronização (~500 ms) ficam fora.
    let mut latencias: Vec<i32> = Vec::new();
    let mut proximo_relatorio = Duration::from_secs(1);
    let (mut frames_seg, mut bytes_seg) = (0u64, 0u64);
    let mut latencias_seg: Vec<i32> = Vec::new();
    let mut avisou_registro = false;
    let mut audio = o.audio.as_deref().map(GravadorAudio::new).transpose()?;
    // --tocar V: toca o áudio como o fluxo-client (V = 0 testa em silêncio).
    let mut tocador = abrir_tocador(o.tocar, o.saida_som)?;

    loop {
        if let (Some(t0), Some(d)) = (inicio, duracao) {
            if t0.elapsed() >= d {
                break;
            }
        }
        // Sem imagem limpa (largada ou depois de uma perda): pede ao host.
        // O fluxo-net segura pedidos repetidos.
        match recuperacao.request(Instant::now()) {
            RecoveryRequest::Keyframe => {
                rx.request_keyframe();
            }
            RecoveryRequest::Refresh => {
                rx.request_refresh();
            }
            RecoveryRequest::None => {}
        }
        if servidor.is_some() && !avisou_registro {
            let s = rx.stats();
            if let Some(publico) = s.public_addr {
                println!("Registrado no servidor de sinalização; endereço público {publico}");
                avisou_registro = true;
            } else if s.room_taken {
                return Err(
                    "o servidor recusou a sala deste código (outro cliente nela); use outro código".into(),
                );
            }
        }
        // Áudio: tudo o que chegou vai para o tocador (--tocar) e para o WAV
        // (--audio, decodificado sem tocar); quadros que
        // faltaram viram som inventado pelo Opus, como no cliente.
        while let Some(a) = rx.recv_audio(Duration::ZERO)? {
            if let Some(t) = tocador.as_mut() {
                t.push(a.seq, a.data.clone());
            }
            if let Some(au) = audio.as_mut() {
                au.receber(&a)?;
            }
        }
        if let Some(t) = tocador.as_mut() {
            t.pump(Instant::now())?;
        }
        // Tocando, o laço precisa voltar logo para alimentar a placa.
        let espera = if tocador.is_some() { 5 } else { 20 };
        let recebido = rx.recv_frame(Duration::from_millis(espera))?;

        if let Some(f) = recebido {
            if inicio.is_none() {
                let de = rx.stats().peer.map(|p| p.to_string()).unwrap_or_default();
                println!("Recebendo de {de}");
                inicio = Some(Instant::now());
            }
            // O arquivo só pode começar num keyframe. Depois de uma perda,
            // grava tudo: quem abrir o arquivo vê o estrago até a onda de
            // intra-refresh (ou o keyframe) pedida limpar a imagem.
            let sujo_desde = recuperacao.dirty_since();
            match recuperacao.on_frame(&f.frame, Instant::now()) {
                FrameAction::Skip => {
                    pulados += 1;
                    continue;
                }
                FrameAction::Decode { show } => sujos += !show as u64,
            }
            if let (Some(t), true) = (sujo_desde, recuperacao.is_clean()) {
                if ja_limpo {
                    congelados.push(t.elapsed().as_secs_f64() * 1000.0);
                }
                ja_limpo = true;
            }
            let arquivo = match arquivo.as_mut() {
                Some(a) => a,
                None => {
                    let codec = f.frame.codec.unwrap_or_default();
                    let nome = saida.get_or_insert_with(|| format!("recebido.{}", crate::extensao(codec)));
                    arquivo.insert(BufWriter::new(File::create(&*nome)?))
                }
            };
            arquivo.write_all(&f.frame.data)?;
            if let Some(d) = &mut decoder {
                decodificados += d.decode(f.frame.codec.unwrap_or_default(), &f.frame.data)?;
            }
            frames += 1;
            bytes += f.frame.data.len() as u64;
            keyframes += f.frame.keyframe as u64;
            frames_seg += 1;
            bytes_seg += f.frame.data.len() as u64;
            if let Some(l) = f.latency_us {
                latencias.push(l);
                latencias_seg.push(l);
            }
        }

        if let Some(t0) = inicio {
            if t0.elapsed() >= proximo_relatorio {
                let s = rx.stats();
                println!(
                    "  {:>3} s: {frames_seg} fps, {:.1} Mbps, latência {}, perdidos {}, RTT {:.2} ms",
                    proximo_relatorio.as_secs(),
                    bytes_seg as f64 * 8.0 / 1e6,
                    match latencias_seg.iter().max() {
                        Some(&max) => format!(
                            "{:.1} ms (máx {:.1})",
                            latencias_seg.iter().map(|&l| l as f64).sum::<f64>()
                                / latencias_seg.len() as f64
                                / 1000.0,
                            max as f64 / 1000.0
                        ),
                        None if s.clock_offset_ms.is_none() => "— (sincronizando relógios)".to_string(),
                        None => "—".to_string(),
                    },
                    s.reassembly.frames_lost,
                    s.rtt.smoothed_ms
                );
                (frames_seg, bytes_seg) = (0, 0);
                latencias_seg.clear();
                proximo_relatorio += Duration::from_secs(1);
            }
        }
    }
    if let Some(a) = arquivo.as_mut() {
        a.flush()?;
    }

    let s = rx.stats();
    let r = s.reassembly;
    let total = inicio.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    latencias.sort_unstable();
    let pct = |p: f64| {
        latencias
            .get(((latencias.len().max(1) - 1) as f64 * p).round() as usize)
            .map(|&us| us as f64 / 1000.0)
            .unwrap_or(0.0)
    };
    println!("\nResultado");
    println!(
        "  frames gravados:          {frames} ({keyframes} keyframes), {pulados} pulados esperando keyframe"
    );
    println!(
        "  bitrate médio:            {:.2} Mbps",
        bytes as f64 * 8.0 / total.max(0.001) / 1e6
    );
    println!(
        "  pacotes: {} recebidos, {} duplicados, {} atrasados, {} inválidos",
        r.packets_received, r.duplicate_packets, r.late_packets, s.invalid_packets
    );
    println!(
        "  frames perdidos na rede:  {} ({} fragmentos reconstruídos pelo FEC; pedidos: {} de limpeza, {} de keyframe)",
        r.frames_lost, r.fec_recovered, s.refresh_requests, s.keyframe_requests
    );
    if r.frames_lost > 0 {
        println!(
            "  grupos de FEC que falharam: {} com 2 pacotes faltando, {} com 3 ou mais; {} frames sem paridade nenhuma",
            r.fec_groups_failed_2, r.fec_groups_failed_3plus, r.frames_abandoned_without_parity
        );
    }
    if !congelados.is_empty() {
        println!(
            "  recuperações:             {} ({} frames com estrago até limpar), média {:.1} ms, máx {:.1} ms",
            congelados.len(),
            sujos,
            congelados.iter().sum::<f64>() / congelados.len() as f64,
            congelados.iter().copied().fold(0.0, f64::max)
        );
    }
    if s.frames_dropped > 0 {
        println!(
            "  descartados na fila:      {} (este programa não leu a tempo)",
            s.frames_dropped
        );
    }
    match (s.clock_offset_ms, s.clock_error_ms) {
        (Some(dif), Some(erro)) => {
            println!(
                "  latência captura→chegada: p50 {:.1} ms, p99 {:.1} ms, máx {:.1} ms ({} frames)",
                pct(0.5),
                pct(0.99),
                pct(1.0),
                latencias.len()
            );
            println!(
                "  relógios:                 host {dif:+.1} ms em relação a este PC \
                 (sincronizados pelos pings, erro ±{erro:.1} ms)"
            );
        }
        _ => println!("  latência:                 — (relógios não sincronizados: nenhum ping respondido)"),
    }
    println!(
        "  RTT:                      {:.2} ms suavizado, {:.2} ms mínimo ({} medições)",
        s.rtt.smoothed_ms, s.rtt.min_ms, s.rtt.samples
    );
    if let Some(d) = &decoder {
        let (w, h) = d.size();
        println!("  decodificados:            {decodificados} de {frames} ({w}x{h})");
    }
    match (&arquivo, &saida) {
        (Some(_), Some(s)) => println!("  gravado em {s}"),
        _ => println!("  nada gravado (nenhum keyframe chegou)"),
    }
    if let Some(t) = &tocador {
        let s = t.stats();
        println!(
            "  áudio tocado: {} quadros, {} perdidos ({} refeitos pelo FEC), {} vezes sem som para tocar, buffer {} quadros + {:.0} ms na placa",
            s.jitter.played, s.jitter.lost, s.fec, s.jitter.underruns, s.jitter.target, s.device_ms
        );
    }
    if let Some(au) = audio {
        au.resumo()?;
    }
    Ok(())
}

/// Lê do terminal os códigos de resposta (numa thread: a medição segue) e
/// fura o NAT na direção de cada um.
fn ler_respostas(furos: Puncher) {
    std::thread::spawn(move || {
        for linha in std::io::stdin().lines() {
            let Ok(linha) = linha else {
                return; // sem terminal
            };
            if linha.trim().is_empty() {
                continue;
            }
            match ReplyCode::parse(&linha) {
                Ok(r) => {
                    let destinos = r.candidates();
                    furos.punch(&destinos);
                    let lista: Vec<String> = destinos.iter().map(|d| d.to_string()).collect();
                    println!("Abrindo caminho até {}…", lista.join(" e "));
                }
                Err(e) => println!("{e}. Cole de novo:"),
            }
        }
    });
}

/// `--audio x.wav`: decodifica os quadros de áudio que chegam (sem tocar) e
/// grava, contando perdas e latência.
struct GravadorAudio {
    caminho: String,
    dec: fluxo_audio::Decoder,
    wav: crate::wav::WavWriter,
    proximo: Option<u32>,
    recebidos: u64,
    perdidos: u64,
    atrasados: u64,
    latencias: Vec<i32>,
}

impl GravadorAudio {
    fn new(caminho: &str) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            caminho: caminho.to_string(),
            dec: fluxo_audio::Decoder::new()?,
            wav: crate::wav::WavWriter::create(
                caminho,
                fluxo_audio::SAMPLE_RATE,
                fluxo_audio::CHANNELS as u16,
            )?,
            proximo: None,
            recebidos: 0,
            perdidos: 0,
            atrasados: 0,
            latencias: Vec::new(),
        })
    }

    fn receber(&mut self, a: &fluxo_net::AudioPacket) -> Result<(), Box<dyn Error>> {
        self.recebidos += 1;
        if let Some(l) = a.latency_us {
            self.latencias.push(l);
        }
        let esperado = self.proximo.unwrap_or(a.seq);
        let falta = a.seq.wrapping_sub(esperado);
        if falta > u32::MAX / 2 {
            self.atrasados += 1; // chegou depois do seguinte: já foi inventado
            return Ok(());
        }
        // Quadros que faltaram: o Opus inventa (o último, a partir do FEC deste).
        for i in 0..falta.min(50) {
            let pcm = match i + 1 == falta {
                true => self.dec.decode_fec(&a.data)?,
                false => self.dec.decode(None)?,
            };
            self.wav.write(&pcm)?;
        }
        self.perdidos += falta as u64;
        self.wav.write(&self.dec.decode(Some(&a.data))?)?;
        self.proximo = Some(a.seq.wrapping_add(1));
        Ok(())
    }

    fn resumo(mut self) -> Result<(), Box<dyn Error>> {
        self.latencias.sort_unstable();
        let p50 = self
            .latencias
            .get(self.latencias.len() / 2)
            .map(|&us| format!(", latência captura→chegada p50 {:.1} ms", us as f64 / 1000.0))
            .unwrap_or_default();
        let s = self.wav.seconds(fluxo_audio::SAMPLE_RATE);
        println!(
            "  áudio: {} quadros recebidos, {} perdidos (inventados/FEC), {} atrasados{p50}",
            self.recebidos, self.perdidos, self.atrasados
        );
        self.wav.finish()?;
        println!("  áudio gravado em {} ({s:.1} s)", self.caminho);
        Ok(())
    }
}

#[cfg(windows)]
type Tocador = fluxo_audio::AudioPlayer;

/// Sem saída de som fora do Windows: `--tocar` dá erro.
#[cfg(not(windows))]
enum Tocador {}

#[cfg(not(windows))]
impl Tocador {
    fn push(&mut self, _: u32, _: Vec<u8>) {
        match *self {}
    }
    fn pump(&mut self, _: Instant) -> Result<(), fluxo_audio::AudioError> {
        match *self {}
    }
    fn stats(&self) -> TocadorStats {
        match *self {}
    }
}

#[cfg(not(windows))]
struct TocadorStats {
    jitter: fluxo_audio::JitterStats,
    fec: u64,
    device_ms: f64,
}

/// `--tocar V`: abre a saída de som com volume V (0 a 100).
#[cfg_attr(not(windows), allow(unused_variables))]
fn abrir_tocador(volume: Option<f32>, saida: Option<usize>) -> Result<Option<Tocador>, Box<dyn Error>> {
    match volume {
        None => Ok(None),
        #[cfg(windows)]
        Some(v) => Ok(Some(fluxo_audio::AudioPlayer::new(v / 100.0, saida)?)),
        #[cfg(not(windows))]
        Some(_) => Err("--tocar só existe no Windows (use --audio para gravar)".into()),
    }
}
