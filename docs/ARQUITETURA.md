# Arquitetura

Este documento descreve **como o Fluxo funciona por dentro** e por que ele é
organizado assim. Ao mudar algo estrutural, atualize este arquivo e registre a
decisão em [DECISOES.md](DECISOES.md).

## 1. Visão geral do pipeline

```text
HOST
 [Thread de captura]      [Thread de encode]        [Thread de rede]
  DesktopCapturer ──tex──► Encoder HW ──bytes──► Fragmenter ──► socket UDP ───┐
        ▲                                                       ▲ Ping/Pong    │
        │                                                       │ NACK/Keyframe│ internet
        └──────────── pedidos de keyframe / ajuste de bitrate ──┘   ou LAN    │
                                                                               │
CLIENTE                                                                        │
 [Thread de rede]          [Thread de decode]       [Thread de apresentação]   │
  socket UDP ──► Reassembler ──frame──► Decoder HW ──tex──► Swapchain D3D11 ◄──┘
```

Cada estágio roda na **sua própria thread**, ligado ao próximo por uma fila
curta (1–2 itens). Se um estágio atrasa, o anterior **descarta** frames em vez
de acumular: em streaming ao vivo, fila longa = latência alta.

**Hoje (fase 3):** no host, captura e encode dividem uma thread (o encode é
síncrono e leva ~2,5 ms); a rede tem duas threads próprias (envio com pacing
e pings). No `fluxo-client`, uma thread de rede remonta os frames e a thread
da janela decodifica e apresenta (decode leva ~0,3 ms de CPU com DXVA).

## 2. Crates e responsabilidades

| Crate | Faz | Não faz | Plataforma |
|---|---|---|---|
| `fluxo-proto` | Formato de pacote, fragmentação, remontagem, sequência | Nenhum I/O | Qualquer |
| `fluxo-capture` | Captura frames da tela como textura D3D11 | Encode, rede | Windows (stub nos outros) |
| `fluxo-encode` | Textura → H.264 via GPU (NVENC ou Media Foundation) | Captura | Windows (tipos em qualquer) |
| `fluxo-net` | Sockets UDP, pacing, RTT (NAT traversal na fase 5) | Formato dos bytes | Qualquer |
| `fluxo-decode` | H.264 → textura NV12 via GPU (Media Foundation + DXVA) | Rede, tela | Windows (erro em qualquer) |
| `apps/fluxo-host` | Captura → encode → rede | Decode, tela | Windows |
| `apps/fluxo-client` | Rede → decode → janela (swapchain flip model) | Captura | Windows |

**Regra de dependência:** crates de baixo nível nunca dependem de crates de
alto nível. `fluxo-proto` não depende de nada; `apps/*` dependem de tudo.

## 3. Por que a imagem nunca sai da GPU

Um frame 1080p BGRA tem 1920 × 1080 × 4 = **8,3 MB**. A 60 fps são
**~500 MB/s** só de cópia. Copiar para a RAM e de volta custa vários ms e
CPU. Por isso:

1. `DesktopCapturer` devolve `ID3D11Texture2D` (na GPU).
2. O encoder (NVENC direto, ou AMF/QuickSync/NVENC via Media Foundation)
   aceita textura D3D11 como entrada, **usando o mesmo `ID3D11Device`** da
   captura (`DesktopCapturer::d3d_device()`).
3. A GPU converte BGRA → NV12 (Video Processor do D3D11, BT.709 limitado) e
   comprime. Só ~40 KB por frame vão para a CPU.

`Frame::copy_to_cpu` existe apenas para debug e screenshots.

## 4. Orçamento de latência (meta, LAN)

| Etapa | Alvo | Observação |
|---|---|---|
| Captura | 1–2 ms | Desktop Duplication entrega assim que o compositor desenha |
| Encode | 3–6 ms | Preset de baixa latência, sem B-frames. Medido: 2,3 ms p50 / 2,8 ms p99 (NVENC, RTX 5050, 1080p) |
| Rede (LAN) | 1–3 ms | Internet: + metade do ping. Pacing: ~1–6 ms por frame de 20–40 KB |
| Remontagem | ~0 ms | Entrega ao chegar o último fragmento |
| Decode | 2–4 ms | Hardware (DXVA). CPU medida: ~0,3 ms; o resto corre na GPU |
| Apresentação | 0–16 ms | Depende de vsync; sem vsync + tearing ≈ 0 |
| **Total** | **~10–35 ms** | Parsec/Moonlight ficam nessa faixa |

**Medido até agora** (mesma máquina, RTX 5050, 1080p, 20 Mbps):

| Trecho | p50 | p99 |
|---|---|---|
| Captura → frame completo no receptor (vídeo de jogo a 60 fps) | 5,5 ms | 7,6 ms |
| Captura → `Present` no `fluxo-client` (uso normal da área de trabalho) | 3,1 ms | 9,3 ms |
| Captura → frame completo, **entre dois PCs no Wi-Fi** (relógios sincronizados pelos pings) | 5,1 ms | 29,9 ms |

Falta medir com cronômetro filmado (o `Present` ainda não é o instante em
que o monitor acende o pixel) e com o cliente com tela num segundo PC Windows.

## 5. Rede: por que UDP próprio (resumo do ADR-003)

TCP retransmite tudo e entrega em ordem — um pacote perdido trava todos os
seguintes (*head-of-line blocking*). Para vídeo ao vivo é melhor **pular** o
frame quebrado e seguir. Usamos UDP com nosso protocolo
([PROTOCOLO.md](PROTOCOLO.md)) e vamos adicionando, por fase:

1. ✅ Fragmentação e remontagem com descarte de frames velhos; envio com
   pacing e medição de RTT (`fluxo-net`).
2. ✅ Pedido de keyframe quando um frame se perde (o cliente congela a última
   imagem boa até ele chegar).
3. ✅ FEC (correção de erro): 1 pacote de paridade XOR a cada 10 reconstrói
   uma perda por grupo sem esperar retransmissão. Com 1% de perda: 98,7% dos
   frames inteiros, contra 68% sem FEC.
4. ✅ Bitrate adaptativo: o cliente relata perda e atraso de fila (pacote
   `Feedback`); o host baixa o bitrate rápido e sobe devagar (AIMD).
5. ⏳ Criptografia e travessia de NAT (STUN + *hole punching*, relay de
   fallback).

## 6. Tratamento de erros

- Cada crate tem seu enum de erro (`ProtoError`, `CaptureError`, `EncodeError`, `NetError`) com
  mensagens em português e `impl std::error::Error`.
- Erros **esperados** em operação normal não são falhas: tela parada
  (`Ok(None)`), acesso perdido na troca de resolução (`AccessLost`, recriado
  sozinho), pacote inválido da rede (descartado e contado em estatística).
- `panic!` só para bugs de programação (ex.: `max_payload` = 0).

## 7. Threads e `unsafe`

- Objetos COM/D3D11 ficam na thread que os criou. `DesktopCapturer` não é
  `Send` de propósito.
- Todo `unsafe` fica dentro de `fluxo-capture` e `fluxo-encode` (e
  futuramente decode), em blocos pequenos, com comentário explicando a
  invariante. `fluxo-proto` e `fluxo-net` não têm nenhum `unsafe`.
