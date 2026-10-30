# Convenções de código e documentação

Objetivo: qualquer pessoa (ou você daqui a 6 meses) consegue abrir um arquivo,
entender o que ele faz e alterá-lo com segurança.

## Documentação — onde fica cada coisa

| O quê | Onde |
|---|---|
| O que um crate faz e como usar | `//!` no topo do `lib.rs` + `docs/modulos/<crate>.md` |
| O que um arquivo/módulo faz | `//!` no topo do arquivo |
| O que uma função/tipo/campo público faz | `///` logo acima |
| Por que uma linha é do jeito que é | `//` comentário comum |
| Desenho geral do sistema | `docs/ARQUITETURA.md` |
| Formato de bytes na rede | `docs/PROTOCOLO.md` |
| Por que escolhemos X em vez de Y | `docs/DECISOES.md` (novo ADR) |
| O que mudou | `CHANGELOG.md` |

### Regras

1. **Todo item `pub` tem `///`.** Diga *o que* faz, *quando* usar e os casos
   especiais (erros, `None`, panics).
2. Comentários explicam o **porquê**, não repetem o código.
3. Exemplos em `///` com ` ``` ` são **testes de verdade** (`cargo test` roda).
   Prefira exemplos curtos e executáveis.
4. Mudou comportamento? Atualize o `///`, o doc do módulo em `docs/modulos/`
   e o `CHANGELOG.md` **no mesmo commit**.
5. Todo bloco `unsafe` tem por perto um comentário dizendo por que é seguro
   ou qual regra da API está sendo respeitada.

## Código

- Idioma: nomes de código em **inglês** (padrão do ecossistema Rust, combina
  com as APIs), comentários e documentação em **português**.
- Formatação: `cargo fmt` (configuração em `rustfmt.toml`).
- Lint: `cargo clippy --workspace --all-targets` sem avisos.
- Erros: um `enum` por crate, com `Display` em português. Nada de `unwrap()`
  em caminho que dependa de dados externos (rede, SO).
- Testes unitários ficam no próprio arquivo, em `mod tests` no fim.
- Nomes de teste descrevem o comportamento: `frame_vazio_gera_um_pacote`.

## Antes de cada commit

```powershell
cargo fmt
cargo clippy --workspace --all-targets
cargo test --workspace
```

## Mensagens de commit

`<área>: <o que mudou>` — ex.: `proto: adiciona pacote KeyframeRequest`,
`capture: trata DXGI_ERROR_SESSION_DISCONNECTED`, `docs: atualiza roadmap`.
