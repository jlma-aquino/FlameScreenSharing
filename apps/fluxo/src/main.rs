//! # fluxo
//!
//! A janela do Fluxo: compartilhar a tela ou assistir, sem terminal. Roda o
//! `fluxo-host` e o `fluxo-client` da mesma pasta por baixo e mostra os
//! códigos para trocar. Detalhes: `docs/modulos/fluxo.md`.

// Sem janela de console ao abrir com dois cliques.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod atualizacao;
mod comandos;
mod config;
mod janela;
mod lugar;
mod processo;
mod saida;
mod tema;

/// O nome que aparece para quem usa. Por dentro (crates, comandos) continua Fluxo.
pub const NOME: &str = "FlameScreenSharing";

fn main() -> eframe::Result {
    // Aberto pela versão anterior, logo depois de instalar esta.
    let atualizado = std::env::args().any(|a| a == "--atualizado");
    atualizacao::limpar_antigos();
    let opcoes = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(NOME)
            .with_inner_size([960.0, 660.0])
            .with_min_inner_size([520.0, 460.0]),
        ..Default::default()
    };
    eframe::run_native(
        NOME,
        opcoes,
        Box::new(|cc| Ok(Box::new(janela::Janela::new(lugar::hwnd_de(cc), atualizado)))),
    )
}
