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
mod grupo;
mod janela;
mod lugar;
mod perfil;
mod processo;
mod saida;
mod tema;

/// O nome que aparece para quem usa. Por dentro (crates, comandos) continua Fluxo.
pub const NOME: &str = "FlameScreenSharing";

/// O ícone da janela e da barra de tarefas: o maior bitmap de
/// `pacote/icone/fss.ico` (o mesmo embutido no executável pelo build.rs).
fn icone() -> eframe::egui::IconData {
    let ico: &[u8] = include_bytes!("../../../pacote/icone/fss.ico");
    let u16_em = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]) as usize;
    let u32_em = |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]) as usize;
    // Diretório: 16 bytes por imagem; lado 0 quer dizer 256.
    let (lado, inicio) = (0..u16_em(4))
        .map(|n| 6 + 16 * n)
        .map(|e| (if ico[e] == 0 { 256 } else { ico[e] as usize }, u32_em(e + 12)))
        .max()
        .unwrap_or((0, 0));
    // Bitmap de 32 bits (cabeçalho de 40 bytes), BGRA de baixo para cima.
    let pixels = inicio + 40;
    let mut rgba = Vec::with_capacity(lado * lado * 4);
    for y in (0..lado).rev() {
        for x in 0..lado {
            let i = pixels + (y * lado + x) * 4;
            rgba.extend_from_slice(&[ico[i + 2], ico[i + 1], ico[i], ico[i + 3]]);
        }
    }
    eframe::egui::IconData {
        rgba,
        width: lado as u32,
        height: lado as u32,
    }
}

fn main() -> eframe::Result {
    // Aberto pela versão anterior, logo depois de instalar esta.
    let atualizado = std::env::args().any(|a| a == "--atualizado");
    atualizacao::limpar_antigos();
    let opcoes = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(NOME)
            .with_inner_size([960.0, 660.0])
            .with_min_inner_size([520.0, 460.0])
            .with_icon(std::sync::Arc::new(icone())),
        ..Default::default()
    };
    eframe::run_native(
        NOME,
        opcoes,
        Box::new(|cc| Ok(Box::new(janela::Janela::new(lugar::hwnd_de(cc), atualizado)))),
    )
}

#[cfg(test)]
mod testes_icone {
    #[test]
    fn icone_256_com_transparencia() {
        let i = super::icone();
        assert_eq!((i.width, i.height), (256, 256));
        assert_eq!(i.rgba.len(), 256 * 256 * 4);
        assert_eq!(i.rgba[3], 0, "canto de cima à esquerda transparente");
        // Meio da tela do monitor: a chama, opaca.
        assert_eq!(i.rgba[(150 * 256 + 100) * 4 + 3], 255);
    }
}
