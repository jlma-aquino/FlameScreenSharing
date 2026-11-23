//! O grupo da chave (ADR-022): quem está com o FSS aberto e quem está
//! compartilhando, numa thread que fala com o servidor
//! ([`fluxo_net::GroupClient`]). A janela só lê o [`Estado`] e conta o
//! apelido e a sala ([`Grupo::status`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use fluxo_net::{GroupClient, GroupMember, ServerKey};

/// O que a janela mostra.
#[derive(Debug, Clone, Default)]
pub struct Estado {
    /// Nome do grupo (depois da primeira resposta do servidor).
    pub nome: Option<String>,
    /// Online primeiro, por apelido.
    pub membros: Vec<GroupMember>,
    /// O servidor respondeu há pouco.
    pub conectado: bool,
    /// Não deu nem para achar o servidor (nome que não resolve, sem rede).
    pub erro: Option<String>,
}

#[derive(Default)]
struct Compartilhado {
    estado: Estado,
    /// O que contar ao grupo: apelido e sala.
    apelido: String,
    sala: Option<String>,
    assistindo: Option<String>,
}

/// Presença num grupo, enquanto existir (some ao trocar a chave ou fechar).
pub struct Grupo {
    chave: String,
    dados: Arc<Mutex<Compartilhado>>,
    parar: Arc<AtomicBool>,
}

impl Grupo {
    /// Entra no grupo da chave `texto` (já conferida), como o membro `membro`.
    pub fn iniciar(texto: &str, membro: u64, ctx: egui::Context) -> Result<Self, String> {
        let chave = ServerKey::parse(texto).map_err(|e| e.to_string())?;
        let dados = Arc::new(Mutex::new(Compartilhado::default()));
        let parar = Arc::new(AtomicBool::new(false));
        {
            let (dados, parar) = (dados.clone(), parar.clone());
            std::thread::Builder::new()
                .name("fluxo-grupo".into())
                .spawn(move || rodar(chave, membro, &dados, &parar, &ctx))
                .map_err(|e| e.to_string())?;
        }
        Ok(Self {
            chave: normalizar(texto),
            dados,
            parar,
        })
    }

    /// A chave deste grupo (sem espaços), para saber se a das configurações mudou.
    pub fn chave(&self) -> &str {
        &self.chave
    }

    /// Conta ao grupo o apelido, a sala que compartilha e a que assiste.
    pub fn status(&self, apelido: &str, sala: Option<&str>, assistindo: Option<&str>) {
        if let Ok(mut d) = self.dados.lock() {
            if d.apelido != apelido || d.sala.as_deref() != sala || d.assistindo.as_deref() != assistindo {
                d.apelido = apelido.to_string();
                d.sala = sala.map(str::to_string);
                d.assistindo = assistindo.map(str::to_string);
            }
        }
    }

    pub fn ver(&self) -> Estado {
        self.dados.lock().map(|d| d.estado.clone()).unwrap_or_default()
    }
}

impl Drop for Grupo {
    fn drop(&mut self) {
        self.parar.store(true, Ordering::Relaxed);
    }
}

/// A chave sem espaços nem quebras de linha.
pub fn normalizar(texto: &str) -> String {
    texto.split_whitespace().collect()
}

fn rodar(
    chave: ServerKey,
    membro: u64,
    dados: &Mutex<Compartilhado>,
    parar: &AtomicBool,
    ctx: &egui::Context,
) {
    let mut cliente = None;
    let mut tentou: Option<Instant> = None;
    let mut conectado = false;
    while !parar.load(Ordering::Relaxed) {
        if cliente.is_none() && tentou.is_none_or(|t| t.elapsed() >= Duration::from_secs(10)) {
            tentou = Some(Instant::now());
            match GroupClient::connect(chave.clone(), membro) {
                Ok(c) => cliente = Some(c),
                Err(e) => {
                    if let Ok(mut d) = dados.lock() {
                        d.estado.erro = Some(format!("não achei o servidor do grupo: {e}"));
                    }
                    ctx.request_repaint();
                }
            }
        }
        if let Some(c) = cliente.as_mut() {
            let (apelido, sala, assistindo) = match dados.lock() {
                Ok(d) => (d.apelido.clone(), d.sala.clone(), d.assistindo.clone()),
                Err(_) => break,
            };
            c.set_status(&apelido, sala.as_deref(), assistindo.as_deref());
            let agora = Instant::now();
            let mudou = c.poll(agora);
            let agora_conectado = c.connected(agora);
            if mudou || agora_conectado != conectado {
                conectado = agora_conectado;
                if let Ok(mut d) = dados.lock() {
                    d.estado = Estado {
                        nome: c.name().map(str::to_string),
                        membros: c.members().to_vec(),
                        conectado,
                        erro: None,
                    };
                }
                ctx.request_repaint();
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}
