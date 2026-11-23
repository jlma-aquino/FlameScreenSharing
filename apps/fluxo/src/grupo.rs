//! O grupo da chave (ADR-022): quem está com o FSS aberto e quem está
//! compartilhando, numa thread que fala com o servidor
//! ([`fluxo_net::GroupClient`]). A janela só lê o [`Estado`] e conta o
//! apelido e a sala ([`Grupo::status`]) e a foto ([`Grupo::foto`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use fluxo_net::{GroupClient, GroupKey, GroupMember, ServerKey};

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
    /// As fotos que já chegaram (JPEG), pelo número do membro: (número da foto, bytes).
    pub fotos: HashMap<u64, (u32, Arc<Vec<u8>>)>,
}

#[derive(Default)]
struct Compartilhado {
    estado: Estado,
    /// O que contar ao grupo: apelido e sala.
    apelido: String,
    sala: Option<String>,
    assistindo: Option<String>,
    /// A foto deste FSS (JPEG).
    foto: Option<Vec<u8>>,
}

/// Presença num grupo, enquanto existir (some ao trocar a chave ou fechar).
pub struct Grupo {
    chave: String,
    dados: Arc<Mutex<Compartilhado>>,
    parar: Arc<AtomicBool>,
}

impl Grupo {
    /// Entra no grupo do convite `convite`, pelo servidor da chave `texto`,
    /// como o membro `membro`. As fotos baixadas ficam em `pasta`.
    pub fn iniciar(
        texto: &str,
        convite: &str,
        membro: u64,
        pasta: Option<PathBuf>,
        ctx: egui::Context,
    ) -> Result<Self, String> {
        let chave = ServerKey::parse(texto).map_err(|e| format!("chave do servidor: {e}"))?;
        let grupo = GroupKey::parse(convite).map_err(|e| e.to_string())?;
        let dados = Arc::new(Mutex::new(Compartilhado::default()));
        let parar = Arc::new(AtomicBool::new(false));
        {
            let (dados, parar) = (dados.clone(), parar.clone());
            std::thread::Builder::new()
                .name("fluxo-grupo".into())
                .spawn(move || rodar(chave, grupo, membro, pasta, &dados, &parar, &ctx))
                .map_err(|e| e.to_string())?;
        }
        Ok(Self {
            chave: identidade(texto, convite),
            dados,
            parar,
        })
    }

    /// A chave e o convite deste grupo (sem espaços), para saber se mudaram.
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

    /// A foto deste FSS (JPEG), ou nenhuma. Sobe ao grupo só se mudou.
    pub fn foto(&self, jpeg: Option<&[u8]>) {
        if let Ok(mut d) = self.dados.lock() {
            if d.foto.as_deref() != jpeg {
                d.foto = jpeg.map(<[u8]>::to_vec);
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

/// A chave e o convite sem espaços nem quebras de linha, juntos.
pub fn identidade(chave: &str, convite: &str) -> String {
    let limpo = |t: &str| t.split_whitespace().collect::<String>();
    format!("{}|{}", limpo(chave), limpo(convite))
}

fn rodar(
    chave: ServerKey,
    grupo: GroupKey,
    membro: u64,
    pasta: Option<PathBuf>,
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
            match GroupClient::connect(chave.clone(), grupo.clone(), membro) {
                Ok(c) => {
                    cliente = Some(match &pasta {
                        Some(p) => c.with_photo_cache(p.clone()),
                        None => c,
                    })
                }
                Err(e) => {
                    if let Ok(mut d) = dados.lock() {
                        d.estado.erro = Some(format!("não achei o servidor do grupo: {e}"));
                    }
                    ctx.request_repaint();
                }
            }
        }
        if let Some(c) = cliente.as_mut() {
            let (apelido, sala, assistindo, foto) = match dados.lock() {
                Ok(d) => (
                    d.apelido.clone(),
                    d.sala.clone(),
                    d.assistindo.clone(),
                    d.foto.clone(),
                ),
                Err(_) => break,
            };
            c.set_status(&apelido, sala.as_deref(), assistindo.as_deref());
            let mudou_foto = c.photo(membro).map(|f| f.1) != foto.as_deref();
            c.set_photo(foto.as_deref());
            let agora = Instant::now();
            let mudou = c.poll(agora);
            let agora_conectado = c.connected(agora);
            if mudou || mudou_foto || agora_conectado != conectado {
                conectado = agora_conectado;
                let antes = dados.lock().map(|d| d.estado.fotos.clone()).unwrap_or_default();
                // Só copia os bytes das fotos que mudaram.
                let fotos = c
                    .members()
                    .iter()
                    .map(|m| m.member)
                    .chain([membro])
                    .filter_map(|m| {
                        let (id, jpeg) = c.photo(m)?;
                        Some(match antes.get(&m) {
                            Some(f) if f.0 == id => (m, f.clone()),
                            _ => (m, (id, Arc::new(jpeg.to_vec()))),
                        })
                    })
                    .collect();
                if let Ok(mut d) = dados.lock() {
                    d.estado = Estado {
                        nome: c.name().map(str::to_string),
                        membros: c.members().to_vec(),
                        conectado,
                        erro: None,
                        fotos,
                    };
                }
                ctx.request_repaint();
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}
