//! A janela (egui), no jeito do Meet e do Discord: na tela inicial, um
//! cartão para compartilhar e outro para entrar numa sala; durante a
//! transmissão, uma barra embaixo com os botões (trocar de tela, som, pausa,
//! qualidade, volume, encerrar); e as configurações numa página própria.
//!
//! Quem faz o trabalho são o `fluxo-host` e o `fluxo-client`, em
//! [`Processo`]. Os botões mandam linhas pela entrada padrão deles e a
//! transmissão segue sem reiniciar; a janela só mostra o novo estado quando
//! o programa confirma (as linhas lidas em [`crate::saida`]).
//!
//! Cores e formas vêm só do [`Tema`] (ver `tema.rs`).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};

use crate::atualizacao::{self, Pronta, Resultado, Versao};
use crate::comandos::{
    linha_de_resposta, linha_espectador_internet, linha_espectador_rede, Assistir, Compartilhar, Modo, FPS,
    RESOLUCOES,
};
use crate::config::{self, Configuracoes, CODECS, ENCODERS, MBPS};
use crate::lugar;
use crate::processo::{caminho_do_programa, Aviso, Processo};
use crate::saida::{ler, Evento};
use crate::tema::{Aparencia, Paleta, Tema};

/// O limite do host (`espectadores.rs`, `MAX_ESPECTADORES`), para o texto da janela.
const MAX_ESPECTADORES: usize = 4;

/// Linhas guardadas no registro.
const MAX_REGISTRO: usize = 500;

/// Largura máxima do conteúdo: numa janela grande, ele fica no meio.
const LARGURA_CONTEUDO: f32 = 760.0;

/// A tela sem transmissão.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Tela {
    Inicio,
    /// Pela internet trocando códigos, ou na mesma rede: sem servidor.
    Outras,
}

/// Lado escolhido nas outras formas de conectar.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Lado {
    Compartilhar,
    Assistir,
}

/// Seções da página de configurações.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Secao {
    Aparencia,
    Transmissao,
    Assistir,
    Conexao,
    Sobre,
}

/// Onde a atualização está.
#[derive(Debug, Clone)]
enum Atualizacao {
    Nada,
    Verificando,
    EmDia,
    /// Há versão nova, mas esta cópia não se atualiza sozinha.
    Disponivel {
        versao: Versao,
        pagina: String,
    },
    /// Baixada e conferida: instala assim que não houver transmissão.
    Pronta(Pronta),
    Erro(String),
}

impl Secao {
    const TODAS: [Secao; 5] = [
        Secao::Aparencia,
        Secao::Transmissao,
        Secao::Assistir,
        Secao::Conexao,
        Secao::Sobre,
    ];

    fn rotulo(self) -> &'static str {
        match self {
            Secao::Aparencia => "Aparência",
            Secao::Transmissao => "Compartilhar",
            Secao::Assistir => "Assistir",
            Secao::Conexao => "Conexão",
            Secao::Sobre => "Sobre",
        }
    }
}

/// Um `fluxo-host` ou `fluxo-client` rodando (ou que acabou de terminar).
struct Sessao {
    processo: Processo,
    /// Host (quem compartilha) ou cliente.
    host: bool,
    modo: Modo,
    /// Host: código da sala.
    sala: Option<String>,
    /// Cliente: entrou numa sala (espera quem compartilha, sem código para mostrar).
    entrando_na_sala: bool,
    /// Cliente, mesma rede.
    pareamento: Option<String>,
    destino: Option<String>,
    /// Cliente, redes diferentes.
    conexao: Option<String>,
    /// Host, redes diferentes.
    resposta: Option<String>,
    /// Host: respostas de quem entrou depois e ainda não conectou.
    respostas: Vec<(u32, String)>,
    /// Host: quantos estão assistindo.
    assistindo: usize,
    transmitindo: bool,
    /// Cliente: o vídeo chegou (codec e decoder).
    recebendo: Option<String>,
    relatorio: Option<String>,
    erro: Option<String>,
    /// Problema com um espectador ou com um botão; a transmissão segue.
    aviso: Option<String>,
    registro: VecDeque<String>,
    /// `Some(sucesso)` quando o programa terminou.
    fim: Option<bool>,

    // O estado dos botões, como o programa confirmou.
    /// Host: tela transmitida (a partir de 0).
    monitor: usize,
    /// Host: o som do PC está indo.
    som: bool,
    /// Host: vídeo pausado.
    pausado: bool,
    /// Host: resolução e fps depois de uma mudança.
    qualidade: Option<String>,
    /// Cliente: volume escolhido (o mudo não mexe nele).
    volume: u32,
    mudo: bool,
    /// Cliente: último volume mandado ao programa.
    volume_enviado: u32,
    /// Cliente: o vídeo abriu no lugar desta janela, que está escondida.
    video_no_lugar: bool,
}

impl Sessao {
    fn new(processo: Processo, host: bool, modo: Modo, cfg: &Configuracoes) -> Self {
        Self {
            processo,
            host,
            modo,
            sala: None,
            entrando_na_sala: !host && modo == Modo::Sala,
            pareamento: None,
            destino: None,
            conexao: None,
            resposta: None,
            respostas: Vec::new(),
            assistindo: 0,
            transmitindo: false,
            recebendo: None,
            relatorio: None,
            erro: None,
            aviso: None,
            registro: VecDeque::new(),
            fim: None,
            monitor: cfg.monitor,
            som: cfg.som,
            pausado: false,
            qualidade: None,
            volume: cfg.volume,
            mudo: false,
            volume_enviado: cfg.volume,
            video_no_lugar: false,
        }
    }

    /// Lê o que o programa imprimiu e vê se ele terminou.
    fn atualizar(&mut self) {
        for linha in self.processo.linhas_novas() {
            match ler(&linha) {
                Evento::Sala(c) => self.sala = Some(c),
                Evento::TentouEntrar(motivo) => {
                    self.aviso = Some(format!("Alguém tentou entrar e não conectou: {motivo}"))
                }
                Evento::Pareamento { codigo, destino } => {
                    self.pareamento = Some(codigo);
                    self.destino = destino.or(self.destino.take());
                }
                Evento::Conexao(c) => self.conexao = Some(c),
                Evento::Resposta(c) => self.resposta = Some(c),
                Evento::RespostaDe { numero, codigo } => self.respostas.push((numero, codigo)),
                Evento::Conectou { numero, total } => {
                    self.assistindo = total;
                    self.respostas.retain(|(n, _)| *n != numero);
                }
                Evento::Saiu { total, .. } => self.assistindo = total,
                Evento::NaoConectou { numero, motivo } => {
                    self.respostas.retain(|(n, _)| *n != numero);
                    self.aviso = Some(format!("Espectador {numero} não conectou: {motivo}"));
                }
                Evento::Transmitindo => self.transmitindo = true,
                Evento::Recebendo(v) => self.recebendo = Some(v),
                Evento::Relatorio(r) => self.relatorio = Some(r),
                Evento::Tela(n) => self.monitor = n.saturating_sub(1),
                Evento::Som(sim) => self.som = sim,
                Evento::Pausa(sim) => self.pausado = sim,
                Evento::Qualidade(q) => self.qualidade = Some(q),
                Evento::Volume(_) => {}
                Evento::Recusado(m) => self.aviso = Some(m),
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

    /// Manda um comando ao programa; um erro vira aviso na janela.
    fn mandar(&mut self, linha: &str) {
        if let Err(e) = self.processo.enviar(linha) {
            self.aviso = Some(format!("Não consegui mandar o comando: {e}"));
        }
    }

    fn rodando(&self) -> bool {
        self.fim.is_none() && self.erro.is_none()
    }
}

pub struct Janela {
    /// Verificação de atualização, feita numa thread (rede).
    atualizacao: Arc<Mutex<Atualizacao>>,
    /// Já verificou ao abrir.
    verificou: bool,
    /// Aberta logo depois de se atualizar: mostra um aviso.
    atualizado_agora: bool,
    /// HWND desta janela (Windows), para dar o lugar ao vídeo de quem assiste.
    hwnd: Option<isize>,
    tema: Tema,
    cfg: Configuracoes,
    /// O que está no arquivo: quando `cfg` muda, grava.
    cfg_salva: Configuracoes,
    erro_cfg: Option<String>,
    /// O `tema.txt` tem erro (vale o padrão).
    erro_tema: Option<String>,
    /// A aparência que o egui está usando.
    aparencia_aplicada: Option<Aparencia>,
    /// Página de configurações aberta, na seção escolhida.
    configuracoes: Option<Secao>,
    tela: Tela,
    lado: Lado,
    compartilhar: Compartilhar,
    assistir: Assistir,
    /// Monitores desta máquina, para escolher qual compartilhar.
    monitores: Vec<String>,
    /// Saídas de som do Windows (número e nome).
    saidas_som: Vec<(usize, String)>,
    sessao: Option<Sessao>,
    /// Problema no formulário (código errado, programa não encontrado).
    aviso: Option<String>,
    resposta_colada: String,
    /// Mais um espectador: código de conexão, ou endereço + código na mesma rede.
    novo_conexao: String,
    novo_destino: String,
    novo_codigo: String,
}

impl Janela {
    pub fn new(hwnd: Option<isize>, atualizado_agora: bool) -> Self {
        let monitores = fluxo_capture::list_monitors()
            .map(|l| {
                l.iter()
                    .map(|m| format!("Tela {} ({}x{})", m.index + 1, m.width, m.height))
                    .collect()
            })
            .unwrap_or_default();
        let cfg = Configuracoes::carregar();
        let (tema, erro_tema) = crate::tema::carregar();
        Self {
            atualizacao: Arc::new(Mutex::new(Atualizacao::Nada)),
            verificou: false,
            atualizado_agora,
            hwnd,
            tema,
            erro_tema,
            cfg_salva: cfg.clone(),
            cfg,
            erro_cfg: None,
            aparencia_aplicada: None,
            configuracoes: None,
            tela: Tela::Inicio,
            lado: Lado::Compartilhar,
            compartilhar: Compartilhar::default(),
            assistir: Assistir::default(),
            monitores,
            saidas_som: saidas_de_som(),
            sessao: None,
            aviso: None,
            resposta_colada: String::new(),
            novo_conexao: String::new(),
            novo_destino: String::new(),
            novo_codigo: String::new(),
        }
    }

    fn paleta(&self, ctx: &egui::Context) -> Paleta {
        self.tema.paleta(ctx.theme()).clone()
    }

    fn iniciar_host(&mut self, ctx: &egui::Context, modo: Modo) {
        self.compartilhar.modo = modo;
        self.compartilhar.monitor = self.cfg.monitor;
        self.compartilhar.audio = self.cfg.som;
        match self.compartilhar.args(&self.cfg) {
            Ok(args) => self.iniciar(ctx, "host", args, modo),
            Err(e) => self.aviso = Some(e),
        }
    }

    fn iniciar_cliente(&mut self, ctx: &egui::Context, modo: Modo) {
        self.assistir.modo = modo;
        match self.assistir.args(&self.cfg) {
            Ok(args) => self.iniciar(ctx, "client", args, modo),
            Err(e) => self.aviso = Some(e),
        }
    }

    fn iniciar(&mut self, ctx: &egui::Context, programa: &str, mut args: Vec<String>, modo: Modo) {
        // Quem assiste: o vídeo abre no lugar desta janela (ver lugar.rs).
        let lugar_do_video = match (programa, self.hwnd) {
            ("client", Some(h)) => lugar::retangulo(h).map(|r| (h, r)),
            _ => None,
        };
        if let Some((_, (x, y, l, a))) = lugar_do_video {
            args.extend(["--janela".to_string(), format!("{x},{y},{l},{a}")]);
        }
        let ctx = ctx.clone();
        // Onde o vídeo estava ao fechar: a janela volta ali.
        let onde: Arc<Mutex<Option<lugar::Retangulo>>> = Arc::default();
        let avisar: Aviso = Arc::new(move |linha| {
            match (linha, lugar_do_video) {
                (Some(l), _) => {
                    if let Some(r) = lugar::ler_retangulo(l) {
                        *onde.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
                    }
                }
                // O cliente fechou: esta janela volta (com ela escondida, o
                // egui não se redesenharia para perceber).
                (None, Some((h, _))) => lugar::mostrar(h, *onde.lock().unwrap_or_else(|e| e.into_inner())),
                (None, None) => {}
            }
            ctx.request_repaint();
        });
        // Chave e códigos pela entrada padrão, não pela linha de comando.
        let (args, segredos) = crate::comandos::separar_segredos(programa, args);
        let aberto = caminho_do_programa(programa)
            .and_then(|caminho| Processo::iniciar(&caminho, &args, avisar))
            .and_then(|mut p| {
                for linha in &segredos {
                    p.enviar(linha)?;
                }
                Ok(p)
            });
        match aberto {
            Ok(p) => {
                let mut sessao = Sessao::new(p, programa == "host", modo, &self.cfg);
                if let Some((h, _)) = lugar_do_video {
                    lugar::permitir_foco(sessao.processo.pid());
                    lugar::esconder(h);
                    sessao.video_no_lugar = true;
                }
                self.sessao = Some(sessao);
                self.aviso = None;
                self.resposta_colada.clear();
            }
            Err(e) => self.aviso = Some(format!("Não consegui abrir o programa: {e}")),
        }
    }

    /// Procura versão nova numa thread; achando, ela baixa e confere.
    fn verificar_atualizacao(&mut self, ctx: &egui::Context) {
        let estado = self.atualizacao.clone();
        *estado.lock().unwrap_or_else(|e| e.into_inner()) = Atualizacao::Verificando;
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("fss-atualizacao".into())
            .spawn(move || {
                let novo = match atualizacao::verificar_e_baixar() {
                    Ok(Resultado::EmDia) => Atualizacao::EmDia,
                    Ok(Resultado::Disponivel(l)) => Atualizacao::Disponivel {
                        versao: l.versao,
                        pagina: l.pagina,
                    },
                    Ok(Resultado::Pronta(p)) => Atualizacao::Pronta(p),
                    Err(e) => Atualizacao::Erro(e),
                };
                *estado.lock().unwrap_or_else(|e| e.into_inner()) = novo;
                ctx.request_repaint();
            })
            .ok();
    }

    fn estado_atualizacao(&self) -> Atualizacao {
        self.atualizacao.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Atualização baixada e nenhuma transmissão: instala e reinicia.
    fn instalar_se_puder(&mut self) {
        if self.sessao.is_some() {
            return;
        }
        if let Atualizacao::Pronta(p) = self.estado_atualizacao() {
            // Só volta daqui se não deu certo.
            let erro = atualizacao::instalar_e_reiniciar(&p).err().unwrap_or_default();
            *self.atualizacao.lock().unwrap_or_else(|e| e.into_inner()) = Atualizacao::Erro(erro);
        }
    }

    /// Aviso no topo: versão nova chegando, ou acabou de atualizar.
    fn faixa_de_aviso(&mut self, ctx: &egui::Context, p: &Paleta) {
        let texto = match (self.estado_atualizacao(), self.atualizado_agora) {
            (Atualizacao::Pronta(pronta), _) if self.sessao.is_some() => Some(format!(
                "Versão {} baixada: instala sozinha quando a transmissão acabar.",
                pronta.versao
            )),
            (Atualizacao::Disponivel { versao, .. }, _) => Some(format!(
                "Versão {versao} disponível. Esta cópia não se atualiza sozinha: baixe pela página."
            )),
            (_, true) => Some(format!("Atualizado para a versão {}.", atualizacao::VERSAO)),
            _ => None,
        };
        let Some(texto) = texto else { return };
        let mut fechar = false;
        let mut abrir = None;
        egui::TopBottomPanel::top("aviso")
            .frame(
                egui::Frame::new()
                    .fill(p.destaque.gamma_multiply(0.18))
                    .inner_margin(Margin::symmetric(16, 6)),
            )
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(texto).color(p.texto));
                    if let Atualizacao::Disponivel { pagina, .. } = self.estado_atualizacao() {
                        if !pagina.is_empty() && ui.link("Abrir a página").clicked() {
                            abrir = Some(pagina);
                        }
                    }
                    if self.atualizado_agora && ui.small_button("OK").clicked() {
                        fechar = true;
                    }
                });
            });
        if let Some(url) = abrir {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
        if fechar {
            self.atualizado_agora = false;
        }
    }

    // ---------------------------------------------------------------- topo

    fn barra_de_cima(&mut self, ctx: &egui::Context, p: &Paleta) {
        egui::TopBottomPanel::top("topo")
            .frame(
                egui::Frame::new()
                    .fill(p.painel)
                    .inner_margin(Margin::symmetric(16, 10))
                    .stroke(Stroke::new(1.0_f32, p.borda)),
            )
            .show_separator_line(false)
            .show(ctx, |ui| {
                egui::Sides::new().show(
                    ui,
                    |ui| {
                        ui.label(RichText::new("🔥").size(20.0));
                        ui.label(RichText::new(crate::NOME).size(17.0).strong().color(p.texto));
                        if let Some(s) = &self.sessao {
                            if s.rodando() {
                                let (texto, cor) = estado_curto(s, p);
                                ui.add_space(8.0);
                                ui.label(RichText::new(format!("● {texto}")).color(cor).size(13.0));
                            }
                        }
                    },
                    |ui| {
                        let aberta = self.configuracoes.is_some();
                        let rotulo = if aberta {
                            "✖  Fechar"
                        } else {
                            "⚙  Configurações"
                        };
                        if ui
                            .add(
                                egui::Button::new(RichText::new(rotulo).size(14.0))
                                    .min_size(egui::vec2(0.0, 32.0)),
                            )
                            .on_hover_text(if aberta {
                                "Voltar (Esc)"
                            } else {
                                "Padrões, aparência e conexão"
                            })
                            .clicked()
                        {
                            self.configuracoes = match aberta {
                                true => None,
                                false => Some(Secao::Aparencia),
                            };
                        }
                    },
                );
            });
    }

    // -------------------------------------------------------------- início

    fn tela_inicio(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, p: &Paleta) {
        ui.add_space(28.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("Compartilhe sua tela")
                    .size(30.0)
                    .strong()
                    .color(p.texto),
            );
            ui.label(
                RichText::new("Com pouco atraso, direto entre os PCs, até 4 pessoas assistindo.")
                    .size(15.0)
                    .color(p.texto_fraco),
            );
        });
        ui.add_space(24.0);
        let lado_a_lado = ui.available_width() >= 620.0;
        if lado_a_lado {
            ui.columns(2, |c| {
                self.cartao_compartilhar(&mut c[0], ctx, p);
                self.cartao_entrar(&mut c[1], ctx, p);
            });
        } else {
            self.cartao_compartilhar(ui, ctx, p);
            ui.add_space(12.0);
            self.cartao_entrar(ui, ctx, p);
        }
        ui.add_space(16.0);
        ui.vertical_centered(|ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("Outras formas de conectar (sem servidor)").color(p.texto_fraco),
                    )
                    .frame(false),
                )
                .clicked()
            {
                self.tela = Tela::Outras;
                self.aviso = None;
            }
        });
    }

    fn cartao_compartilhar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, p: &Paleta) {
        cartao(ui, p, self.tema.raio, |ui| {
            ui.label(
                RichText::new("🖥  Compartilhar a tela")
                    .size(19.0)
                    .strong()
                    .color(p.texto),
            );
            ui.label(
                RichText::new("Abre uma sala com um código. Quem tiver o código entra sozinho.")
                    .color(p.texto_fraco),
            );
            ui.add_space(6.0);
            if !cfg!(windows) {
                ui.colored_label(
                    p.aviso,
                    "Compartilhar ainda só funciona no Windows; assistir funciona aqui.",
                );
            }
            if self.monitores.len() > 1 {
                escolher_tela(ui, "inicio_tela", &self.monitores, &mut self.cfg.monitor);
            }
            ui.checkbox(&mut self.cfg.som, "Transmitir o som do PC");
            ui.add_space(6.0);
            let tem_chave = self.falta_chave(ui, p);
            let botao = principal(ui, p, "Compartilhar", self.tema.raio, cfg!(windows) && tem_chave);
            if botao.clicked() {
                self.iniciar_host(ctx, Modo::Sala);
            }
        });
    }

    fn cartao_entrar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, p: &Paleta) {
        cartao(ui, p, self.tema.raio, |ui| {
            ui.label(
                RichText::new("▶  Entrar numa sala")
                    .size(19.0)
                    .strong()
                    .color(p.texto),
            );
            ui.label(RichText::new("Digite o código que quem compartilha mandou.").color(p.texto_fraco));
            ui.add_space(6.0);
            let campo = ui.add(
                egui::TextEdit::singleline(&mut self.assistir.sala)
                    .font(egui::FontId::monospace(20.0))
                    .hint_text("XXXX-XXXX")
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(10, 8)),
            );
            if campo.changed() {
                self.assistir.sala = self.assistir.sala.to_uppercase();
            }
            let enter = campo.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(6.0);
            let tem_chave = self.falta_chave(ui, p);
            if (principal(ui, p, "Entrar", self.tema.raio, tem_chave).clicked() || enter) && tem_chave {
                self.iniciar_cliente(ctx, Modo::Sala);
            }
        });
    }

    /// Sem chave do servidor, a sala não funciona: avisa e leva às configurações.
    /// Devolve se há chave.
    fn falta_chave(&mut self, ui: &mut egui::Ui, p: &Paleta) -> bool {
        if !self.cfg.chave.trim().is_empty() {
            return true;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Falta a chave do servidor.").color(p.aviso));
            if ui.link("Colar a chave").clicked() {
                self.configuracoes = Some(Secao::Conexao);
            }
        });
        false
    }

    /// Pela internet trocando códigos, ou na mesma rede (o jeito antigo, sem servidor).
    fn tela_outras(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, p: &Paleta) {
        ui.add_space(16.0);
        if ui.button("Voltar").clicked() {
            self.tela = Tela::Inicio;
            self.aviso = None;
        }
        ui.add_space(8.0);
        cartao(ui, p, self.tema.raio, |ui| {
            ui.label(
                RichText::new("Outras formas de conectar")
                    .size(19.0)
                    .strong()
                    .color(p.texto),
            );
            ui.label(
                RichText::new(
                    "Sem o servidor das salas: os dois lados trocam códigos, ou estão na mesma rede.",
                )
                .color(p.texto_fraco),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.lado, Lado::Compartilhar, "Vou compartilhar");
                ui.selectable_value(&mut self.lado, Lado::Assistir, "Vou assistir");
            });
            ui.add_space(4.0);
            match self.lado {
                Lado::Compartilhar => {
                    let c = &mut self.compartilhar;
                    if c.modo == Modo::Sala {
                        c.modo = Modo::Internet;
                    }
                    ui.radio_value(&mut c.modo, Modo::Internet, "Pela internet, trocando códigos");
                    ui.radio_value(&mut c.modo, Modo::Rede, "Na mesma rede");
                    ui.add_space(4.0);
                    if c.modo == Modo::Internet {
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
                    if self.monitores.len() > 1 {
                        escolher_tela(ui, "outras_tela", &self.monitores, &mut self.cfg.monitor);
                    }
                    ui.checkbox(&mut self.cfg.som, "Transmitir o som do PC");
                    ui.add_space(6.0);
                    let modo = self.compartilhar.modo;
                    if principal(ui, p, "Começar", self.tema.raio, cfg!(windows)).clicked() {
                        self.iniciar_host(ctx, modo);
                    }
                }
                Lado::Assistir => {
                    let a = &mut self.assistir;
                    if a.modo == Modo::Sala {
                        a.modo = Modo::Internet;
                    }
                    ui.radio_value(&mut a.modo, Modo::Internet, "Pela internet, trocando códigos");
                    ui.radio_value(&mut a.modo, Modo::Rede, "Na mesma rede");
                    ui.label(
                        RichText::new(
                            "Depois de começar, a janela mostra o código para mandar a quem compartilha.",
                        )
                        .color(p.texto_fraco),
                    );
                    ui.add_space(6.0);
                    let modo = self.assistir.modo;
                    if principal(ui, p, "Começar", self.tema.raio, true).clicked() {
                        self.iniciar_cliente(ctx, modo);
                    }
                }
            }
        });
    }

    // ------------------------------------------------------------- sessão

    /// O meio da janela durante a transmissão: o código, o estado, os avisos.
    fn painel_sessao(&mut self, ui: &mut egui::Ui, p: &Paleta) {
        let raio = self.tema.raio;
        let Some(s) = self.sessao.as_mut() else {
            return;
        };
        let mut enviar_resposta = false;
        ui.add_space(20.0);

        // Quem compartilha, sala: o código grande, porque mais gente pode
        // entrar a qualquer momento.
        if let (Some(c), true) = (&s.sala, s.rodando()) {
            cartao(ui, p, raio, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("Código da sala").color(p.texto_fraco));
                    ui.label(RichText::new(c).monospace().size(44.0).strong().color(p.texto));
                    if ui.add(botao_comum("📋  Copiar código", raio)).clicked() {
                        ui.ctx().copy_text(c.clone());
                    }
                    ui.label(
                        RichText::new(format!(
                            "Mande para quem vai assistir. Até {MAX_ESPECTADORES} pessoas; cada uma usa mais uma cópia do vídeo no seu upload."
                        ))
                        .color(p.texto_fraco)
                        .size(13.0),
                    );
                });
            });
            ui.add_space(12.0);
        }

        // Códigos das outras formas de conectar.
        let mut codigos = Vec::new();
        if s.recebendo.is_none() {
            if let Some(c) = &s.pareamento {
                codigos.push((
                    "Mande este código para quem vai compartilhar:".to_string(),
                    c.clone(),
                ));
                if let Some(d) = &s.destino {
                    codigos.push(("E este endereço:".to_string(), d.clone()));
                }
            }
        }
        if let (Some(c), false) = (&s.resposta, s.transmitindo) {
            codigos.push((
                "Mande este código de resposta para quem vai assistir:".to_string(),
                c.clone(),
            ));
        }
        for (n, c) in &s.respostas {
            codigos.push((
                format!("Mande este código de resposta para o espectador {n}:"),
                c.clone(),
            ));
        }
        let pedir_resposta = s.recebendo.is_none() && s.conexao.is_some();
        if !codigos.is_empty() || pedir_resposta {
            cartao(ui, p, raio, |ui| {
                if let (Some(c), true) = (&s.conexao, pedir_resposta) {
                    codigo(ui, p, "Mande este código para quem vai compartilhar:", c);
                    ui.add_space(6.0);
                    ui.label("Depois cole aqui o código de resposta que ele mandar de volta:");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.resposta_colada)
                                .desired_width(ui.available_width() - 80.0)
                                .hint_text("código de resposta"),
                        );
                        enviar_resposta = ui.button("Enviar").clicked();
                    });
                }
                for (titulo, c) in &codigos {
                    codigo(ui, p, titulo, c);
                }
            });
            ui.add_space(12.0);
        }
        if enviar_resposta {
            match linha_de_resposta(&self.resposta_colada) {
                Ok(l) => {
                    s.mandar(&l);
                    self.resposta_colada.clear();
                    self.aviso = None;
                }
                Err(e) => self.aviso = Some(e),
            }
        }

        // O estado.
        cartao(ui, p, raio, |ui| {
            let (titulo, cor) = estado_longo(s, p);
            ui.horizontal(|ui| {
                if s.rodando() && !s.transmitindo && s.recebendo.is_none() {
                    ui.spinner();
                }
                ui.label(RichText::new(titulo).size(17.0).strong().color(cor));
            });
            if let Some(e) = &s.erro {
                ui.colored_label(p.perigo, e);
            }
            if s.host && s.transmitindo && s.rodando() {
                let mut linha = vec![format!("Tela {}", s.monitor + 1)];
                linha.push(if s.som {
                    "som ligado".into()
                } else {
                    "sem som".into()
                });
                if let Some(q) = &s.qualidade {
                    linha.push(q.clone());
                }
                ui.label(RichText::new(linha.join(" · ")).color(p.texto_fraco));
                if let Some(r) = &s.relatorio {
                    ui.label(RichText::new(r).color(p.texto_fraco).size(13.0));
                }
            }
            if let (Some(v), true) = (&s.recebendo, s.rodando()) {
                ui.label(
                    RichText::new("O vídeo está na outra janela (F11: tela cheia, F2: estatísticas).")
                        .color(p.texto_fraco),
                );
                ui.label(RichText::new(v).color(p.texto_fraco).size(13.0));
            }
            if let Some(a) = &s.aviso {
                ui.colored_label(p.aviso, a);
            }
        });

        // Mais um espectador, nas formas sem sala.
        if s.host && s.transmitindo && s.rodando() && s.modo != Modo::Sala {
            ui.add_space(12.0);
            cartao(ui, p, raio, |ui| {
                egui::CollapsingHeader::new("Adicionar espectador").show(ui, |ui| {
                    ui.label(
                        RichText::new(format!(
                            "Até {MAX_ESPECTADORES}. Cada um usa mais uma cópia do vídeo no seu upload."
                        ))
                        .color(p.texto_fraco),
                    );
                    let linha = if s.modo == Modo::Internet {
                        ui.label("Código de conexão dele:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.novo_conexao)
                                .desired_width(f32::INFINITY)
                                .hint_text("cole aqui"),
                        );
                        ui.button("Adicionar")
                            .clicked()
                            .then(|| linha_espectador_internet(&self.novo_conexao))
                    } else {
                        ui.label("Endereço e código dele:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.novo_destino)
                                .desired_width(f32::INFINITY)
                                .hint_text("192.168.0.21:9000"),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut self.novo_codigo)
                                .desired_width(f32::INFINITY)
                                .hint_text("XXXX-XXXX"),
                        );
                        ui.button("Adicionar")
                            .clicked()
                            .then(|| linha_espectador_rede(&self.novo_destino, &self.novo_codigo))
                    };
                    match linha {
                        Some(Ok(l)) => {
                            s.mandar(&l);
                            s.aviso = None;
                            self.novo_conexao.clear();
                            self.novo_destino.clear();
                            self.novo_codigo.clear();
                        }
                        Some(Err(e)) => self.aviso = Some(e),
                        None => {}
                    }
                });
            });
        }

        ui.add_space(12.0);
        egui::CollapsingHeader::new(RichText::new("Detalhes").color(p.texto_fraco)).show(ui, |ui| {
            if ui.small_button("Copiar detalhes").clicked() {
                let texto: Vec<&str> = s.registro.iter().map(String::as_str).collect();
                ui.ctx().copy_text(texto.join("\n"));
            }
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for l in &s.registro {
                        ui.label(RichText::new(l).monospace().size(11.0).color(p.texto_fraco));
                    }
                });
        });
    }

    /// A barra de baixo, como no Meet: os botões no meio, sair à direita.
    fn barra_de_baixo(&mut self, ctx: &egui::Context, p: &Paleta) {
        let raio = self.tema.raio;
        egui::TopBottomPanel::bottom("barra")
            .frame(
                egui::Frame::new()
                    .fill(p.painel)
                    .inner_margin(Margin::symmetric(16, 12))
                    .stroke(Stroke::new(1.0_f32, p.borda)),
            )
            .show_separator_line(false)
            .show(ctx, |ui| {
                let Some(s) = self.sessao.as_mut() else {
                    return;
                };
                let (rodando, host) = (s.rodando(), s.host);
                let mut sair = false;
                egui::Sides::new().show(
                    ui,
                    |ui| {
                        if !rodando {
                            return;
                        }
                        if s.host {
                            controles_host(ui, s, p, raio, &self.monitores);
                        } else {
                            controles_cliente(ui, s, p, raio);
                        }
                    },
                    |ui| {
                        let (texto, botao) = match (rodando, host) {
                            (false, _) => ("Voltar", botao_comum("Voltar", raio)),
                            (true, true) => ("Encerrar", botao_perigo(p, "⏹  Encerrar", raio)),
                            (true, false) => ("Sair", botao_perigo(p, "⏏  Sair", raio)),
                        };
                        sair = ui.add(botao).on_hover_text(texto).clicked();
                    },
                );
                if sair {
                    self.sessao = None; // soltar encerra o programa
                }
            });
    }

    // ------------------------------------------------------ configurações

    fn pagina_configuracoes(&mut self, ctx: &egui::Context, p: &Paleta) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.configuracoes = None;
            return;
        }
        let mut secao = self.configuracoes.unwrap_or(Secao::Aparencia);
        egui::SidePanel::left("secoes")
            .resizable(false)
            .exact_width(190.0)
            .frame(
                egui::Frame::new()
                    .fill(p.painel)
                    .inner_margin(Margin::symmetric(10, 16)),
            )
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("CONFIGURAÇÕES")
                        .size(11.0)
                        .strong()
                        .color(p.texto_fraco),
                );
                ui.add_space(4.0);
                for s in Secao::TODAS {
                    let escolhida = s == secao;
                    let texto = RichText::new(s.rotulo()).size(15.0).color(if escolhida {
                        p.texto
                    } else {
                        p.texto_fraco
                    });
                    let b = egui::Button::new(texto)
                        .fill(if escolhida {
                            p.controle
                        } else {
                            Color32::TRANSPARENT
                        })
                        .corner_radius(CornerRadius::same(self.tema.raio))
                        .min_size(egui::vec2(ui.available_width(), 34.0));
                    if ui.add(b).clicked() {
                        secao = s;
                    }
                }
            });
        self.configuracoes = Some(secao);
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                centralizar(ui, 600.0, |ui| {
                    ui.add_space(16.0);
                    ui.label(RichText::new(secao.rotulo()).size(24.0).strong().color(p.texto));
                    ui.add_space(8.0);
                    match secao {
                        Secao::Aparencia => self.secao_aparencia(ui, p),
                        Secao::Transmissao => self.secao_transmissao(ui, p),
                        Secao::Assistir => self.secao_assistir(ui, p),
                        Secao::Conexao => self.secao_conexao(ui, p),
                        Secao::Sobre => self.secao_sobre(ui, p),
                    }
                    ui.add_space(24.0);
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        let onde = config::caminho()
                            .map(|c| format!("Guardadas em {}", c.display()))
                            .unwrap_or_else(|| "Sem pasta para guardar as configurações".into());
                        ui.label(RichText::new(onde).size(12.0).color(p.texto_fraco));
                        if ui.small_button("Restaurar padrões").clicked() {
                            self.cfg = Configuracoes::default();
                        }
                    });
                    if let Some(e) = &self.erro_cfg {
                        ui.colored_label(p.perigo, e);
                    }
                });
            });
        });
    }

    fn secao_aparencia(&mut self, ui: &mut egui::Ui, p: &Paleta) {
        titulo_campo(ui, p, "Tema", "Muda na hora.");
        ui.horizontal_wrapped(|ui| {
            for a in Aparencia::TODAS {
                let escolhido = self.cfg.aparencia == a;
                let b = egui::Button::new(RichText::new(a.rotulo()).size(15.0).color(if escolhido {
                    p.sobre_destaque
                } else {
                    p.texto
                }))
                .fill(if escolhido { p.destaque } else { p.controle })
                .corner_radius(CornerRadius::same(self.tema.raio))
                .min_size(egui::vec2(150.0, 44.0));
                if ui.add(b).clicked() {
                    self.cfg.aparencia = a;
                }
            }
        });
        ui.add_space(12.0);
        titulo_campo(
            ui,
            p,
            "Tema personalizado",
            &format!(
                "Tema atual: {}. Para usar cores próprias, crie um tema.txt na pasta das configurações \
                 (o botão abaixo cria um com as cores atuais), mude as cores e clique em Recarregar.",
                self.tema.nome
            ),
        );
        ui.horizontal_wrapped(|ui| {
            if let Some(c) = crate::tema::caminho_personalizado() {
                if !c.exists() && ui.button("Criar tema.txt").clicked() {
                    let texto = format!(
                        "# Tema do FlameScreenSharing: mude as cores (#RRGGBB) e clique em Recarregar.\n\
                         # O que faltar vem do tema padrão. Apague este arquivo para voltar ao padrão.\n{}",
                        self.tema.para_texto()
                    );
                    let gravado = c
                        .parent()
                        .map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|_| std::fs::write(&c, texto));
                    self.erro_tema = gravado
                        .err()
                        .map(|e| format!("Não consegui criar o tema.txt: {e}"));
                }
            }
            if ui.button("Recarregar").clicked() {
                let (tema, erro) = crate::tema::carregar();
                self.tema = tema;
                self.erro_tema = erro;
                self.aparencia_aplicada = None;
            }
            if ui
                .button("Usar o padrão")
                .on_hover_text("Ignora o tema.txt até recarregar")
                .clicked()
            {
                self.tema = Tema::padrao();
                self.erro_tema = None;
                self.aparencia_aplicada = None;
            }
        });
        if let Some(c) = crate::tema::caminho_personalizado() {
            ui.label(
                RichText::new(c.display().to_string())
                    .size(12.0)
                    .color(p.texto_fraco),
            );
        }
        if let Some(e) = &self.erro_tema {
            ui.colored_label(p.perigo, e);
        }
    }

    fn secao_transmissao(&mut self, ui: &mut egui::Ui, p: &Paleta) {
        ui.label(
            RichText::new("Valem a partir da próxima transmissão. Durante uma, a barra de baixo troca tela, som e qualidade na hora.")
                .color(p.texto_fraco),
        );
        ui.add_space(8.0);
        let c = &mut self.cfg;
        egui::Grid::new("transmissao")
            .num_columns(2)
            .spacing([24.0, 12.0])
            .show(ui, |ui| {
                if !self.monitores.is_empty() {
                    ui.label("Tela");
                    escolher_tela(ui, "cfg_tela", &self.monitores, &mut c.monitor);
                    ui.end_row();
                }
                ui.label("Resolução");
                egui::ComboBox::from_id_salt("cfg_resolucao")
                    .selected_text(RESOLUCOES[c.resolucao.min(RESOLUCOES.len() - 1)].0)
                    .show_ui(ui, |ui| {
                        for (i, (nome, _)) in RESOLUCOES.iter().enumerate() {
                            ui.selectable_value(&mut c.resolucao, i, *nome);
                        }
                    });
                ui.end_row();
                ui.label("Quadros por segundo");
                egui::ComboBox::from_id_salt("cfg_fps")
                    .selected_text(format!("{} fps", c.fps))
                    .show_ui(ui, |ui| {
                        for f in FPS {
                            ui.selectable_value(&mut c.fps, f, format!("{f} fps"));
                        }
                    });
                ui.end_row();
                ui.label("Codec");
                egui::ComboBox::from_id_salt("cfg_codec")
                    .selected_text(CODECS[c.codec.min(CODECS.len() - 1)].0)
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        for (i, (nome, _)) in CODECS.iter().enumerate() {
                            ui.selectable_value(&mut c.codec, i, *nome);
                        }
                    });
                ui.end_row();
                ui.label("Encoder");
                egui::ComboBox::from_id_salt("cfg_encoder")
                    .selected_text(ENCODERS[c.encoder.min(ENCODERS.len() - 1)].0)
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        for (i, (nome, _)) in ENCODERS.iter().enumerate() {
                            ui.selectable_value(&mut c.encoder, i, *nome);
                        }
                    });
                ui.end_row();
                ui.label("Bitrate máximo");
                ui.horizontal(|ui| {
                    let mut auto = c.mbps == 0;
                    if ui.checkbox(&mut auto, "Automático").changed() {
                        c.mbps = if auto { 0 } else { 20 };
                    }
                    if !auto {
                        ui.add(egui::Slider::new(&mut c.mbps, MBPS).suffix(" Mbps"));
                    }
                });
                ui.end_row();
                ui.label("Som");
                ui.checkbox(&mut c.som, "Começar com o som do PC ligado");
                ui.end_row();
                if !self.saidas_som.is_empty() {
                    ui.label("Capturar o som de");
                    escolher_saida(ui, "cfg_som_de", &self.saidas_som, &mut c.som_de);
                    ui.end_row();
                }
            });
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Automático escolhe o melhor codec que os dois PCs aceitam (AV1, depois HEVC, depois H.264) \
                 e o bitrate pela resolução e pelos fps; o bitrate ainda desce sozinho se a rede reclamar.",
            )
            .size(13.0)
            .color(p.texto_fraco),
        );
    }

    fn secao_assistir(&mut self, ui: &mut egui::Ui, p: &Paleta) {
        ui.label(RichText::new("Valem a partir da próxima vez que entrar numa sala.").color(p.texto_fraco));
        ui.add_space(8.0);
        let c = &mut self.cfg;
        egui::Grid::new("assistir")
            .num_columns(2)
            .spacing([24.0, 12.0])
            .show(ui, |ui| {
                ui.label("Volume inicial");
                ui.add(egui::Slider::new(&mut c.volume, 0..=100).suffix("%"));
                ui.end_row();
                if !self.saidas_som.is_empty() {
                    ui.label("Tocar o som em");
                    escolher_saida(ui, "cfg_tocar_em", &self.saidas_som, &mut c.tocar_em);
                    ui.end_row();
                }
                ui.label("Estatísticas");
                ui.checkbox(&mut c.estatisticas, "Mostrar sobre o vídeo (F2 muda)");
                ui.end_row();
                ui.label("Janela do vídeo");
                ui.checkbox(&mut c.sempre_no_topo, "Sempre por cima das outras");
                ui.end_row();
            });
    }

    fn secao_sobre(&mut self, ui: &mut egui::Ui, p: &Paleta) {
        titulo_campo(
            ui,
            p,
            &format!("{} {}", crate::NOME, atualizacao::VERSAO),
            "Compartilhamento de tela com pouco atraso, direto entre os PCs.",
        );
        ui.add_space(12.0);
        titulo_campo(
            ui,
            p,
            "Atualizações",
            "Ao abrir, o programa procura versão nova no GitHub. Achando, baixa, confere e instala sozinho, \
             e abre de novo (se houver transmissão, espera ela acabar).",
        );
        ui.checkbox(
            &mut self.cfg.atualizar_sozinho,
            "Procurar e instalar atualizações sozinho",
        );
        let estado = self.estado_atualizacao();
        let (texto, cor) = match &estado {
            Atualizacao::Nada => ("Ainda não verificou.".to_string(), p.texto_fraco),
            Atualizacao::Verificando => ("Procurando…".to_string(), p.texto_fraco),
            Atualizacao::EmDia => ("Esta é a versão mais nova.".to_string(), p.sucesso),
            Atualizacao::Disponivel { versao, .. } => (format!("Versão {versao} disponível."), p.aviso),
            Atualizacao::Pronta(pr) => (
                format!("Versão {} baixada, pronta para instalar.", pr.versao),
                p.sucesso,
            ),
            Atualizacao::Erro(e) => (format!("Não deu para atualizar: {e}"), p.perigo),
        };
        ui.horizontal(|ui| {
            if matches!(estado, Atualizacao::Verificando) {
                ui.spinner();
            }
            ui.label(RichText::new(texto).color(cor));
        });
        let ocupado = matches!(estado, Atualizacao::Verificando);
        if ui
            .add_enabled(!ocupado, egui::Button::new("Verificar agora"))
            .clicked()
        {
            self.verificar_atualizacao(ui.ctx());
        }
        if let Atualizacao::Pronta(pr) = &estado {
            if !pr.notas.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new("Novidades").strong());
                ui.label(RichText::new(&pr.notas).color(p.texto_fraco));
            }
        }
    }

    fn secao_conexao(&mut self, ui: &mut egui::Ui, p: &Paleta) {
        titulo_campo(
            ui,
            p,
            "Chave do servidor",
            "As salas precisam de um servidor onde os dois PCs se encontram (o vídeo não passa por ele). \
             Quem roda o servidor gera uma chave e manda junto com o programa; cole aqui.",
        );
        ui.add(
            egui::TextEdit::multiline(&mut self.cfg.chave)
                .hint_text("FSS1-…")
                .font(egui::FontId::monospace(14.0))
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        if self.cfg.chave.trim().is_empty() {
            ui.label(
                RichText::new("Sem chave, só as outras formas de conectar funcionam (trocando códigos ou na mesma rede).")
                    .color(p.texto_fraco),
            );
        } else {
            match crate::comandos::ler_chave(&self.cfg.chave) {
                Ok(k) => {
                    ui.colored_label(p.sucesso, format!("Chave válida · servidor {}", k.server()));
                }
                Err(e) => {
                    ui.colored_label(p.perigo, e);
                }
            }
        }
        ui.label(
            RichText::new("Quem compartilha e quem assiste precisam da mesma chave.")
                .size(13.0)
                .color(p.texto_fraco),
        );
    }
}

// ------------------------------------------------------------ peças soltas

/// Os botões de quem compartilha: tela, som, pausa, qualidade.
fn controles_host(ui: &mut egui::Ui, s: &mut Sessao, p: &Paleta, raio: u8, monitores: &[String]) {
    ui.horizontal(|ui| {
        let tela = botao_comum(&format!("🖥  Tela {}  ⏷", s.monitor + 1), raio);
        egui::containers::menu::MenuButton::from_button(tela).ui(ui, |ui| {
            if monitores.is_empty() {
                ui.label("Nenhuma tela encontrada");
            }
            for (i, nome) in monitores.iter().enumerate() {
                if ui.selectable_label(i == s.monitor, nome).clicked() {
                    s.mandar(&format!("tela {i}"));
                    ui.close();
                }
            }
        });
        let (rotulo, botao) = match s.som {
            true => ("Desligar o som do PC", botao_comum("🔊  Som", raio)),
            false => ("Ligar o som do PC", botao_desligado(p, "🔇  Sem som", raio)),
        };
        if ui.add(botao).on_hover_text(rotulo).clicked() {
            let novo = !s.som;
            s.mandar(if novo { "som ligado" } else { "som desligado" });
        }
        let (rotulo, botao) = match s.pausado {
            false => (
                "A última imagem fica parada para quem assiste",
                botao_comum("⏸  Pausar", raio),
            ),
            true => ("Voltar a transmitir", botao_desligado(p, "▶  Continuar", raio)),
        };
        if ui.add(botao).on_hover_text(rotulo).clicked() {
            s.mandar(if s.pausado { "continuar" } else { "pausar" });
        }
        let qualidade = botao_comum("⚙  Qualidade  ⏷", raio);
        egui::containers::menu::MenuButton::from_button(qualidade).ui(ui, |ui| {
            ui.label(RichText::new("Resolução").strong());
            for (nome, valor) in RESOLUCOES {
                if ui.button(nome).clicked() {
                    s.mandar(&format!("resolucao {valor}"));
                    ui.close();
                }
            }
            ui.separator();
            ui.label(RichText::new("Quadros por segundo").strong());
            for f in FPS {
                if ui.button(format!("{f} fps")).clicked() {
                    s.mandar(&format!("fps {f}"));
                    ui.close();
                }
            }
        });
    });
}

/// Os botões de quem assiste: mudo e volume.
fn controles_cliente(ui: &mut egui::Ui, s: &mut Sessao, p: &Paleta, raio: u8) {
    ui.horizontal(|ui| {
        let botao = match s.mudo || s.volume == 0 {
            false => botao_comum("🔊", raio),
            true => botao_desligado(p, "🔇", raio),
        };
        if ui
            .add(botao)
            .on_hover_text(if s.mudo { "Ligar o som" } else { "Sem som" })
            .clicked()
        {
            s.mudo = !s.mudo;
        }
        ui.add_enabled(
            !s.mudo,
            egui::Slider::new(&mut s.volume, 0..=100).show_value(false),
        )
        .on_hover_text(format!("Volume: {}%", s.volume));
        ui.label(RichText::new(format!("{}%", if s.mudo { 0 } else { s.volume })).color(p.texto_fraco));
    });
    let agora = if s.mudo { 0 } else { s.volume };
    if agora != s.volume_enviado {
        s.volume_enviado = agora;
        s.mandar(&format!("volume {agora}"));
    }
}

/// Estado em poucas palavras, para a barra de cima.
fn estado_curto(s: &Sessao, p: &Paleta) -> (String, Color32) {
    match (s.host, s.transmitindo, &s.recebendo) {
        (true, true, _) if s.pausado => ("Pausado".into(), p.aviso),
        (true, true, _) => (
            match s.assistindo {
                1 => "Ao vivo · 1 assistindo".to_string(),
                n => format!("Ao vivo · {n} assistindo"),
            },
            p.sucesso,
        ),
        (false, _, Some(_)) => ("Assistindo".into(), p.sucesso),
        _ => ("Conectando".into(), p.aviso),
    }
}

/// Estado por extenso, para o cartão do meio.
fn estado_longo(s: &Sessao, p: &Paleta) -> (String, Color32) {
    if s.erro.is_some() {
        return ("Parou com um erro".into(), p.perigo);
    }
    if s.fim.is_some() {
        return ("Terminou".into(), p.texto_fraco);
    }
    match (s.host, s.transmitindo, &s.recebendo) {
        (true, true, _) if s.pausado => ("Pausado: quem assiste vê a última imagem".into(), p.aviso),
        (true, true, _) => (
            match s.assistindo {
                0 => "Transmitindo: ninguém assistindo agora".to_string(),
                1 => "Transmitindo para 1 pessoa".to_string(),
                n => format!("Transmitindo para {n} pessoas"),
            },
            p.sucesso,
        ),
        (false, _, Some(_)) => ("Assistindo".into(), p.sucesso),
        (true, false, _) if s.sala.is_some() => ("Esperando a primeira pessoa entrar…".into(), p.texto),
        (false, _, None) if s.entrando_na_sala => ("Esperando quem compartilha…".into(), p.texto),
        _ => ("Esperando o outro lado…".into(), p.texto),
    }
}

/// Um cartão: fundo próprio, cantos arredondados, conteúdo com respiro.
fn cartao<R>(ui: &mut egui::Ui, p: &Paleta, raio: u8, conteudo: impl FnOnce(&mut egui::Ui) -> R) -> R {
    // A largura de fora, menos a margem de dentro e a borda dos dois lados.
    let largura = (ui.available_width() - 2.0 * 20.0 - 2.0).max(0.0);
    egui::Frame::new()
        .fill(p.cartao)
        .stroke(Stroke::new(1.0_f32, p.borda))
        .corner_radius(CornerRadius::same(raio.saturating_add(4)))
        .inner_margin(Margin::same(20))
        .show(ui, |ui| {
            ui.set_width(largura);
            conteudo(ui)
        })
        .inner
}

/// Conteúdo no meio, com largura máxima.
fn centralizar<R>(ui: &mut egui::Ui, largura: f32, conteudo: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let livre = ui.available_width();
    let largura = largura.min(livre);
    let canto = ui.cursor().min + egui::vec2((livre - largura) / 2.0, 0.0);
    let area = egui::Rect::from_min_size(canto, egui::vec2(largura, ui.available_height().max(0.0)));
    let construtor = egui::UiBuilder::new()
        .max_rect(area)
        .layout(egui::Layout::top_down(egui::Align::Min));
    ui.scope_builder(construtor, |ui| {
        ui.set_width(largura);
        conteudo(ui)
    })
    .inner
}

fn titulo_campo(ui: &mut egui::Ui, p: &Paleta, titulo: &str, explicacao: &str) {
    ui.label(RichText::new(titulo).size(15.0).strong().color(p.texto));
    ui.label(RichText::new(explicacao).color(p.texto_fraco));
}

/// O botão principal (cor de destaque), da largura disponível.
fn principal(ui: &mut egui::Ui, p: &Paleta, texto: &str, raio: u8, ativo: bool) -> egui::Response {
    let botao = egui::Button::new(RichText::new(texto).size(16.0).strong().color(p.sobre_destaque))
        .fill(p.destaque)
        .corner_radius(CornerRadius::same(raio));
    let tamanho = egui::vec2(ui.available_width(), 42.0);
    ui.add_enabled_ui(ativo, |ui| ui.add_sized(tamanho, botao)).inner
}

fn botao_comum(texto: &str, raio: u8) -> egui::Button<'static> {
    egui::Button::new(RichText::new(texto.to_string()).size(15.0))
        .corner_radius(CornerRadius::same(raio))
        .min_size(egui::vec2(44.0, 40.0))
}

/// Algo desligado (som, vídeo pausado): vermelho, como o microfone no Meet.
fn botao_desligado(p: &Paleta, texto: &str, raio: u8) -> egui::Button<'static> {
    egui::Button::new(RichText::new(texto.to_string()).size(15.0).color(Color32::WHITE))
        .fill(p.perigo.gamma_multiply(0.85))
        .corner_radius(CornerRadius::same(raio))
        .min_size(egui::vec2(44.0, 40.0))
}

fn botao_perigo(p: &Paleta, texto: &str, raio: u8) -> egui::Button<'static> {
    egui::Button::new(
        RichText::new(texto.to_string())
            .size(15.0)
            .strong()
            .color(Color32::WHITE),
    )
    .fill(p.perigo)
    .corner_radius(CornerRadius::same(raio))
    .min_size(egui::vec2(110.0, 40.0))
}

fn escolher_tela(ui: &mut egui::Ui, id: &str, monitores: &[String], monitor: &mut usize) {
    let atual = monitores
        .get(*monitor)
        .cloned()
        .unwrap_or_else(|| format!("Tela {}", *monitor + 1));
    egui::ComboBox::from_id_salt(id)
        .selected_text(atual)
        .width(220.0)
        .show_ui(ui, |ui| {
            for (i, nome) in monitores.iter().enumerate() {
                ui.selectable_value(monitor, i, nome);
            }
        });
}

fn escolher_saida(ui: &mut egui::Ui, id: &str, saidas: &[(usize, String)], escolhida: &mut Option<usize>) {
    let atual = match escolhida {
        None => "A padrão do Windows".to_string(),
        Some(n) => saidas
            .iter()
            .find(|(i, _)| i == n)
            .map(|(_, nome)| nome.clone())
            .unwrap_or_else(|| format!("Saída {n} (não encontrada)")),
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(atual)
        .width(280.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(escolhida, None, "A padrão do Windows");
            for (i, nome) in saidas {
                ui.selectable_value(escolhida, Some(*i), nome);
            }
        });
}

/// Um código em letra grande, com o botão de copiar.
fn codigo(ui: &mut egui::Ui, p: &Paleta, titulo: &str, texto: &str) {
    ui.label(RichText::new(titulo).color(p.texto_fraco));
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(texto)
                .monospace()
                .size(18.0)
                .strong()
                .color(p.texto),
        );
        if ui.button("📋 Copiar").clicked() {
            ui.ctx().copy_text(texto.to_string());
        }
    });
    ui.add_space(4.0);
}

/// As saídas de som do Windows; fora dele, nenhuma (o campo some).
fn saidas_de_som() -> Vec<(usize, String)> {
    #[cfg(windows)]
    {
        fluxo_audio::output_devices()
            .map(|l| l.into_iter().map(|d| (d.index, d.name)).collect())
            .unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

impl eframe::App for Janela {
    /// Fechar a janela encerra o host ou o cliente que estiver rodando.
    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
        self.sessao = None;
    }

    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if self.aparencia_aplicada != Some(self.cfg.aparencia) {
            self.tema.aplicar(ctx, self.cfg.aparencia);
            self.aparencia_aplicada = Some(self.cfg.aparencia);
        }
        if let Some(s) = self.sessao.as_mut() {
            s.atualizar();
            // Quem assiste saiu (Sair ou fechou o vídeo): de volta ao início.
            // Com erro, a sessão fica, para mostrar o que houve.
            if s.video_no_lugar && s.fim == Some(true) && s.erro.is_none() {
                self.sessao = None;
            }
        }
        if self.sessao.is_some() {
            // O programa pode terminar sem imprimir nada.
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if !self.verificou {
            self.verificou = true;
            if self.cfg.atualizar_sozinho {
                self.verificar_atualizacao(ctx);
            }
        }
        self.instalar_se_puder();
        let p = self.paleta(ctx);

        self.barra_de_cima(ctx, &p);
        self.faixa_de_aviso(ctx, &p);
        if self.configuracoes.is_some() {
            self.pagina_configuracoes(ctx, &p);
        } else {
            if self.sessao.is_some() {
                self.barra_de_baixo(ctx, &p);
            }
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(p.fundo)
                        .inner_margin(Margin::symmetric(16, 0)),
                )
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        centralizar(ui, LARGURA_CONTEUDO, |ui| {
                            if self.sessao.is_some() {
                                self.painel_sessao(ui, &p);
                            } else {
                                match self.tela {
                                    Tela::Inicio => self.tela_inicio(ui, ctx, &p),
                                    Tela::Outras => self.tela_outras(ui, ctx, &p),
                                }
                            }
                            if let Some(a) = &self.aviso {
                                ui.add_space(8.0);
                                ui.colored_label(p.perigo, a);
                            }
                            ui.add_space(16.0);
                        });
                    });
                });
        }

        if self.cfg != self.cfg_salva {
            self.erro_cfg = self
                .cfg
                .salvar()
                .err()
                .map(|e| format!("Não consegui guardar as configurações: {e}"));
            self.cfg_salva = self.cfg.clone();
        }
    }
}
