//! De onde vem o som da transmissão.
//!
//! - Transmitindo uma janela: só o som do programa dela (e dos filhos).
//! - Transmitindo a tela: tudo o que o PC toca, menos o Discord (quem está na
//!   call não ouve a própria voz de volta) ou, sem o Discord aberto, menos o
//!   próprio FSS (o som de quem você assiste não volta na sua transmissão).
//!   O Windows só deixa tirar um programa por captura: com os dois, sai o
//!   Discord.
//! - Com uma saída de som escolhida nas Configurações: aquela saída inteira.

/// O que a thread de áudio deve capturar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modo {
    /// A saída inteira (`None`: a padrão do Windows).
    Saida(Option<usize>),
    /// Tudo menos este processo (e os filhos); `discord` diz qual é.
    Sem { pid: u32, discord: bool },
    /// Só este processo (e os filhos): o da janela transmitida.
    So(u32),
}

impl Modo {
    pub fn descricao(&self) -> &'static str {
        match self {
            Modo::Saida(_) => "a saída de som inteira",
            Modo::Sem { discord: true, .. } => "tudo o que o PC toca, menos o Discord",
            Modo::Sem { discord: false, .. } => "tudo o que o PC toca, menos o som do FSS",
            Modo::So(_) => "só o programa da janela transmitida",
        }
    }
}

/// Escolhe o modo. `janela`: processo da janela transmitida (`None` na tela).
pub fn escolher(
    janela: Option<u32>,
    saida: Option<usize>,
    discord: Option<u32>,
    sem_fss: Option<u32>,
) -> Modo {
    if let Some(pid) = janela {
        return Modo::So(pid);
    }
    if saida.is_some() {
        return Modo::Saida(saida);
    }
    match (discord, sem_fss) {
        (Some(pid), _) => Modo::Sem { pid, discord: true },
        (None, Some(pid)) => Modo::Sem { pid, discord: false },
        (None, None) => Modo::Saida(None),
    }
}

fn eh_discord(nome: &str) -> bool {
    let n = nome.to_ascii_lowercase();
    matches!(n.as_str(), "discord.exe" | "discordptb.exe" | "discordcanary.exe")
}

/// O processo raiz do Discord numa lista `(pid, pai, nome)`: um Discord cujo
/// pai não é Discord. Tirando a árvore dele, saem todos os processos do
/// Discord, inclusive o que toca a voz da call.
pub fn raiz_discord(processos: &[(u32, u32, String)]) -> Option<u32> {
    processos
        .iter()
        .filter(|(_, _, nome)| eh_discord(nome))
        .find(|(_, pai, _)| {
            !processos
                .iter()
                .any(|(pid, _, nome)| pid == pai && eh_discord(nome))
        })
        .map(|(pid, _, _)| *pid)
}

/// O Discord aberto agora (o processo raiz), se houver.
#[cfg(windows)]
pub fn discord() -> Option<u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };

    let mut lista = Vec::new();
    // SAFETY: foto dos processos; só lê as entradas, e o handle é fechado no fim.
    unsafe {
        let foto = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut tem = Process32FirstW(foto, &mut e).is_ok();
        while tem {
            let fim = e
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(e.szExeFile.len());
            lista.push((
                e.th32ProcessID,
                e.th32ParentProcessID,
                String::from_utf16_lossy(&e.szExeFile[..fim]),
            ));
            tem = Process32NextW(foto, &mut e).is_ok();
        }
        let _ = CloseHandle(foto);
    }
    raiz_discord(&lista)
}

/// O processo dono da janela `hwnd` (`None` se ela não existe mais).
#[cfg(windows)]
pub fn processo_da_janela(hwnd: isize) -> Option<u32> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    let mut pid = 0u32;
    // SAFETY: só lê o dono da janela; com uma janela inválida, devolve 0.
    unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

#[cfg(test)]
mod testes {
    use super::*;

    fn p(pid: u32, pai: u32, nome: &str) -> (u32, u32, String) {
        (pid, pai, nome.to_string())
    }

    #[test]
    fn acha_a_raiz_do_discord() {
        let l = [
            p(10, 1, "explorer.exe"),
            p(21, 20, "Discord.exe"),
            p(20, 5, "Update.exe"),
            p(22, 21, "Discord.exe"),
            p(23, 21, "Discord.exe"),
        ];
        assert_eq!(raiz_discord(&l), Some(21));
        assert_eq!(raiz_discord(&[p(30, 1, "DiscordPTB.exe")]), Some(30));
        assert_eq!(raiz_discord(&[p(10, 1, "explorer.exe")]), None);
    }

    #[test]
    fn janela_so_o_programa_dela() {
        assert_eq!(escolher(Some(7), Some(2), Some(9), Some(5)), Modo::So(7));
    }

    #[test]
    fn tela_sem_o_discord_ou_sem_o_fss() {
        assert_eq!(
            escolher(None, None, Some(9), Some(5)),
            Modo::Sem {
                pid: 9,
                discord: true
            }
        );
        assert_eq!(
            escolher(None, None, None, Some(5)),
            Modo::Sem {
                pid: 5,
                discord: false
            }
        );
        assert_eq!(escolher(None, None, None, None), Modo::Saida(None));
    }

    #[test]
    fn saida_escolhida_vale_na_tela() {
        assert_eq!(escolher(None, Some(2), Some(9), Some(5)), Modo::Saida(Some(2)));
    }
}
