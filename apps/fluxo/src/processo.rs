//! Roda o `fluxo-host` ou o `fluxo-client` por baixo da janela: sem console,
//! com as linhas que eles imprimem chegando por um canal e a entrada padrão
//! aberta (é por ela que o cliente recebe o código de resposta).

use std::io::{self, BufRead, BufReader, Read, Write};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

#[cfg(windows)]
/// `CREATE_NO_WINDOW`: sem janela de console. A janela de vídeo do cliente
/// não depende disso.
const SEM_CONSOLE: u32 = 0x0800_0000;

/// Um programa em execução. Soltar encerra o programa.
pub struct Processo {
    filho: Child,
    entrada: Option<ChildStdin>,
    linhas: Receiver<String>,
}

/// Caminho de um programa (`host` ou `client`): na mesma pasta deste
/// executável, com o nome do pacote (`FSS-host.exe`) ou o da compilação
/// (`fluxo-host.exe`).
pub fn caminho_do_programa(nome: &str) -> io::Result<PathBuf> {
    let eu = std::env::current_exe()?;
    let pasta = eu
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "pasta do programa"))?;
    ["FSS", "fluxo"]
        .iter()
        .map(|prefixo| pasta.join(format!("{prefixo}-{nome}{}", std::env::consts::EXE_SUFFIX)))
        .find(|caminho| caminho.exists())
        .ok_or_else(|| {
            let falta = format!("falta FSS-{nome}.exe nesta pasta. Extraia o zip inteiro antes de abrir");
            io::Error::new(io::ErrorKind::NotFound, falta)
        })
}

impl Processo {
    /// Inicia `programa`. `avisar` é chamado com cada linha nova (para a janela
    /// se redesenhar) e com `None` quando a saída padrão fecha (o programa
    /// terminou), da thread que lê: serve para reagir mesmo com a janela
    /// escondida, quando ela não se redesenha.
    pub fn iniciar(programa: &Path, args: &[String], avisar: Aviso) -> io::Result<Self> {
        let mut comando = Command::new(programa);
        comando
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        comando.creation_flags(SEM_CONSOLE);
        let mut filho = comando.spawn()?;
        let entrada = filho.stdin.take();
        let (tx, linhas) = channel();
        if let Some(saida) = filho.stdout.take() {
            ler_em_thread(saida, tx.clone(), avisar.clone(), true);
        }
        if let Some(erros) = filho.stderr.take() {
            ler_em_thread(erros, tx, avisar, false);
        }
        Ok(Self {
            filho,
            entrada,
            linhas,
        })
    }

    /// Número do processo (para deixar a janela dele tomar o foco).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn pid(&self) -> u32 {
        self.filho.id()
    }

    /// As linhas que chegaram desde a última chamada.
    pub fn linhas_novas(&self) -> Vec<String> {
        self.linhas.try_iter().collect()
    }

    /// Manda uma linha para a entrada padrão do programa.
    pub fn enviar(&mut self, linha: &str) -> io::Result<()> {
        let entrada = self
            .entrada
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "entrada fechada"))?;
        writeln!(entrada, "{linha}")?;
        entrada.flush()
    }

    /// `Some(sucesso)` se o programa já terminou.
    pub fn terminou(&mut self) -> Option<bool> {
        self.filho.try_wait().ok().flatten().map(|s| s.success())
    }
}

impl Drop for Processo {
    fn drop(&mut self) {
        let _ = self.filho.kill();
        let _ = self.filho.wait();
    }
}

/// Chamado com cada linha e com `None` no fim (ver [`Processo::iniciar`]).
pub type Aviso = Arc<dyn Fn(Option<&str>) + Send + Sync>;

/// Uma thread por saída (stdout e stderr), até o programa fechar. Sem
/// `lines()`: um byte fora do UTF-8 não pode calar o resto. `fim`: avisa
/// quando esta saída fecha.
fn ler_em_thread(saida: impl Read + Send + 'static, tx: Sender<String>, avisar: Aviso, fim: bool) {
    std::thread::spawn(move || {
        let mut leitor = BufReader::new(saida);
        let mut bytes = Vec::new();
        loop {
            bytes.clear();
            match leitor.read_until(b'\n', &mut bytes) {
                Ok(0) | Err(_) => {
                    if fim {
                        avisar(None);
                    }
                    return;
                }
                Ok(_) => {}
            }
            let linha = String::from_utf8_lossy(&bytes).trim_end().to_string();
            avisar(Some(&linha));
            if tx.send(linha).is_err() {
                return;
            }
        }
    });
}

// Os testes usam o PowerShell como programa de eco.
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Prazo folgado: o PowerShell demora para abrir com a máquina ocupada.
    const PRAZO: Duration = Duration::from_secs(30);

    fn esperar_linha(p: &Processo) -> Option<String> {
        let limite = Instant::now() + PRAZO;
        while Instant::now() < limite {
            if let Some(l) = p.linhas_novas().pop() {
                return Some(l);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    /// PowerShell rodando `comando`: existe em todo Windows e, ao contrário
    /// do findstr, não segura a saída em buffer quando ela vai para um pipe.
    fn powershell(comando: &str) -> Processo {
        let sistema = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let exe = Path::new(&sistema).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        let args = ["-NoProfile", "-NonInteractive", "-Command", comando].map(String::from);
        Processo::iniciar(&exe, &args, Arc::new(|_| {})).unwrap()
    }

    #[test]
    fn entrada_e_saida_passam_pelo_processo() {
        let mut p = powershell("[Console]::Out.WriteLine('eco ' + [Console]::In.ReadLine())");
        p.enviar("ABC-123").unwrap();
        assert_eq!(esperar_linha(&p).as_deref(), Some("eco ABC-123"));
    }

    #[test]
    fn erro_tambem_chega_e_o_fim_e_percebido() {
        let mut p = powershell("[Console]::Error.WriteLine('erro: falhou'); exit 3");
        assert_eq!(esperar_linha(&p).as_deref(), Some("erro: falhou"));
        let limite = Instant::now() + PRAZO;
        loop {
            match p.terminou() {
                Some(sucesso) => {
                    assert!(!sucesso, "saiu com código 3");
                    break;
                }
                None => assert!(Instant::now() < limite, "não terminou"),
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn soltar_encerra_o_programa() {
        // Fica lendo a entrada para sempre; só um kill o derruba.
        let mut p = powershell("while ($true) { Start-Sleep 1 }");
        assert_eq!(p.terminou(), None);
        let pid = p.filho.id();
        drop(p);
        // Sem o processo, abrir pelo PID falha ou mostra outro programa.
        let lista = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .creation_flags(SEM_CONSOLE)
            .output()
            .unwrap();
        assert!(!String::from_utf8_lossy(&lista.stdout).contains("powershell"));
    }

    #[test]
    fn programa_que_nao_existe_avisa() {
        assert!(caminho_do_programa("nao-existe-fluxo").is_err());
    }
}
