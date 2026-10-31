# Changelog

Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/).
Versões seguem [SemVer](https://semver.org/lang/pt-BR/).

## [Não lançado]

### Adicionado
- **`fluxo-bench enviar`**: frames sintéticos a bitrate fixo para o
  `receber`, sem captura nem encode — mede a rede com carga controlada.
- `ReassemblerStats::fec_groups_failed_2` / `fec_groups_failed_3plus` /
  `frames_abandoned_without_parity`: nos frames perdidos, quantos pacotes
  faltaram em cada grupo de FEC (perda espalhada ou em rajada).
- **Painel de estatísticas sobre a imagem** no `fluxo-client` (Direct2D +
  DirectWrite): fps, Mbps, captura→tela, RTT, fila, perdidos e FEC; F2
  mostra/esconde, `--sem-painel` começa escondido. Custa ~0,13 ms de CPU por
  frame. O resumo ganhou "espera no cliente" e "apresentação (CPU)".
- **Protocolo v7 — intra-refresh opcional** (`fluxo-host --refresh N`,
  NVENC): depois de uma perda, o host pode redesenhar a imagem em faixas ao
  longo de N frames em vez de mandar keyframe. Flags de início/fim da onda no
  vídeo; o KeyframeRequest diz se só keyframe serve (largada, erro de
  decode). `Encoder::start_intra_refresh`, `EncoderConfig::intra_refresh_frames`,
  `VideoReceiver::request_refresh`, `VideoSender::take_refresh_request` e
  `send_frame_with_flags`. Desligado por padrão (ver Medido).
- **`fluxo_net::Recovery`:** a lógica de largada/perda/recuperação do
  cliente, pura e testada, usada pelo `fluxo-client` e pelo `fluxo-bench
  receber`.
- **`fluxo-bench encode --perder-a-cada S`** (com `--verificar`): descarta
  frames, pede a recuperação e compara com um decoder que recebeu tudo.
- **Protocolo v6 — hora de saída dos pacotes:** cabeçalho de 24 bytes com
  `send_us`, gravado na hora do envio (`fluxo_proto::stamp_send_time`). O
  atraso de fila do bitrate adaptativo passa a medir só a rede.
- **Limite de fila configurável** (`fluxo-host --fila-max MS`,
  `BitrateController::with_queue_limit`), padrão 15 ms (antes 25).
- **Atraso e jitter simulados** (`fluxo-host --simular-atraso MS
  --simular-jitter MS`, `SenderConfig::simulated_delay`/`simulated_jitter`):
  atraso fixo mais um extra sorteado a cada 20 ms, nos dois sentidos, sem
  os pacotes se ultrapassarem (`fluxo_net::sim`).
- **Protocolo v5 — sincronização de relógio:** o Pong leva 4 bytes com o
  relógio de quem responde; `fluxo_net::sync` estima a diferença entre os
  relógios estilo NTP (menor RTT das últimas 16 medições, erro ±meio RTT).
  `ReceivedFrame::latency_us` traz a latência real captura → chegada entre
  PCs; `ReceiverStats`/`SenderStats::clock_offset_ms`. O `fluxo-client` e o
  `fluxo-bench receber` mostram a latência sincronizada e a diferença dos
  relógios.
- **Protocolo v4 — bitrate adaptativo:** pacote `Feedback` (`kind = 6`), que
  o cliente manda a cada 250 ms com os pacotes recebidos (acumulado) e o
  atraso de fila (trânsito de cada frame menos o menor dos últimos 10 s).
  O host calcula a perda do intervalo e o `BitrateController` (AIMD: cai
  ×0,85–0,7 com perda ≥ 3% ou fila ≥ 25 ms, sobe +8%/s com a rede folgada)
  ajusta o encoder em tempo real (`Encoder::set_bitrate`: NVENC via
  `nvEncReconfigureEncoder`, MF via `ICodecAPI`) e o pacing. `fluxo-host`:
  `--mbps` vira teto, novos `--mbps-min`, `--bitrate-fixo` e
  `--simular-banda M` (link simulado com fila e descarte, para testes).
  `fluxo-bench encoders` testa a mudança de bitrate em cada backend.
- **Protocolo v3 — FEC:** pacote `Parity` (`kind = 5`) com o XOR de um grupo
  de fragmentos; o byte 13 do cabeçalho vira `fec_group`. Novo módulo
  `fluxo_proto::fec`; `Fragmenter::with_fec_group` intercala a paridade
  depois de cada grupo; o `Reassembler` reconstrói um fragmento quando o
  grupo tem exatamente um faltando (`ReassemblerStats::fec_recovered`).
  Ligado por padrão com grupos de 10 (`SenderConfig::fec_group`,
  `fluxo-host --fec K`; exemplo `loopback --fec K`).
- **Protocolo v2:** pacote `KeyframeRequest` (`kind = 4`), cliente → host.
  Pacotes da versão 1 passam a ser recusados.
- Pedido de keyframe de ponta a ponta: `VideoReceiver::request_keyframe`
  (limitado a um a cada 50 ms ou 2 RTTs) e `VideoSender::take_keyframe_request`.
  O `fluxo-client` pede ao começar, ao detectar perda e quando o decode
  falha, e **congela a última imagem boa** em vez de mostrar imagem
  corrompida; o `fluxo-host` força keyframe no próximo encode e, com a tela
  parada, recodifica a última imagem enviada. `fluxo-bench receber` também
  pede e não grava frames estragados.
- Perda simulada para testes: `SenderConfig::simulated_loss` e
  `fluxo-host --simular-perda P`.
- Largada aquecida: o host manda um ping antes do primeiro frame e espera
  até 50 ms pela resposta, para não perder a primeira rajada enquanto o
  sistema resolve o endereço do cliente.
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
- `fluxo-net`: `VideoSender` (fila + thread de envio com pacing por token
  bucket, 2,5× o bitrate, rajadas de 10 pacotes) e `VideoReceiver` (thread de
  recepção com remontagem e fila de frames). Ping/Pong nos dois sentidos com
  RTT suavizado; relógio `clock_us` para medir o trânsito captura → chegada.
  Só std, sem `unsafe`, testado com sockets reais em 127.0.0.1.
- `apps/fluxo-host`: captura → encode → rede, com keyframe periódico,
  recriação do encoder ao mudar a resolução e estatísticas por segundo.
- `fluxo-bench receber`: receptor provisório até a fase 3 (grava o `.h264`
  a partir do primeiro keyframe; `--verificar` decodifica tudo).
- `fluxo-decode`: decoder H.264 com o MFT da Microsoft acelerado por DXVA,
  saída em textura NV12 na GPU, modo de baixa latência; reserva por
  software quando o driver não oferece DXVA. ADR-007.
- `apps/fluxo-client`: janela Win32 (tela cheia com F11/Alt+Enter),
  swapchain flip model com tearing sem vsync (`--vsync` opcional),
  NV12 → tela pelo Video Processor com barras pretas; decodifica todos os
  frames recebidos e mostra o mais novo; estatísticas no título e resumo
  ao fechar; `--no-topo` mantém a janela por cima das outras.
- `fluxo-bench decode`: captura → encode → decode, mede o decoder isolado.

### Alterado
- `fluxo-host`: keyframe periódico padrão passou de 2 s para 10 s (só rede
  de segurança, agora que o cliente pede quando precisa); com a tela parada,
  a espera por captura caiu de 100 para 10 ms para atender pedidos rápido.
- `fluxo-bench receber` compila e roda em Linux/macOS (só depende do
  `fluxo-net`), para testar a rede com um segundo PC que não seja Windows;
  `--verificar` continua só no Windows. Workspace inteiro sem avisos no
  `cargo check --target x86_64-unknown-linux-gnu`.
- `fluxo-capture`: device D3D11 criado com `D3D11_CREATE_DEVICE_VIDEO_SUPPORT`
  (com fallback sem a flag), exigido pela conversão NV12 e pelo Media Foundation.
- `fluxo-net`: fila de frames prontos do receptor passou de 16 para 120;
  com 16, um travamento de ~0,3 s do cliente a ~160 fps descartou 242
  frames (cada descarte estraga a imagem até o próximo keyframe).
- `fluxo-host`: `--fps` agora é um limite de verdade. Capturas acima do
  limite (monitores de 144/165 Hz) viram pendentes — cópia na GPU — e saem
  no próximo horário de uma grade de `1/fps`, sem perder a imagem final de
  uma animação. Medido com o cliente na tela capturada: de 162 fps / ~60 Mbps
  para 59,8 fps / 15 Mbps.

### Corrigido
- `fluxo-client`: arrastar ou redimensionar a janela parava a imagem (o
  Windows prende a thread da janela até soltar o mouse; frames esperavam até
  5,6 s). Decode e apresentação agora rodam numa thread própria (`exibir.rs`):
  maior espera de 22 ms com a janela presa 3 s.
- `fluxo-client`: aberto por outro programa, a janela podia nascer
  minimizada (o Windows aplica o modo de quem lançou); agora sempre abre
  normal.
- `fluxo-host --fec K` aparecia na ajuda, mas a linha de comando o recusava.
- Bitrate adaptativo no Wi-Fi: a "fila" incluía encode, espera do limite de
  fps e pacing, e o bitrate caía sem congestionamento (com limite de 10 ms,
  7,5 Mbps num link de 30+). Agora mede desde a saída de cada pacote.
- Largada do bitrate adaptativo em até 20 Mbps (`START_KBPS`): começar em
  `--mbps 60` deixava ~2 s com 100–200 ms de atraso (p99 no Wi-Fi: ~200 →
  70 ms).
- Com a tela parada o alvo do bitrate não sobe mais (só sobe se o envio
  real usar metade dele): quando o movimento voltava, a rajada enchia a fila.
- Bitrate adaptativo: "perda fantasma" no Wi-Fi. O host contava os pacotes
  de um frame como enviados no início do pacing, tratava pacotes ainda na
  fila do Wi-Fi como perdidos e usava intervalos de 250 ms; o resultado era
  3–60% de perda calculada com zero pacotes perdidos, e o bitrate caía de 20
  para 2,5 Mbps. Agora cada pacote conta ao sair, a comparação desconta 1
  RTT, a janela é de 1 s e só vale perda confirmada pelo cliente (FEC ou
  frame perdido).
- Build: habilitada a feature `Win32_Graphics_Gdi` do crate `windows`,
  necessária para `IDXGIOutput::GetDesc`.

### Medido
- Perdas no Wi-Fi (Windows → notebook Linux, carga sintética, 20 s por
  carga): nenhum pacote perdido de 10 a 40 Mbps; acima da capacidade
  (~45 Mbps), perdas em rajada — a 60 Mbps, 3.348 grupos de FEC falharam
  com 3+ pacotes faltando contra 309 com 2. Reed-Solomon descartado. Sem
  perda, picos de atraso de até ~870 ms (a 10 Mbps) aparecem mesmo assim.
- Intra-refresh vs. keyframe (mesma máquina, NVENC): as ondas deixam a
  imagem idêntica a um decoder sem perda (48 de 48), mas com 3% de perda de
  pacotes sem FEC a imagem ficou congelada em média 231 ms (ondas de 4)
  contra 31 ms (keyframe), e 10 contra 32 fps mostrados. Os keyframes do
  NVENC têm ~38 KB contra ~31 KB de um P-frame (VBV de 1 frame).
  Keyframe continua o padrão. No Wi-Fi (até o notebook Linux, 2% de perda
  sem FEC) o protocolo v7 funcionou de ponta a ponta: keyframe limpou a
  imagem em média em 22 ms, ondas de 4 em 152 ms.
- Latência de ida real entre dois PCs (host Windows → notebook Linux no
  Wi-Fi, vídeo, 30 s, relógios sincronizados pelos pings a +837,4 ms ±
  0,5 ms): p50 5,1 ms, p99 29,9 ms, máx 85 ms; 1803 frames, 0 perdidos.
- Wi-Fi real (host Windows → notebook Linux no Wi-Fi, vídeo a 60 fps, 30 s
  por rodada): a 20 Mbps com FEC e adaptativo, 0 frames perdidos e o
  bitrate fica em 20 Mbps; pedindo 60 Mbps com bitrate fixo, 447 frames
  perdidos e RTT de 48 ms; pedindo 60 Mbps com adaptativo, estabiliza em
  ~35 Mbps (fila 11–13 ms) com 0 frames perdidos.
- Bitrate adaptativo num link simulado de 8 Mbps (host querendo 20, cliente
  na mesma máquina, 15 s): frames perdidos de 255 (bitrate fixo) para 44,
  mostrados de 470/593 para 762/873, captura → tela p99 de 223 para 95 ms;
  desceu de 20 para ~6 Mbps em 2 s e ficou sem perda, com fila de 5–10 ms.
- FEC (grupos de 10), exemplo `loopback` com 300 frames a 20 Mbps: com 1%
  de perda, 98,7% dos frames inteiros (sem FEC: 68%); com 3%, 85% (sem:
  35%; grupos de 5: 91%). No pipeline real com 1% de perda (cliente, 15 s):
  frames perdidos de 236 para 8, mostrados de 461/708 para 803/813, tempo
  congelado de ~28% para ~0,6% (8 perdas de 12 ms em média).
- Pedido de keyframe com 1% de perda simulada (cliente na mesma máquina,
  15 s): 110 perdas detectadas, 159 pedidos, 158 atendidos, 0 erros de
  decode; imagem congelada em média 38 ms (máx 184 ms) por perda. Com 2% e
  a tela parada: 16 pedidos, 16 atendidos recodificando a última imagem.
- Captura 1080p na RTX 5050 com vídeo YouTube 60 fps (15 s): 68,9 fps médio,
  regime estável 62–63 fps, intervalo p50 16,6 ms / p99 22,3 ms, 1 frame
  perdido, 0 perdas de acesso. Fase 1 validada.
- Encode 1080p a 20 Mbps na RTX 5050: NVENC 2,3 ms p50 / 2,8 ms p99 por
  frame, keyframe de 39 KB; Media Foundation 2,0 / 3,4 ms, keyframe ~170 KB.
  Todos os frames decodificam no decoder do Windows; PSNR de luminância
  48,8 dB (RGB ~32 dB, limitado pelo 4:2:0 em texto colorido).
- Transmissão na mesma máquina (fluxo-host → fluxo-bench receber, NVENC
  20 Mbps): 488 de 488 frames recebidos e decodificados, 0 perdidos;
  captura → frame completo no receptor p50 2,9 ms, p99 7,1 ms.
- Mesma transmissão com vídeo de jogo a 60 fps (16 s): 60 fps estáveis,
  13,3 Mbps médio, 900 de 900 frames recebidos e decodificados, 0 perdidos;
  trânsito p50 5,5 ms, p99 7,6 ms, máx 9,5 ms; maior envio de um frame
  6,5 ms; encode p50 2,4 ms / p99 3,4 ms; PSNR RGB 34,9 dB médio.
- Decode DXVA na RTX 5050: 255 de 255 frames, nenhum retido; CPU p50
  0,17 ms / p99 0,34 ms por frame.
- Pipeline completo na mesma máquina (fluxo-host → fluxo-client, 12 s):
  464 frames recebidos e 464 mostrados, 0 perdidos, 0 erros de decode;
  captura → `Present` p50 3,1 ms, p99 9,3 ms.
- Cliente 30 s a 60 fps: 1801 recebidos, 1799 mostrados, captura → tela
  p50 7,4 ms / p99 8,0 ms. Com a janela do cliente na tela capturada
  (~170 fps): 2035 de 2035 mostrados, 0 descartados, p50 2,6 / p99 7,2 ms.
- Entre dois PCs (host Windows → notebook Linux no Wi-Fi, 30 s): 40 085
  pacotes, 0 duplicados/atrasados/inválidos, 1 frame perdido na largada e
  0 durante o vídeo; RTT 1,0 ms mínimo / 2,9 ms suavizado, subindo até
  4,6 ms sob carga; variação do trânsito p99 +17 ms, pico +56 ms.

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
