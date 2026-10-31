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

**Status:** Substituído por ADR-006 (2026-10-29)

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

## ADR-005 — Payload máximo de 1200 bytes (1180 desde o protocolo v8)

**Status:** Aceito (2026-10-29)

**Contexto.** Pacote UDP maior que o MTU do caminho é fragmentado pelo IP; se
qualquer fragmento IP se perde, o pacote inteiro some, e alguns roteadores
descartam fragmentos.

**Decisão.** 1200 bytes de payload, abaixo do mínimo de 1280 do IPv6 com os
cabeçalhos. Mesma escolha do QUIC. **Atualização (protocolo v8):** 1180
bytes, porque o cabeçalho passou a 28 bytes e a cifra acrescenta 16 de tag:
1180 + 28 + 16 + 8 (UDP) + 40 (IPv6) = 1272.

**Consequências.**
- ➕ Funciona em praticamente qualquer rede, inclusive VPNs e túneis.
- ➖ ~2–4% a mais de pacotes que com 1400 bytes. Em LAN dá para subir via
  configuração no futuro (descoberta de MTU).

---

## ADR-006 — Encoder: NVENC direto + Media Foundation, escolhível

**Status:** Aceito (2026-10-29)

**Contexto.** O host principal tem NVIDIA (RTX 5050), mas o Fluxo precisa
rodar em PCs com AMD e Intel. O ADR-004 propunha FFmpeg para cobrir todos,
mas compilar FFmpeg com bindings Rust no Windows exige libclang, DLLs extras
e um build frágil.

**Decisão.** Crate `fluxo-encode` com uma trait `Encoder` e dois backends:

| Backend | GPUs | Uso |
|---|---|---|
| `nvenc` | NVIDIA | Padrão em NVIDIA. API C do driver (`nvEncodeAPI64.dll`), versão 12.1. |
| `mf` | NVIDIA, AMD, Intel | Media Foundation: MFT de hardware do fabricante (AMF, QuickSync, NVENC). |

Escolha por `--encoder auto|nvenc|mf` (futuramente também em arquivo de
configuração; a linha de comando vence). `auto` usa NVENC em placas NVIDIA e
cai para Media Foundation se ele falhar, informando o motivo.

**Consequências.**
- ➕ Zero dependências novas: o NVENC vem com o driver e o Media Foundation
  com o Windows; bindings via crate `windows` e FFI própria.
- ➕ Os dois backends rodam na RTX 5050 — dá para comparar lado a lado.
- ➕ NVENC direto expõe intra-refresh e controle fino de VBV (fase 4).
- ➖ Bindings do NVENC escritos à mão; o layout é conferido contra o
  cabeçalho oficial (`crates/fluxo-encode/nvenc/`) por um programa C.
- ➖ MFTs de hardware variam por fabricante e driver; AMD/Intel só são
  testados de verdade em máquinas com essas placas.
- 🔁 Se o Media Foundation for lento ou limitado em AMD/Intel, adicionar
  backends `amf`/`qsv` diretos atrás da mesma trait.

---

## ADR-007 — Decoder: Media Foundation com D3D11 (DXVA)

**Status:** Aceito (2026-10-30)

**Contexto.** O cliente precisa decodificar H.264 por hardware, sem tirar o
frame da GPU, em placas NVIDIA, AMD e Intel. Opções:

| Opção | Prós | Contras |
|---|---|---|
| `ID3D11VideoDecoder` direto | Controle total | Exige parser de H.264 próprio (slice headers, DPB): semanas de trabalho |
| FFmpeg `d3d11va` | Maduro | Mesma dor de build do ADR-004 |
| MFT H.264 da Microsoft + D3D11 | Vem com o Windows, usa DXVA de qualquer fabricante, entrega texturas NV12 | Menos controle sobre a fila interna |

**Decisão.** `fluxo-decode` usa o MFT H.264 da Microsoft com um
`IMFDXGIDeviceManager`: a decodificação roda no hardware da GPU (DXVA) e a
saída já é uma textura D3D11. Modo `MF_LOW_LATENCY` para 1 frame de saída por
frame de entrada. Se o driver não oferecer DXVA, o mesmo MFT decodifica por
software e o frame sobe para uma textura (mais lento, mas funciona).

A apresentação usa o Video Processor do D3D11 (NV12 → BGRA, com escala e
barras pretas) direto no back buffer de uma swapchain *flip model* com
*tearing* permitido quando não há vsync — sem shaders próprios.

**Consequências.**
- ➕ Zero dependências novas; funciona em qualquer GPU com driver de vídeo.
- ➕ Frame decodificado nunca passa pela CPU (no caminho de hardware).
- ➖ Fila interna do MFT é opaca; se ela segurar frames, medimos e reavaliamos.
- 🔁 Se a latência de decode passar de ~5 ms, avaliar `ID3D11VideoDecoder`
  direto com um parser mínimo (só os perfis que o nosso encoder gera).

---

## ADR-008 — Segurança: código de pareamento com SPAKE2 e ChaCha20-Poly1305

**Status:** Aceito (2026-10-31)

**Contexto.** Até a fase 4 o cliente aceitava o primeiro endereço que
mandasse vídeo, e tudo trafegava em claro. Para sair da rede local é preciso
saber **com quem** se fala e impedir que outros leiam ou injetem pacotes.
Opções:

| Opção | Prós | Contras |
|---|---|---|
| DTLS/QUIC prontos | Padrões | Certificados/PKI, handshake de vários RTTs, dependências grandes, controle fino de pacotes perdido |
| Troca de chaves (X25519) + confirmação visual do código | Simples | Quem está no meio, na hora do pareamento, engana quem não confere com atenção |
| **PAKE (SPAKE2) com código curto** + AEAD por pacote | Código de 8 caracteres basta; um palpite por tentativa, sem ataque offline | Precisa de uma biblioteca de PAKE |

**Decisão.** O cliente mostra um código de 8 caracteres (31 símbolos sem
ambíguos, ~40 bits); quem compartilha digita no host. SPAKE2 (Ed25519)
transforma o código numa chave de sessão em 1 RTT (PairInit/PairReply), com
confirmação explícita do cliente. HKDF-SHA256 deriva uma chave por sentido;
cada pacote vai cifrado com ChaCha20-Poly1305, `seq` do cabeçalho como nonce
e o cabeçalho como dado autenticado, com janela anti-replay de 64. Só crates
do RustCrypto (`spake2`, `chacha20poly1305`, `hkdf`, `sha2`, `getrandom`),
isoladas no `fluxo-crypto`; nenhum algoritmo escrito à mão.

**Consequências.**
- ➕ Só o cliente com o código recebe; intruso sem ele não lê, não injeta e
  não derruba a sessão (a sessão nova só vale quando um pacote decifra).
- ➕ Medido: no Wi-Fi, a 30 Mbps, latência p50 13,7 ms cifrado contra 13,2 ms
  em claro; um notebook antigo decifra sem esforço.
- ➖ 16 bytes de tag por pacote; payload máximo cai de 1200 para 1180.
- ➖ Cabeçalho visível (tamanhos e tempos de frame vazam), como em qualquer
  protocolo de vídeo cifrado por pacote.
- 🔁 Reavaliar ao chegar a sinalização pela internet: o código também vai
  servir para achar o outro lado no servidor, que não deve conseguir lê-lo.

---

## Modelo

```markdown
## ADR-XXX — Título curto

**Status:** Proposto | Aceito | Substituído por ADR-YYY (data)

**Contexto.** Qual problema, quais opções.

**Decisão.** O que foi escolhido.

**Consequências.** ➕ ganhos, ➖ custos, 🔁 quando reavaliar.
```
