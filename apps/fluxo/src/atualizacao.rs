//! Atualizações pelo GitHub: ao abrir, a janela pergunta à API do GitHub qual
//! é a versão mais nova publicada (Releases). Se for mais nova que esta, baixa
//! o zip, confere o SHA-256 com o `SHA256SUMS.txt` publicado junto, troca os
//! arquivos da pasta do programa e abre a versão nova.
//!
//! Para baixar e abrir o zip, usa o `curl.exe` e o `tar.exe` que vêm com o
//! Windows 10 (1803+) e 11, pelo caminho completo da pasta do sistema; nada de
//! biblioteca de rede no programa. A troca funciona com o programa aberto: o
//! Windows deixa renomear um .exe em uso, então cada arquivo antigo vira
//! `*.antigo` (apagado na próxima abertura) e o novo entra no lugar.
//!
//! O SHA-256 garante que o zip chegou inteiro e é o da página da versão; não
//! protege se a conta do GitHub for invadida (para isso, assinatura digital).
//!
//! Só a versão empacotada (`FlameScreenSharing.exe`) se atualiza: rodando da
//! compilação (`fluxo.exe`), a janela só avisa.

use std::path::{Path, PathBuf};

/// Onde as versões são publicadas.
pub const REPOSITORIO: &str = "jlma-aquino/FlameScreenSharing";

/// A versão deste programa.
pub const VERSAO: &str = env!("CARGO_PKG_VERSION");

/// Nome do zip e da lista de SHA-256 anexados a cada versão.
pub const ZIP: &str = "FlameScreenSharing.zip";
pub const SOMAS: &str = "SHA256SUMS.txt";

/// Nome do programa empacotado (o que se atualiza).
pub const PROGRAMA: &str = "FlameScreenSharing.exe";

/// Variável de ambiente que troca o endereço da API (testes, com `file://`).
const ENDERECO_DE_TESTE: &str = "FSS_ATUALIZACOES";

/// `v0.2.0` ou `0.2.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Versao(pub u32, pub u32, pub u32);

impl Versao {
    pub fn ler(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        // "0.2.0-beta": vale a parte numérica.
        let s = s.split(['-', '+']).next()?;
        let mut p = s.split('.').map(|n| n.parse::<u32>().ok());
        let v = Versao(
            p.next()??,
            p.next().unwrap_or(Some(0))?,
            p.next().unwrap_or(Some(0))?,
        );
        p.next().is_none().then_some(v)
    }

    pub fn desta() -> Self {
        Self::ler(VERSAO).unwrap_or(Versao(0, 0, 0))
    }
}

impl std::fmt::Display for Versao {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// Uma versão publicada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lancamento {
    pub versao: Versao,
    /// O que mudou (texto da página da versão).
    pub notas: String,
    /// Página da versão no GitHub.
    pub pagina: String,
    pub zip: Option<String>,
    pub somas: Option<String>,
}

/// Lê a resposta de `GET /repos/{dono}/{repo}/releases/latest`.
pub fn ler_lancamento(json: &str) -> Result<Lancamento, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("resposta estranha do GitHub: {e}"))?;
    let tag = v["tag_name"]
        .as_str()
        .ok_or("a versão publicada não tem número (tag_name)")?;
    let versao = Versao::ler(tag).ok_or_else(|| format!("número de versão estranho: {tag}"))?;
    let anexo = |nome: &str| {
        v["assets"].as_array().and_then(|l| {
            l.iter()
                .find(|a| a["name"].as_str() == Some(nome))
                .and_then(|a| a["browser_download_url"].as_str())
                .map(str::to_string)
        })
    };
    Ok(Lancamento {
        versao,
        notas: v["body"].as_str().unwrap_or_default().trim().to_string(),
        pagina: v["html_url"].as_str().unwrap_or_default().to_string(),
        zip: anexo(ZIP),
        somas: anexo(SOMAS),
    })
}

/// O SHA-256 de `arquivo` numa lista no formato do `sha256sum`
/// (`<hex>  <nome>`, um por linha; `*` antes do nome também vale).
pub fn soma_esperada(lista: &str, arquivo: &str) -> Option<[u8; 32]> {
    lista.lines().find_map(|l| {
        let (hex, nome) = l.trim().split_once(char::is_whitespace)?;
        if nome.trim().trim_start_matches('*') != arquivo {
            return None;
        }
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
            .collect::<Option<_>>()?;
        bytes.try_into().ok()
    })
}

pub fn sha256(dados: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(dados).into()
}

/// A atualização baixada, conferida e aberta numa pasta, pronta para instalar.
#[derive(Debug, Clone)]
pub struct Pronta {
    pub versao: Versao,
    pub notas: String,
    /// Pasta com os arquivos novos (a que tem o `FlameScreenSharing.exe`).
    pub origem: PathBuf,
}

/// O que a verificação achou.
#[derive(Debug, Clone)]
pub enum Resultado {
    /// Esta já é a mais nova (ou mais nova que a publicada).
    EmDia,
    /// Há uma mais nova, mas este programa não se atualiza sozinho (rodando
    /// da compilação, ou fora do Windows): só avisa, com a página.
    Disponivel(Lancamento),
    Pronta(Pronta),
}

/// A pasta e o nome deste programa, se ele for o empacotado.
fn programa_empacotado() -> Option<PathBuf> {
    let eu = std::env::current_exe().ok()?;
    let nome = eu.file_name()?.to_str()?;
    (cfg!(windows) && nome.eq_ignore_ascii_case(PROGRAMA)).then(|| eu.parent().map(Path::to_path_buf))?
}

/// Verifica e, se houver versão nova, baixa, confere e abre o zip. Demora
/// (rede): chame numa thread.
pub fn verificar_e_baixar() -> Result<Resultado, String> {
    let endereco = std::env::var(ENDERECO_DE_TESTE)
        .unwrap_or_else(|_| format!("https://api.github.com/repos/{REPOSITORIO}/releases/latest"));
    let pasta = std::env::temp_dir().join("FlameScreenSharing-atualizacao");
    let _ = std::fs::remove_dir_all(&pasta);
    std::fs::create_dir_all(&pasta).map_err(|e| format!("pasta temporária: {e}"))?;

    let resposta = pasta.join("versao.json");
    baixar(&endereco, &resposta).map_err(|e| match e.contains("404") {
        true => "não há versão publicada (ou o repositório não é público)".to_string(),
        false => e,
    })?;
    let texto = std::fs::read_to_string(&resposta).map_err(|e| e.to_string())?;
    let l = ler_lancamento(&texto)?;
    if l.versao <= Versao::desta() {
        return Ok(Resultado::EmDia);
    }
    if programa_empacotado().is_none() {
        return Ok(Resultado::Disponivel(l));
    }
    let (Some(url_zip), Some(url_somas)) = (&l.zip, &l.somas) else {
        return Err(format!("a versão {} não tem {ZIP} e {SOMAS} anexados", l.versao));
    };

    let zip = pasta.join(ZIP);
    let somas = pasta.join(SOMAS);
    baixar(url_somas, &somas)?;
    baixar(url_zip, &zip)?;
    let lista = std::fs::read_to_string(&somas).map_err(|e| e.to_string())?;
    let esperada = soma_esperada(&lista, ZIP).ok_or(format!("{SOMAS} não tem a soma do {ZIP}"))?;
    let dados = std::fs::read(&zip).map_err(|e| e.to_string())?;
    if sha256(&dados) != esperada {
        return Err("o zip baixado não confere com o SHA-256 publicado; nada foi trocado".into());
    }
    let extraido = pasta.join("extraido");
    std::fs::create_dir_all(&extraido).map_err(|e| e.to_string())?;
    extrair(&zip, &extraido)?;
    let origem = achar_programa(&extraido).ok_or(format!("o zip não tem o {PROGRAMA}"))?;
    Ok(Resultado::Pronta(Pronta {
        versao: l.versao,
        notas: l.notas,
        origem,
    }))
}

/// A pasta (o próprio `dir` ou uma subpasta) que tem o programa.
fn achar_programa(dir: &Path) -> Option<PathBuf> {
    if dir.join(PROGRAMA).is_file() {
        return Some(dir.to_path_buf());
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .find_map(|p| achar_programa(&p))
}

/// Troca os arquivos da pasta do programa pelos de `pronta.origem` e abre a
/// versão nova. Se der certo, não volta (este processo termina).
pub fn instalar_e_reiniciar(pronta: &Pronta) -> Result<(), String> {
    let destino = programa_empacotado().ok_or("esta cópia não se atualiza sozinha")?;
    let novos: Vec<PathBuf> = std::fs::read_dir(&pronta.origem)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    let mut trocados: Vec<PathBuf> = Vec::new();
    let desfazer = |trocados: &[PathBuf]| {
        for alvo in trocados {
            let _ = std::fs::remove_file(alvo);
            let _ = std::fs::rename(antigo(alvo), alvo);
        }
    };
    for novo in &novos {
        let Some(nome) = novo.file_name() else { continue };
        let alvo = destino.join(nome);
        let r = (|| -> std::io::Result<()> {
            if alvo.exists() {
                let velho = antigo(&alvo);
                let _ = std::fs::remove_file(&velho);
                std::fs::rename(&alvo, &velho)?;
            }
            std::fs::copy(novo, &alvo).map(|_| ())
        })();
        if let Err(e) = r {
            desfazer(&trocados);
            return Err(format!(
                "não consegui trocar {} ({e}); a versão antiga continua",
                alvo.display()
            ));
        }
        trocados.push(alvo);
    }
    let programa = destino.join(PROGRAMA);
    std::process::Command::new(&programa)
        .arg("--atualizado")
        .current_dir(&destino)
        .spawn()
        .map_err(|e| format!("instalei, mas não consegui abrir a versão nova: {e}"))?;
    std::process::exit(0);
}

/// Apaga os `*.antigo` que a última atualização deixou (na abertura).
pub fn limpar_antigos() {
    let Some(pasta) = programa_empacotado() else {
        return;
    };
    let Ok(lista) = std::fs::read_dir(pasta) else {
        return;
    };
    for e in lista.flatten() {
        if e.path().extension().is_some_and(|x| x == "antigo") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn antigo(alvo: &Path) -> PathBuf {
    let mut s = alvo.as_os_str().to_owned();
    s.push(".antigo");
    PathBuf::from(s)
}

/// Programa do sistema (`curl.exe`, `tar.exe`), pelo caminho completo: um
/// `tar` do Git no PATH não entra no lugar.
fn do_sistema(nome: &str) -> PathBuf {
    let raiz = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(raiz).join("System32").join(nome)
}

fn rodar(programa: &Path, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let mut c = std::process::Command::new(programa);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let saida = c
        .output()
        .map_err(|e| format!("não consegui rodar {}: {e}", programa.display()))?;
    match saida.status.success() {
        true => Ok(()),
        false => Err(String::from_utf8_lossy(&saida.stderr).trim().to_string()),
    }
}

/// Baixa `url` para `arquivo` (segue redirecionamentos; erro HTTP é erro).
fn baixar(url: &str, arquivo: &Path) -> Result<(), String> {
    let agente = format!("FlameScreenSharing/{VERSAO}");
    let r = rodar(
        &do_sistema("curl.exe"),
        &[
            "--fail".as_ref(),
            "--silent".as_ref(),
            "--show-error".as_ref(),
            "--location".as_ref(),
            "--max-time".as_ref(),
            "300".as_ref(),
            "--user-agent".as_ref(),
            agente.as_ref(),
            "--header".as_ref(),
            "Accept: application/vnd.github+json".as_ref(),
            "--output".as_ref(),
            arquivo.as_os_str(),
            url.as_ref(),
        ],
    );
    r.map_err(|e| format!("não consegui baixar: {e}"))
}

fn extrair(zip: &Path, pasta: &Path) -> Result<(), String> {
    rodar(
        &do_sistema("tar.exe"),
        &["-xf".as_ref(), zip.as_os_str(), "-C".as_ref(), pasta.as_os_str()],
    )
    .map_err(|e| format!("não consegui abrir o zip: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versoes_em_ordem() {
        assert_eq!(Versao::ler("v0.2.0"), Some(Versao(0, 2, 0)));
        assert_eq!(Versao::ler("1.10"), Some(Versao(1, 10, 0)));
        assert_eq!(Versao::ler("0.3.1-beta"), Some(Versao(0, 3, 1)));
        assert_eq!(Versao::ler("abc"), None);
        assert_eq!(Versao::ler("1.2.3.4"), None);
        assert!(Versao(0, 10, 0) > Versao(0, 9, 9));
        assert!(Versao(1, 0, 0) > Versao(0, 99, 0));
        assert_eq!(Versao::desta().to_string(), VERSAO);
    }

    #[test]
    fn resposta_do_github() {
        let json = r#"{
            "tag_name": "v0.3.0",
            "html_url": "https://github.com/x/y/releases/tag/v0.3.0",
            "body": "  Barra nova.\r\n",
            "assets": [
                {"name": "SHA256SUMS.txt", "browser_download_url": "https://a/SHA256SUMS.txt"},
                {"name": "FlameScreenSharing.zip", "browser_download_url": "https://a/FlameScreenSharing.zip"}
            ]
        }"#;
        let l = ler_lancamento(json).unwrap();
        assert_eq!(l.versao, Versao(0, 3, 0));
        assert_eq!(l.notas, "Barra nova.");
        assert_eq!(l.zip.as_deref(), Some("https://a/FlameScreenSharing.zip"));
        assert_eq!(l.somas.as_deref(), Some("https://a/SHA256SUMS.txt"));
        let sem_anexos = ler_lancamento(r#"{"tag_name": "0.3.0"}"#).unwrap();
        assert_eq!(sem_anexos.zip, None);
        assert!(ler_lancamento(r#"{"message": "Not Found"}"#).is_err());
        assert!(ler_lancamento("<html>").is_err());
    }

    #[test]
    fn soma_da_lista() {
        let soma = sha256(b"abc");
        let hex: String = soma.iter().map(|b| format!("{b:02x}")).collect();
        assert!(hex.starts_with("ba7816bf"));
        let lista = format!("0000  outro.zip\n{hex}  {ZIP}\n");
        assert_eq!(soma_esperada(&lista, ZIP), Some(soma));
        let com_asterisco = format!("{}  *{ZIP}", hex.to_uppercase());
        assert_eq!(soma_esperada(&com_asterisco, ZIP), Some(soma));
        assert_eq!(soma_esperada("abc  FlameScreenSharing.zip", ZIP), None);
        assert_eq!(soma_esperada("", ZIP), None);
    }
}
