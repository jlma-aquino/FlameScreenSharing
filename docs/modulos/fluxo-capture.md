# fluxo-capture

**Caminho:** `crates/fluxo-capture` · **Dependências:** `windows` (só no Windows) · **Plataforma:** Windows 8+

Captura a tela via DXGI Desktop Duplication e entrega cada frame como textura
D3D11 na GPU. Por que essa API: ADR-002 em [DECISOES.md](../DECISOES.md).

## Arquivos

| Arquivo | Conteúdo |
|---|---|
| `src/lib.rs` | Tipos comuns: `MonitorInfo`, `FrameInfo`, `CpuImageInfo`, `CaptureError` |
| `src/dxgi.rs` | Implementação Windows: `DesktopCapturer`, `Frame`, `list_monitors` |
| `src/stub.rs` | Mesma API, sempre `Unsupported` (outros sistemas) |

## API

| Item | Para quê |
|---|---|
| `enable_dpi_awareness()` | Chamar 1x no início do `main`, para enxergar pixels reais em telas com escala |
| `list_monitors()` | Lista monitores com índice, nome, GPU, posição e tamanho |
| `DesktopCapturer::new(i)` | Começa a capturar o monitor `i` |
| `cap.next_frame(timeout_ms)` | `Ok(Some(frame))`, `Ok(None)` se nada mudou, `Err(AccessLost)` se precisa recriar |
| `cap.d3d_device()` | Device D3D11 — o encoder deve usar o **mesmo** |
| `frame.texture()` | Textura BGRA na GPU, válida enquanto o `Frame` existir |
| `frame.info()` | Se a imagem mudou ou só o cursor; frames acumulados (perdidos) |
| `frame.copy_to_cpu(&mut buf)` | Pixels na RAM (lento; debug/screenshot) |

## Regras da API do Windows que o código respeita

1. **Um frame por vez.** O próximo só chega depois de liberar o atual.
   `Frame` empresta o capturador com `&mut` e libera no `Drop` — o compilador
   impede segurar dois.
2. **Device na GPU certa.** A duplicação precisa de um device criado no
   mesmo adapter do monitor; `new()` cuida disso.
3. **ACCESS_LOST é normal.** Troca de resolução, UAC, tela de bloqueio, jogo
   em tela cheia exclusiva. O capturador marca e recria na próxima chamada.
4. **Liberar rápido.** Segurar o frame atrasa o próximo. Copie a textura se
   precisar dela depois.

## Limitações conhecidas

- Captura monitor inteiro (não janela).
- Formato sempre BGRA 8 bits: telas HDR são convertidas pelo Windows para
  SDR. Suporte HDR viria com `IDXGIOutput5::DuplicateOutput1` (futuro).
- O cursor **não** vem desenhado na textura (Windows o trata à parte). Vamos
  desenhá-lo no cliente ou compor no host (fase 6).

## Como testar

Com o app [fluxo-bench](fluxo-bench.md).
