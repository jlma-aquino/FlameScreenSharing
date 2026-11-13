# fluxo-decode

**Caminho:** `crates/fluxo-decode` · **Dependências:** `fluxo-proto` (só o `VideoCodec`), `windows` (só no Windows) · **Plataforma:** Windows 10+

Decodifica H.264, HEVC ou AV1 em texturas NV12 na GPU com os decoders do
Media Foundation (MFT) acelerados por DXVA. H.264: o da própria Microsoft,
que vem em todo Windows. HEVC e AV1: o MFT síncrono instalado, das
extensões da Microsoft Store ("HEVC Video Extensions", "AV1 Video
Extension"); os MFTs de hardware assíncronos não servem para o laço de
decode e ficam de fora. Por que esse caminho: ADR-007 em [DECISOES.md](../DECISOES.md).

## Arquivos

| Arquivo | Conteúdo |
|---|---|
| `src/lib.rs` | `DecodeError` (compila em qualquer sistema) |
| `src/mf.rs` | `create_device`, `VideoDecoder`, `DecodedFrame`, `decodable_codecs`, `available_decoders` |

## API

| Item | Para quê |
|---|---|
| `create_device()` | Device D3D11 na GPU padrão com suporte a vídeo e proteção multithread |
| `VideoDecoder::new(&device)` | Decoder H.264; o tamanho do vídeo vem do primeiro keyframe |
| `VideoDecoder::with_codec(&device, codec)` | Decoder de H.264, HEVC ou AV1 (erro se o Windows não tiver) |
| `decodable_codecs()` | Máscara dos codecs que dá para decodificar **na placa** aqui: o que o cliente anuncia ao host. HEVC e AV1 só com o decoder instalado e o perfil de decode na GPU (`codecs_da_placa`); sem o perfil, a extensão da Store decodificaria pelo processador (lento, imagem piscando: visto numa AMD sem AV1) |
| `codecs_da_placa()` | Codecs com perfil de decode D3D11 (NV12) na placa padrão |
| `available_decoders()` | Todos os MFTs de cada codec, de hardware e de software (`fluxo-bench encoders`) |
| `dec.decode(&bytes)` | Um frame completo → `Some(DecodedFrame)` ou `None` (antes do 1º keyframe) |
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

- HEVC sem decoder neste PC (a extensão da Store não está instalada): só o
  encode foi testado. AV1: 158 de 159 frames no `fluxo-bench decode`
  (o último ainda estava no decoder), decode p50 0,13 ms.
- Bitstream corrompido por perda de pacotes pode gerar artefatos até o
  próximo keyframe; o pedido de keyframe e o FEC vêm na fase 4.

## Como testar

```powershell
.\target\release\fluxo-bench.exe decode --segundos 10
```

Ponta a ponta: [fluxo-client](fluxo-client.md).
