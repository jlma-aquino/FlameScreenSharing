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

## 2. Crates e responsabilidades

| Crate | Faz | Não faz | Plataforma |
|---|---|---|---|
| `fluxo-proto` | Formato de pacote, fragmentação, remontagem, sequência | Nenhum I/O | Qualquer |
| `fluxo-capture` | Captura frames da tela como textura D3D11 | Encode, rede | Windows (stub nos outros) |
| `fluxo-encode` *(fase 2)* | Textura → H.264/HEVC/AV1 via GPU | Captura | Windows |
| `fluxo-net` *(fase 2)* | Sockets UDP, pacing, RTT, NAT traversal | Formato dos bytes | Qualquer |
| `fluxo-decode` *(fase 3)* | Bytes → textura via GPU | Rede | Windows |
| `apps/fluxo-host`, `apps/fluxo-client` *(fase 3)* | Juntam tudo | — | Windows |

**Regra de dependência:** crates de baixo nível nunca dependem de crates de
alto nível. `fluxo-proto` não depende de nada; `apps/*` dependem de tudo.

## 3. Por que a imagem nunca sai da GPU

Um frame 1080p BGRA tem 1920 × 1080 × 4 = **8,3 MB**. A 60 fps são
**~500 MB/s** só de cópia. Copiar para a RAM e de volta custa vários ms e
CPU. Por isso:

1. `DesktopCapturer` devolve `ID3D11Texture2D` (na GPU).
2. O encoder (NVENC/AMF/QuickSync) aceita textura D3D11 como entrada,
   **usando o mesmo `ID3D11Device`** da captura (`DesktopCapturer::d3d_device()`).
3. A GPU converte BGRA → NV12 e comprime. Só ~40 KB por frame vão para a CPU.

`Frame::copy_to_cpu` existe apenas para debug e screenshots.

## 4. Orçamento de latência (meta, LAN)

| Etapa | Alvo | Observação |
|---|---|---|
| Captura | 1–2 ms | Desktop Duplication entrega assim que o compositor desenha |
| Encode | 3–6 ms | Preset de baixa latência, sem B-frames |
| Rede (LAN) | 1–3 ms | Internet: + metade do ping |
| Remontagem | ~0 ms | Entrega ao chegar o último fragmento |
| Decode | 2–4 ms | Hardware |
| Apresentação | 0–16 ms | Depende de vsync; modo sem vsync reduz |
| **Total** | **~10–35 ms** | Parsec/Moonlight ficam nessa faixa |

## 5. Rede: por que UDP próprio (resumo do ADR-003)

TCP retransmite tudo e entrega em ordem — um pacote perdido trava todos os
seguintes (*head-of-line blocking*). Para vídeo ao vivo é melhor **pular** o
frame quebrado e seguir. Usamos UDP com nosso protocolo
([PROTOCOLO.md](PROTOCOLO.md)) e vamos adicionando, por fase:

1. ✅ Fragmentação e remontagem com descarte de frames velhos.
2. ⏳ Pedido de keyframe quando um frame se perde.
3. ⏳ FEC (correção de erro): pacotes extras que reconstroem perdas sem
   esperar retransmissão. O teste de loopback mostra a necessidade: 0,5% de
   perda de pacotes vira ~18% de frames perdidos sem FEC.
4. ⏳ Controle de bitrate pela taxa de perda e RTT.
5. ⏳ Criptografia e travessia de NAT (STUN + *hole punching*, relay de
   fallback).

## 6. Tratamento de erros

- Cada crate tem seu enum de erro (`ProtoError`, `CaptureError`) com
  mensagens em português e `impl std::error::Error`.
- Erros **esperados** em operação normal não são falhas: tela parada
  (`Ok(None)`), acesso perdido na troca de resolução (`AccessLost`, recriado
  sozinho), pacote inválido da rede (descartado e contado em estatística).
- `panic!` só para bugs de programação (ex.: `max_payload` = 0).

## 7. Threads e `unsafe`

- Objetos COM/D3D11 ficam na thread que os criou. `DesktopCapturer` não é
  `Send` de propósito.
- Todo `unsafe` fica dentro de `fluxo-capture` (e futuramente encode/decode),
  em blocos pequenos, com comentário explicando a invariante.
  `fluxo-proto` não tem nenhum `unsafe`.
