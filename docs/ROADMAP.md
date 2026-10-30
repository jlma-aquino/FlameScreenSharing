# Roadmap

Construção incremental: cada fase termina com algo **executável e medível**.
Marque os itens conforme forem concluídos e registre no CHANGELOG.

## Fase 1 — Fundação ✅ (v0.1.0)

- [x] Workspace Rust, documentação e convenções
- [x] `fluxo-proto`: cabeçalho, fragmentação, remontagem, números de sequência
- [x] Teste de carga UDP local (`examples/loopback.rs`)
- [x] `fluxo-capture`: Desktop Duplication, recuperação de ACCESS_LOST, cópia para RAM
- [x] `fluxo-bench`: listar monitores, medir FPS da captura, screenshot BMP
- [ ] **Validar no seu PC:** compilar e rodar `fluxo-bench bench` com um vídeo/jogo a 60 fps

## Fase 2 — Encode e transmissão (próxima)

- [ ] Decidir backend de encoder (ADR-004) conforme a GPU
- [ ] `fluxo-encode`: trait `Encoder` + primeiro backend de hardware
  - entrada: textura D3D11 do capturador, mesmo device
  - saída: H.264 Annex B, preset baixa latência, sem B-frames, GOP longo
  - bitrate configurável (alvo inicial 20 Mbps em 1080p60)
- [ ] `fluxo-bench encode`: medir tempo de encode por frame e salvar `.h264` (abre no VLC/ffplay)
- [ ] `fluxo-net`: socket UDP, envio com *pacing* (espalhar pacotes no intervalo do frame), Ping/Pong
- [ ] `apps/fluxo-host`: captura → encode → rede, linha de comando

## Fase 3 — Cliente e primeira imagem na tela

- [ ] `fluxo-decode`: decoder de hardware (D3D11 Video / FFmpeg d3d11va)
- [ ] `apps/fluxo-client`: janela, swapchain, apresentação sem vsync opcional
- [ ] Overlay de estatísticas: fps, bitrate, perda, RTT, latência estimada
- [ ] Primeiro teste ponta a ponta em LAN, medindo latência real (cronômetro filmado)

## Fase 4 — Robustez de rede

- [ ] Pedido de keyframe ao detectar `preceded_by_loss`
- [ ] Intra-refresh no encoder (recuperação sem keyframes gigantes)
- [ ] FEC (paridade por grupo de pacotes)
- [ ] Controle adaptativo de bitrate (perda + RTT)
- [ ] Testes com perda/atraso simulados

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
- [ ] Interface gráfica
