//! # fluxo
//!
//! A janela do Fluxo: compartilhar a tela ou assistir, sem terminal. Roda o
//! `fluxo-host` e o `fluxo-client` da mesma pasta por baixo e mostra os
//! códigos para trocar. Detalhes: `docs/modulos/fluxo.md`.

// Sem janela de console ao abrir com dois cliques.
#![cfg_attr(windows, windows_subsystem = "windows")]
// A janela só existe no Windows; a lógica é testada em qualquer sistema.
#![cfg_attr(not(windows), allow(dead_code))]

mod comandos;
#[cfg(windows)]
mod janela;
#[cfg(windows)]
mod processo;
mod saida;

/// O nome que aparece para quem usa. Por dentro (crates, comandos) continua Fluxo.
pub const NOME: &str = "FlameScreenSharing";

#[cfg(windows)]
fn main() -> eframe::Result {
    let opcoes = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(NOME)
            .with_inner_size([480.0, 520.0])
            .with_min_inner_size([380.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(NOME, opcoes, Box::new(|_| Ok(Box::new(janela::Janela::new()))))
}

#[cfg(not(windows))]
fn main() {
    eprintln!("a janela do Fluxo só roda no Windows; use o fluxo-bench receber");
}
