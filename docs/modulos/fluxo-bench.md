# fluxo-bench

**Caminho:** `apps/fluxo-bench` · **Plataforma:** Windows

Ferramenta de linha de comando para validar a captura de tela isoladamente.

## Arquivos

| Arquivo | Conteúdo |
|---|---|
| `src/main.rs` | Parsing de argumentos e os comandos `listar`, `bench`, `screenshot` |
| `src/bmp.rs` | Gravação de BMP 32 bits sem dependências |

## Comandos

```text
fluxo-bench listar
fluxo-bench bench      [--monitor N] [--segundos S] [--copiar-cpu]
fluxo-bench screenshot [--monitor N] [--saida arquivo.bmp]
```

## Lendo o resultado do `bench`

| Linha | Significado | Esperado com vídeo/jogo a 60 fps |
|---|---|---|
| frames com imagem nova | Frames em que a tela mudou | ~60 fps médio |
| updates só de cursor | Só o mouse mexeu | Qualquer valor |
| esperas sem mudança | Blocos de 100 ms sem nenhuma mudança | ~0 |
| frames perdidos (Windows acumulou) | O Windows desenhou e não pegamos | 0 ou perto disso |
| perdas de acesso | ACCESS_LOST (recuperado sozinho) | 0, salvo troca de resolução/UAC |
| intervalo entre frames p50 / p99 | Regularidade | p50 ≈ 16,7 ms; p99 < 25 ms |
| cópia GPU→RAM (com `--copiar-cpu`) | Custo de trazer o frame para a RAM | 1–5 ms em 1080p |

O FPS medido é limitado pelo que está **na tela**: um vídeo de 30 fps dá 30;
um jogo a 144 fps num monitor de 144 Hz pode dar 144.

**O que é ruim:** "frames perdidos" crescendo, ou p99 muito acima do
intervalo do monitor — sinal de que algo (CPU ocupada, `--copiar-cpu`) está
atrasando a captura.
