# fluxo-encode

**Caminho:** `crates/fluxo-encode` · **Dependências:** `fluxo-proto` (só o `VideoCodec`), `windows` (só no Windows) · **Plataforma:** Windows 10+

Comprime em H.264, HEVC ou AV1 (`EncoderConfig::codec`), por hardware, as texturas do `fluxo-capture`, sem o frame
sair da GPU. Dois backends atrás da trait `Encoder`; por que esse desenho:
ADR-006 em [DECISOES.md](../DECISOES.md).

## Backends e escolha

| `--encoder` | Backend | GPUs | Observação |
|---|---|---|---|
| `nvenc` | NVENC direto (API 12.1) | NVIDIA, driver 531+ | Padrão em NVIDIA |
| `mf` | Media Foundation (MFT de hardware) | NVIDIA, AMD, Intel | Usa o encoder que o driver de vídeo instala |
| `auto` (padrão) | NVENC → Media Foundation em NVIDIA; Media Foundation nas outras | — | Se o NVENC falhar, cai para MF e informa o motivo |

`open_encoder` devolve, além do encoder, a lista `skipped` com os backends
que falharam e por quê — mostre ao usuário.

## Codecs

| Codec | NVENC | Media Foundation | Bitstream |
|---|---|---|---|
| H.264 (padrão) | perfil High, toda NVIDIA | MFT H.264 do driver | Annex B, SPS/PPS em cada keyframe |
| HEVC | perfil Main, NVIDIA Maxwell 2ª geração+ | MFT HEVC do driver | Annex B, VPS/SPS/PPS em cada keyframe |
| AV1 | perfil Main, NVIDIA RTX 40+ | MFT AV1 do driver (AMD RX 7000+, Intel Arc) | OBUs, cabeçalho de sequência em cada keyframe |

Os ajustes de baixa latência (abaixo) são os mesmos nos três; no NVENC só
muda onde cada campo fica na config (a parte do codec é uma union): tabela
`campos` em `nvenc/mod.rs`. A GPU sem o codec pedido devolve
`BackendUnavailable` ("esta GPU não codifica AV1"); quem escolhe o codec da
sessão é o host (ADR-012).

## Arquivos

| Arquivo | Conteúdo |
|---|---|
| `src/lib.rs` | Tipos puros e testáveis: `Backend`, `BackendChoice`, `EncoderConfig`, `EncodedFrame`, `EncodeError`, `backend_order` |
| `src/platform.rs` | Trait `Encoder`, `open_encoder`, `probe_backend`, `gpu_vendor` |
| `src/convert.rs` | BGRA → NV12 na GPU (Video Processor do D3D11), comum aos dois backends |
| `src/nvenc/mod.rs` | `NvencEncoder` |
| `src/nvenc/api.rs` | Carga da DLL, tabela de funções, structs como blocos de bytes (`Blob`) |
| `src/nvenc/layout.rs` | **Gerado**: tamanhos, offsets, versões e GUIDs do cabeçalho oficial |
| `src/mf.rs` | `MfEncoder` |
| `nvenc/nvEncodeAPI.h` | Cabeçalho oficial do NVENC 12.1 (licença MIT, via FFmpeg/nv-codec-headers) |
| `nvenc/gen_layout.c` + `gerar_layout.ps1` | Gerador do `layout.rs` (compila com o MSVC) |

## API

| Item | Para quê |
|---|---|
| `EncoderConfig::new(w, h)` | 60 fps, 20 Mbps, `auto`; ajuste os campos depois |
| `open_encoder(device, &cfg)` | Abre seguindo `cfg.backend`; `device` = `cap.d3d_device()` |
| `probe_backend(device, &cfg, b)` | Abre exatamente o backend `b` (diagnóstico/comparação) |
| `enc.encode(textura, forcar_keyframe)` | Devolve `Vec<EncodedFrame>` — normalmente 1 |
| `enc.set_bitrate(kbps)` | Muda o bitrate a partir do próximo frame, sem keyframe (NVENC: `nvEncReconfigureEncoder`; MF: `ICodecAPI`) |
| `EncodedFrame` | `data` (H.264 Annex B), `keyframe`, `index`, `refresh_start`/`refresh_end` (onda de intra-refresh) |
| `enc.start_intra_refresh()` | Pede uma onda de intra-refresh de `EncoderConfig::intra_refresh_frames` frames no próximo encode; `false` se o backend não tem (só NVENC tem; a GPU precisa de `NV_ENC_CAPS_SUPPORT_INTRA_REFRESH`) |

## Configuração de baixa latência

Os dois backends seguem o mesmo princípio; diferenças na tabela.

| Item | NVENC | Media Foundation |
|---|---|---|
| Preset | P4 + tuning *ultra low latency* | `CODECAPI_AVLowLatencyMode` |
| B-frames | nenhum | nenhum |
| Keyframes | só quando pedidos (GOP infinito) | pedidos + 1 por minuto (sem GOP infinito padronizado) |
| Bitrate | CBR, VBV de 1 frame | CBR; VBV de 1 frame é pedido, mas pode ser ignorado |
| Cor | BT.709 limitado, sinalizado no VUI | BT.709 limitado no tipo de mídia |

## Por que o layout do NVENC é gerado

As structs do NVENC têm milhares de bytes, unions, campos reservados e
bitfields. Em vez de transcrevê-las para Rust (onde um erro vira crash ou
vídeo corrompido), `gen_layout.c` inclui o cabeçalho oficial, e o compilador
C imprime cada offset usado. O Rust lê e escreve as structs como blocos de
bytes por esses offsets. Para trocar de versão do NVENC: substitua o `.h`,
rode `crates\fluxo-encode\nvenc\gerar_layout.ps1` e faça commit do
`layout.rs`.

## Armadilhas encontradas

1. **Tabela de funções por encoder.** Depois que o MFT H.264 da própria
   NVIDIA roda no processo, uma tabela NVENC criada antes passa a recusar
   sessões novas com `NV_ENC_ERR_INVALID_VERSION`. Cada `NvencEncoder` pede
   a sua com `NvEncodeAPICreateInstance`.
2. **Device com `VIDEO_SUPPORT`.** A conversão NV12 e o Media Foundation
   precisam de `D3D11_CREATE_DEVICE_VIDEO_SUPPORT`; o `fluxo-capture` cria o
   device com essa flag (e tenta sem ela se o driver recusar).
3. **Media Foundation é assíncrono.** O `encode` espera os eventos
   `NeedInput`/`HaveOutput`; se o MFT pedir mais entrada antes de soltar a
   saída, o frame sai na chamada seguinte em vez de travar.

## Medições (RTX 5050, 1080p, 20 Mbps, área de trabalho)

| | NVENC | Media Foundation (MFT NVIDIA) |
|---|---|---|
| Encode por frame (p50 / p99) | 2,3 / 2,8 ms | 2,0 / 3,4 ms |
| Keyframe | 39 KB, até 5 ms | **~170 KB** (ignora o VBV), até 4,5 ms |
| Frames decodificados (decoder do Windows) | todos | todos |
| PSNR de luminância | 48,8 dB | 48,8 dB |

O PSNR RGB fica em ~32 dB: a perda vem da cor em meia resolução (4:2:0) em
texto e ícones coloridos, não do encoder. Melhorar isso exigiria 4:4:4
(pulado: o decoder do Windows não decodifica).

HEVC e AV1 (mesma máquina, tela em movimento, 3 s cada; o bitrate depende
do que estava na tela, não compare entre linhas):

| | Encode P (p50 / p99) | Keyframe | Decode (`fluxo-bench decode`) |
|---|---|---|---|
| NVENC HEVC | 2,6 / 3,6 ms | 37 KB, 5,4 ms | — (sem decoder HEVC aqui) |
| NVENC AV1 | 2,2 / 2,4 ms | 33 KB, 5,3 ms | 158 de 159, CPU p50 0,13 ms |
| MF HEVC (MFT NVIDIA) | 2,3 / 2,6 ms | — | — |
| MF AV1 (MFT NVIDIA) | 1,6 / 2,0 ms | — | 91 de 91, CPU p50 0,16 ms |

## Intra-refresh (opcional)

Com `intra_refresh_frames` > 0, o NVENC liga `enableIntraRefresh` (período
automático tão longo que não acontece) e cada `start_intra_refresh` vira
`forceIntraRefreshWithFrameCnt` no frame seguinte. Também limita o encoder a
**1 quadro de referência**: com mais, um P-frame depois da onda podia apontar
para um quadro de antes dela, ainda com o estrago da perda (medido com
`fluxo-bench encode --verificar --perder-a-cada`). Com 1 referência, 48 de
48 recuperações ficaram idênticas a um decoder que não perdeu nada.

## Limitações conhecidas

- Só 4:2:0, 8 bits. 4:4:4 foi pulado (o decoder do Windows não decodifica).
- Resolução fixa: se a textura mudar de tamanho, `encode` devolve
  `SizeMismatch` e o encoder deve ser recriado.
- Sem encoder por software: máquinas sem encoder de hardware (VMs, GPUs muito
  antigas) recebem `NoBackend`.
- AMD e Intel só foram testados pelo caminho comum (Media Foundation numa
  NVIDIA); falta rodar em máquinas com essas placas.

## Como testar

```powershell
.\target\release\fluxo-bench.exe encoders
.\target\release\fluxo-bench.exe encode --segundos 10 --verificar
```

Detalhes em [fluxo-bench](fluxo-bench.md).
