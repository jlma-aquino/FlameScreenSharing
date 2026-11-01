//! A janela (egui): escolher entre compartilhar e assistir, preencher o
//! mínimo e trocar os códigos. Quem faz o trabalho são o `fluxo-host` e o
//! `fluxo-client`, em [`Processo`].

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Color32, RichText};

use crate::comandos::{linha_de_resposta, Assistir, Compartilhar, FPS, RESOLUCOES};
use crate::processo::{caminho_do_programa, Processo};
use crate::saida::{ler, Evento};

/// Linhas guardadas no registro.
const MAX_REGISTRO: usize = 500;

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tela {
    Inicio,
    Compartilhar,
    Assistir,
}

/// Um `fluxo-host` ou `fluxo-client` rodando (ou que acabou de terminar).
struct Sessao {
    processo: Processo,
    /// Cliente, mesma rede.
    pareamento: Option<String>,
    destino: Option<String>,
    /// Cliente, redes diferentes.
    conexao: Option<String>,
    /// Host, redes diferentes.
    resposta: Option<String>,
    transmitindo: bool,
    /// Cliente: o vídeo chegou (codec e decoder).
    recebendo: Option<String>,
    relatorio: Option<String>,
    erro: Option<String>,
    registro: VecDeque<String>,
    /// `Some(sucesso)` quando o programa terminou.
    fim: Option<bool>,
}

impl Sessao {
    fn new(processo: Processo) -> Self {
        Self {
            processo,
            pareamento: None,
            destino: None,
            conexao: None,
            resposta: None,
            transmitindo: false,
            recebendo: None,
            relatorio: None,
            erro: None,
            registro: VecDeque::new(),
            fim: None,
        }
    }

    /// Lê o que o programa imprimiu e vê se ele terminou.
    fn atualizar(&mut self) {
        for linha in self.processo.linhas_novas() {
            match ler(&linha) {
                Evento::Pareamento { codigo, destino } => {
                    self.pareamento = Some(codigo);
                    self.destino = destino.or(self.destino.take());
                }
                Evento::Conexao(c) => self.conexao = Some(c),
                Evento::Resposta(c) => self.resposta = Some(c),
                Evento::Transmitindo => self.transmitindo = true,
                Evento::Recebendo(v) => self.recebendo = Some(v),
                Evento::Relatorio(r) => self.relatorio = Some(r),
                Evento::Erro(e) => self.erro = Some(e),
                Evento::Outra => {}
            }
            self.registro.push_back(linha);
            if self.registro.len() > MAX_REGISTRO {
                self.registro.pop_front();
            }
        }
        if self.fim.is_none() {
            self.fim = self.processo.terminou();
        }
    }
}

pub struct Janela {
    tela: Tela,
    compartilhar: Compartilhar,
    assistir: Assistir,
    /// Monitores desta máquina, para escolher qual compartilhar.
    monitores: Vec<String>,
    sessao: Option<Sessao>,
    /// Problema no formulário (código errado, programa não encontrado).
    aviso: Option<String>,
    resposta_colada: String,
}

impl Janela {
    pub fn new() -> Self {
        let monitores = fluxo_capture::list_monitors()
            .map(|l| {
                l.iter()
                    .map(|m| format!("Monitor {} ({}x{})", m.index + 1, m.width, m.height))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            tela: Tela::Inicio,
            compartilhar: Compartilhar::default(),
            assistir: Assistir::default(),
            monitores,
            sessao: None,
            aviso: None,
            resposta_colada: String::new(),
        }
    }

    fn iniciar(&mut self, ctx: &egui::Context, programa: &str, args: Vec<String>) {
        let ctx = ctx.clone();
        let avisar = Arc::new(move || ctx.request_repaint());
        let aberto =
            caminho_do_programa(programa).and_then(|caminho| Processo::iniciar(&caminho, &args, avisar));
        match aberto {
            Ok(p) => {
                self.sessao = Some(Sessao::new(p));
                self.aviso = None;
                self.resposta_colada.clear();
            }
            Err(e) => self.aviso = Some(format!("Não consegui abrir o programa: {e}")),
        }
    }

    fn tela_inicio(&mut self, ui: &mut egui::Ui) {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.heading(RichText::new(crate::NOME).size(32.0));
            ui.label("Compartilhe a tela direto entre dois PCs, sem servidor.");
            ui.add_space(24.0);
            let tamanho = egui::vec2(280.0, 56.0);
            if ui
                .add_sized(
                    tamanho,
                    egui::Button::new(RichText::new("Compartilhar minha tela").size(18.0)),
                )
                .clicked()
            {
                self.tela = Tela::Compartilhar;
            }
            ui.add_space(8.0);
            if ui
                .add_sized(tamanho, egui::Button::new(RichText::new("Assistir").size(18.0)))
                .clicked()
            {
                self.tela = Tela::Assistir;
            }
        });
    }

    fn form_compartilhar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Compartilhar minha tela");
        ui.add_space(8.0);
        let c = &mut self.compartilhar;
        ui.radio_value(&mut c.internet, true, "Pela internet (redes diferentes)");
        ui.radio_value(&mut c.internet, false, "Na mesma rede");
        ui.add_space(8.0);
        if c.internet {
            ui.label("Código de conexão de quem vai assistir:");
            ui.add(
                egui::TextEdit::singleline(&mut c.conexao)
                    .desired_width(f32::INFINITY)
                    .hint_text("cole aqui"),
            );
        } else {
            ui.label("Endereço de quem vai assistir:");
            ui.add(
                egui::TextEdit::singleline(&mut c.destino)
                    .desired_width(f32::INFINITY)
                    .hint_text("192.168.0.20:9000"),
            );
            ui.label("Código:");
            ui.add(
                egui::TextEdit::singleline(&mut c.codigo)
                    .desired_width(f32::INFINITY)
                    .hint_text("XXXX-XXXX"),
            );
        }
        ui.add_space(8.0);
        egui::Grid::new("opcoes").num_columns(2).show(ui, |ui| {
            if self.monitores.len() > 1 {
                ui.label("Tela:");
                let atual = self.monitores.get(c.monitor).cloned().unwrap_or_default();
                egui::ComboBox::from_id_salt("monitor")
                    .selected_text(atual)
                    .show_ui(ui, |ui| {
                        for (i, nome) in self.monitores.iter().enumerate() {
                            ui.selectable_value(&mut c.monitor, i, nome);
                        }
                    });
                ui.end_row();
            }
            ui.label("Resolução:");
            egui::ComboBox::from_id_salt("resolucao")
                .selected_text(RESOLUCOES[c.resolucao].0)
                .show_ui(ui, |ui| {
                    for (i, (nome, _)) in RESOLUCOES.iter().enumerate() {
                        ui.selectable_value(&mut c.resolucao, i, *nome);
                    }
                });
            ui.end_row();
            ui.label("Quadros por segundo:");
            egui::ComboBox::from_id_salt("fps")
                .selected_text(c.fps.to_string())
                .show_ui(ui, |ui| {
                    for f in FPS {
                        ui.selectable_value(&mut c.fps, f, f.to_string());
                    }
                });
            ui.end_row();
        });
        ui.checkbox(&mut c.audio, "Transmitir o som do PC");
        ui.add_space(12.0);
        if ui.button(RichText::new("Começar").size(16.0)).clicked() {
            match self.compartilhar.args() {
                Ok(args) => self.iniciar(ctx, "host", args),
                Err(e) => self.aviso = Some(e),
            }
        }
    }

    fn form_assistir(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Assistir");
        ui.add_space(8.0);
        let a = &mut self.assistir;
        ui.radio_value(&mut a.internet, true, "Pela internet (redes diferentes)");
        ui.radio_value(&mut a.internet, false, "Na mesma rede");
        ui.add_space(8.0);
        ui.checkbox(&mut a.audio, "Tocar o som");
        if a.audio {
            ui.add(egui::Slider::new(&mut a.volume, 0..=100).text("Volume"));
        }
        ui.add_space(12.0);
        if ui.button(RichText::new("Começar").size(16.0)).clicked() {
            let args = self.assistir.args();
            self.iniciar(ctx, "client", args);
        }
    }

    fn painel_sessao(&mut self, ui: &mut egui::Ui) {
        let Some(s) = self.sessao.as_mut() else {
            return;
        };
        let mut enviar_resposta = false;

        // Quem vai assistir: os códigos para mandar a quem compartilha (com o
        // vídeo chegando, já serviram).
        if s.recebendo.is_none() {
            if let Some(c) = &s.conexao {
                codigo(ui, "Mande este código para quem vai compartilhar:", c);
                ui.add_space(8.0);
                ui.label("Depois cole aqui o código de resposta que ele mandar de volta:");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.resposta_colada)
                            .desired_width(ui.available_width() - 70.0)
                            .hint_text("código de resposta"),
                    );
                    enviar_resposta = ui.button("Enviar").clicked();
                });
            }
            if let Some(c) = &s.pareamento {
                codigo(ui, "Mande este código para quem vai compartilhar:", c);
                if let Some(d) = &s.destino {
                    codigo(ui, "E este endereço:", d);
                }
            }
        }
        // Quem compartilha, pela internet: a resposta para mandar de volta.
        if let Some(c) = &s.resposta {
            if !s.transmitindo {
                codigo(ui, "Mande este código de resposta para quem vai assistir:", c);
            }
        }

        if enviar_resposta {
            let envio = linha_de_resposta(&self.resposta_colada).and_then(|l| {
                s.processo
                    .enviar(&l)
                    .map_err(|e| format!("Não consegui enviar: {e}"))
            });
            self.aviso = envio.err();
            if self.aviso.is_none() {
                self.resposta_colada.clear();
            }
        }

        ui.add_space(8.0);
        let verde = Color32::from_rgb(80, 180, 100);
        match (&s.erro, s.fim, s.transmitindo, &s.recebendo) {
            (Some(e), ..) => {
                ui.colored_label(Color32::from_rgb(220, 80, 80), e);
            }
            (None, Some(_), ..) => {
                ui.label("Terminou.");
            }
            (None, None, true, _) => {
                ui.colored_label(verde, "Transmitindo");
                if let Some(r) = &s.relatorio {
                    ui.small(r);
                }
            }
            (None, None, false, Some(video)) => {
                ui.colored_label(verde, "Recebendo: o vídeo está na outra janela");
                ui.small(video);
            }
            (None, None, false, None) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Esperando o outro lado…");
                });
            }
        }

        ui.add_space(8.0);
        egui::CollapsingHeader::new("Detalhes").show(ui, |ui| {
            if ui.small_button("Copiar detalhes").clicked() {
                let texto: Vec<&str> = s.registro.iter().map(String::as_str).collect();
                ui.ctx().copy_text(texto.join(
                    "
",
                ));
            }
            egui::ScrollArea::vertical()
                .max_height(160.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for l in &s.registro {
                        ui.small(l);
                    }
                });
        });
        ui.add_space(8.0);
        let rotulo = if s.fim.is_some() { "Voltar" } else { "Parar" };
        if ui.button(rotulo).clicked() {
            self.sessao = None; // soltar encerra o programa
        }
    }
}

/// Um código em letra grande, com o botão de copiar.
fn codigo(ui: &mut egui::Ui, titulo: &str, texto: &str) {
    ui.label(titulo);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(texto).monospace().size(16.0).strong());
        if ui.button("Copiar").clicked() {
            ui.ctx().copy_text(texto.to_string());
        }
    });
}

impl eframe::App for Janela {
    /// Fechar a janela encerra o host ou o cliente que estiver rodando.
    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
        self.sessao = None;
    }

    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if let Some(s) = self.sessao.as_mut() {
            s.atualizar();
            // O programa pode terminar sem imprimir nada.
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                if self.sessao.is_some() {
                    self.painel_sessao(ui);
                } else {
                    if self.tela != Tela::Inicio && ui.small_button("Voltar").clicked() {
                        self.tela = Tela::Inicio;
                        self.aviso = None;
                    }
                    match self.tela {
                        Tela::Inicio => self.tela_inicio(ui),
                        Tela::Compartilhar => self.form_compartilhar(ui, ctx),
                        Tela::Assistir => self.form_assistir(ui, ctx),
                    }
                }
                if let Some(a) = &self.aviso {
                    ui.add_space(8.0);
                    ui.colored_label(Color32::from_rgb(220, 80, 80), a);
                }
            });
        });
    }
}
