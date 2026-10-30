# Changelog

Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/).
Versões seguem [SemVer](https://semver.org/lang/pt-BR/).

## [Não lançado]

### Adicionado
- `fluxo-encode`: encoder H.264 por hardware com trait `Encoder` e dois
  backends — NVENC direto (API 12.1, padrão em NVIDIA) e Media Foundation
  (NVIDIA, AMD, Intel). Escolha por `BackendChoice` (`auto`, `nvenc`, `mf`);
  `auto` cai para Media Foundation se o NVENC falhar e informa o motivo.
  Conversão BGRA → NV12 BT.709 na GPU, CBR com VBV de 1 frame, sem
  B-frames, GOP infinito com keyframes sob demanda. ADR-006.
- Layout das structs do NVENC gerado a partir do cabeçalho oficial
  (`nvenc/gen_layout.c`), em vez de transcrito à mão.
- `fluxo-bench`: comandos `encoders` e `encode` (opções `--encoder`,
  `--mbps`, `--fps`, `--keyframe-a-cada`, `--verificar`).

### Alterado
- `fluxo-capture`: device D3D11 criado com `D3D11_CREATE_DEVICE_VIDEO_SUPPORT`
  (com fallback sem a flag), exigido pela conversão NV12 e pelo Media Foundation.

### Corrigido
- Build: habilitada a feature `Win32_Graphics_Gdi` do crate `windows`,
  necessária para `IDXGIOutput::GetDesc`.

### Medido
- Captura 1080p na RTX 5050 com vídeo YouTube 60 fps (15 s): 68,9 fps médio,
  regime estável 62–63 fps, intervalo p50 16,6 ms / p99 22,3 ms, 1 frame
  perdido, 0 perdas de acesso. Fase 1 validada.
- Encode 1080p a 20 Mbps na RTX 5050: NVENC 2,3 ms p50 / 2,8 ms p99 por
  frame, keyframe de 39 KB; Media Foundation 2,0 / 3,4 ms, keyframe ~170 KB.
  Todos os frames decodificam no decoder do Windows; PSNR de luminância
  48,8 dB (RGB ~32 dB, limitado pelo 4:2:0 em texto colorido).

## [0.1.0] — 2026-10-29

### Adicionado
- Workspace Rust com crates `fluxo-proto`, `fluxo-capture` e app `fluxo-bench`.
- `fluxo-proto`: cabeçalho de 20 bytes (versão de protocolo 1), tipos Video,
  Ping e Pong; `Fragmenter`; `Reassembler` com descarte de frames atrasados,
  limite de memória e detecção de perda; comparação de sequência com
  wrap-around. 20 testes unitários + 3 doctests.
- `fluxo-proto/examples/loopback.rs`: teste de carga UDP local com perda simulada.
- `fluxo-capture`: captura via DXGI Desktop Duplication com device D3D11 na
  GPU do monitor, recuperação automática de `ACCESS_LOST`, cópia opcional
  para RAM, suporte a DPI alto. Stub para sistemas não-Windows.
- `fluxo-bench`: comandos `listar`, `bench` e `screenshot` (BMP).
- Documentação: README, arquitetura, protocolo, ADRs 001–005, roadmap,
  guia de compilação, convenções e documentos por módulo.

### Medido
- Loopback 1080p60 a 20 Mbps: 180/180 frames, ~2200 pacotes/s.
- Com 0,5% de perda de pacotes: ~18% de frames perdidos → motiva FEC (fase 4).
