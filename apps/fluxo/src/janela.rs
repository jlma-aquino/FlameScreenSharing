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

use std::collections::{HashMap, VecDeque};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};

use crate::atualizacao::{self, Pronta, Resultado, Versao};
use crate::comandos::{
    linha_de_resposta, linha_espectador_internet, linha_espectador_rede, Assistir, Compartilhar, Modo, FPS,
    RESOLUCOES,
};
use crate::config::{self, Configuracoes, CODECS, ENCODERS, MBPS};
use crate::grupo::{self, Grupo};
use crate::lugar;
use crate::perfil::{self, Fotos};
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
    Perfil,
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
    const TODAS: [Secao; 6] = [
        Secao::Perfil,
        Secao::Aparencia,
        Secao::Transmissao,
        Secao::Assistir,
        Secao::Conexao,
        Secao::Sobre,
    ];

    fn rotulo(self) -> &'static str {
        match self {
            Secao::Perfil => "Perfil",
            Secao::Aparencia => "Aparência",
            Secao::Transmissao => "Compartilhar",
            Secao::Assistir => "Assistir",
            Secao::Conexao => "Conexão",
            Secao::Sobre => "Sobre",
        }
    }
}

/// A foto escolhida chegando da thread: `None` se a pessoa desistiu.
type FotoEscolhida = Option<Result<Vec<u8>, String>>;

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
    /// Host: números de quem está assistindo (para expulsar).
    espectadores: Vec<u32>,
    /// Host: o apelido de cada espectador que mandou (FSS com grupo).
    nomes: HashMap<u32, String>,
    /// Host: o número de membro do grupo de cada espectador (para a foto).
    membros: HashMap<u32, u64>,
    /// Host: desde quando cada espectador está assistindo.
    desde: HashMap<u32, Instant>,
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
    /// Host: janela transmitida, no lugar do monitor.
    janela: Option<isize>,
    /// Host: o som do PC está indo.
    som: bool,
    /// Host: vídeo pausado.
    pausado: bool,
    /// Host: resolução e fps depois de uma mudança.
    qualidade: Option<String>,
    /// Host: codec, resolução, fps e bitrate valendo agora.
    configuracao: Option<String>,
    /// Host: resolução (valor de `--resolucao`) e fps escolhidos, para marcar no menu.
    resolucao_atual: &'static str,
    fps_atual: u32,
    /// Cliente: volume escolhido (o mudo não mexe nele).
    volume: u32,
    mudo: bool,
    /// Cliente: último volume mandado ao programa.
    volume_enviado: u32,
    /// Cliente: o vídeo abriu no lugar desta janela, que está escondida.
    video_no_lugar: bool,
    /// Cliente: o código da sala onde entrou (para o nome da aba).
    sala_assistida: Option<String>,
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
            espectadores: Vec::new(),
            nomes: HashMap::new(),
            membros: HashMap::new(),
            desde: HashMap::new(),
            transmitindo: false,
            recebendo: None,
            relatorio: None,
            erro: None,
            aviso: None,
            registro: VecDeque::new(),
            fim: None,
            monitor: cfg.monitor,
            janela: None,
            som: cfg.som,
            pausado: false,
            qualidade: None,
            configuracao: None,
            resolucao_atual: RESOLUCOES[cfg.resolucao.min(RESOLUCOES.len() - 1)].1,
            fps_atual: cfg.fps,
            volume: cfg.volume,
            mudo: false,
            volume_enviado: cfg.volume,
            video_no_lugar: false,
            sala_assistida: None,
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
                    self.espectadores.push(numero);
                    self.desde.insert(numero, Instant::now());
                    self.respostas.retain(|(n, _)| *n != numero);
                }
                Evento::Saiu { numero, total } => {
                    self.assistindo = total;
                    self.espectadores.retain(|n| *n != numero);
                    self.nomes.remove(&numero);
                    self.membros.remove(&numero);
                    self.desde.remove(&numero);
                }
                Evento::NomeEspectador { numero, nome } => {
                    self.nomes.insert(numero, nome);
                }
                Evento::MembroEspectador { numero, membro } => {
                    self.membros.insert(numero, membro);
                }
                Evento::NaoConectou { numero, motivo } => {
                    self.respostas.retain(|(n, _)| *n != numero);
                    self.aviso = Some(format!("Espectador {numero} não conectou: {motivo}"));
                }
                Evento::Transmitindo => self.transmitindo = true,
                Evento::Recebendo(v) => self.recebendo = Some(v),
                Evento::Relatorio(r) => self.relatorio = Some(r),
                Evento::Tela(n) => {
                    self.monitor = n.saturating_sub(1);
                    self.janela = None;
                }
                Evento::Janela(h) => self.janela = Some(h),
                Evento::Som(sim) => self.som = sim,
                Evento::Pausa(sim) => self.pausado = sim,
                Evento::Qualidade(q) => self.qualidade = Some(q),
                Evento::Configuracao(c) => self.configuracao = Some(c),
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
    /// Sessões rodando ao mesmo tempo: no máximo uma compartilhando e quantas
    /// quiser assistindo (cada uma é um programa à parte).
    sessoes: Vec<Sessao>,
    /// A aba aberta (`None`: a tela inicial, para começar outra).
    atual: Option<usize>,
    /// Problema no formulário (código errado, programa não encontrado).
    aviso: Option<String>,
    resposta_colada: String,
    /// Mais um espectador: código de conexão, ou endereço + código na mesma rede.
    novo_conexao: String,
    novo_destino: String,
    novo_codigo: String,
    /// O grupo da chave: quem está online e quem compartilha.
    grupo: Option<Grupo>,
    /// Por que não deu para entrar no grupo (chave inválida, por exemplo).
    erro_grupo: Option<String>,
    /// O que o grupo mostrava neste quadro (lido uma vez por quadro).
    estado_grupo: grupo::Estado,
    /// A lista do grupo à direita (o botão Grupo, em cima, esconde).
    mostrar_grupo: bool,
    /// A foto deste FSS (JPEG) e o número dela.
    minha_foto: Option<(u32, Vec<u8>)>,
    /// Texturas das fotos, pelo número do membro.
    fotos: Fotos,
    /// Foto sendo escolhida ou preparada (numa thread).
    escolhendo_foto: Option<mpsc::Receiver<FotoEscolhida>>,
    erro_foto: Option<String>,
    /// Primeira vez: o campo onde se cola a chave e o convite.
    colar: String,
    /// "Copiar código" clicado: mostra "Copiado" até este instante.
    copiado_ate: Option<Instant>,
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
            sessoes: Vec::new(),
            atual: None,
            aviso: None,
            resposta_colada: String::new(),
            novo_conexao: String::new(),
            novo_destino: String::new(),
            novo_codigo: String::new(),
            grupo: None,
            erro_grupo: None,
            estado_grupo: grupo::Estado::default(),
            mostrar_grupo: true,
            minha_foto: perfil::ler_foto().map(|j| (fluxo_crypto::photo_id(&j), j)),
            fotos: Fotos::default(),
            escolhendo_foto: None,
            erro_foto: None,
            colar: String::new(),
            copiado_ate: None,
        }
    }

    /// Abre a janela "Abrir" do Windows numa thread e prepara a foto escolhida.
    fn escolher_foto(&mut self) {
        if self.escolhendo_foto.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let dono = self.hwnd;
        let aberta = std::thread::Builder::new()
            .name("fss-foto".into())
            .spawn(move || {
                let r = perfil::escolher_arquivo(dono).map(|c| perfil::trocar_foto(&c));
                let _ = tx.send(r);
            });
        if aberta.is_ok() {
            self.escolhendo_foto = Some(rx);
        }
    }

    /// Prepara a foto de um arquivo arrastado para a janela.
    fn foto_do_arquivo(&mut self, caminho: std::path::PathBuf) {
        let (tx, rx) = mpsc::channel();
        let aberta = std::thread::Builder::new()
            .name("fss-foto".into())
            .spawn(move || {
                let _ = tx.send(Some(perfil::trocar_foto(&caminho)));
            });
        if aberta.is_ok() {
            self.escolhendo_foto = Some(rx);
        }
    }

    /// A foto escolhida ficou pronta (ou deu erro, ou a pessoa desistiu).
    fn foto_pronta(&mut self) {
        let Some(rx) = &self.escolhendo_foto else { return };
        match rx.try_recv() {
            Ok(Some(Ok(jpeg))) => {
                self.minha_foto = Some((fluxo_crypto::photo_id(&jpeg), jpeg));
                self.erro_foto = None;
                self.escolhendo_foto = None;
            }
            Ok(Some(Err(e))) => {
                self.erro_foto = Some(e);
                self.escolhendo_foto = None;
            }
            Ok(None) | Err(mpsc::TryRecvError::Disconnected) => self.escolhendo_foto = None,
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    /// A textura da foto de um membro (a deste FSS vem do arquivo, mesmo sem grupo).
    fn textura(&mut self, ctx: &egui::Context, membro: u64) -> Option<egui::TextureHandle> {
        textura_de(
            &mut self.fotos,
            &self.estado_grupo,
            self.minha_foto.as_ref(),
            self.cfg.membro,
            ctx,
            membro,
        )
    }

    /// Mantém o grupo igual à chave das configurações e conta a ele o apelido
    /// e a sala que este FSS está compartilhando.
    fn atualizar_grupo(&mut self, ctx: &egui::Context) {
        let tem_grupo = !self.cfg.chave.trim().is_empty() && !self.cfg.grupo.trim().is_empty();
        let chave = grupo::identidade(&self.cfg.chave, &self.cfg.grupo);
        if self.grupo.as_ref().map(Grupo::chave) != Some(chave.as_str())
            && (tem_grupo || self.grupo.is_some() || self.erro_grupo.is_some())
        {
            self.grupo = None;
            self.erro_grupo = None;
            if tem_grupo {
                match Grupo::iniciar(
                    &self.cfg.chave,
                    &self.cfg.grupo,
                    self.cfg.membro,
                    perfil::pasta_das_fotos(),
                    ctx.clone(),
                ) {
                    Ok(g) => self.grupo = Some(g),
                    Err(e) => self.erro_grupo = Some(e),
                }
            }
        }
        let sala = self
            .sessoes
            .iter()
            .find(|s| s.host && s.modo == Modo::Sala && s.rodando())
            .and_then(|s| s.sala.clone());
        // A sala que está assistindo (a primeira, se forem várias): os outros
        // do grupo veem quem assiste quem.
        let assistindo = self
            .sessoes
            .iter()
            .find(|s| !s.host && s.modo == Modo::Sala && s.rodando())
            .and_then(|s| s.sala_assistida.clone());
        if let Some(g) = &self.grupo {
            g.status(
                &self.cfg.apelido_efetivo(),
                sala.as_deref(),
                assistindo.as_deref(),
            );
            g.foto(self.minha_foto.as_ref().map(|f| f.1.as_slice()));
            self.estado_grupo = g.ver();
        } else if self.estado_grupo.nome.is_some() || !self.estado_grupo.membros.is_empty() {
            self.estado_grupo = grupo::Estado::default();
        }
    }

    /// A lista do grupo, à direita, no jeito do Discord: foto, nome e o que
    /// cada um está fazendo. A linha de quem está ao vivo é um botão para
    /// assistir. Os offline ficam embaixo, recolhíveis.
    fn painel_grupo(&mut self, ctx: &egui::Context, p: &Paleta) {
        if self.grupo.is_none() || !self.mostrar_grupo || ctx.screen_rect().width() < 560.0 {
            return;
        }
        let estado = self.estado_grupo.clone();
        let assistindo: Vec<String> = self
            .sessoes
            .iter()
            .filter(|s| !s.host && s.rodando())
            .filter_map(|s| s.sala_assistida.clone())
            .collect();
        let mut entrar: Option<String> = None;
        egui::SidePanel::right("grupo")
            .resizable(false)
            .exact_width(240.0)
            .frame(
                egui::Frame::new()
                    .fill(p.painel)
                    .inner_margin(Margin::symmetric(12, 12)),
            )
            .show(ctx, |ui| {
                let nome = estado.nome.clone().unwrap_or_else(|| "Grupo".to_string());
                ui.label(RichText::new(nome).size(17.0).strong().color(p.texto));
                let (situacao, cor) = match (&estado.erro, estado.conectado) {
                    (Some(_), _) => ("sem conexão com o servidor", p.perigo),
                    (None, true) => ("conectado", p.sucesso),
                    (None, false) => ("conectando…", p.texto_fraco),
                };
                ui.label(RichText::new(situacao).size(12.0).color(cor))
                    .on_hover_text(estado.erro.clone().unwrap_or_default());
                ui.add_space(10.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let online: Vec<&fluxo_net::GroupMember> =
                        estado.membros.iter().filter(|m| m.online).collect();
                    let offline: Vec<&fluxo_net::GroupMember> =
                        estado.membros.iter().filter(|m| !m.online).collect();
                    ui.label(
                        RichText::new(format!("ONLINE — {}", online.len()))
                            .size(11.0)
                            .strong()
                            .color(p.texto_fraco),
                    );
                    ui.add_space(2.0);
                    for m in &online {
                        // Quem está assistindo este membro (pela nota de cada um).
                        let quem: Vec<&str> = match &m.room {
                            Some(sala) => estado
                                .membros
                                .iter()
                                .filter(|o| o.online && o.watching.as_ref() == Some(sala))
                                .map(|o| o.nick.as_str())
                                .collect(),
                            None => Vec::new(),
                        };
                        let ja = m.room.as_ref().is_some_and(|s| assistindo.contains(s));
                        let subtitulo = match (&m.room, &m.watching) {
                            (Some(_), _) if ja => Some(("você está assistindo".to_string(), p.sucesso)),
                            (Some(_), _) if quem.is_empty() => Some(("ao vivo".to_string(), p.perigo)),
                            (Some(_), _) => Some((format!("ao vivo · {} assistindo", quem.len()), p.perigo)),
                            (None, Some(sala)) => estado
                                .membros
                                .iter()
                                .find(|o| o.room.as_ref() == Some(sala))
                                .map(|o| (format!("assistindo {}", o.nick), p.texto_fraco)),
                            _ => None,
                        };
                        let pode_entrar = m.room.is_some() && !m.me && !ja;
                        let foto = textura_de(
                            &mut self.fotos,
                            &estado,
                            self.minha_foto.as_ref(),
                            self.cfg.membro,
                            ctx,
                            m.member,
                        );
                        let cor = if m.room.is_some() { p.perigo } else { p.sucesso };
                        let mut linha = linha_de_membro(ui, p, m, foto.as_ref(), subtitulo, Some(cor), pode_entrar);
                        if !quem.is_empty() {
                            linha = linha.on_hover_text(format!("Assistindo: {}", quem.join(", ")));
                        } else if pode_entrar {
                            linha = linha.on_hover_text("Clique para assistir");
                        }
                        if pode_entrar && linha.clicked() {
                            entrar = m.room.clone();
                        }
                    }
                    if estado.conectado && online.len() <= 1 {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(
                                "Só você por aqui. Quem tiver o mesmo convite aparece nesta lista ao abrir o FSS.",
                            )
                            .size(12.0)
                            .color(p.texto_fraco),
                        );
                    }
                    if !offline.is_empty() {
                        ui.add_space(8.0);
                        egui::CollapsingHeader::new(
                            RichText::new(format!("OFFLINE — {}", offline.len()))
                                .size(11.0)
                                .strong()
                                .color(p.texto_fraco),
                        )
                        .id_salt("grupo_offline")
                        .default_open(offline.len() <= 6)
                        .show(ui, |ui| {
                            for m in &offline {
                                let foto = textura_de(
                                    &mut self.fotos,
                                    &estado,
                                    self.minha_foto.as_ref(),
                                    self.cfg.membro,
                                    ctx,
                                    m.member,
                                );
                                linha_de_membro(ui, p, m, foto.as_ref(), None, None, false);
                            }
                        });
                    }
                });
            });
        if let Some(sala) = entrar {
            self.assistir.sala = sala;
            self.iniciar_cliente(ctx, Modo::Sala);
        }
    }

    fn paleta(&self, ctx: &egui::Context) -> Paleta {
        self.tema.paleta(ctx.theme()).clone()
    }

    fn iniciar_host(&mut self, ctx: &egui::Context, modo: Modo) {
        self.compartilhar.modo = modo;
        // A janela escolhida pode ter fechado nesse meio tempo.
        if let Some(h) = self.compartilhar.janela {
            if !janelas_para_compartilhar().iter().any(|j| j.hwnd == h) {
                self.compartilhar.janela = None;
            }
        }
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
        // Com outra sessão rodando, esta janela fica (é dela que se controla a
        // outra), e o vídeo abre ao lado.
        let lugar_do_video = match (programa, self.hwnd, self.sessoes.iter().any(Sessao::rodando)) {
            ("client", Some(h), false) => lugar::retangulo(h).map(|r| (h, r)),
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
                // Botão da casinha na barra do vídeo: esta janela volta, o vídeo segue.
                (Some("Mostrar o FSS"), Some((h, _))) => lugar::mostrar(h, None),
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
                if programa == "client" && modo == Modo::Sala {
                    sessao.sala_assistida = Some(self.assistir.sala.trim().to_uppercase());
                }
                if let Some((h, _)) = lugar_do_video {
                    lugar::permitir_foco(sessao.processo.pid());
                    lugar::esconder(h);
                    sessao.video_no_lugar = true;
                }
                self.sessoes.push(sessao);
                self.atual = Some(self.sessoes.len() - 1);
                self.aviso = None;
                self.resposta_colada.clear();
            }
            Err(e) => self.aviso = Some(format!("Não consegui abrir o programa: {e}")),
        }
    }

    /// Tira a sessão `i` (soltar encerra o programa dela) e acerta a aba aberta.
    fn fechar_sessao(&mut self, i: usize) {
        if i >= self.sessoes.len() {
            return;
        }
        self.sessoes.remove(i);
        self.atual = match self.atual {
            Some(a) if a == i => self.sessoes.len().checked_sub(1),
            Some(a) if a > i => Some(a - 1),
            outra => outra,
        };
    }

    /// As abas: uma por sessão, e a tela inicial para começar outra.
    fn barra_de_abas(&mut self, ctx: &egui::Context, p: &Paleta) {
        if self.sessoes.is_empty() {
            return;
        }
        egui::TopBottomPanel::top("abas")
            .frame(
                egui::Frame::new()
                    .fill(p.fundo)
                    .inner_margin(Margin::symmetric(16, 8)),
            )
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    // Curtas: o estado inteiro está no cartão do meio; aqui,
                    // a bolinha de cor (e o texto ao passar o mouse).
                    for (i, s) in self.sessoes.iter().enumerate() {
                        let (texto, cor) = estado_curto(s, p);
                        let nome = match (s.host, &s.sala_assistida) {
                            (true, _) => "Compartilhando".to_string(),
                            (false, Some(sala)) => format!("Sala {sala}"),
                            (false, None) => "Assistindo".to_string(),
                        };
                        bolinha_de_estado(ui, if s.rodando() { cor } else { p.borda });
                        let escolhida = self.atual == Some(i);
                        let rotulo =
                            RichText::new(nome).color(if escolhida { p.texto } else { p.texto_fraco });
                        if ui
                            .selectable_label(escolhida, rotulo)
                            .on_hover_text(texto)
                            .clicked()
                        {
                            self.atual = Some(i);
                        }
                        ui.add_space(6.0);
                    }
                    if ui
                        .selectable_label(self.atual.is_none(), "+ Começar outra")
                        .on_hover_text("Compartilhar enquanto assiste, ou assistir a mais alguém")
                        .clicked()
                    {
                        self.atual = None;
                        self.tela = Tela::Inicio;
                    }
                });
            });
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
        if !self.sessoes.is_empty() {
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
            (Atualizacao::Pronta(pronta), _) if !self.sessoes.is_empty() => Some(format!(
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
        let minha_foto = self.textura(ctx, self.cfg.membro);
        let apelido = self.cfg.apelido_efetivo();
        let online = self.estado_grupo.membros.iter().filter(|m| m.online).count();
        let tem_grupo = self.grupo.is_some();
        let mut abrir_perfil = false;
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
                    },
                    |ui| {
                        // Da direita para a esquerda.
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
                        if tem_grupo && !aberta {
                            let botao =
                                egui::Button::new(RichText::new(format!("Grupo · {online}")).size(14.0))
                                    .selected(self.mostrar_grupo)
                                    .min_size(egui::vec2(0.0, 32.0));
                            if ui
                                .add(botao)
                                .on_hover_text("Mostrar ou esconder a lista do grupo")
                                .clicked()
                            {
                                self.mostrar_grupo = !self.mostrar_grupo;
                            }
                        }
                        ui.add_space(4.0);
                        let nome = ui
                            .add(
                                egui::Button::new(RichText::new(&apelido).size(14.0).color(p.texto))
                                    .frame(false),
                            )
                            .on_hover_text("Seu perfil: nome e foto");
                        let foto = perfil::avatar(ui, self.cfg.membro, &apelido, minha_foto.as_ref(), 28.0)
                            .on_hover_text("Seu perfil: nome e foto");
                        abrir_perfil = nome.clicked() || foto.clicked();
                    },
                );
            });
        if abrir_perfil {
            self.configuracoes = Some(Secao::Perfil);
        }
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
        if self.cfg.chave.trim().is_empty() {
            self.primeira_vez(ui, p);
            ui.add_space(12.0);
        }
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
            escolher_fonte(
                ui,
                "inicio_tela",
                &self.monitores,
                &mut self.cfg.monitor,
                &mut self.compartilhar.janela,
            );
            ui.checkbox(&mut self.cfg.som, "Transmitir o som do PC");
            ui.add_space(6.0);
            let tem_chave = self.falta_chave(ui, p);
            let ja_compartilha = self.sessoes.iter().any(|s| s.host && s.rodando());
            if ja_compartilha {
                ui.colored_label(p.aviso, "Você já está compartilhando (veja a aba de cima).");
            }
            let botao = principal(
                ui,
                p,
                "Compartilhar",
                self.tema.raio,
                cfg!(windows) && tem_chave && !ja_compartilha,
            );
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
            // Quem do grupo está ao vivo: um clique, sem código.
            let ao_vivo: Vec<(u64, String, String)> = self
                .estado_grupo
                .membros
                .iter()
                .filter(|m| m.online && !m.me)
                .filter_map(|m| Some((m.member, m.nick.clone(), m.room.clone()?)))
                .collect();
            if !ao_vivo.is_empty() {
                ui.label(RichText::new("Ao vivo no grupo").color(p.texto_fraco));
                for (membro, nick, sala) in &ao_vivo {
                    let foto = self.textura(ctx, *membro);
                    ui.horizontal(|ui| {
                        perfil::avatar(ui, *membro, nick, foto.as_ref(), 28.0);
                        ui.label(RichText::new(nick).color(p.texto));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(botao_comum("Assistir", self.tema.raio)).clicked() {
                                self.assistir.sala = sala.clone();
                                self.iniciar_cliente(ctx, Modo::Sala);
                            }
                        });
                    });
                }
                ui.add_space(6.0);
                ui.label(RichText::new("Ou digite o código que mandaram:").color(p.texto_fraco));
            } else {
                ui.label(RichText::new("Digite o código que quem compartilha mandou.").color(p.texto_fraco));
            }
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

    /// Primeira vez (sem chave): um campo só, onde se cola o que veio junto
    /// com o programa. Ele acha a chave (`FSS1-…`) e o convite (`FSSG-…`).
    fn primeira_vez(&mut self, ui: &mut egui::Ui, p: &Paleta) {
        cartao(ui, p, self.tema.raio, |ui| {
            ui.label(
                RichText::new("Comece colando a chave")
                    .size(19.0)
                    .strong()
                    .color(p.texto),
            );
            ui.label(
                RichText::new(
                    "Quem roda o servidor manda uma chave (FSS1-…) e, se você for de um grupo, um convite \
                     (FSSG-…). Cole aqui os dois juntos, do jeito que vieram.",
                )
                .color(p.texto_fraco),
            );
            ui.add_space(6.0);
            let campo = ui.add(
                egui::TextEdit::multiline(&mut self.colar)
                    .hint_text("FSS1-…   FSSG-…")
                    .font(egui::FontId::monospace(14.0))
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            if campo.changed() {
                let (chave, convite) = separar_colado(&self.colar);
                if let Some(c) = chave {
                    self.cfg.chave = c;
                    if let Some(g) = convite {
                        self.cfg.grupo = g;
                    }
                    self.colar.clear();
                }
            }
            if !self.colar.trim().is_empty() {
                ui.colored_label(p.aviso, "Não achei uma chave válida no que foi colado.");
            }
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Sem chave?").color(p.texto_fraco));
                if ui.link("Outras formas de conectar (sem servidor)").clicked() {
                    self.tela = Tela::Outras;
                }
            });
        });
    }

    /// Sem chave do servidor, a sala não funciona: o botão fica apagado, com
    /// uma linha curta apontando para o campo de cima (`primeira_vez`).
    /// Devolve se há chave.
    fn falta_chave(&mut self, ui: &mut egui::Ui, p: &Paleta) -> bool {
        if !self.cfg.chave.trim().is_empty() {
            return true;
        }
        ui.label(RichText::new("Cole a chave acima para usar.").size(12.0).color(p.texto_fraco));
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
        let ctx = ui.ctx().clone();
        let copiado = self.copiado_ate.is_some_and(|t| Instant::now() < t);
        let mut copiou = false;
        let Some(s) = self.atual.and_then(|i| self.sessoes.get_mut(i)) else {
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
                    let texto = if copiado { "Copiado!" } else { "📋  Copiar código" };
                    if ui.add(botao_comum(texto, raio)).clicked() {
                        ui.ctx().copy_text(c.clone());
                        copiou = true;
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
                ui.label(RichText::new(linha.join(" · ")).color(p.texto_fraco));
                match (&s.configuracao, &s.qualidade) {
                    (Some(c), _) => {
                        ui.label(RichText::new(c).strong().color(p.texto));
                    }
                    (None, Some(q)) => {
                        ui.label(RichText::new(q).color(p.texto_fraco));
                    }
                    _ => {}
                }
                if let Some(r) = &s.relatorio {
                    let (texto, cor) = match qualidade_da_conexao(r) {
                        Some(Conexao::Boa) => ("Conexão boa", p.sucesso),
                        Some(Conexao::Instavel) => ("Conexão instável", p.aviso),
                        Some(Conexao::Ruim) => ("Conexão ruim: perdendo pacotes", p.perigo),
                        None => ("Conexão", p.texto_fraco),
                    };
                    ui.horizontal(|ui| {
                        bolinha_de_estado(ui, cor);
                        ui.label(RichText::new(texto).color(cor).size(13.0))
                            .on_hover_text(r);
                        ui.label(RichText::new(format!("· {r}")).color(p.texto_fraco).size(12.0));
                    });
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

        // Quem está assistindo, com o botão de expulsar.
        if s.host && s.rodando() && !s.espectadores.is_empty() {
            ui.add_space(12.0);
            let mut expulsar = None;
            let fotos: HashMap<u32, Option<egui::TextureHandle>> = s
                .espectadores
                .iter()
                .map(|n| {
                    let foto = s.membros.get(n).and_then(|m| {
                        textura_de(
                            &mut self.fotos,
                            &self.estado_grupo,
                            self.minha_foto.as_ref(),
                            self.cfg.membro,
                            &ctx,
                            *m,
                        )
                    });
                    (*n, foto)
                })
                .collect();
            cartao(ui, p, raio, |ui| {
                ui.label(
                    RichText::new(format!(
                        "Assistindo · {} de {MAX_ESPECTADORES}",
                        s.espectadores.len()
                    ))
                    .strong()
                    .color(p.texto),
                );
                ui.add_space(4.0);
                for n in &s.espectadores {
                    ui.horizontal(|ui| {
                        let nome = s.nomes.get(n).cloned().unwrap_or_else(|| format!("Espectador {n}"));
                        let membro = s.membros.get(n).copied().unwrap_or(*n as u64);
                        perfil::avatar(ui, membro, &nome, fotos.get(n).and_then(Option::as_ref), 28.0);
                        ui.label(RichText::new(&nome).color(p.texto));
                        if let Some(d) = s.desde.get(n) {
                            ui.label(
                                RichText::new(ha_quanto(d.elapsed()))
                                    .size(12.0)
                                    .color(p.texto_fraco),
                            );
                        }
                        ui.menu_button("Expulsar ⏷", |ui| {
                            for (texto, bloqueio) in [
                                ("Tirar da transmissão", ""),
                                ("Tirar e bloquear por 10 min", " 600"),
                                ("Tirar e bloquear por 1 hora", " 3600"),
                                ("Tirar e bloquear até eu encerrar", " sempre"),
                            ] {
                                if ui.button(texto).clicked() {
                                    expulsar = Some(format!("{n}{bloqueio}"));
                                    ui.close();
                                }
                            }
                        })
                        .response
                        .on_hover_text("O vídeo de quem sai fecha com um aviso; bloqueado, ele não entra de novo pelo tempo escolhido");
                    });
                }
            });
            if let Some(n) = expulsar {
                s.mandar(&format!("expulsar {n}"));
            }
        }
        if copiou {
            self.copiado_ate = Some(Instant::now() + Duration::from_millis(1500));
            ctx.request_repaint_after(Duration::from_millis(1600));
        }

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
                // IPs da internet mascarados: o texto pode ir parar num chat.
                let texto: Vec<&str> = s.registro.iter().map(String::as_str).collect();
                ui.ctx().copy_text(crate::saida::mascarar_ips(&texto.join("\n")));
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
                let Some(s) = self.atual.and_then(|i| self.sessoes.get_mut(i)) else {
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
                    // Soltar encerra o programa.
                    if let Some(i) = self.atual {
                        self.fechar_sessao(i);
                    }
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
                        Secao::Perfil => self.secao_perfil(ui, p),
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

    fn secao_perfil(&mut self, ui: &mut egui::Ui, p: &Paleta) {
        ui.label(
            RichText::new(
                "Como os outros do grupo veem você: na lista do grupo e em quem está assistindo. \
                 O perfil é deste FSS (não do IP: ele muda e é o mesmo para quem mora junto).",
            )
            .color(p.texto_fraco),
        );
        ui.add_space(12.0);
        let apelido = self.cfg.apelido_efetivo();
        let foto = self.textura(ui.ctx(), self.cfg.membro);
        ui.horizontal(|ui| {
            perfil::avatar(ui, self.cfg.membro, &apelido, foto.as_ref(), 96.0);
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(&apelido).size(20.0).strong().color(p.texto));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let ocupado = self.escolhendo_foto.is_some();
                    if ui
                        .add_enabled(!ocupado, egui::Button::new("Escolher foto…"))
                        .clicked()
                    {
                        self.escolher_foto();
                    }
                    if self.minha_foto.is_some()
                        && ui
                            .add_enabled(!ocupado, egui::Button::new("Tirar foto"))
                            .clicked()
                    {
                        perfil::tirar_foto();
                        self.minha_foto = None;
                        self.fotos.esquecer(self.cfg.membro);
                    }
                    if ocupado {
                        ui.spinner();
                    }
                });
                ui.label(
                    RichText::new("Ou arraste uma imagem para esta janela.")
                        .size(12.0)
                        .color(p.texto_fraco),
                );
            });
        });
        if let Some(e) = &self.erro_foto {
            ui.colored_label(p.perigo, e);
        }
        ui.add_space(16.0);
        titulo_campo(ui, p, "Nome", "Vazio: o nome do seu usuário do Windows.");
        let padrao = {
            let mut c = self.cfg.clone();
            c.apelido.clear();
            c.apelido_efetivo()
        };
        ui.add(
            egui::TextEdit::singleline(&mut self.cfg.apelido)
                .hint_text(padrao)
                .char_limit(fluxo_net::MAX_NICK)
                .desired_width(280.0),
        );
        ui.add_space(16.0);
        ui.label(
            RichText::new(format!(
                "A foto é reduzida para {lado}x{lado} e vai cifrada com o convite do grupo: o servidor guarda \
                 sem conseguir ver, e só quem tem o convite abre. Cada um baixa a sua foto uma vez, e de novo \
                 só quando ela muda.",
                lado = perfil::LADO
            ))
            .size(12.0)
            .color(p.texto_fraco),
        );
        if self.grupo.is_none() {
            ui.label(
                RichText::new("Sem convite de grupo, a foto e o nome só aparecem aqui.")
                    .size(12.0)
                    .color(p.aviso),
            );
        }
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
                ui.vertical(|ui| {
                    ui.checkbox(&mut c.som, "Começar com o som do PC ligado");
                    ui.checkbox(&mut c.sem_discord, "Não transmitir o som do Discord");
                    ui.label(
                        RichText::new(
                            "Assim quem está na call com você não ouve a própria voz. \
                             Transmitindo uma janela, vai só o som do programa dela.",
                        )
                        .size(12.0),
                    );
                });
                ui.end_row();
                if !self.saidas_som.is_empty() {
                    ui.label("Capturar o som de");
                    escolher_saida(ui, "cfg_som_de", &self.saidas_som, &mut c.som_de);
                    ui.end_row();
                }
                ui.label("Placa de vídeo");
                escolher_gpu(ui, "cfg_gpu_compartilhar", &mut c.gpu);
                ui.end_row();
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
                ui.label("Placa de vídeo");
                escolher_gpu(ui, "cfg_gpu_assistir", &mut c.gpu);
                ui.end_row();
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
        if let Some(e) = &self.erro_grupo {
            ui.colored_label(p.perigo, format!("Grupo: {e}"));
        }
        ui.label(
            RichText::new("Quem compartilha e quem assiste precisam da mesma chave.")
                .size(13.0)
                .color(p.texto_fraco),
        );
        ui.add_space(16.0);
        titulo_campo(
            ui,
            p,
            "Convite do grupo",
            "Com o convite, você vê quem do grupo está online e quem está ao vivo, e entra nas salas do \
             grupo com um clique. Só quem é do grupo acha essas salas. Quem roda o servidor gera o convite.",
        );
        ui.add(
            egui::TextEdit::multiline(&mut self.cfg.grupo)
                .hint_text("FSSG-…")
                .font(egui::FontId::monospace(14.0))
                .desired_rows(1)
                .desired_width(f32::INFINITY),
        );
        if !self.cfg.grupo.trim().is_empty() {
            match crate::comandos::ler_grupo(&self.cfg.grupo) {
                Ok(_) => {
                    ui.colored_label(p.sucesso, "Convite válido");
                }
                Err(e) => {
                    ui.colored_label(p.perigo, e);
                }
            }
        }
        ui.add_space(16.0);
        titulo_campo(
            ui,
            p,
            "Porta no roteador (UPnP)",
            "Só para assistir pela internet trocando códigos: abre uma porta no roteador para quem \
             compartilha chegar direto, por 1 hora, renovada enquanto o programa roda e fechada ao sair. \
             As salas não precisam.",
        );
        ui.checkbox(&mut self.cfg.upnp, "Abrir a porta no roteador (UPnP)");
    }
}

// ------------------------------------------------------------ peças soltas

/// Os botões de quem compartilha: tela, som, pausa, qualidade.
fn controles_host(ui: &mut egui::Ui, s: &mut Sessao, p: &Paleta, raio: u8, monitores: &[String]) {
    ui.horizontal(|ui| {
        let rotulo = match s.janela {
            Some(_) => "🖥  Janela  ⏷".to_string(),
            None => format!("🖥  Tela {}  ⏷", s.monitor + 1),
        };
        egui::containers::menu::MenuButton::from_button(botao_comum(&rotulo, raio)).ui(ui, |ui| {
            ui.label(RichText::new("Telas").strong());
            if monitores.is_empty() {
                ui.label("Nenhuma tela encontrada");
            }
            for (i, nome) in monitores.iter().enumerate() {
                if ui
                    .selectable_label(s.janela.is_none() && i == s.monitor, nome)
                    .clicked()
                {
                    s.mandar(&format!("tela {i}"));
                    ui.close();
                }
            }
            let janelas = janelas_para_compartilhar();
            if !janelas.is_empty() {
                ui.separator();
                ui.label(RichText::new("Janelas").strong());
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for j in janelas {
                        if ui
                            .selectable_label(s.janela == Some(j.hwnd), nome_da_janela(&j))
                            .clicked()
                        {
                            s.mandar(&format!("janela {}", j.hwnd));
                            ui.close();
                        }
                    }
                });
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
                if ui.selectable_label(s.resolucao_atual == valor, nome).clicked() {
                    s.resolucao_atual = valor;
                    s.mandar(&format!("resolucao {valor}"));
                    ui.close();
                }
            }
            ui.separator();
            ui.label(RichText::new("Quadros por segundo").strong());
            for f in FPS {
                if ui
                    .selectable_label(s.fps_atual == f, format!("{f} fps"))
                    .clicked()
                {
                    s.fps_atual = f;
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

/// As janelas abertas que dá para compartilhar (sem as do próprio FSS).
fn janelas_para_compartilhar() -> Vec<fluxo_capture::WindowInfo> {
    fluxo_capture::list_windows()
        .into_iter()
        .filter(|j| !j.title.starts_with("FSS —") && j.title != crate::NOME)
        .collect()
}

/// Título curto, para listas.
fn nome_da_janela(j: &fluxo_capture::WindowInfo) -> String {
    let mut t: String = j.title.chars().take(48).collect();
    if j.title.chars().count() > 48 {
        t.push('…');
    }
    if j.minimized {
        t.push_str(" (minimizada)");
    }
    t
}

/// Tela inteira ou uma janela, para começar a compartilhar.
fn escolher_fonte(
    ui: &mut egui::Ui,
    id: &str,
    monitores: &[String],
    monitor: &mut usize,
    janela: &mut Option<isize>,
) {
    let atual = match janela {
        Some(h) => janelas_para_compartilhar()
            .into_iter()
            .find(|j| j.hwnd == *h)
            .map(|j| nome_da_janela(&j))
            .unwrap_or_else(|| "Janela (fechou?)".into()),
        None => monitores
            .get(*monitor)
            .cloned()
            .unwrap_or_else(|| format!("Tela {}", *monitor + 1)),
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(atual)
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            ui.label(RichText::new("Telas").strong());
            for (i, nome) in monitores.iter().enumerate() {
                if ui
                    .selectable_label(janela.is_none() && *monitor == i, nome)
                    .clicked()
                {
                    *monitor = i;
                    *janela = None;
                }
            }
            let janelas = janelas_para_compartilhar();
            if !janelas.is_empty() {
                ui.separator();
                ui.label(RichText::new("Janelas").strong());
                for j in janelas {
                    if ui
                        .selectable_label(*janela == Some(j.hwnd), nome_da_janela(&j))
                        .clicked()
                    {
                        *janela = Some(j.hwnd);
                    }
                }
            }
        });
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

/// Placa de vídeo: a do Windows ou uma da lista (notebook com duas placas:
/// escolha a dedicada). A mesma escolha vale para compartilhar e assistir.
fn escolher_gpu(ui: &mut egui::Ui, id: &str, gpu: &mut Option<usize>) {
    let placas = fluxo_capture::list_adapters();
    let nome = |i: usize| {
        placas
            .iter()
            .find(|p| p.index == i)
            .map(|p| format!("{} · {:.1} GB", p.name, p.dedicated_mb as f64 / 1024.0))
            .unwrap_or_else(|| format!("Placa {i} (não encontrada)"))
    };
    let atual = match gpu {
        None => "Automática (a do Windows)".to_string(),
        Some(i) => nome(*i),
    };
    ui.vertical(|ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(atual)
            .width(300.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(gpu, None, "Automática (a do Windows)");
                for p in &placas {
                    ui.selectable_value(gpu, Some(p.index), nome(p.index));
                }
            });
        ui.label(
            RichText::new(
                "Num notebook com duas placas, escolha a dedicada (a com mais memória). \
                 Escolhendo uma, a captura passa a ser outra (o cursor vai na imagem).",
            )
            .size(12.0),
        );
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
        self.sessoes.clear();
    }

    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if self.aparencia_aplicada != Some(self.cfg.aparencia) {
            self.tema.aplicar(ctx, self.cfg.aparencia);
            self.aparencia_aplicada = Some(self.cfg.aparencia);
        }
        for s in &mut self.sessoes {
            s.atualizar();
        }
        // Quem assiste saiu (Sair ou fechou o vídeo): a aba some. Com erro, a
        // sessão fica, para mostrar o que houve.
        while let Some(i) = self
            .sessoes
            .iter()
            .position(|s| !s.host && s.fim == Some(true) && s.erro.is_none())
        {
            self.fechar_sessao(i);
        }
        if !self.sessoes.is_empty() {
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
        self.foto_pronta();
        if self.escolhendo_foto.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        // Uma imagem arrastada para a janela vira a foto do perfil.
        let arrastada = ctx.input(|i| i.raw.dropped_files.iter().find_map(|f| f.path.clone()));
        if let Some(c) = arrastada {
            self.configuracoes = Some(Secao::Perfil);
            self.foto_do_arquivo(c);
        }
        self.atualizar_grupo(ctx);
        let p = self.paleta(ctx);

        self.barra_de_cima(ctx, &p);
        self.faixa_de_aviso(ctx, &p);
        if self.configuracoes.is_none() {
            self.barra_de_abas(ctx, &p);
        }
        if self.configuracoes.is_some() {
            self.pagina_configuracoes(ctx, &p);
        } else {
            if self.atual.is_some() {
                self.barra_de_baixo(ctx, &p);
            }
            self.painel_grupo(ctx, &p);
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(p.fundo)
                        .inner_margin(Margin::symmetric(16, 0)),
                )
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        centralizar(ui, LARGURA_CONTEUDO, |ui| {
                            if self.atual.is_some() {
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

/// A textura da foto de `membro`: a deste FSS vem do arquivo (vale sem
/// grupo); a dos outros, do grupo.
fn textura_de(
    fotos: &mut Fotos,
    estado: &grupo::Estado,
    minha: Option<&(u32, Vec<u8>)>,
    eu: u64,
    ctx: &egui::Context,
    membro: u64,
) -> Option<egui::TextureHandle> {
    let (id, jpeg) = if membro == eu {
        minha.map(|(id, j)| (*id, j.as_slice()))?
    } else {
        estado.fotos.get(&membro).map(|(id, j)| (*id, j.as_slice()))?
    };
    fotos.textura(ctx, membro, id, jpeg)
}

/// Uma linha da lista do grupo: a foto com a bolinha de estado, o nome e o
/// que está fazendo. `clicavel`: a linha inteira é um botão (assistir).
fn linha_de_membro(
    ui: &mut egui::Ui,
    p: &Paleta,
    m: &fluxo_net::GroupMember,
    foto: Option<&egui::TextureHandle>,
    subtitulo: Option<(String, Color32)>,
    estado: Option<Color32>,
    clicavel: bool,
) -> egui::Response {
    let sentido = if clicavel {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resposta) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 42.0), sentido);
    if !ui.is_rect_visible(rect) {
        return resposta;
    }
    if clicavel && resposta.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), p.controle);
    }
    let lado = 30.0;
    let foto_rect = egui::Rect::from_min_size(
        rect.min + egui::vec2(4.0, (rect.height() - lado) / 2.0),
        egui::vec2(lado, lado),
    );
    perfil::pintar_avatar(ui, foto_rect, m.member, &m.nick, foto, !m.online);
    if let Some(cor) = estado {
        let c = foto_rect.right_bottom() - egui::vec2(4.0, 4.0);
        ui.painter().circle_filled(c, 6.0, p.painel);
        ui.painter().circle_filled(c, 4.5, cor);
    }
    let direita = if clicavel { 64.0 } else { 4.0 };
    let texto_rect = egui::Rect::from_min_max(
        egui::pos2(foto_rect.right() + 10.0, rect.top()),
        egui::pos2(rect.right() - direita, rect.bottom()),
    );
    let pintor = ui.painter().with_clip_rect(texto_rect);
    let nome = if m.me {
        format!("{} (você)", m.nick)
    } else {
        m.nick.clone()
    };
    let cor_nome = if m.online { p.texto } else { p.texto_fraco };
    match &subtitulo {
        Some((sub, cor)) => {
            pintor.text(
                egui::pos2(texto_rect.left(), rect.center().y - 1.0),
                egui::Align2::LEFT_BOTTOM,
                nome,
                egui::FontId::proportional(14.0),
                cor_nome,
            );
            pintor.text(
                egui::pos2(texto_rect.left(), rect.center().y + 1.0),
                egui::Align2::LEFT_TOP,
                sub,
                egui::FontId::proportional(11.5),
                *cor,
            );
        }
        None => {
            pintor.text(
                egui::pos2(texto_rect.left(), rect.center().y),
                egui::Align2::LEFT_CENTER,
                nome,
                egui::FontId::proportional(14.0),
                cor_nome,
            );
        }
    }
    if clicavel {
        ui.painter().text(
            egui::pos2(rect.right() - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "Assistir",
            egui::FontId::proportional(13.0),
            if resposta.hovered() { p.texto } else { p.destaque },
        );
    }
    resposta.on_hover_cursor(if clicavel {
        egui::CursorIcon::PointingHand
    } else {
        egui::CursorIcon::Default
    })
}

/// O que se colou na primeira vez: a chave do servidor e o convite do grupo,
/// em qualquer ordem, com ou sem outras palavras em volta.
fn separar_colado(texto: &str) -> (Option<String>, Option<String>) {
    let mut chave = None;
    let mut convite = None;
    for palavra in texto.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
        let p = palavra.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
        if chave.is_none() && crate::comandos::ler_chave(p).is_ok() {
            chave = Some(p.to_string());
        } else if convite.is_none() && matches!(crate::comandos::ler_grupo(p), Ok(Some(_))) {
            convite = Some(p.to_string());
        }
    }
    (chave, convite)
}

/// Como está a conexão, pelo relatório do host (perda e RTT).
#[derive(Debug, PartialEq, Eq)]
enum Conexao {
    Boa,
    Instavel,
    Ruim,
}

fn qualidade_da_conexao(relatorio: &str) -> Option<Conexao> {
    let numero = |depois: &str, ate: char| -> Option<f32> {
        let resto = relatorio.split(depois).nth(1)?;
        resto.split(ate).next()?.trim().replace(',', ".").parse().ok()
    };
    let perda = numero("perda ", '%')?;
    let rtt = numero("RTT ", 'm').unwrap_or(0.0);
    Some(if perda >= 5.0 {
        Conexao::Ruim
    } else if perda >= 1.0 || rtt >= 150.0 {
        Conexao::Instavel
    } else {
        Conexao::Boa
    })
}

/// "agora", "há 3 min", "há 1 h 5 min".
fn ha_quanto(d: Duration) -> String {
    let min = d.as_secs() / 60;
    match min {
        0 => "agora".into(),
        1..=59 => format!("há {min} min"),
        _ if min % 60 == 0 => format!("há {} h", min / 60),
        _ => format!("há {} h {} min", min / 60, min % 60),
    }
}

/// A bolinha de estado da lista do grupo (desenhada: a fonte não tem "●").
fn bolinha_de_estado(ui: &mut egui::Ui, cor: Color32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), 4.5, cor);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colar_acha_chave_e_convite_em_qualquer_ordem() {
        let chave = fluxo_net::ServerKey::generate("203.0.113.10:3478")
            .unwrap()
            .to_string();
        let convite = fluxo_net::GroupKey::generate().unwrap().to_string();
        let texto = format!("Oi! Convite: {convite}\nChave do servidor: {chave}.");
        assert_eq!(
            separar_colado(&texto),
            (Some(chave.clone()), Some(convite.clone()))
        );
        assert_eq!(separar_colado(&chave), (Some(chave), None));
        assert_eq!(separar_colado("nada aqui"), (None, None));
    }

    #[test]
    fn qualidade_pelo_relatorio() {
        assert_eq!(
            qualidade_da_conexao("60 fps · 15.8 Mbps · perda 0.0% · RTT 1.89 ms"),
            Some(Conexao::Boa)
        );
        assert_eq!(
            qualidade_da_conexao("60 fps · 15.8 Mbps · perda 2.5% · RTT 30 ms"),
            Some(Conexao::Instavel)
        );
        assert_eq!(
            qualidade_da_conexao("60 fps · 15.8 Mbps · perda 0.0% · RTT 210 ms"),
            Some(Conexao::Instavel)
        );
        assert_eq!(
            qualidade_da_conexao("30 fps · 4.0 Mbps · perda 9.1%"),
            Some(Conexao::Ruim)
        );
        assert_eq!(qualidade_da_conexao("algo estranho"), None);
    }

    #[test]
    fn tempo_assistindo() {
        assert_eq!(ha_quanto(Duration::from_secs(30)), "agora");
        assert_eq!(ha_quanto(Duration::from_secs(5 * 60)), "há 5 min");
        assert_eq!(ha_quanto(Duration::from_secs(2 * 3600)), "há 2 h");
        assert_eq!(ha_quanto(Duration::from_secs(65 * 60)), "há 1 h 5 min");
    }
}
