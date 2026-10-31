# Roadmap

Construção incremental: cada fase termina com algo **executável e medível**.
Marque os itens conforme forem concluídos e registre no CHANGELOG.

## Fase 1 — Fundação ✅ (v0.1.0)

- [x] Workspace Rust, documentação e convenções
- [x] `fluxo-proto`: cabeçalho, fragmentação, remontagem, números de sequência
- [x] Teste de carga UDP local (`examples/loopback.rs`)
- [x] `fluxo-capture`: Desktop Duplication, recuperação de ACCESS_LOST, cópia para RAM
- [x] `fluxo-bench`: listar monitores, medir FPS da captura, screenshot BMP
- [x] **Validar no seu PC:** compilar e rodar `fluxo-bench bench` com um vídeo/jogo a 60 fps

## Fase 2 — Encode e transmissão (em andamento)

- [x] Decidir backend de encoder → ADR-006: NVENC direto + Media Foundation, `--encoder auto|nvenc|mf`
- [x] `fluxo-encode`: trait `Encoder` + backends NVENC e Media Foundation
  - entrada: textura D3D11 do capturador, mesmo device
  - saída: H.264 Annex B, preset baixa latência, sem B-frames, GOP infinito
  - bitrate configurável (alvo inicial 20 Mbps em 1080p60)
- [x] `fluxo-bench encode`: medir tempo de encode por frame e salvar `.h264` (abre no VLC/ffplay)
- [x] `fluxo-bench encode --verificar`: decodificar com o decoder do Windows e medir PSNR
- [ ] Validar `--encoder mf` num PC com GPU AMD e num com Intel
- [ ] Media Foundation: keyframes ignoram o VBV de 1 frame no MFT da NVIDIA (~170 KB);
      investigar como limitar o tamanho em cada fabricante
- [x] `fluxo-net`: socket UDP, envio com *pacing* (token bucket, 2,5× o bitrate), Ping/Pong nos dois sentidos
- [x] `apps/fluxo-host`: captura → encode → rede, linha de comando
- [x] `fluxo-bench receber`: receptor provisório (mede trânsito, perdas, RTT; grava e verifica o `.h264`)
- [x] Teste ponta a ponta com vídeo a 60 fps na mesma máquina (900/900 frames, trânsito p99 7,6 ms)
- [x] Teste entre dois PCs, Wi-Fi: host Windows → notebook Linux no Wi-Fi, 30 s: 0 frames perdidos
      durante o vídeo (1 na largada), RTT 1–4,6 ms, variação do trânsito p99 +17 ms, pico +56 ms
- [ ] Teste entre dois PCs, ambos no cabo
- [ ] Conferir a imagem do `.h264` recebido no notebook (`ffplay recebido.h264`)
- [x] Limitar o host ao `--fps` em monitores de 144/165 Hz sem perder a imagem final de uma animação
      (antes: 162 fps e ~60 Mbps com o cliente na tela capturada; depois: 59,8 fps, 15 Mbps)

## Fase 3 — Cliente e primeira imagem na tela (em andamento)

- [x] `fluxo-decode`: decoder de hardware → ADR-007: MFT H.264 da Microsoft + D3D11 (DXVA), reserva por software
- [x] `apps/fluxo-client`: janela, swapchain flip model, tearing sem vsync, `--vsync` opcional, tela cheia (F11)
- [x] `fluxo-bench decode`: mede o decoder isolado
- [x] Estatísticas no título da janela: fps, bitrate, captura→tela, RTT, perdas
- [x] Teste ponta a ponta na mesma máquina (464/464 frames, captura→tela p99 9,3 ms)
- [x] Conferência visual da imagem na janela do cliente (`--no-topo` para a janela ficar por cima)
- [x] Fila do receptor de 120 frames: um travamento curto do cliente não descarta frames (antes: 242 descartes a ~160 fps)
- [ ] Overlay de estatísticas desenhado sobre a imagem (Direct2D)
- [ ] Primeiro teste ponta a ponta em LAN, medindo latência real (cronômetro filmado)
- [ ] Não travar a recepção enquanto a janela é arrastada (decode/apresentação em thread própria)

## Fase 4 — Robustez de rede (em andamento)

- [x] Pedido de keyframe (protocolo v2, `KeyframeRequest`): ao detectar `preceded_by_loss`, falha de
      decode ou falta de imagem; o cliente congela a última imagem boa em vez de mostrar imagem
      corrompida; com a tela parada o host recodifica a última imagem. Medido com 1% de perda:
      congelamento médio de 38 ms por perda (antes: imagem borrada até o keyframe periódico)
- [x] Intra-refresh no encoder (protocolo v7, `--refresh N`, NVENC): ondas marcadas no pacote, cliente
      congela até o fim de uma onda que começou depois da perda; imagem idêntica a um decoder sem perda
      (48/48). **Desligado por padrão:** os keyframes do NVENC já são pequenos (VBV de 1 frame) e a onda
      recupera mais devagar (3% de perda sem FEC: 231 ms congelado em média com ondas de 4 vs 31 ms)
- [x] FEC por paridade XOR (protocolo v3, `--fec K`, padrão 10 = +10% de pacotes). Com 1% de perda no
      pipeline real: frames perdidos de 236 para 8, tempo congelado de ~28% para ~0,6%
- [ ] FEC Reed-Solomon (recupera 2+ perdas por grupo), se perdas em rajada aparecerem no Wi-Fi
- [x] Bitrate adaptativo (protocolo v4, `Feedback`): o cliente relata perda e atraso de fila a cada 250 ms;
      o host cai ×0,85–0,7 e sobe +8%/s. Link simulado de 8 Mbps: perdidos 255 → 44, p99 captura→tela
      223 → 95 ms, regime estável sem perda e com fila de 5–10 ms
- [x] Bitrate adaptativo no Wi-Fi real (Windows → notebook): corrigida a "perda fantasma" (contagem por
      pacote, desconto de 1 RTT, janela de 1 s, perda só confirmada pelo cliente). A 20 Mbps fica em
      20 Mbps sem perda; pedindo 60 Mbps, estabiliza em ~35 Mbps pela fila real, sem perder frames
      (bitrate fixo em 60 Mbps: 447 frames perdidos e RTT de 48 ms)
- [x] Afinar o limiar de fila: a fila agora é medida pela hora de saída de cada pacote (protocolo v6,
      `send_us`) — antes, encode/limite de fps/pacing pareciam fila. Limite padrão 15 ms (`--fila-max`),
      largada em até 20 Mbps (p99 no Wi-Fi de ~200 para 70 ms) e o alvo não sobe com a tela parada
- [x] Banda limitada simulada (`fluxo-host --simular-banda M`)
- [x] Perda simulada (`fluxo-host --simular-perda P`)
- [x] Atraso/jitter simulados (`--simular-atraso MS`, `--simular-jitter MS`, nos dois sentidos)
- [x] Não perder o 1º keyframe: o host manda um ping antes do primeiro frame e espera até 50 ms pela
      resposta (causa provável: o Windows descarta parte da primeira rajada para um IP novo enquanto
      resolve o ARP — visto no teste Windows → Linux); falta confirmar entre dois PCs
- [x] Sincronizar relógios pelo Ping/Pong (protocolo v5, estilo NTP): erro ±0,5 ms; latência de ida real
      no Wi-Fi (Windows → notebook, vídeo): p50 5,1 ms, p99 29,9 ms

## Fase 5 — Internet e segurança

- [ ] Handshake e código de pareamento
- [ ] Criptografia ChaCha20-Poly1305
- [ ] Servidor de sinalização mínimo (troca de endereços)
- [ ] STUN + UDP hole punching
- [ ] Relay de fallback para CGNAT

## Fase 6 — Interação e conforto

- [ ] Mouse e teclado remotos (opcional, com permissão do host)
- [ ] Áudio (WASAPI loopback + Opus)
- [ ] Cursor desenhado no cliente (latência menor de ponteiro)
- [ ] HEVC/AV1, 1440p/4K, 120 fps
- [ ] H.264 4:4:4 (NVENC) para texto colorido nítido — o 4:2:0 limita o PSNR RGB a ~32 dB em área de trabalho
- [ ] Encoder por software (fallback para VMs/PCs sem encoder de hardware)
- [ ] Interface gráfica
