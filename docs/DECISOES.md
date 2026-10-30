# Registro de decisões técnicas (ADRs)

Cada decisão importante fica registrada aqui com **contexto, decisão e
consequências**, para que daqui a meses dê para entender por que algo é do
jeito que é — e saber quando vale reabrir a discussão.

Para adicionar: copie o modelo no fim do arquivo, use o próximo número, e
nunca apague um ADR antigo (marque como "Substituído por ADR-XXX").

---

## ADR-001 — Linguagem: Rust

**Status:** Aceito (2026-10-29)

**Contexto.** O projeto é nativo, sensível a latência e lida com memória
compartilhada entre threads e APIs COM do Windows. Opções: C++, Rust, C#.

**Decisão.** Rust, com o crate oficial da Microsoft `windows` para as APIs
Win32/D3D11.

**Consequências.**
- ➕ Desempenho de C++ sem vazamentos de memória ou *data races* (o
  compilador barra).
- ➕ `cargo` resolve build, testes, documentação e dependências.
- ➕ Partes sem SO (protocolo) rodam e são testadas em qualquer máquina.
- ➖ Chamadas Win32 exigem `unsafe`; mantemos isso confinado em poucos módulos.
- ➖ Alguns SDKs (NVENC, AMF) só têm cabeçalhos C; precisaremos de bindings.

---

## ADR-002 — Captura: DXGI Desktop Duplication

**Status:** Aceito (2026-10-29)

**Contexto.** Windows oferece duas APIs modernas: *DXGI Desktop Duplication*
(Win 8+) e *Windows.Graphics.Capture* (WGC, Win 10 1903+).

**Decisão.** Desktop Duplication para captura de monitor inteiro.

**Consequências.**
- ➕ Latência mínima e menos camadas; é o que Parsec e Sunshine usam.
- ➕ Sem borda amarela de "captura em andamento" (WGC mostra em algumas versões).
- ➕ Informa quantos frames foram perdidos (`AccumulatedFrames`).
- ➖ Captura monitor inteiro, não janela isolada. Se precisarmos capturar uma
  janela, adicionamos um backend WGC atrás da mesma API.
- ➖ Precisa recriar a duplicação em troca de resolução/UAC (já tratado).
- ➖ Em notebooks com duas GPUs, o processo deve rodar na GPU ligada à tela.

---

## ADR-003 — Transporte: protocolo próprio sobre UDP

**Status:** Aceito (2026-10-29)

**Contexto.** Opções: WebRTC (pronto, com NAT/criptografia/congestionamento),
QUIC, ou UDP com protocolo próprio.

**Decisão.** UDP com protocolo próprio (`fluxo-proto`), adicionando recursos
por fase.

**Consequências.**
- ➕ Controle total sobre latência: sem *jitter buffer* genérico, sem
  retransmissões que atrasam frames.
- ➕ Pequeno, entendível e testável sem rede.
- ➖ Temos que implementar FEC, controle de bitrate, criptografia e NAT
  traversal nós mesmos (planejados no ROADMAP).
- 🔁 Reavaliar se o esforço de NAT traversal ficar grande: dá para usar só a
  parte ICE de uma biblioteca WebRTC mantendo nosso protocolo de vídeo.

---

## ADR-004 — Encoder: hardware, começando por FFmpeg

**Status:** Proposto (decidir no início da fase 2)

**Contexto.** Encode por CPU (x264) em 1080p60 custa caro e adiciona latência.
As GPUs têm encoders dedicados: NVENC (NVIDIA), AMF (AMD), QuickSync (Intel).
Usar cada SDK direto dá o melhor resultado, mas são três integrações.

**Proposta.** Começar com FFmpeg (`h264_nvenc`, `h264_amf`, `h264_qsv`)
recebendo textura D3D11 via `hwcontext_d3d11`, atrás de uma *trait*
`Encoder`. Depois, se medirmos ganho, trocar o backend da GPU do usuário por
uma integração direta com o SDK.

**A decidir.** Qual GPU o host principal usa — define qual backend vem
primeiro e é testado mais a fundo.

---

## ADR-005 — Payload máximo de 1200 bytes

**Status:** Aceito (2026-10-29)

**Contexto.** Pacote UDP maior que o MTU do caminho é fragmentado pelo IP; se
qualquer fragmento IP se perde, o pacote inteiro some, e alguns roteadores
descartam fragmentos.

**Decisão.** 1200 bytes de payload (1248–1268 com cabeçalhos), abaixo do
mínimo de 1280 do IPv6. Mesma escolha do QUIC.

**Consequências.**
- ➕ Funciona em praticamente qualquer rede, inclusive VPNs e túneis.
- ➖ ~2–4% a mais de pacotes que com 1400 bytes. Em LAN dá para subir via
  configuração no futuro (descoberta de MTU).

---

## Modelo

```markdown
## ADR-XXX — Título curto

**Status:** Proposto | Aceito | Substituído por ADR-YYY (data)

**Contexto.** Qual problema, quais opções.

**Decisão.** O que foi escolhido.

**Consequências.** ➕ ganhos, ➖ custos, 🔁 quando reavaliar.
```
