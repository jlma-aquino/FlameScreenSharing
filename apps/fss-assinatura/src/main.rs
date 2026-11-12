//! # fss-assinatura
//!
//! A chave que assina as atualizações (ADR-020): só quem publica as versões
//! usa, no próprio PC. A chave privada fica num arquivo fora do git
//! (`.secrets\`); a pública vai dentro do programa (`atualizacao.rs`).
//!
//! ```text
//! fss-assinatura gerar ARQUIVO
//! fss-assinatura publica ARQUIVO
//! fss-assinatura assinar --chave ARQUIVO --versao X.Y.Z --arquivo ZIP --saida MANIFESTO.txt
//! ```

use std::path::Path;
use std::process::ExitCode;

use fluxo_crypto::{de_hex, hex, update_public_key, UpdateManifest};
use sha2::{Digest, Sha256};

const AJUDA: &str = "\
fss-assinatura — assina as versões publicadas do FlameScreenSharing

Uso:
  fss-assinatura gerar ARQUIVO      cria a chave privada em ARQUIVO (não sobrescreve)
                                    e mostra a chave pública, para pôr no programa
  fss-assinatura publica ARQUIVO    mostra a chave pública de uma chave privada
  fss-assinatura assinar --chave ARQUIVO --versao X.Y.Z --arquivo ZIP --saida MANIFESTO.txt
                                    assina a versão: versão, nome e SHA-256 do zip

Guarde a chave privada fora do git e com cópia de segurança: sem ela, não dá
para publicar versões que os programas já instalados aceitem.";

fn ler_privada(arquivo: &str) -> Result<[u8; 32], String> {
    let texto = std::fs::read_to_string(arquivo).map_err(|e| format!("ler {arquivo}: {e}"))?;
    de_hex(texto.trim())
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| format!("{arquivo} não tem uma chave privada (64 caracteres hexadecimais)"))
}

fn executar(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("gerar") => {
            let arquivo = args.get(1).ok_or("gerar ARQUIVO")?;
            if Path::new(arquivo).exists() {
                return Err(format!("{arquivo} já existe: não vou sobrescrever uma chave"));
            }
            let mut k = [0u8; 32];
            fluxo_crypto::random_bytes(&mut k).map_err(|e| e.to_string())?;
            std::fs::write(arquivo, format!("{}\n", hex(&k)))
                .map_err(|e| format!("gravar {arquivo}: {e}"))?;
            println!("Chave privada em {arquivo}. Chave pública (para o programa):");
            println!("{}", hex(&update_public_key(&k)));
            Ok(())
        }
        Some("publica") => {
            let k = ler_privada(args.get(1).ok_or("publica ARQUIVO")?)?;
            println!("{}", hex(&update_public_key(&k)));
            Ok(())
        }
        Some("assinar") => {
            let opcao = |nome: &str| -> Result<String, String> {
                let i = args
                    .iter()
                    .position(|a| a == nome)
                    .ok_or(format!("falta {nome}"))?;
                args.get(i + 1)
                    .cloned()
                    .ok_or(format!("{nome} precisa de um valor"))
            };
            let chave = ler_privada(&opcao("--chave")?)?;
            let versao = opcao("--versao")?;
            let arquivo = opcao("--arquivo")?;
            let saida = opcao("--saida")?;
            let dados = std::fs::read(&arquivo).map_err(|e| format!("ler {arquivo}: {e}"))?;
            let nome = Path::new(&arquivo)
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or("nome de arquivo estranho")?
                .to_string();
            let m = UpdateManifest {
                version: versao.trim_start_matches('v').to_string(),
                file: nome,
                sha256: Sha256::digest(&dados).into(),
            };
            std::fs::write(&saida, m.sign(&chave)).map_err(|e| format!("gravar {saida}: {e}"))?;
            println!(
                "Assinado: {saida} (versão {}, {}, SHA-256 {})",
                m.version,
                m.file,
                hex(&m.sha256)
            );
            Ok(())
        }
        _ => Err(String::new()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match executar(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.is_empty() => {
            println!("{AJUDA}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("erro: {e}\n\n{AJUDA}");
            ExitCode::from(2)
        }
    }
}
