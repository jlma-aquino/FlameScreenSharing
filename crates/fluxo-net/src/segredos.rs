//! Opções secretas pela entrada padrão, em vez da linha de comando.
//!
//! No Windows, qualquer programa do mesmo usuário lê a linha de comando dos
//! outros (Gerenciador de Tarefas, WMI). Com `--segredos-pela-entrada`, o
//! `fluxo-host` e o `fluxo-client` leem, antes de qualquer outra coisa, linhas
//! `--opcao valor` (a chave do servidor, códigos) até uma linha `fim`, e as
//! juntam às opções. É assim que a janela do FSS passa os segredos.
//!
//! Não protege de um programa que já rode como o mesmo usuário e leia a
//! memória dos outros; tira os segredos de onde qualquer um olha.

use std::io::BufRead;

/// A opção que liga a leitura.
pub const SECRETS_FROM_STDIN: &str = "--segredos-pela-entrada";

/// Lê `--opcao valor` (um por linha) até `fim`; devolve as palavras, na ordem,
/// para juntar às opções da linha de comando.
pub fn read_secret_args(entrada: impl BufRead) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    for linha in entrada.lines() {
        let linha = linha.map_err(|e| format!("ler os segredos da entrada: {e}"))?;
        let linha = linha.trim();
        if linha == "fim" {
            return Ok(args);
        }
        if linha.is_empty() {
            continue;
        }
        let (opcao, valor) = linha
            .split_once(' ')
            .ok_or_else(|| "segredo sem valor (use --opcao valor)".to_string())?;
        if !opcao.starts_with("--") {
            return Err(format!("segredo estranho: {opcao}"));
        }
        args.push(opcao.to_string());
        args.push(valor.trim().to_string());
    }
    Err("a entrada fechou antes do fim dos segredos".into())
}

/// As opções do programa: as da linha de comando e, com
/// [`SECRETS_FROM_STDIN`], as lidas da entrada padrão (sem a própria opção).
pub fn args_with_secrets(mut args: Vec<String>) -> Result<Vec<String>, String> {
    let Some(i) = args.iter().position(|a| a == SECRETS_FROM_STDIN) else {
        return Ok(args);
    };
    args.remove(i);
    args.extend(read_secret_args(std::io::stdin().lock())?);
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_ate_o_fim() {
        let entrada = "--chave FSS1-AAAAA-BBBBB\n\n--sala K7M4-QX9P\nfim\nvolume 30\n".as_bytes();
        assert_eq!(
            read_secret_args(entrada).unwrap(),
            ["--chave", "FSS1-AAAAA-BBBBB", "--sala", "K7M4-QX9P"]
        );
    }

    #[test]
    fn erros() {
        assert!(read_secret_args("--chave X\n".as_bytes()).is_err());
        assert!(read_secret_args("--chave\nfim\n".as_bytes()).is_err());
        assert!(read_secret_args("chave X\nfim\n".as_bytes()).is_err());
        assert_eq!(
            read_secret_args("fim\n".as_bytes()).unwrap(),
            Vec::<String>::new()
        );
    }
}
