# fluxo-decode

**Caminho:** `crates/fluxo-decode` · **Dependências:** `windows` (só no Windows) · **Plataforma:** Windows 10+

Decodifica H.264 em texturas NV12 na GPU com o decoder da própria Microsoft
(MFT) acelerado por DXVA — qualquer GPU com driver de vídeo. Por que esse
caminho: ADR-007 em [DECISOES.md](../DECISOES.md).

## Arquivos

| Arquivo | Conteúdo |
|---|---|
| `src/lib.rs` | `DecodeError` (compila em qualquer sistema) |
| `src/mf.rs` | `create_device`, `VideoDecoder`, `DecodedFrame` |

## API

| Item | Para quê |
|---|---|
| `create_device()` | Device D3D11 na GPU padrão com suporte a vídeo e proteção multithread |
| `VideoDecoder::new(&device)` | Decoder; o tamanho do vídeo vem do primeiro keyframe |
| `dec.decode(&bytes)` | Um frame H.264 completo → `Some(DecodedFrame)` ou `None` (antes do 1º keyframe) |
| `dec.is_hardware()` / `dec.name()` | Se o DXVA está ativo |
| `frame.texture()` + `frame.subresource()` | NV12 na GPU (fatia de um array de texturas do decoder) |
| `frame.width()` / `frame.height()` | Tamanho visível (1080, mesmo com superfície de 1088) |

## Caminhos

1. **Hardware (DXVA):** o MFT recebe nosso device por um
   `IMFDXGIDeviceManager` e devolve fatias de um array de texturas NV12. O
   `DecodedFrame` segura a amostra: enquanto existir, o decoder não reutiliza
   a fatia — solte-o logo depois de desenhar.
2. **Software (reserva):** se o driver recusar o device, o mesmo MFT
   decodifica na CPU e o NV12 sobe para uma textura nossa
   (`UpdateSubresource`). Mais lento, mas a imagem aparece.

`MF_LOW_LATENCY` faz o MFT devolver cada frame imediatamente (sem B-frames
não há o que reordenar).

## Medições (RTX 5050, 1080p)

| | Resultado |
|---|---|
| Frames decodificados | 255 de 255 (nenhum retido) |
| Tempo de CPU por frame | p50 0,17 ms, p99 0,34 ms (a GPU termina o trabalho em paralelo) |

## Limitações conhecidas

- Só H.264 (o que o `fluxo-encode` gera).
- Bitstream corrompido por perda de pacotes pode gerar artefatos até o
  próximo keyframe; o pedido de keyframe e o FEC vêm na fase 4.

## Como testar

```powershell
.\target\release\fluxo-bench.exe decode --segundos 10
```

Ponta a ponta: [fluxo-client](fluxo-client.md).
