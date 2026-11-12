//! Thread de exibição: rede → decode → tela, separada da janela.
//!
//! Enquanto a janela é arrastada ou redimensionada, o Windows prende a thread
//! dela num laço interno até o mouse soltar. Se decode e apresentação
//! morassem lá, a imagem parava e os frames se acumulavam na fila (medido:
//! frames esperando até 5,6 s). Aqui eles têm thread própria; a da janela só
//! processa mensagens e repassa os comandos por [`Compartilhado`].
//!
//! Tudo de D3D11 (device, decoder, swapchain) é criado e usado só nesta
//! thread: o contexto do D3D11 não é seguro entre threads.

use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use fluxo_decode::{create_device, VideoDecoder};
use fluxo_net::{clock_diff_us, clock_us, FrameAction, Recovery, RecoveryRequest, VideoReceiver};
use windows::Win32::Foundation::HWND;

use crate::present::Presenter;

/// O que as duas threads trocam.
#[derive(Default)]
pub struct Compartilhado {
    /// Janela → exibição: a janela foi fechada.
    pub fechar: AtomicBool,
    /// Janela → exibição: tamanho novo da área de desenho.
    pub tamanho: Mutex<Option<(u32, u32)>>,
    /// Exibição → janela: título novo (as estatísticas do último segundo).
    pub titulo: Mutex<Option<String>>,
    /// Janela → exibição: mostrar o painel de estatísticas sobre a imagem (F2).
    pub painel: AtomicBool,
}

/// Parâmetros da thread de exibição.
pub struct Config {
    /// Janela onde desenhar. `HWND` não é `Send`; o número é.
    pub hwnd: isize,
    pub tamanho: (u32, u32),
    pub vsync: bool,
    /// Fecha sozinho depois disso, contado do primeiro frame (0 = não).
    pub segundos: f64,
    /// Texto do painel até chegarem estatísticas (código de pareamento e
    /// endereço para o host).
    pub espera: String,
}

/// Durações em ms, com percentis.
#[derive(Default)]
struct Serie(Vec<f64>);

impl Serie {
    fn push(&mut self, ms: f64) {
        self.0.push(ms);
    }
    fn media(&self) -> f64 {
        self.0.iter().sum::<f64>() / self.0.len().max(1) as f64
    }
    fn percentil(&mut self, p: f64) -> f64 {
        if self.0.is_empty() {
            return 0.0;
        }
        self.0.sort_by(f64::total_cmp);
        self.0[((self.0.len() - 1) as f64 * p).round() as usize]
    }
}

/// Laço da thread de exibição; volta quando a janela fecha ou o tempo
/// (`Config::segundos`) acaba, depois de imprimir o resumo.
pub fn exibir(rx: VideoReceiver, cfg: Config, c: &Compartilhado) -> Result<(), Box<dyn Error>> {
    let device = create_device()?;
    let mut decoder = VideoDecoder::new(&device)?;
    let hwnd = HWND(cfg.hwnd as *mut _);
    let mut tela = Presenter::new(&device, hwnd, cfg.tamanho, cfg.vsync)?;
    println!(
        "Escutando em {} | decoder: {} | {}",
        rx.local_addr(),
        decoder.name(),
        if tela.tearing() {
            "sem vsync (tearing permitido)"
        } else if cfg.vsync {
            "com vsync"
        } else {
            "sem vsync (o sistema não permite tearing nesta janela)"
        }
    );

    let duracao = (cfg.segundos > 0.0).then(|| Duration::from_secs_f64(cfg.segundos));
    let mut inicio: Option<Instant> = None;
    // Quando mostrar e o que pedir ao host: keyframe na largada e depois de
    // erro do decoder; depois de uma perda, uma onda de intra-refresh (ou
    // keyframe, se o host não tiver).
    let mut recuperacao = Recovery::new(Instant::now());
    // Quanto a imagem ficou congelada em cada recuperação (fora a largada).
    let mut recuperacoes = Serie::default();
    let mut ja_mostrou = false;
    // Último codec avisado no terminal.
    let mut anunciado = None;
    let mut escondidos = 0u64;
    let mut proximo_titulo = Instant::now() + Duration::from_secs(1);

    // Estatísticas: por segundo (título) e totais (resumo).
    let (mut frames_seg, mut bytes_seg) = (0u64, 0u64);
    let mut latencia_seg = Serie::default();
    let mut latencias = Serie::default();
    let mut decodes = Serie::default();
    // Quanto cada frame esperou entre chegar pela rede e ser processado aqui:
    // mostra travamentos do laço (ex.: janela sendo arrastada).
    let mut esperas = Serie::default();
    // Tempo de CPU para desenhar e apresentar cada frame (inclui o painel).
    let mut apresentacoes = Serie::default();
    let (mut recebidos, mut mostrados, mut pulados, mut erros_decode) = (0u64, 0u64, 0u64, 0u64);

    // Painel sobre a imagem: o texto muda 1×/s; F2 liga e desliga.
    let mut texto_painel = cfg.espera.clone();
    let mut painel_ligado = false;
    // Última versão do cursor desenhada, e quantas vezes ele foi redesenhado
    // sem frame novo.
    let mut versao_cursor: Option<u64> = None;
    let mut cursor_redesenhos = 0u64;

    while !c.fechar.load(Ordering::Relaxed) {
        let quer_painel = c.painel.load(Ordering::Relaxed);
        if quer_painel != painel_ligado {
            painel_ligado = quer_painel;
            tela.set_overlay(painel_ligado.then_some(texto_painel.as_str()))?;
        }
        if let (Some(t0), Some(d)) = (inicio, duracao) {
            if t0.elapsed() >= d {
                break;
            }
        }
        if let Some(t) = c.tamanho.lock().unwrap_or_else(|e| e.into_inner()).take() {
            tela.resize(t)?;
        }

        // Imagem não está boa: pede ao host. Pode chamar a cada volta: o
        // fluxo-net segura pedidos repetidos.
        match recuperacao.request(Instant::now()) {
            RecoveryRequest::Keyframe => {
                rx.request_keyframe();
            }
            RecoveryRequest::Refresh => {
                rx.request_refresh();
            }
            RecoveryRequest::None => {}
        }

        // Cursor do host: posição ou forma nova vale para o próximo desenho.
        let cursor = rx.cursor();
        let cursor_mudou = cursor.as_ref().map(|c| c.version) != versao_cursor;
        if cursor_mudou {
            versao_cursor = cursor.as_ref().map(|c| c.version);
            tela.set_cursor(cursor)?;
        }

        // Espera curta para ver `fechar` e o tamanho novo logo.
        let recebido = rx.recv_frame(Duration::from_millis(5))?;
        // Sem frame novo, mas o cursor mexeu: redesenha a última imagem com
        // ele no lugar novo (não espera o próximo frame).
        if recebido.is_none() && cursor_mudou && tela.has_image() && recuperacao.is_clean() {
            tela.present_again()?;
            cursor_redesenhos += 1;
        }
        // Tela parada do outro lado não manda frame: o título e o painel
        // seguem atualizando mesmo assim (abaixo).
        if let Some(primeiro) = recebido {
            inicio.get_or_insert_with(Instant::now);

            // Decodifica tudo o que chegou (cada P-frame depende do anterior),
            // mas só mostra o mais novo: se o cliente atrasou, ele alcança. Com
            // limite de tempo: se a fila nunca esvazia, a tela ainda atualiza.
            let lote = Instant::now();
            let mut chegou = Some(primeiro);
            let mut para_mostrar = None;
            while let Some(f) = chegou.take() {
                recebidos += 1;
                esperas.push(clock_diff_us(clock_us(), f.received_us) as f64 / 1000.0);
                bytes_seg += f.frame.data.len() as u64;
                // Depois de uma perda os P-frames apontam para uma imagem que o
                // decoder não tem: continua decodificando (para o decoder
                // acompanhar a onda de limpeza), mas congela a última imagem boa
                // na tela até a recuperação terminar.
                let sujo_desde = recuperacao.dirty_since();
                match recuperacao.on_frame(&f.frame, Instant::now()) {
                    FrameAction::Skip => pulados += 1,
                    FrameAction::Decode { show } => {
                        // O codec vem marcado em cada frame; troca o decoder se
                        // mudou (na prática só no primeiro, que é keyframe).
                        let codec = f.frame.codec.unwrap_or_default();
                        if codec != decoder.codec() {
                            decoder = VideoDecoder::with_codec(&device, codec)?;
                        }
                        // Também no primeiro frame: é assim que a janela do
                        // Fluxo sabe que o vídeo chegou (ver apps/fluxo/src/saida.rs).
                        if anunciado != Some(codec) {
                            println!("  vídeo em {} | decoder: {}", codec.name(), decoder.name());
                            anunciado = Some(codec);
                        }
                        let t = Instant::now();
                        match decoder.decode(&f.frame.data) {
                            Ok(Some(d)) if show => para_mostrar = Some((d, f.frame.timestamp_us)),
                            Ok(_) => escondidos += !show as u64,
                            Err(e) => {
                                // Bitstream estragado: recomeça no próximo
                                // keyframe com um decoder limpo.
                                erros_decode += 1;
                                eprintln!("(decode falhou: {e}; pedindo keyframe)");
                                decoder = VideoDecoder::with_codec(&device, decoder.codec())?;
                                recuperacao.on_decode_error(Instant::now());
                            }
                        }
                        decodes.push(t.elapsed().as_secs_f64() * 1000.0);
                    }
                }
                if let (Some(t), true) = (sujo_desde, recuperacao.is_clean()) {
                    if ja_mostrou {
                        recuperacoes.push(t.elapsed().as_secs_f64() * 1000.0);
                    }
                    ja_mostrou = true;
                }
                if lote.elapsed() < Duration::from_millis(33) {
                    chegou = rx.recv_frame(Duration::ZERO)?;
                }
            }

            if let Some((d, capturado_us)) = para_mostrar {
                let t = Instant::now();
                tela.present(&d)?;
                apresentacoes.push(t.elapsed().as_secs_f64() * 1000.0);
                drop(d); // devolve a superfície ao decoder
                mostrados += 1;
                frames_seg += 1;
                // Captura no host → frame entregue ao monitor aqui. A captura vem
                // no relógio do host; somando a diferença entre os relógios
                // (medida pelos pings) ela vira o nosso relógio.
                if let Some(dif_ms) = rx.stats().clock_offset_ms {
                    let ms = clock_diff_us(clock_us(), capturado_us) as f64 / 1000.0 + dif_ms;
                    latencia_seg.push(ms);
                    latencias.push(ms);
                }
            }
        }

        // Antes do primeiro frame fica o texto de espera (código, sala).
        if inicio.is_some() && Instant::now() >= proximo_titulo {
            let s = rx.stats();
            let latencia = match latencia_seg.0.is_empty() {
                true => "sincronizando relógios".to_string(),
                false => format!(
                    "captura→tela {:.1} ms (máx {:.1})",
                    latencia_seg.media(),
                    latencia_seg.percentil(1.0)
                ),
            };
            let mbps = bytes_seg as f64 * 8.0 / 1e6;
            *c.titulo.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!(
                "FSS — {frames_seg} fps · {mbps:.1} Mbps · {latencia} · RTT {:.1} ms · perdidos {}{}",
                s.rtt.smoothed_ms,
                s.reassembly.frames_lost,
                if s.encrypted { " · cifrado" } else { "" },
            ));
            texto_painel = format!(
                "{frames_seg} fps · {mbps:.1} Mbps\n{latencia}\nRTT {:.1} ms · fila {:.0} ms\nperdidos {} · FEC {}\n{}F2 esconde",
                s.rtt.smoothed_ms, s.queue_delay_ms, s.reassembly.frames_lost, s.reassembly.fec_recovered,
                if s.encrypted { "cifrado · " } else { "SEM CIFRA · " },
            );
            if painel_ligado {
                tela.set_overlay(Some(&texto_painel))?;
            }
            (frames_seg, bytes_seg) = (0, 0);
            latencia_seg = Serie::default();
            proximo_titulo = Instant::now() + Duration::from_secs(1);
        }
    }

    let s = rx.stats();
    let total = inicio.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    println!("\nResultado ({:.1} s)", total);
    println!(
        "  frames: {recebidos} recebidos, {mostrados} mostrados ({:.1} fps), {pulados} pulados esperando keyframe, \
         {escondidos} decodificados sem mostrar (esperando limpeza)",
        mostrados as f64 / total.max(0.001)
    );
    println!(
        "  perdidos na rede: {} | reconstruídos pelo FEC: {} fragmentos | erros de decode: {erros_decode} | descartados na fila: {}",
        s.reassembly.frames_lost, s.reassembly.fec_recovered, s.frames_dropped
    );
    println!(
        "  cursor: {} pacotes ({} formas pedidas de novo), {cursor_redesenhos} redesenhos sem frame novo",
        s.cursor_packets, s.cursor_requests
    );
    println!(
        "  recuperação:      {} perdas, {} pedidos de limpeza e {} de keyframe; {} recuperações, imagem congelada média {:.1} ms, máx {:.1} ms",
        recuperacao.losses(),
        s.refresh_requests,
        s.keyframe_requests,
        recuperacoes.0.len(),
        recuperacoes.media(),
        recuperacoes.percentil(1.0)
    );
    println!(
        "  espera no cliente: p99 {:.1} ms, máx {:.1} ms (chegou pela rede → processado)",
        esperas.percentil(0.99),
        esperas.percentil(1.0)
    );
    println!(
        "  decode (CPU):     média {:.2} ms, p99 {:.2} ms",
        decodes.media(),
        decodes.percentil(0.99)
    );
    println!(
        "  apresentação (CPU): média {:.2} ms, p99 {:.2} ms (desenho, painel e Present)",
        apresentacoes.media(),
        apresentacoes.percentil(0.99)
    );
    println!(
        "  captura → tela:   p50 {:.1} ms, p99 {:.1} ms, máx {:.1} ms",
        latencias.percentil(0.5),
        latencias.percentil(0.99),
        latencias.percentil(1.0)
    );
    if let (Some(dif), Some(erro)) = (s.clock_offset_ms, s.clock_error_ms) {
        println!("  relógios:         host {dif:+.1} ms em relação a este PC (sincronizados pelos pings, erro ±{erro:.1} ms)");
    }
    println!(
        "  RTT:              {:.2} ms suavizado, {:.2} ms mínimo",
        s.rtt.smoothed_ms, s.rtt.min_ms
    );
    Ok(())
}
