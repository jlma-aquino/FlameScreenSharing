//! Cliente portátil (Linux): mesma rede, mesmos códigos e mesma saída no
//! terminal do cliente de Windows, com peças que não dependem do sistema.
//!
//! | Etapa | Windows (`exibir.rs`) | Aqui |
//! |---|---|---|
//! | Decode | Media Foundation, na GPU | OpenH264, na CPU (só H.264) |
//! | Tela | Direct3D 11 | OpenGL, numa janela egui ([`tela`]) |
//! | Som | WASAPI | um tocador do sistema, por pipe ([`som`]) |
//!
//! Compila também no Windows (`--features portatil`, opção `--portatil`): dá
//! para testar quase tudo aqui, comparando com o cliente normal.

mod som;
mod tela;
mod video;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use fluxo_net::{CursorView, VideoReceiver};

use crate::Opts;

/// Um frame decodificado, em I420, pronto para a GPU.
pub struct Quadro {
    pub largura: u32,
    pub altura: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    /// Bytes por linha: Y, e U/V.
    pub passo: (usize, usize),
    /// Captura no host, no relógio de lá, e a diferença entre os relógios.
    pub capturado_us: u32,
    pub dif_relogio_ms: Option<f64>,
}

/// O que a thread do vídeo e a da janela trocam.
#[derive(Default)]
pub struct Compartilhado {
    /// A janela fechou, ou o vídeo terminou.
    pub fechar: AtomicBool,
    /// O frame mais novo: um que a janela não pegou a tempo é substituído.
    pub quadro: Mutex<Option<Quadro>>,
    pub cursor: Mutex<Option<CursorView>>,
    pub titulo: Mutex<Option<String>>,
    /// Texto do painel de estatísticas.
    pub painel: Mutex<String>,
    /// Captura → tela de cada frame mostrado, em ms.
    pub latencias: Mutex<Vec<f64>>,
}

/// Abre a janela e fica nela até fechar.
pub fn run(
    o: &Opts,
    rx: VideoReceiver,
    titulo: String,
    espera: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let comp = Arc::new(Compartilhado::default());
    *comp.painel.lock().unwrap_or_else(|e| e.into_inner()) = espera.clone();

    let audio = match (o.sem_audio, rx.take_audio()) {
        (false, Some(fila)) => {
            let c = comp.clone();
            Some(
                std::thread::Builder::new()
                    .name("fluxo-audio".into())
                    .spawn(move || som::tocar(fila, &c))?,
            )
        }
        _ => None,
    };

    let mut janela = eframe::egui::ViewportBuilder::default()
        .with_title(titulo)
        .with_inner_size([o.largura as f32, o.altura as f32]);
    if o.no_topo {
        janela = janela.with_always_on_top();
    }
    let opcoes = eframe::NativeOptions {
        viewport: janela,
        // Sem vsync, a menos que pedido: é menos atraso, como no Windows.
        vsync: o.vsync,
        ..Default::default()
    };

    let segundos = o.segundos;
    let painel = !o.sem_painel;
    let c = comp.clone();
    let video = Arc::new(Mutex::new(None));
    let v = video.clone();
    let aberta = eframe::run_native(
        "FSS",
        opcoes,
        Box::new(move |cc| {
            let tela = tela::Tela::new(cc, c.clone(), espera, painel)?;
            // Só agora: a thread do vídeo avisa a janela, que precisa existir.
            let ctx = cc.egui_ctx.clone();
            let t = std::thread::Builder::new()
                .name("fluxo-exibicao".into())
                .spawn(move || video::laco(rx, segundos, ctx, &c).map_err(|e| e.to_string()))?;
            *v.lock().unwrap_or_else(|e| e.into_inner()) = Some(t);
            Ok(Box::new(tela))
        }),
    );

    comp.fechar.store(true, Ordering::Relaxed);
    let t = video.lock().unwrap_or_else(|e| e.into_inner()).take();
    let resultado_video = t.map(|t| t.join());
    match audio.map(|t| t.join()) {
        Some(Ok(Ok(resumo))) => println!("  {resumo}"),
        Some(Ok(Err(e))) => println!("  áudio desligado: {e}"),
        Some(Err(_)) => println!("  áudio: a thread terminou com pânico"),
        None => {}
    }
    aberta.map_err(|e| format!("não consegui abrir a janela: {e}"))?;
    match resultado_video {
        Some(Ok(Err(e))) => Err(e.into()),
        Some(Err(_)) => Err("a thread do vídeo terminou com pânico".into()),
        _ => Ok(()),
    }
}
