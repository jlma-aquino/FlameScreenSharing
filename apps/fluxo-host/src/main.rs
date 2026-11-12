//! # fluxo-host
//!
//! Lado de quem compartilha a tela: captura → encode → rede.
//!
//! ```text
//! fluxo-host --sala | (--destino IP:PORTA | --sinal SERVIDOR) --codigo XXXX-XXXX [--monitor N] [--encoder auto|nvenc|mf] [--codec auto|h264|hevc|av1]
//!            [--resolucao R] [--mbps M] [--fps F] [--pacing-mbps P] [--keyframe-a-cada S]
//!            [--segundos S] [--porta-local P]
//! ```
//!
//! Com `--sala`, o host cria o código e quem assiste entra com ele, pelo
//! servidor de sinalização, quantos quiserem (até o limite). Sem sala, o
//! cliente é achado pelo endereço (`--destino`, mesma rede) ou pelo
//! servidor de sinalização (`--sinal`, redes diferentes), e o pareamento com
//! o código vem antes de qualquer imagem. Keyframes saem quando o cliente
//! pede (e, de segurança, a cada `--keyframe-a-cada` segundos).
//!
//! Detalhes e como ler a saída: `docs/modulos/fluxo-host.md`.

// Só o host Windows usa; os testes da lógica de tempo rodam em qualquer sistema.
#[cfg(any(windows, test))]
mod espectadores;
#[cfg(any(windows, test))]
mod limite;

use std::process::ExitCode;

const AJUDA: &str = "\
fluxo-host — transmite a tela para um cliente Fluxo

Uso:
  fluxo-host --sala --chave FSS1-… [opções]
  fluxo-host --destino IP:PORTA --codigo XXXX-XXXX [opções]
  fluxo-host --sinal SERVIDOR --codigo XXXX-XXXX [opções]
  fluxo-host --conexao CÓDIGO-DE-CONEXÃO [opções]

Opções:
  --sala                abre uma sala: mostra um código, e quem assiste entra com ele
                        (fluxo-client --sala CÓDIGO), sem responder nada. Precisa
                        do servidor: --chave; --codigo fixa o código
  --chave FSS1-…        chave de acesso ao servidor de sinalização (quem roda o
                        fluxo-sinal gera com --nova-chave): o endereço dele e o
                        segredo que assina os pedidos
  --segredos-pela-entrada
                        lê da entrada, antes de tudo, linhas --opcao valor até a linha fim
                        (a chave e os códigos fora da linha de comando, que outros
                        programas do PC conseguem ler)
  --destino IP:PORTA    para onde enviar, ex.: 192.168.0.20:9000 (mesma rede)
  --sinal SERVIDOR      acha o cliente pelo servidor de sinalização (fluxo-sinal),
                        ex.: meu-servidor.com ou 203.0.113.5:7000 (redes diferentes)
  --conexao C           código de conexão mostrado pelo fluxo-client --internet (redes
                        diferentes, sem servidor): já traz o código de pareamento; o
                        host mostra um código de resposta para mandar de volta
  --ttl-furo N          com --conexao ou --sala: TTL dos pedidos até o furo do cliente chegar
                        (padrão 12; 0 = TTL normal)
  --codigo XXXX-XXXX    código de pareamento mostrado pelo fluxo-client (obrigatório):
                        só o cliente com esse código recebe, e tudo vai cifrado
  --sem-cifra           TESTE (só em compilação de teste, não na de produção): sem pareamento nem cifra (o cliente também)
  --monitor N           monitor a transmitir (padrão 0)
  --sem-audio           não transmite o som do PC (por padrão vai junto, em Opus)
  --audio-dispositivo N captura o som da saída N (padrão: a saída padrão do Windows;
                        a lista aparece em fluxo-bench audio)
  --encoder E           auto, nvenc ou mf (padrão auto)
  --codec C             auto, h264, hevc ou av1 (padrão auto: o melhor que a GPU
                        codifica e o cliente decodifica, na ordem AV1, HEVC, H.264)
  --resolucao R         transmite em outra resolução: 720p, 1080p, 1440p, 4k (2160p)
                        ou o número de linhas; a largura segue a proporção da tela
                        (padrão nativa). O redimensionamento é na GPU, junto da
                        conversão de cor
  --mbps M              bitrate máximo do vídeo (padrão: 20 em 1080p60,
                        proporcional a resolução × fps, de 6 a 100). O bitrate adaptativo
                        começa em até 20 Mbps, sobe até este valor se a rede
                        deixar e desce quando ela reclama
  --mbps-min M          piso do bitrate adaptativo (padrão mbps/8, mínimo 1)
  --bitrate-fixo        desliga o bitrate adaptativo (fica sempre em --mbps)
  --fila-max MS         bitrate adaptativo: atraso de fila tolerado (padrão 15).
                        Menor = menos latência e menos bitrate
  --fps F               máximo de frames por segundo (padrão 60; 30 com o encoder
                        por software); capturas mais rápidas esperam o próximo
                        horário, sem perder a última
  --pacing-mbps P       taxa de envio dos pacotes (padrão 2,5 × mbps; 0 = sem pacing)
  --keyframe-a-cada S   keyframe periódico de segurança, em segundos (padrão 10;
                        0 = só o primeiro). O cliente pede um quando perde algo.
  --refresh N           recupera perdas com uma onda de intra-refresh de N frames
                        em vez de keyframe (NVENC; 0 = sempre keyframe)
  --segundos S          encerra depois de S segundos (padrão 0 = até Ctrl+C)
  --porta-local P       porta UDP de saída (padrão: automática)
  --simular-perda P     TESTE: descarta P% dos pacotes de vídeo antes de enviar
  --fec K               FEC: 1 pacote de paridade a cada K de dados (padrão 10;
                        0 = sem FEC). Reconstrói 1 perda por grupo no cliente
  --simular-banda M     TESTE: simula um link de M Mbps (fila de ~4 frames e
                        descarte quando enche), como um Wi-Fi congestionado
  --simular-atraso MS   TESTE: soma MS de atraso em cada sentido (RTT + 2×MS),
                        como uma conexão pela internet
  --simular-jitter MS   TESTE: atraso extra sorteado entre 0 e MS a cada 20 ms,
                        em cada sentido (os pacotes saem em rajadas, como no Wi-Fi)

Mais espectadores: com o host rodando, digite no terminal (até 4 no total, e cada
um é mais uma cópia do vídeo no seu upload):
  conexao CÓDIGO-DE-CONEXÃO
  destino IP:PORTA XXXX-XXXX

Para testar sem cliente, rode antes em outro terminal (ou outro PC):
  fluxo-bench receber --porta 9000";

// Fora do Windows o host só avisa que não roda: as opções ficam sem uso.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone)]
struct Opts {
    destino: String,
    sinal: Option<String>,
    /// Chave de acesso ao servidor (endereço + segredo que assina os pedidos).
    chave: Option<fluxo_net::ServerKey>,
    /// Sala: quem assiste entra com o código, pelo servidor.
    sala: bool,
    /// Código de conexão do cliente (sem servidor): pareamento + endereço dele.
    conexao: Option<fluxo_net::ConnectionCode>,
    /// Sem áudio, e de qual saída de som capturar (`None`: a padrão).
    sem_audio: bool,
    audio_dispositivo: Option<usize>,
    /// TTL dos pedidos até o furo do cliente chegar (0 = normal).
    ttl_furo: u32,
    monitor: usize,
    encoder: fluxo_encode::BackendChoice,
    /// Codec pedido (`None`: o melhor que os dois lados suportam).
    codec: Option<fluxo_encode::VideoCodec>,
    /// Bitrate máximo (`None`: proporcional a resolução × fps).
    mbps: Option<f64>,
    /// Linhas do vídeo transmitido (`None`: a resolução da tela).
    resolucao: Option<u32>,
    /// Máximo de fps (`None`: 60, ou 30 com o encoder por software).
    fps: Option<u32>,
    pacing_mbps: Option<f64>,
    keyframe_a_cada: f64,
    refresh: u32,
    segundos: f64,
    porta_local: u16,
    simular_perda: f64,
    fec: u8,
    mbps_min: Option<f64>,
    bitrate_fixo: bool,
    fila_max: f64,
    codigo: Option<String>,
    sem_cifra: bool,
    simular_banda: f64,
    simular_atraso: f64,
    simular_jitter: f64,
}

/// `--resolucao`: `nativa` (None), `720p`, `1440p`, `4k`… ou o número de linhas.
fn parse_resolucao(s: &str) -> Result<Option<u32>, String> {
    let s = s.to_ascii_lowercase();
    let linhas = match s.as_str() {
        "nativa" => return Ok(None),
        "4k" | "uhd" => 2160,
        "2k" => 1440,
        _ => s
            .trim_end_matches('p')
            .parse()
            .map_err(|_| format!("resolução inválida: {s} (720p, 1080p, 1440p, 4k, nativa)"))?,
    };
    match linhas {
        144..=4320 => Ok(Some(linhas)),
        _ => Err(format!("resolução fora de 144p..4320p: {s}")),
    }
}

fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        destino: String::new(),
        sinal: None,
        chave: None,
        sala: false,
        conexao: None,
        ttl_furo: fluxo_net::PUNCH_TTL,
        sem_audio: false,
        audio_dispositivo: None,
        monitor: 0,
        encoder: Default::default(),
        codec: None,
        mbps: None,
        resolucao: None,
        fps: None,
        pacing_mbps: None,
        keyframe_a_cada: 10.0,
        refresh: 0,
        segundos: 0.0,
        porta_local: 0,
        simular_perda: 0.0,
        fec: 10,
        mbps_min: None,
        bitrate_fixo: false,
        fila_max: fluxo_net::DEFAULT_QUEUE_LIMIT_MS,
        codigo: None,
        sem_cifra: false,
        simular_banda: 0.0,
        simular_atraso: 0.0,
        simular_jitter: 0.0,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut valor = || it.next().ok_or(format!("{a} precisa de um valor"));
        match a.as_str() {
            "--destino" => o.destino = valor()?.clone(),
            "--sinal" => o.sinal = Some(valor()?.clone()),
            "--chave" => o.chave = Some(fluxo_net::ServerKey::parse(valor()?).map_err(|e| e.to_string())?),
            "--sala" => o.sala = true,
            "--ttl-furo" => o.ttl_furo = valor()?.parse().map_err(|_| "ttl-furo inválido")?,
            "--sem-audio" => o.sem_audio = true,
            "--audio-dispositivo" => {
                o.audio_dispositivo = Some(valor()?.parse().map_err(|_| "audio-dispositivo inválido")?)
            }
            "--conexao" => {
                o.conexao =
                    Some(fluxo_net::ConnectionCode::parse(valor()?).map_err(|e| format!("--conexao: {e}"))?)
            }
            "--monitor" => o.monitor = valor()?.parse().map_err(|_| "monitor inválido")?,
            "--encoder" => o.encoder = valor()?.parse()?,
            "--codec" => {
                o.codec = match valor()?.as_str() {
                    "auto" => None,
                    c => Some(c.parse()?),
                }
            }
            "--mbps" => o.mbps = Some(valor()?.parse().map_err(|_| "mbps inválido")?),
            "--resolucao" => o.resolucao = parse_resolucao(valor()?)?,
            "--fps" => o.fps = Some(valor()?.parse().map_err(|_| "fps inválido")?),
            "--pacing-mbps" => o.pacing_mbps = Some(valor()?.parse().map_err(|_| "pacing-mbps inválido")?),
            "--refresh" => o.refresh = valor()?.parse().map_err(|_| "refresh inválido")?,
            "--fec" => o.fec = valor()?.parse().map_err(|_| "fec inválido (0 a 255)")?,
            "--keyframe-a-cada" => {
                o.keyframe_a_cada = valor()?.parse().map_err(|_| "keyframe-a-cada inválido")?
            }
            "--segundos" => o.segundos = valor()?.parse().map_err(|_| "segundos inválido")?,
            "--porta-local" => o.porta_local = valor()?.parse().map_err(|_| "porta-local inválida")?,
            "--simular-perda" => o.simular_perda = valor()?.parse().map_err(|_| "simular-perda inválido")?,
            "--mbps-min" => o.mbps_min = Some(valor()?.parse().map_err(|_| "mbps-min inválido")?),
            "--bitrate-fixo" => o.bitrate_fixo = true,
            "--fila-max" => o.fila_max = valor()?.parse().map_err(|_| "fila-max inválido")?,
            "--codigo" => o.codigo = Some(valor()?.clone()),
            // Só em compilação de teste: na de produção a cifra não desliga.
            "--sem-cifra" if cfg!(debug_assertions) => o.sem_cifra = true,
            "--simular-banda" => o.simular_banda = valor()?.parse().map_err(|_| "simular-banda inválido")?,
            "--simular-atraso" => {
                o.simular_atraso = valor()?.parse().map_err(|_| "simular-atraso inválido")?
            }
            "--simular-jitter" => {
                o.simular_jitter = valor()?.parse().map_err(|_| "simular-jitter inválido")?
            }
            "--help" | "-h" | "ajuda" => return Err(String::new()),
            outro => return Err(format!("opção desconhecida: {outro}")),
        }
    }
    if let (Some(k), None) = (&o.chave, &o.sinal) {
        o.sinal = Some(k.server().to_string());
    }
    if o.sala {
        if !o.destino.is_empty() || o.conexao.is_some() || o.sem_cifra {
            return Err(
                "--sala acha quem assiste pelo servidor: sem --destino, --conexao ou --sem-cifra".into(),
            );
        }
        if o.sinal.is_none() {
            return Err("--sala precisa da chave do servidor (--chave FSS1-…) ou de --sinal".into());
        }
        let codigo = match &o.codigo {
            Some(c) => fluxo_net::PairingCode::parse(c).map_err(|e| e.to_string())?,
            None => fluxo_net::PairingCode::generate().map_err(|e| e.to_string())?,
        };
        o.codigo = Some(codigo.to_string());
        return Ok(o);
    }
    if let Some(c) = &o.conexao {
        if !o.destino.is_empty() || o.sinal.is_some() || o.codigo.is_some() || o.sem_cifra {
            return Err(
                "--conexao já traz o destino e o código: sem --destino, --sinal, --codigo ou --sem-cifra"
                    .into(),
            );
        }
        o.codigo = Some(c.pairing.to_string());
        return Ok(o);
    }
    match (o.destino.is_empty(), &o.sinal) {
        (true, None) => return Err(
            "falta --destino IP:PORTA (mesma rede), --sinal SERVIDOR ou --conexao CÓDIGO (redes diferentes)"
                .into(),
        ),
        (false, Some(_)) => return Err("use --destino ou --sinal, não os dois".into()),
        (_, Some(_)) if o.sem_cifra => return Err("--sinal precisa do código (sem --sem-cifra)".into()),
        _ => {}
    }
    match (&o.codigo, o.sem_cifra) {
        (None, false) => {
            return Err(
                "falta --codigo XXXX-XXXX (o código aparece no fluxo-client; --sem-cifra só para teste)"
                    .into(),
            )
        }
        (Some(c), false) => {
            fluxo_net::PairingCode::parse(c).map_err(|e| e.to_string())?;
        }
        (_, true) => o.codigo = None,
    }
    Ok(o)
}

fn main() -> ExitCode {
    // Segredos (chave, códigos) pela entrada, fora da linha de comando (fluxo-net, segredos.rs).
    let args = match fluxo_net::args_with_secrets(std::env::args().skip(1).collect()) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("erro: {e}");
            return ExitCode::from(2);
        }
    };
    let opts = match parse_opts(&args) {
        Ok(o) => o,
        Err(e) if e.is_empty() => {
            println!("{AJUDA}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("erro: {e}\n\n{AJUDA}");
            return ExitCode::from(2);
        }
    };
    match run(&opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn run(_: &Opts) -> Result<(), Box<dyn std::error::Error>> {
    Err("o fluxo-host só roda no Windows".into())
}

/// Contadores do relatório por segundo.
#[cfg(windows)]
#[derive(Default)]
struct Contadores {
    frames: u64,
    bytes: u64,
    encode: std::time::Duration,
    /// Capturas que chegaram cedo demais para o limite de fps.
    adiadas: u64,
    /// Saiu um keyframe desde a última vez que o laço olhou.
    keyframe: bool,
    /// Pedidos de recuperação do cliente atendidos (keyframe ou onda).
    pedidos: u64,
    /// Ondas de intra-refresh começadas.
    ondas: u64,
    /// Saiu o primeiro / o último frame de uma onda desde a última vez que o
    /// laço olhou.
    onda_inicio: bool,
    onda_fim: bool,
    /// Mudanças de bitrate feitas pelo controle adaptativo.
    mudancas: u64,
}

/// fps e linhas do vídeo: os pedidos (`--fps`, `--resolucao`) ou os padrões.
/// No encoder por `software` os padrões são mais baixos: 30 fps e no máximo
/// 1080 linhas (a CPU faz um frame 1080p em ~7,5 ms com 8 núcleos, mas uma
/// VM com poucos núcleos não; e 4K na CPU não dá tempo real).
#[cfg_attr(not(windows), allow(dead_code))]
fn limites(fps: Option<u32>, linhas: Option<u32>, tela: (u32, u32), software: bool) -> (u32, Option<u32>) {
    match software {
        false => (fps.unwrap_or(60), linhas),
        true => (fps.unwrap_or(30), linhas.or((tela.1 > 1080).then_some(1080))),
    }
}

/// Codecs a tentar, na ordem: o `pedido` (se o cliente decodifica) ou, em
/// automático, todos os que o cliente decodifica, do melhor ao mais
/// compatível. `cliente`: máscara recebida; `None` = só H.264.
#[cfg_attr(not(windows), allow(dead_code))]
fn codecs_candidatos(
    pedido: Option<fluxo_encode::VideoCodec>,
    cliente: Option<u8>,
) -> Result<Vec<fluxo_encode::VideoCodec>, String> {
    use fluxo_encode::VideoCodec;

    let mascara = cliente.unwrap_or(VideoCodec::H264.bit());
    let decodifica = |c: VideoCodec| mascara & c.bit() != 0;
    match pedido {
        Some(c) if decodifica(c) => Ok(vec![c]),
        Some(c) => {
            let tem: Vec<&str> = VideoCodec::PREFERENCE
                .into_iter()
                .filter(|&c| decodifica(c))
                .map(VideoCodec::name)
                .collect();
            Err(format!(
                "o cliente não decodifica {} (decodifica: {}); use --codec auto",
                c.name(),
                tem.join(", ")
            ))
        }
        None => Ok(VideoCodec::PREFERENCE
            .into_iter()
            .filter(|&c| decodifica(c))
            .collect()),
    }
}

/// Abre o encoder com o primeiro codec de [`codecs_candidatos`] que esta
/// GPU codifica.
#[cfg(windows)]
fn abrir_com_codec(
    device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
    pedido: Option<fluxo_encode::VideoCodec>,
    cliente: Option<u8>,
    config: impl Fn(fluxo_encode::VideoCodec) -> fluxo_encode::EncoderConfig,
) -> Result<(fluxo_encode::VideoCodec, fluxo_encode::OpenedEncoder), Box<dyn std::error::Error>> {
    let candidatos = codecs_candidatos(pedido, cliente)?;
    if cliente.is_none() {
        println!("(o cliente não disse que codecs decodifica; usando H.264)");
    }
    let ultimo = candidatos.len() - 1;
    for (i, codec) in candidatos.into_iter().enumerate() {
        match fluxo_encode::open_encoder(device, &config(codec)) {
            Ok(aberto) => return Ok((codec, aberto)),
            Err(e) if i < ultimo => println!("(sem {} nesta GPU: {e})", codec.name()),
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!("a lista nunca é vazia (o cliente sempre decodifica H.264) e o último devolve")
}

/// Codifica `textura` e enfileira o resultado na rede. Devolve o tamanho
/// novo se a resolução mudou (o encoder precisa ser recriado).
#[cfg(windows)]
fn codificar(
    enc: &mut dyn fluxo_encode::Encoder,
    codec: fluxo_encode::VideoCodec,
    tx: &espectadores::Espectadores,
    textura: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    capturado_us: u32,
    pedir_keyframe: bool,
    c: &mut Contadores,
) -> Result<Option<(u32, u32)>, Box<dyn std::error::Error>> {
    use fluxo_encode::EncodeError;

    let t = std::time::Instant::now();
    let pacotes = match enc.encode(textura, pedir_keyframe) {
        Ok(p) => p,
        Err(EncodeError::SizeMismatch { got, .. }) => return Ok(Some(got)),
        Err(e) => return Err(e.into()),
    };
    c.encode += t.elapsed();
    for p in pacotes {
        c.keyframe |= p.keyframe;
        c.onda_inicio |= p.refresh_start;
        c.onda_fim |= p.refresh_end;
        c.ondas += p.refresh_start as u64;
        c.frames += 1;
        c.bytes += p.data.len() as u64;
        let flags = [
            (p.keyframe, fluxo_net::FLAG_KEYFRAME),
            (p.refresh_start, fluxo_net::FLAG_REFRESH_START),
            (p.refresh_end, fluxo_net::FLAG_REFRESH_END),
        ]
        .iter()
        .filter(|(sim, _)| *sim)
        .fold(codec.flags(), |f, (_, bit)| f | bit);
        tx.send_frame_with_flags(p.data, flags, capturado_us)?;
    }
    Ok(None)
}

/// Thread de áudio: captura o que o PC toca, codifica em Opus (quadros de
/// 10 ms) e manda a todos os espectadores, até `parar`. Devolve o resumo.
#[cfg(windows)]
fn transmitir_audio(
    tx: &espectadores::Espectadores,
    dispositivo: Option<usize>,
    parar: &std::sync::atomic::AtomicBool,
    ligado: &std::sync::atomic::AtomicBool,
) -> Result<String, fluxo_audio::AudioError> {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use fluxo_audio::{output_devices, Encoder, LoopbackCapture, DEFAULT_BITRATE};

    let nome = output_devices()
        .ok()
        .and_then(|l| {
            l.into_iter()
                .find(|d| dispositivo.map_or(d.default, |i| d.index == i))
        })
        .map(|d| d.name)
        .unwrap_or_default();
    let mut cap = LoopbackCapture::new(dispositivo)?;
    // FEC para ~5% de perda: um quadro perdido é refeito pelo seguinte.
    let mut enc = Encoder::new(DEFAULT_BITRATE, 5)?;
    println!(
        "  áudio: {nome} (Opus {} kbps, quadros de 10 ms)",
        DEFAULT_BITRATE / 1000
    );
    let (mut seq, mut bytes) = (0u32, 0u64);
    let inicio = Instant::now();
    while !parar.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(5));
        for mut q in cap.read_frames()? {
            if !ligado.load(Ordering::Relaxed) {
                q.fill(0.0);
            }
            let pacote = enc.encode(&q)?;
            bytes += pacote.len() as u64;
            // A captura deste quadro terminou agora; a primeira amostra, 10 ms antes.
            tx.send_audio(seq, fluxo_net::clock_us().wrapping_sub(10_000), &pacote);
            seq = seq.wrapping_add(1);
        }
    }
    let t = inicio.elapsed().as_secs_f64();
    Ok(format!(
        "áudio: {seq} quadros, {:.0} kbps médio, {:.1} s vieram da placa (o resto foi silêncio)",
        bytes as f64 * 8.0 / t.max(0.001) / 1000.0,
        cap.device_seconds()
    ))
}

/// Copia `origem` (textura da captura, que só vale enquanto o frame existir)
/// para `destino`, recriando-a se o tamanho mudou. Cópia na GPU: microssegundos.
#[cfg(windows)]
fn guardar_copia(
    device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
    context: &windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    origem: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    destino: &mut Option<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>,
) -> windows::core::Result<()> {
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    };

    let mut d = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: GetDesc só escreve em `d`; CreateTexture2D com descritor
    // derivado da origem; CopyResource entre texturas de mesmo tamanho e formato.
    unsafe {
        origem.GetDesc(&mut d);
        let serve = destino.as_ref().is_some_and(|t| {
            let mut a = D3D11_TEXTURE2D_DESC::default();
            t.GetDesc(&mut a);
            (a.Width, a.Height, a.Format) == (d.Width, d.Height, d.Format)
        });
        if !serve {
            let desc = D3D11_TEXTURE2D_DESC {
                MipLevels: 1,
                ArraySize: 1,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
                ..d
            };
            let mut nova = None;
            device.CreateTexture2D(&desc, None, Some(&mut nova))?;
            *destino = nova;
        }
        if let Some(t) = destino.as_ref() {
            context.CopyResource(t, origem);
        }
    }
    Ok(())
}

/// Tela e PC acordados enquanto o host roda. Com o monitor apagado por
/// inatividade (alguém vendo um filme sem mexer no mouse, ou longe do PC), o
/// Windows para de entregar quadros à captura, e quem entra fica sem imagem
/// (visto num teste da sala: 19 pedidos de keyframe sem resposta). Vale para
/// esta thread, que é a do laço do vídeo, até o processo terminar.
#[cfg(windows)]
fn manter_tela_ligada() {
    #[link(name = "kernel32")]
    extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x1;
    const ES_DISPLAY_REQUIRED: u32 = 0x2;
    // SAFETY: só flags; sem ponteiros.
    unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED) };
}

#[cfg(windows)]
fn run(o: &Opts) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, Instant};

    use fluxo_capture::{enable_dpi_awareness, CaptureError, DesktopCapturer};
    use fluxo_encode::{
        open_encoder, scaled_size, suggested_bitrate_kbps, Backend, BackendChoice, EncoderConfig,
    };
    use fluxo_net::{clock_us, BitrateController, CursorShape, PairingCode, SenderConfig, VideoSender};

    use crate::limite::FpsLimiter;

    enable_dpi_awareness();
    manter_tela_ligada();
    let mut cap = DesktopCapturer::new(o.monitor)?;
    // Cópias: o frame capturado prende o capturador enquanto existe.
    let mut device = cap.d3d_device().clone();
    let mut context = cap.d3d_context().clone();
    let m = cap.monitor().clone();
    // fps e linhas do vídeo: os pedidos, ou os padrões do encoder (mais
    // baixos no por software). Em Cell porque, no automático, só se sabe que
    // caiu no software depois de abrir.
    let software = |sim: bool| limites(o.fps, o.resolucao, (m.width, m.height), sim);
    let pedido_software = o.encoder == BackendChoice::Only(Backend::Software);
    let (fps_inicial, linhas_inicial) = software(pedido_software);
    let fps_c = std::cell::Cell::new(fps_inicial);
    let linhas_c = std::cell::Cell::new(linhas_inicial);
    // Tamanho do vídeo para uma captura w×h (--resolucao, ou o mesmo).
    let tamanho = |w: u32, h: u32| linhas_c.get().map_or((w, h), |alt| scaled_size((w, h), alt));
    let (vw, vh) = tamanho(m.width, m.height);
    let mbps = o
        .mbps
        .unwrap_or(suggested_bitrate_kbps(vw, vh, fps_c.get()) as f64 / 1000.0);
    let bitrate_bps = (mbps * 1e6).round() as u64;
    let config_para = |w: u32, h: u32, codec| {
        let (vw, vh) = tamanho(w, h);
        EncoderConfig {
            input_size: Some((w, h)),
            fps: fps_c.get(),
            bitrate_kbps: (bitrate_bps / 1000) as u32,
            codec,
            backend: o.encoder,
            intra_refresh_frames: o.refresh,
            ..EncoderConfig::new(vw, vh)
        }
    };

    let net_cfg = SenderConfig {
        pacing_bps: o
            .pacing_mbps
            .map(|p| (p * 1e6).round() as u64)
            .unwrap_or(SenderConfig::for_bitrate(bitrate_bps).pacing_bps),
        simulated_loss: o.simular_perda / 100.0,
        fec_group: o.fec,
        simulated_bandwidth_bps: (o.simular_banda * 1e6).round() as u64,
        simulated_delay: Duration::from_secs_f64(o.simular_atraso.max(0.0) / 1000.0),
        simulated_jitter: Duration::from_secs_f64(o.simular_jitter.max(0.0) / 1000.0),
        // Já conferido em parse_opts; `None` só com --sem-cifra.
        pairing_code: o.codigo.as_deref().and_then(|c| PairingCode::parse(c).ok()),
        // Tempo para abrir o cliente e digitar o código, se preciso; com
        // código de conexão, também para o código de resposta ir e voltar
        // pelo chat.
        pairing_timeout: Duration::from_secs(if o.conexao.is_some() { 300 } else { 30 }),
        signal_key: o.chave.clone(),
        ..SenderConfig::for_bitrate(bitrate_bps)
    };
    let servidor = o
        .sinal
        .as_deref()
        .map(fluxo_net::resolve_signal_server)
        .transpose()?;
    match (&net_cfg.pairing_code, servidor, &o.conexao) {
        (Some(c), Some(_), _) if o.sala => {
            println!("Código da sala: {c}");
            println!(
                "  Quem vai assistir entra com ele, sem responder nada (até {} pessoas).",
                espectadores::MAX_ESPECTADORES
            );
            println!("  No terminal: fluxo-client --sala {c} --chave <a mesma chave do servidor>");
            println!("Esperando a primeira pessoa entrar…");
        }
        (Some(c), Some(s), _) => {
            println!("Procurando o cliente pelo servidor de sinalização {s} (código {c})…")
        }
        (Some(c), None, Some(conexao)) => {
            let lista: Vec<String> = conexao.candidates().iter().map(|a| a.to_string()).collect();
            println!("Conectando ao cliente em {} (código {c})…", lista.join(" e "))
        }
        (Some(c), None, None) => println!("Pareando com {} (código {c})…", o.destino),
        (None, ..) => println!("TESTE: sem pareamento nem cifra (--sem-cifra)"),
    }
    // Bitrate adaptativo: o controlador decide, o encoder e o pacing seguem
    // (o pacing só se não foi fixado à mão).
    let max_kbps = (bitrate_bps / 1000) as u32;
    let min_kbps = o
        .mbps_min
        .map(|m| (m * 1000.0) as u32)
        .unwrap_or(max_kbps / 8)
        .max(1000);
    let mut adaptativo = !o.bitrate_fixo;
    let novo_abr = || BitrateController::new(min_kbps, max_kbps, Instant::now()).with_queue_limit(o.fila_max);
    let pacing_segue_bitrate = o.pacing_mbps.is_none();
    let pacing_mbps = net_cfg.pacing_bps as f64 / 1e6;
    // Para quem entrar depois: cada um pareia por conta própria, com tempo
    // para o código de resposta ir e voltar pelo chat.
    let cfg_novos = SenderConfig {
        on_pairing: Some(fluxo_net::PairingObserver::print_to_terminal()),
        pairing_timeout: Duration::from_secs(300),
        ..net_cfg.clone()
    };
    // Na sala, cada um que entra pareia num socket próprio, um de cada vez;
    // sem prazo total, então nada de observador imprimindo a espera.
    // TTL baixo até o furo de quem entra chegar, como no código de conexão
    // (é o que deixa passar pela internet de celular).
    let cfg_sala = SenderConfig {
        on_pairing: None,
        pairing_timeout: espectadores::PRAZO_SALA,
        punch_ttl: (o.ttl_furo > 0).then_some(o.ttl_furo),
        ..net_cfg.clone()
    };
    // Quem assiste ou está pareando (para o limite) e o número do próximo.
    let ocupados = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let numeros = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let (novos_tx, novos) = std::sync::mpsc::channel();
    let (tx, primeiro_numero, do_cliente) = match (o.sala, servidor) {
        (true, Some(s)) => {
            let (ocupados, numeros, novos_tx) = (ocupados.clone(), numeros.clone(), novos_tx.clone());
            std::thread::Builder::new()
                .name("fluxo-sala".into())
                .spawn(move || espectadores::atender_sala(s, cfg_sala, numeros, novos_tx, ocupados))?;
            let n = novos.recv().map_err(|_| "a sala fechou")?;
            (n.tx, n.numero, Some(n.codecs))
        }
        _ => {
            let tx = match (servidor, &o.conexao) {
                (Some(s), _) => VideoSender::connect_via_signal(("0.0.0.0", o.porta_local), s, net_cfg)?,
                (None, Some(c)) => {
                    // Os endereços públicos valem só para estes sockets: os mesmos levam o vídeo.
                    let internet = fluxo_net::Internet::open(o.porta_local, false)?;
                    println!("\nCódigo de resposta: {}", internet.reply_code(c)?);
                    println!("  Mande para quem vai assistir: ele cola no campo de resposta (ou no terminal do fluxo-client).");
                    println!("  Esperando até 5 min…");
                    for linha in internet.summary() {
                        println!("  {linha}");
                    }
                    // Porta aberta por UPnP do outro lado: direto, sem esperar o furo.
                    let ttl = (o.ttl_furo > 0 && !c.open).then_some(o.ttl_furo);
                    match (ttl, c.open) {
                        (_, true) => {
                            println!("  O cliente abriu a porta no roteador (UPnP): conectando direto")
                        }
                        (Some(t), _) => {
                            println!(
                                "  Pedidos IPv4 com TTL {t} até o furo do outro lado chegar (--ttl-furo)"
                            )
                        }
                        (None, _) => {}
                    }
                    println!();
                    let net_cfg = SenderConfig {
                        on_pairing: Some(fluxo_net::PairingObserver::print_to_terminal()),
                        punch_ttl: ttl,
                        ..net_cfg
                    };
                    VideoSender::connect_with_sockets(internet.v4, internet.v6, c.candidates(), net_cfg)?
                }
                (None, None) => {
                    VideoSender::connect(("0.0.0.0", o.porta_local), o.destino.as_str(), net_cfg)?
                }
            };
            ocupados.store(1, std::sync::atomic::Ordering::Relaxed);
            numeros.store(1, std::sync::atomic::Ordering::Relaxed);
            // Codec: o cliente diz o que decodifica (junto com os pings, a
            // cada 500 ms); usamos o melhor que esta GPU também codifica.
            let do_cliente = tx.wait_client_codecs(Duration::from_millis(1500));
            (tx, 1, do_cliente)
        }
    };
    // O encoder por software só faz H.264: nem tenta os outros.
    let codec_pedido = o
        .codec
        .or(pedido_software.then_some(fluxo_encode::VideoCodec::H264));
    let (mut codec, aberto) = abrir_com_codec(&device, codec_pedido, do_cliente, |c| {
        config_para(m.width, m.height, c)
    })?;
    for (b, motivo) in &aberto.skipped {
        println!("(pulando {b}: {motivo})");
    }
    let mut enc = aberto.encoder;
    // Caiu no encoder por software sem ter pedido: reabre com os padrões dele.
    if enc.backend() == Backend::Software && !pedido_software {
        let (f, l) = software(true);
        if (f, l) != (fps_c.get(), linhas_c.get()) {
            fps_c.set(f);
            linhas_c.set(l);
            let cfg = EncoderConfig {
                backend: BackendChoice::Only(Backend::Software),
                ..config_para(m.width, m.height, codec)
            };
            enc = open_encoder(&device, &cfg)?.encoder;
        }
    }
    if enc.backend() == Backend::Software && (o.fps.is_none() || o.resolucao.is_none()) {
        println!(
            "(encoder por software: até {} fps; mude com --fps e --resolucao)",
            fps_c.get()
        );
    }
    let mut fps = fps_c.get();
    let (vw, vh) = tamanho(m.width, m.height);
    let (local, destino) = (tx.local_addr(), tx.peer_addr());
    // Botões da janela (trocar de tela, som, pausa), também pela entrada padrão.
    let (controles_tx, controles) = std::sync::mpsc::channel();
    // Quem assiste. Mais gente pode entrar pela entrada padrão (a janela manda
    // os códigos por ela), cada um pareando em thread própria.
    let primeiro = tx;
    let tx = std::sync::Arc::new(espectadores::Espectadores::default());
    let quantos = tx.adicionar(
        primeiro_numero,
        primeiro,
        do_cliente.unwrap_or(fluxo_encode::VideoCodec::H264.bit()),
        adaptativo.then(novo_abr),
    );
    println!("Espectador {primeiro_numero} conectou ({quantos} no total)");
    {
        let (ocupados, numeros, ttl) = (ocupados.clone(), numeros.clone(), o.ttl_furo);
        std::thread::Builder::new()
            .name("fluxo-pedidos".into())
            .spawn(move || {
                espectadores::atender_pedidos(cfg_novos, ttl, numeros, novos_tx, ocupados, controles_tx)
            })?;
    }
    // Áudio numa thread própria, para todos (pacotes à parte).
    let parar_audio = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Som ligado ou não (botão da janela). A thread roda mesmo desligado, mandando
    // silêncio: ligar de novo é na hora, sem reabrir nada.
    let som_ligado = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(!o.sem_audio));
    let audio = {
        let (tx, parar, disp, som) = (
            tx.clone(),
            parar_audio.clone(),
            o.audio_dispositivo,
            som_ligado.clone(),
        );
        Some(
            std::thread::Builder::new()
                .name("fluxo-audio".into())
                .spawn(move || transmitir_audio(&tx, disp, &parar, &som))?,
        )
    };

    println!(
        "Transmitindo monitor {} ({}x{}{}) de {} para {}\n  {}\n  {:.1} Mbps, até {} fps, pacing {:.0} Mbps, FEC {}, keyframe de segurança a cada {} s{}",
        m.index,
        m.width,
        m.height,
        match (vw, vh) == (m.width, m.height) {
            true => String::new(),
            false => format!(" → {vw}x{vh}"),
        },
        local,
        destino,
        enc.name(),
        mbps,
        fps,
        pacing_mbps,
        match o.fec {
            0 => "desligado".to_string(),
            k => format!("1 paridade a cada {k} pacotes (+{:.0}%)", 100.0 / k as f64),
        },
        o.keyframe_a_cada,
        match o.simular_perda > 0.0 {
            true => format!("\n  TESTE: descartando {}% dos pacotes de vídeo", o.simular_perda),
            false => String::new(),
        }
    );
    if tx.stats().encrypted {
        println!("  pareado: só esse cliente recebe, tudo cifrado (ChaCha20-Poly1305)");
    }
    match adaptativo {
        true => println!(
            "  bitrate adaptativo: entre {:.1} e {:.1} Mbps, fila máxima {} ms",
            min_kbps as f64 / 1000.0,
            max_kbps as f64 / 1000.0,
            o.fila_max
        ),
        false => println!("  bitrate fixo (--bitrate-fixo)"),
    }
    if o.simular_banda > 0.0 {
        println!("  TESTE: simulando um link de {} Mbps", o.simular_banda);
    }
    if o.simular_atraso > 0.0 || o.simular_jitter > 0.0 {
        println!(
            "  TESTE: atraso de {} ms + jitter de 0–{} ms em cada sentido",
            o.simular_atraso, o.simular_jitter
        );
    }
    if o.segundos <= 0.0 {
        println!("  Ctrl+C para encerrar.");
    }

    let duracao = (o.segundos > 0.0).then(|| Duration::from_secs_f64(o.segundos));
    let keyframe_a_cada = (o.keyframe_a_cada > 0.0).then(|| Duration::from_secs_f64(o.keyframe_a_cada));
    let inicio = Instant::now();
    let mut ultimo_keyframe = inicio;
    let mut proximo_relatorio = Duration::from_secs(1);
    let mut limite = FpsLimiter::new(fps);
    // Frame que chegou cedo demais: cópia na GPU + momento da captura.
    let mut pendente: Option<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D> = None;
    let mut pendente_us = 0u32;
    // Cópia da última imagem enviada: com a tela parada, é ela que vira o
    // keyframe que o cliente pedir.
    let mut ultima: Option<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D> = None;
    let mut seg = Contadores::default();
    let mut total = Contadores::default();
    // Menor bitrate alvo a que o adaptativo chegou (para o resumo).
    let mut alvo_min = enc.bitrate_kbps();
    // Onda de intra-refresh em andamento: com a tela parada, a última imagem
    // é recodificada a cada intervalo de frame até a onda terminar.
    let mut onda_ativa = false;
    let mut ultimo_encode = Instant::now();
    // Tamanho atual da captura (para reabrir o encoder).
    let mut captura = (m.width, m.height);
    let mut intervalo_frame = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
    // Vídeo pausado pela janela: a última imagem fica na tela de quem assiste.
    // IPs de espectadores expulsos (não entram de novo nesta transmissão).
    let mut bloqueados: std::collections::HashSet<std::net::IpAddr> = std::collections::HashSet::new();
    let mut pausado = false;

    while !duracao.is_some_and(|d| inicio.elapsed() >= d) {
        // Botões da janela. Nada aqui derruba quem assiste: trocar de tela ou
        // de qualidade reabre a captura ou o encoder, e o encoder novo começa
        // com um keyframe (a imagem anterior fica na tela até ele chegar).
        while let Ok(c) = controles.try_recv() {
            use espectadores::Controle;
            let mut reabrir = false;
            match c {
                Controle::Tela(n) if n == cap.monitor().index => {
                    println!("Tela: monitor {} ({}x{})", n + 1, captura.0, captura.1)
                }
                Controle::Tela(n) => {
                    // A captura antiga solta a tela antes (o Windows não deixa
                    // duplicar a mesma duas vezes); se a nova falhar, a antiga
                    // se recria sozinha no próximo frame e a transmissão segue.
                    cap.release_duplication();
                    match DesktopCapturer::new(n) {
                        Ok(nova) => {
                            cap = nova;
                            device = cap.d3d_device().clone();
                            context = cap.d3d_context().clone();
                            let novo = cap.monitor();
                            captura = (novo.width, novo.height);
                            println!("Tela: monitor {} ({}x{})", n + 1, novo.width, novo.height);
                            reabrir = true;
                        }
                        Err(e) => println!("Não consegui trocar de tela: {e}"),
                    }
                }
                Controle::Som(sim) => {
                    som_ligado.store(sim, std::sync::atomic::Ordering::Relaxed);
                    println!("Som: {}", if sim { "ligado" } else { "desligado" });
                }
                Controle::Pausa(sim) => {
                    pausado = sim;
                    println!("Vídeo: {}", if sim { "pausado" } else { "transmitindo" });
                }
                Controle::Resolucao(linhas) => {
                    linhas_c.set(linhas);
                    reabrir = true;
                }
                Controle::Expulsar(n) => match tx.expulsar(n) {
                    Some((ip, ficaram)) => {
                        bloqueados.insert(ip);
                        ocupados.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                        println!("Espectador {n} expulso ({ficaram} no total)");
                    }
                    None => println!("(não há espectador {n})"),
                },
                Controle::Fps(f) => {
                    fps_c.set(f);
                    fps = f;
                    intervalo_frame = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
                    reabrir = true;
                }
            }
            if reabrir {
                enc = open_encoder(&device, &config_para(captura.0, captura.1, codec))?.encoder;
                pendente = None;
                ultima = None;
                limite = FpsLimiter::new(fps);
                onda_ativa = false;
                let (vw, vh) = tamanho(captura.0, captura.1);
                println!("Qualidade: {vw}x{vh}, até {fps} fps");
            }
        }
        // Com um pendente, espera no máximo até o horário dele; sem pendente,
        // 10 ms: com a tela parada, um pedido de keyframe do cliente é
        // atendido em até ~10 ms (re-enviando a última imagem).
        let espera_ms = match limite.deadline() {
            Some(d) => d
                .saturating_duration_since(Instant::now())
                .as_micros()
                .div_ceil(1000) as u32,
            None => 10,
        };
        let mut frame = match cap.next_frame(espera_ms) {
            Ok(f) => f,
            Err(CaptureError::AccessLost) => {
                std::thread::sleep(Duration::from_millis(50));
                None
            }
            Err(e) => return Err(e.into()),
        };
        // Cursor: vai à parte, na hora (a imagem capturada não o tem), e o
        // cliente desenha por cima do vídeo. A forma antes da posição.
        if let Some(f) = frame.as_mut() {
            if let Some(s) = f.take_pointer_shape() {
                tx.set_cursor_shape(espectadores::forma_no_video(
                    CursorShape {
                        width: s.width as u16,
                        height: s.height as u16,
                        hot_x: s.hot_x as u16,
                        hot_y: s.hot_y as u16,
                        bgra: s.bgra,
                    },
                    captura,
                    tamanho(captura.0, captura.1),
                ));
            }
            if let Some(p) = f.info().pointer {
                let (x, y) = espectadores::ponto_no_video((p.x, p.y), captura, tamanho(captura.0, captura.1));
                tx.send_cursor_position(x, y, p.visible);
            }
        }

        // Quem pareou agora: entra na lista e recebe um keyframe. Se não
        // decodifica o codec atual, o encoder muda para um que sirva a todos
        // (quem já assistia troca de decoder sozinho, no keyframe).
        let mut keyframe_novo = false;
        while let Ok(n) = novos.try_recv() {
            // Expulso volta com o código? Do mesmo IP, não entra mais nesta transmissão.
            if bloqueados.contains(&n.tx.peer_addr().ip()) {
                println!("Espectador {} não conectou: foi expulso antes", n.numero);
                ocupados.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                std::thread::spawn(move || drop(n.tx));
                continue;
            }
            let comum = tx.codecs_comuns(n.codecs);
            if codec_pedido.is_some_and(|c| comum & c.bit() == 0) {
                println!(
                    "Espectador {} não conectou: não decodifica o codec pedido",
                    n.numero
                );
                ocupados.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                std::thread::spawn(move || drop(n.tx));
                continue;
            }
            if comum & codec.bit() == 0 {
                let (novo, aberto) = abrir_com_codec(&device, codec_pedido, Some(comum), |c| {
                    config_para(captura.0, captura.1, c)
                })?;
                println!(
                    "(trocando para {}: nem todos decodificam {})",
                    novo.name(),
                    codec.name()
                );
                codec = novo;
                enc = aberto.encoder;
                onda_ativa = false;
            }
            let quantos = tx.adicionar(n.numero, n.tx, n.codecs, adaptativo.then(novo_abr));
            println!("Espectador {} conectou ({quantos} no total)", n.numero);
            keyframe_novo = true;
        }
        if inicio.elapsed() >= proximo_relatorio {
            for (numero, quantos) in tx.remover_silenciosos(espectadores::SILENCIO_MAXIMO) {
                println!("Espectador {numero} saiu ({quantos} no total)");
                ocupados.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
        }

        // Relatórios dos clientes → bitrate adaptativo: um encoder só, então
        // vale o do espectador em pior situação.
        let queda = tx.processar_relatorios(Instant::now());
        if let Some(kbps) = tx.alvo_kbps().filter(|k| *k != enc.bitrate_kbps()) {
            match enc.set_bitrate(kbps) {
                Ok(()) => {
                    seg.mudancas += 1;
                    if let Some(q) = queda {
                        println!("  ↓ bitrate {:.1} Mbps ({q})", kbps as f64 / 1000.0);
                    }
                }
                Err(e) => {
                    println!("(o encoder não aceitou mudar o bitrate: {e}; bitrate fixo daqui em diante)");
                    tx.desligar_adaptativo();
                    adaptativo = false;
                }
            }
        }
        if pacing_segue_bitrate {
            // Também para quem acabou de entrar.
            tx.set_pacing_bps(enc.bitrate_kbps() as u64 * 1000 * 5 / 2);
        }

        // Recuperação pedida pelo cliente: keyframe se ele precisa de um (sem
        // imagem, decoder falhou) ou se o encoder não tem intra-refresh;
        // senão, uma onda de intra-refresh. O keyframe periódico é a rede de
        // segurança, caso um pedido se perca.
        let pedido_keyframe = tx.take_keyframe_request() | keyframe_novo;
        let pedido_refresh = tx.take_refresh_request();
        let onda = pedido_refresh && !pedido_keyframe && enc.start_intra_refresh();
        let pedido = pedido_keyframe || pedido_refresh;
        seg.pedidos += pedido as u64;
        let mut pedir_keyframe = pedido_keyframe
            || (pedido_refresh && !onda)
            || keyframe_a_cada.is_some_and(|p| ultimo_keyframe.elapsed() >= p);
        let frames_antes = seg.frames;
        let mut recriar = None;
        if let Some(frame) = frame.filter(|f| f.info().desktop_updated && !pausado) {
            // Momento da captura, no relógio que o cliente usa para medir o trânsito.
            let capturado_us = clock_us();
            if limite.on_frame(Instant::now()) {
                recriar = codificar(
                    &mut *enc,
                    codec,
                    &tx,
                    frame.texture(),
                    capturado_us,
                    pedir_keyframe,
                    &mut seg,
                )?;
                pedir_keyframe = false;
                guardar_copia(&device, &context, frame.texture(), &mut ultima)?;
            } else {
                guardar_copia(&device, &context, frame.texture(), &mut pendente)?;
                pendente_us = capturado_us;
                seg.adiadas += 1;
            }
            drop(frame); // devolve o frame ao Windows o quanto antes
        }
        if recriar.is_none() && !pausado && limite.on_tick(Instant::now()) {
            if let Some(t) = &pendente {
                recriar = codificar(&mut *enc, codec, &tx, t, pendente_us, pedir_keyframe, &mut seg)?;
                pedir_keyframe = false;
                guardar_copia(&device, &context, t, &mut ultima)?;
            }
        }
        // Tela parada, sem frame novo para levar a recuperação: recodifica a
        // última imagem enviada — na hora, se o cliente acabou de pedir; a
        // cada intervalo de frame, enquanto uma onda não termina.
        let continuar_onda = onda_ativa && ultimo_encode.elapsed() >= intervalo_frame;
        if seg.frames == frames_antes && recriar.is_none() && (pedido || continuar_onda) {
            if let Some(t) = &ultima {
                recriar = codificar(&mut *enc, codec, &tx, t, clock_us(), pedir_keyframe, &mut seg)?;
            }
        }
        if seg.frames != frames_antes {
            ultimo_encode = Instant::now();
        }
        if std::mem::take(&mut seg.keyframe) {
            ultimo_keyframe = Instant::now();
            onda_ativa = false;
        }
        if std::mem::take(&mut seg.onda_inicio) {
            onda_ativa = true;
        }
        if std::mem::take(&mut seg.onda_fim) {
            onda_ativa = false;
        }
        if let Some((w, h)) = recriar {
            println!("(resolução mudou para {w}x{h}; recriando o encoder, que começa com keyframe)");
            captura = (w, h);
            enc = open_encoder(&device, &config_para(w, h, codec))?.encoder;
            pendente = None;
            ultima = None;
            limite = FpsLimiter::new(fps);
            onda_ativa = false;
        }

        if inicio.elapsed() >= proximo_relatorio {
            let s = tx.stats();
            println!(
                "  {:>4} s: {} fps, {:.1} Mbps (alvo {:.1}), perda {}, fila {:.0} ms, encode {:.1} ms, envio {:.1} ms (fila de envio {}), RTT {}, {} capturas adiadas, {} pedidos de recuperação, {} espectadores",
                proximo_relatorio.as_secs(),
                seg.frames,
                seg.bytes as f64 * 8.0 / 1e6,
                enc.bitrate_kbps() as f64 / 1000.0,
                s.last_loss.map_or("—".to_string(), |p| format!("{:.1}%", p * 100.0)),
                s.queue_delay_ms,
                seg.encode.as_secs_f64() * 1000.0 / seg.frames.max(1) as f64,
                s.last_frame_send_ms,
                s.queue_len,
                match s.rtt.samples {
                    0 => "— (cliente não respondeu)".to_string(),
                    _ => format!("{:.2} ms", s.rtt.smoothed_ms),
                },
                seg.adiadas,
                seg.pedidos,
                tx.quantos(),
            );
            total.mudancas += seg.mudancas;
            alvo_min = alvo_min.min(enc.bitrate_kbps());
            total.frames += seg.frames;
            total.bytes += seg.bytes;
            total.adiadas += seg.adiadas;
            total.pedidos += seg.pedidos;
            total.ondas += seg.ondas;
            seg = Contadores::default();
            proximo_relatorio += Duration::from_secs(1);
        }
    }

    total.frames += seg.frames;
    total.bytes += seg.bytes;
    total.adiadas += seg.adiadas;
    total.pedidos += seg.pedidos;
    total.ondas += seg.ondas;
    total.mudancas += seg.mudancas;
    alvo_min = alvo_min.min(enc.bitrate_kbps());
    let alvo_final = enc.bitrate_kbps();
    drop(enc);
    parar_audio.store(true, std::sync::atomic::Ordering::Relaxed);
    let resumo_audio = match audio.map(|t| t.join()) {
        Some(Ok(Ok(r))) => r,
        Some(Ok(Err(e))) => format!("áudio desligado: {e}"),
        Some(Err(_)) => "áudio: a thread terminou com pânico".to_string(),
        None => "áudio desligado (--sem-audio)".to_string(),
    };
    let s = tx.stats();
    drop(tx); // espera as filas de envio esvaziarem
    let t = inicio.elapsed().as_secs_f64();
    println!("\nResultado");
    println!(
        "  frames: {} ({:.1} fps médio, limite {}), {:.2} Mbps médio; {} capturas adiadas pelo limite",
        total.frames,
        total.frames as f64 / t,
        fps,
        total.bytes as f64 * 8.0 / t / 1e6,
        total.adiadas
    );
    println!(
        "  rede: {} pacotes, {} erros de envio, maior tempo de envio de um frame {:.1} ms",
        s.packets_sent, s.send_errors, s.max_frame_send_ms
    );
    println!(
        "  pedidos de recuperação do cliente: {} de keyframe e {} de limpeza após perda recebidos, {} atendidos ({} ondas de intra-refresh){}",
        s.keyframe_requests,
        s.refresh_requests,
        total.pedidos,
        total.ondas,
        match s.simulated_drops {
            0 => String::new(),
            n => format!("; {n} pacotes descartados pela perda simulada"),
        }
    );
    if adaptativo {
        println!(
            "  bitrate adaptativo: {} mudanças, chegou a {:.1} Mbps, terminou em {:.1} Mbps ({} relatórios do cliente)",
            total.mudancas,
            alvo_min as f64 / 1000.0,
            alvo_final as f64 / 1000.0,
            s.feedbacks_received
        );
    }
    println!(
        "  RTT: {:.2} ms suavizado, {:.2} ms mínimo ({} medições)",
        s.rtt.smoothed_ms, s.rtt.min_ms, s.rtt.samples
    );
    println!("  {resumo_audio}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::codecs_candidatos;
    use fluxo_encode::VideoCodec::{Av1, Hevc, H264};

    #[test]
    fn automatico_tenta_do_melhor_ao_mais_compativel() {
        let todos = H264.bit() | Hevc.bit() | Av1.bit();
        assert_eq!(codecs_candidatos(None, Some(todos)), Ok(vec![Av1, Hevc, H264]));
        assert_eq!(
            codecs_candidatos(None, Some(H264.bit() | Av1.bit())),
            Ok(vec![Av1, H264])
        );
    }

    #[test]
    fn sem_noticia_do_cliente_so_h264() {
        assert_eq!(codecs_candidatos(None, None), Ok(vec![H264]));
        assert!(codecs_candidatos(Some(Av1), None).is_err());
    }

    #[test]
    fn pedido_que_o_cliente_nao_decodifica_explica_o_que_ele_tem() {
        let e = codecs_candidatos(Some(Hevc), Some(H264.bit() | Av1.bit())).unwrap_err();
        assert!(e.contains("HEVC") && e.contains("AV1, H.264"), "{e}");
        assert_eq!(codecs_candidatos(Some(Av1), Some(Av1.bit() | 1)), Ok(vec![Av1]));
    }
}

#[cfg(test)]
mod testes_resolucao {
    use super::parse_resolucao;

    #[test]
    fn software_tem_padroes_mais_baixos_mas_obedece_o_pedido() {
        use super::limites;
        let tela_4k = (3840, 2160);
        assert_eq!(limites(None, None, tela_4k, false), (60, None));
        assert_eq!(limites(None, None, tela_4k, true), (30, Some(1080)));
        assert_eq!(limites(None, None, (1920, 1080), true), (30, None));
        assert_eq!(limites(Some(60), Some(2160), tela_4k, true), (60, Some(2160)));
    }

    #[test]
    fn nomes_e_numeros() {
        assert_eq!(parse_resolucao("nativa"), Ok(None));
        assert_eq!(parse_resolucao("1440p"), Ok(Some(1440)));
        assert_eq!(parse_resolucao("4K"), Ok(Some(2160)));
        assert_eq!(parse_resolucao("900"), Ok(Some(900)));
        assert!(parse_resolucao("8000").is_err());
        assert!(parse_resolucao("alta").is_err());
    }
}
