# fluxo-client

**Caminho:** `apps/fluxo-client` · **Plataforma:** Windows 10+

Lado de quem assiste: rede (`fluxo-net`) → decode (`fluxo-decode`) → tela.

## Uso

```text
fluxo-client [--porta P] [--vsync] [--segundos S] [--largura L] [--altura A] [--no-topo]
```

`--no-topo` mantém a janela sempre por cima das outras: útil para acompanhar
enquanto usa o PC, e necessário quando o cliente é aberto por outro programa
em segundo plano (o Windows não deixa essa janela tomar o foco).

```powershell
# No PC que assiste
.\target\release\fluxo-client.exe --porta 9000
# No PC que compartilha
.\target\release\fluxo-host.exe --destino <IP do PC que assiste>:9000
```

Na janela: **F11** ou **Alt+Enter** alterna tela cheia sem bordas; **Esc**
sai da tela cheia. Teste na mesma máquina (`--destino 127.0.0.1:9000`)
funciona, mas a janela aparece dentro da própria imagem ("espelho infinito").

## Arquivos

| Arquivo | Conteúdo |
|---|---|
| `src/main.rs` | Opções, laço principal, estatísticas |
| `src/window.rs` | Janela Win32: redimensionar, tela cheia, fechar |
| `src/present.rs` | Swapchain e Video Processor (NV12 → tela, escala, barras pretas) |

## Laço principal

```text
pump() das mensagens da janela
recv_frame(2 ms) ──► decodifica TODOS os frames que chegaram
                     (cada P-frame depende do anterior)
                 ──► mostra só o mais novo (se atrasou, alcança)
1×/s: estatísticas no título da janela
```

- Antes do primeiro keyframe, frames são pulados (o host manda um a cada 2 s).
- Se o decode falhar (bitstream estragado), o decoder é recriado e espera o
  próximo keyframe.

## Apresentação de baixa latência

- Swapchain **flip model** (`FLIP_DISCARD`), 2 buffers, no máximo 1 frame na
  fila do driver.
- Sem `--vsync`: `Present` com **tearing permitido** quando o sistema
  suporta — o frame aparece na hora. Com `--vsync`: sem rasgos na imagem, até
  um quadro do monitor a mais de atraso.
- Conversão NV12 (BT.709 limitado) → RGB pelo Video Processor, que também
  faz escala e barras pretas mantendo a proporção.

## Lendo os números

Título da janela, a cada segundo:

| Campo | Significado | Bom (mesma máquina) |
|---|---|---|
| fps | Frames mostrados | ~60 com vídeo |
| Mbps | Bitrate recebido | ≤ `--mbps` do host |
| captura→tela | Da captura no host ao `Present` aqui (relógios iguais) | < 10 ms |
| RTT | Ida e volta | < 1 ms em LAN cabeada |
| perdidos | Frames que não chegaram inteiros | 0 |

Ao fechar, o terminal mostra o resumo (percentis de captura→tela, decode,
perdas, erros).

## Medições (mesma máquina, RTX 5050, 1080p, 20 Mbps)

| | Resultado |
|---|---|
| Frames (30 s, 60 fps) | 1801 recebidos, 1799 mostrados, 0 perdidos, 0 erros de decode |
| Captura → tela (60 fps) | p50 7,4 ms, p99 8,0 ms |
| Frames (12 s, ~170 fps, janela do cliente na tela capturada) | 2035 recebidos e mostrados, 0 descartados |
| Captura → tela (~170 fps) | p50 2,6 ms, p99 7,2 ms |
| Decode (CPU) | ~0,2 ms médio |

Com a janela do cliente dentro da tela capturada, cada frame mostrado muda a
tela e gera nova captura: o host passa a codificar na frequência do monitor
(até ~165 fps), não nos 60 do `--fps`. Ver a pendência de limite de fps no
roadmap.

## Limitações conhecidas

- **Estatísticas no título**, não sobre a imagem (overlay desenhado fica
  para depois).
- **Arrastar/redimensionar a janela** pausa o laço (o Windows segura a thread
  durante o arraste); os frames esperam na fila do `fluxo-net` (até 120) e o
  cliente alcança depois. Arrastes mais longos que ~2 s descartam frames.
- A janela não toma o foco se for aberta por outro programa em segundo
  plano: clique nela na barra de tarefas ou use `--no-topo`.
- Sem áudio, mouse ou teclado remotos (fase 6).
