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
  **Feito no ADR-009:** o servidor vê só 20 bits derivados do código.

---

## ADR-009 — Internet: sinalização própria por UDP e furo de NAT

**Status:** Aceito (2026-10-31)

**Contexto.** Em redes diferentes, cada lado está atrás de um NAT: não sabe
o próprio endereço público, e o NAT descarta o que chega sem ter saído algo
antes para aquele destino. É preciso um ponto de encontro público. Opções:

| Opção | Prós | Contras |
|---|---|---|
| ICE/WebRTC (biblioteca) | Completo (STUN, TURN, candidatos) | Dependência grande, negociação lenta (SDP), outro socket e outro formato fora do nosso controle |
| STUN público (Google) + sinalização à parte | Sem servidor nosso para descobrir o endereço | Ainda precisa de um servidor para trocar endereços; depende de terceiros |
| **Servidor próprio por UDP, no mesmo socket do vídeo** | Um servidor só faz as duas coisas; o endereço visto é o do socket certo; mensagens minúsculas | Hospedar e manter o servidor (uma VPS barata basta) |

**Decisão.** `fluxo-sinal`: o cliente registra a **sala** do código, o host
procura a sala, e o servidor conta a cada um o endereço público do outro
(de onde o pedido chegou: o papel do STUN) e os locais que cada um
informou. Depois o cliente manda um Punch e o host o PairInit a cada
endereço do outro, abrindo os dois NATs; o primeiro PairReply válido define
o caminho. O servidor nunca vê vídeo nem o código: a sala são 20 bits do
SHA-256 do código, e o SPAKE2 limita quem controla o servidor a um palpite
por pareamento (1 em ~800 mil). Pedidos são completados até 100 bytes e as
respostas são menores, para o servidor não amplificar ataques.

**Consequências.**
- ➕ Sem dependências novas; o mesmo protocolo e o mesmo socket do vídeo.
- ➕ Na mesma rede, o endereço local informado faz o pareamento direto.
- ➕ Medido com o servidor no notebook: pareamento e vídeo normais nos dois
  sentidos, tela real a 20 Mbps com p50 18,1 ms (sem NAT entre os PCs).
- ➖ Alguém precisa hospedar o servidor num endereço público (porta UDP
  7000 aberta).
- ➖ NAT simétrico dos dois lados não fura: precisa de relay (próximo item
  da fase 5). Ainda não testado atrás de NATs de verdade (precisa de duas
  redes e o servidor na internet).
- 🔁 Se muitos usuários ficarem sem conexão direta, avaliar o relay ou só
  a parte ICE de uma biblioteca (ADR-003).

---

## ADR-010 — Conectar sem servidor próprio: código de conexão + STUN público

**Status:** Aceito (2026-10-31)

**Contexto.** O projeto não deve depender de alguém alugar e manter um
servidor (ADR-009 exige um). Apps como o TeamSpeak 6 parecem não ter
servidor, mas usam a infraestrutura da empresa para apresentar os dois
lados; VPNs como Radmin, ZeroTier e Tailscale também (e exigem instalar e
criar conta: no teste, o instalador do ZeroTier quebrou o `apt` do notebook).
Opções sem servidor nosso:

| Opção | Prós | Contras |
|---|---|---|
| **Código de conexão pelo chat + STUN público** | Nada para manter; STUN é gratuito, sem conta, não vê nada além do pedido | Dois copia-e-cola (ida e volta) |
| UPnP no roteador | Um código só | Depende do roteador; rede de celular quase nunca tem |
| DHT do BitTorrent | "Adicionar amigo" sem copia-e-cola, sem empresa | Muito mais complexo, bibliotecas novas |

**Decisão.** O cliente pergunta a servidores STUN públicos o seu endereço
público e mostra um **código de conexão** (pareamento + endereço, 19
símbolos com conferência); o host digita e mostra um **código de resposta**
(o endereço dele, 11 símbolos); o cliente digita esse. Os dois furam os
NATs como no ADR-009. O `fluxo-sinal` continua opcional, para quem quiser
hospedar um e pular os códigos.

**Consequências.**
- ➕ Zero infraestrutura nossa e zero dependências novas (STUN escrito em
  ~100 linhas, testado com o exemplo da RFC 5769).
- ➕ O chat de vocês é o canal de sinalização; o servidor STUN não vê o
  código nem o vídeo.
- ➖ Duas mensagens para conectar (UPnP e IPv6 podem tirar uma depois).
- ➖ NAT simétrico dos dois lados não conecta: sem servidor não há relay.
  Avisamos quando o STUN detecta NAT simétrico.
- 🔁 Medido no celular: o IPv4 precisou de TTL baixo até o furo (ver
  `modulos/fluxo-net.md`); se o copia-e-cola incomodar, avaliar a DHT.
- **Atualização (IPv6 e UPnP):** os códigos levam também o endereço IPv6
  (48 símbolos em vez de 19) e uma marca de porta aberta por UPnP. No teste
  com o notebook no celular, o IPv6 pareou na hora, **sem código de
  resposta** (o celular deixa o IPv6 entrar), e levou a tela real a 60 fps
  e 9 Mbps sem perda. O UPnP só abre porta quando o roteador tem o IP que o
  STUN viu; nas duas redes de teste há CGNAT ou não há UPnP, então ele
  ficou só detectado.

---

## ADR-011 — Áudio: Opus pela libopus traduzida para Rust

**Status:** Aceito (2026-10-31)

**Contexto.** O som do host precisa chegar com pouco atraso e pouca banda.
Opções de codec:

| Opção | Prós | Contras |
|---|---|---|
| PCM sem compressão | Zero dependências, menor atraso | ~1,5 Mbps a mais (pesa na internet do celular) |
| AAC do Windows (Media Foundation) | Sem download | ~40–60 ms a mais de atraso; ruim para tempo real |
| **Opus** | Feito para tempo real: quadros de 10 ms, ~128 kbps, FEC e ocultação de perda | Precisa da libopus |

Para a libopus: os bindings comuns (`opus`, `opus-head-sys`) compilam o
código C com CMake, que aqui só existe dentro do Visual Studio (fora do
PATH) e no notebook nem existe. O `unsafe-libopus` é a mesma libopus de
referência traduzida automaticamente para Rust (c2rust): compila em qualquer
lugar sem ferramentas extras.

**Decisão.** Opus 48 kHz estéreo, quadros de 10 ms, 128 kbps, FEC para ~5%
de perda, via `unsafe-libopus`, isolado em `fluxo-audio/src/opus.rs` (o
único lugar com `unsafe`). Um quadro por pacote (tipo Audio, protocolo v11),
fora da fila do vídeo. No cliente, buffer de jitter adaptativo (30 ms no
começo) e a fila da placa em 20 ms.

**Consequências.**
- ➕ Sem CMake nem compilador C; igual no Windows e no Linux.
- ➕ Medido: 129 kbps, encode ~0,3 ms por quadro, 0 perdas no Wi-Fi.
- ➖ Código traduzido é `unsafe` e um pouco mais lento que o C original (sem
  importância a 100 quadros por segundo).
- ➖ O som sai ~40–50 ms atrás da imagem (jitter + fila da placa).
- 🔁 Se o atraso incomodar, reduzir o alvo inicial do jitter ou atrasar a
  imagem para casar com o som.

---

## ADR-012 — Codec por sessão: o melhor que os dois lados suportam

**Status:** Aceito (2026-10-31)

**Contexto.** HEVC e AV1 dão a mesma imagem com menos bits que o H.264
(em geral 30–50% a menos; não medido aqui), o que importa pela internet do celular.
Mas nenhum dos dois é garantido: codificar depende da GPU do host (AV1 só da
NVIDIA RTX 40 / AMD RX 7000 / Intel Arc em diante), e decodificar depende de
uma extensão da Microsoft Store no cliente (aqui: AV1 instalado, HEVC não).
Opções:

| Opção | Prós | Contras |
|---|---|---|
| Só H.264 | Funciona em todo par de PCs | Desperdiça banda quando os dois têm algo melhor |
| Escolha manual nos dois lados | Simples | Usuário precisa saber o que cada PC tem; errar = tela preta |
| **O cliente anuncia, o host escolhe** | Automático, cai para H.264 sozinho | Um pacote novo e uma espera curta antes do primeiro frame |

**Decisão.** O cliente manda a máscara do que decodifica (pacote Caps,
protocolo v12) junto com cada Ping. Depois do pareamento o host espera até
1,5 s pelo primeiro Caps e tenta abrir o encoder na ordem AV1, HEVC, H.264,
pulando o que o cliente não decodifica e o que a GPU recusar. Cada frame leva
o codec em `flags`, e o cliente troca de decoder quando ele muda.
`fluxo-host --codec` força um codec (erro se o cliente não decodifica).

**Consequências.**
- ➕ Sem configuração: AV1 entre esta máquina e ela mesma; H.264 com o
  notebook (Linux, o `receber` anuncia só H.264).
- ➕ O decoder novo serve para qualquer MFT síncrono do Windows (HEVC e AV1
  da Store); os de hardware assíncronos ficam de fora.
- ➖ O encoder só abre depois do pareamento (antes abria na largada): um erro
  de GPU aparece mais tarde.
- ➖ HEVC não testado de ponta a ponta (este PC não tem o decoder; o encode
  funciona).
- 🔁 Se aparecer diferença de latência entre os codecs no cliente real,
  pesar latência e não só compressão na ordem de preferência.

---

## ADR-013 — Encoder por software: OpenH264 compilado do fonte

**Status:** Aceito (2026-10-31)

**Contexto.** Sem encoder de hardware (VM, GPU antiga) o host não abria.
Opções para comprimir na CPU:

| Opção | Prós | Contras |
|---|---|---|
| x264 | Melhor compressão por bit | GPL; build pesado |
| **OpenH264 (Cisco)**, do fonte (crate `openh264`) | BSD; feito para tempo real; compila com o MSVC já instalado | Sem NASM, só C (mais lento) |
| OpenH264, DLL pronta da Cisco | Assembly e a licença do H.264 paga pela Cisco | Baixar ou distribuir a DLL junto |

**Decisão.** OpenH264 do fonte, só no Windows (o `receber` no Linux não
compila C++ à toa). A conversão de cor continua na GPU; o frame desce em
NV12 e vira I420. Modo "câmera em tempo real" e até 8 fatias em paralelo,
medidos: 36 → 7,5 ms por frame 1080p, mesmo PSNR. No host, padrões de 30
fps e até 1080 linhas. É o último backend do automático.

**Consequências.**
- ➕ Funciona em qualquer PC com Windows, sem dependência de driver de vídeo
  para o encode.
- ➖ Rust mínimo sobe para 1.85 (pedido da crate).
- ➖ Só H.264 Baseline, e sem intra-refresh: perda se recupera com keyframe.
- ➖ Sem NASM no PC que compila, fica só em C.
- 🔁 Se precisar de mais velocidade em VMs fracas: NASM no build ou a DLL da
  Cisco (`openh264` com a feature `libloading`).

---

## ADR-014 — Vários espectadores: um encoder e um VideoSender por pessoa

**Status:** Aceito (2026-11-01)

**Contexto.** Mais de uma pessoa quer assistir, sem servidor. Abrir um host por
espectador repete a captura e o encode; um servidor no meio exige alugar um.

**Decisão.** Um encoder só e um `VideoSender` por espectador
(`apps/fluxo-host/src/espectadores.rs`), cada um com sockets, pareamento, cifra,
pacing e medição próprios. O protocolo não muda: o cliente nem sabe que há
outros. Limite de 4. O que é único segue o pior espectador: o bitrate do encoder
é o menor dos controladores, o codec é um que todos decodificam (se quem entra
não decodifica o atual, o encoder troca) e um keyframe pedido vai para todos.
Mais gente entra pela entrada padrão do host, que é por onde a janela manda os
códigos. Quem fica 15 s calado sai da lista.

**Consequências.**
- ➕ Na GPU, quase o custo de um espectador; um espectador lento não atrasa o
  envio dos outros.
- ➕ Clientes de versões anteriores continuam servindo como espectadores.
- ➖ O upload é multiplicado pelo número de espectadores.
- ➖ Um espectador com rede ruim baixa a qualidade de todos, e cada perda dele
  gera recuperação para todos.
- 🔁 Se isso incomodar: dois encoders (qualidades diferentes), ou tirar do
  cálculo quem estiver muito abaixo dos outros.

---

## ADR-015 — Assistir no Linux: OpenH264, OpenGL e tocador por pipe

**Status:** Aceito (2026-11-01)

**Contexto.** O cliente depende de Media Foundation, Direct3D 11 e WASAPI. Levar
também o lado de compartilhar (captura no Wayland, encoders) é bem mais trabalho,
e só assistir já permite que alguém no Linux participe.

**Decisão.** Um cliente portátil em `apps/fluxo-client/src/portatil/`, atrás da
opção de compilação `portatil`, com a mesma rede, os mesmos códigos e a mesma saída
no terminal. Decode com OpenH264 na CPU (só H.264); imagem numa janela egui, com
os planos I420 em texturas e a conversão de cor num shader; som por pipe para o
primeiro tocador que funcionar (`pacat`, `pw-cat`, `aplay`). Sem biblioteca nova
para o som, e o protocolo não muda.

**Consequências.**
- ➕ O cliente do Windows não engorda, e o `fluxo-bench` no Linux segue sem C++.
- ➕ Compila no Windows (`--portatil`), então quase tudo se testa lá também.
- ➖ CPU fraca não acompanha 1080p: no notebook de teste (2 núcleos), 26–45 ms por
  frame com NASM, a ~6–16 Mbps. O custo segue o bitrate. Para a rede estar bem e o
  vídeo atrasar sem o host saber, o cliente mede a carga do decode e informa no
  campo de atraso de fila do relatório, sem mudar o protocolo: acima de 95% o bitrate
  cai, de 75% a 95% fica parado. Acima de 300 ms atrasado, descarta a fila e pede um
  keyframe. Medido: 1080p60 de ~1,5 s para 22,5 ms (p50), mas a 2,5 Mbps, o piso.
- ➖ Um espectador no Linux leva todos os outros da transmissão para H.264.
- ➖ O OpenH264 não esconde erros: depois de uma perda não recuperada pelo FEC, a
  imagem espera um keyframe.
- ➖ Um espectador em PC fraco baixa o bitrate de todos.
- 🔁 Decode na GPU (VAAPI), para não precisar baixar tanto a qualidade.

## ADR-016 — Sala: quem compartilha cria o código, quem assiste entra

**Status:** Aceito (2026-11-12)

**Contexto.** Trocar código de conexão e código de resposta pelo chat funciona,
mas tira a praticidade, e cada espectador a mais pedia outra troca. A ideia é a
de uma reunião: quem compartilha abre e passa um código; quem tiver o código
entra quando quiser. Testamos a DHT pública como ponto de encontro sem servidor
(branch `fase7/sala-dht`): acha o outro lado, mas devolve endereços falsos, demorou
mais de 2 min em casa e não achou nada pelo 4G. Já havia um servidor na Oracle
Cloud (`fluxo-sinal`, ver `docs/SERVIDOR.md`).

**Decisão.** Usar o `fluxo-sinal` com as mesmas mensagens, invertendo os papéis:
o host sorteia o código (`--sala`) e fica pedindo ao servidor quem entrou; cada
cliente se registra na sala do código (`--sala CÓDIGO`). O servidor guarda até 8
clientes por sala e entrega cada um a um socket do host; o host pareia cada um
com o SPAKE2 de sempre, num socket só dele, até 4 assistindo. O servidor padrão
fica no código (`DEFAULT_SIGNAL_SERVER`); `--sinal` troca. A janela passa a
oferecer a sala como primeira opção; os códigos de conexão e a mesma rede
continuam lá, como reserva.

**Consequências.**
- ➕ Uma troca só, de um código curto, de quem compartilha para todos.
- ➕ Sem mudar o protocolo: o `fluxo-sinal` novo atende os clientes antigos, e o
  fluxo antigo (`--sinal` + `--codigo` do cliente) continua funcionando.
- ➖ Depende do servidor: fora do ar, só os códigos de conexão funcionam.
- ➖ Quem tiver o código entra, sem aprovação. O código vale enquanto o host
  roda; para expulsar alguém, é preciso fechar e abrir outra sala.
- ➖ Quem controla o servidor vê a sala (20 bits) e pode pôr um cliente falso nela:
  cada tentativa do host é um palpite (1 em ~1 milhão), e uma sala aberta dá ao
  host várias tentativas por hora. Aceito porque o servidor é nosso.
- ➖ O servidor não melhora o furo de NAT: pelo 4G, o primeiro teste (sem TTL
  baixo) achou os dois lados e o vídeo não passou. Com TTL baixo, falta testar.
- 🔁 Relay pelo servidor, para as redes em que o furo falha; aprovação de quem
  entra, se o código vazar.

## ADR-017 — Janela como a do Meet: botões ao vivo pela entrada padrão e tema em arquivo

**Status:** Aceito (2026-11-12)

**Contexto.** A janela era um formulário: as opções só valiam ao abrir, e mudar
tela, som ou qualidade pedia encerrar e começar de novo, derrubando quem
assistia. O pedido foi uma interface como a do Meet ou do Discord, com botões
embaixo durante a transmissão, configurações padrão numa janela própria, modo
claro e escuro, e espaço para personalização. Dava para levar o vídeo para
dentro da janela egui, mas o caminho de vídeo do cliente (Direct3D, sem vsync) é
o que garante os ~22 ms.

**Decisão.**
- A janela continua rodando o `fluxo-host` e o `fluxo-client` por baixo. Os
  botões mandam linhas pela entrada padrão (a mesma que já levava os códigos):
  `tela`, `som`, `pausar`/`continuar`, `resolucao`, `fps` e `volume`. Cada
  programa muda na hora e confirma com uma linha, e só então a janela mostra o
  estado novo.
- Trocar de tela reabre a captura e o encoder; o encoder novo começa com
  keyframe, e o protocolo não muda. O som desligado manda silêncio, para ligar
  de novo sem reabrir nada.
- As cores ficam todas num `Tema` com papéis nomeados e duas paletas; um `Tema`
  cabe num texto (`escuro.destaque = #F26B1D`), lido de `tema.txt`.
- As configurações ficam num arquivo `chave = valor` em `%APPDATA%`, sem
  dependência nova; valor estranho volta ao padrão.

**Consequências.**
- ➕ Trocar de tela, som, pausa e qualidade sem derrubar ninguém: no teste, o fps
  nem caiu ao reabrir a captura.
- ➕ Personalizar é trocar um arquivo; um editor de temas pode vir depois sem
  mexer no resto.
- ➖ O texto impresso e as linhas de comando viram contrato entre os programas
  (testes em `saida.rs`, `espectadores.rs` e no cliente).
- ➖ A barra de quem assiste fica na janela principal, separada do vídeo.
- 🔁 Fase seguinte: a barra dentro da janela do vídeo (aparece com o mouse).
  Se os comandos crescerem muito, trocar as linhas de texto por mensagens
  estruturadas.

## ADR-018 — Atualização automática pelas Releases do GitHub

**Status:** Aceito (2026-11-12); a conferência passou a ser a do ADR-020

**Contexto.** Cada versão nova ia por zip, mandado à mão, e todos precisam da
mesma versão para se conectar. O pedido: verificar se há atualização e, havendo,
baixar, instalar e reiniciar sozinho. O código já está no GitHub, e o
repositório pode ser público.

**Decisão.**
- Cada versão é uma Release do GitHub (`vX.Y.Z`) com `FlameScreenSharing.zip` e
  `SHA256SUMS.txt`. A janela lê `releases/latest` ao abrir e compara com a
  própria versão (`CARGO_PKG_VERSION`).
- Baixa e abre o zip com o `curl.exe` e o `tar.exe` que vêm com o Windows, pelo
  caminho completo da pasta do sistema; nenhuma biblioteca de rede ou TLS no
  programa. Só o `serde_json` entra, para ler a resposta.
- Confere o SHA-256 antes de trocar qualquer coisa. Troca renomeando o arquivo
  antigo para `*.antigo` (funciona com o .exe aberto), desfaz tudo se uma troca
  falhar, e abre a versão nova com `--atualizado`. Nunca no meio de uma
  transmissão.
- Só a cópia empacotada se atualiza; a compilada só avisa.

**Consequências.**
- ➕ Todos ficam na mesma versão sem mandar zip.
- ➕ Sem servidor próprio, sem custo, sem dependência pesada.
- ➖ O repositório precisa ser público (a API sem senha só lê os públicos).
- ➖ O SHA-256 vem da mesma Release: protege contra download corrompido, não
  contra conta do GitHub invadida. Sem assinatura digital, o Windows continua
  avisando ao abrir e o antivírus pode desconfiar de um programa que se troca.
- ➖ Depende de Windows 10 1803 ou mais novo (`curl.exe` e `tar.exe`).
- 🔁 Assinatura digital (certificado de código), quando valer o custo; no Linux,
  o mesmo esquema com o `.tar.gz`.

## ADR-019 — Chave de acesso ao servidor de sinalização

**Status:** Aceito (2026-11-12)

**Contexto.** O endereço do servidor da Oracle ia dentro do programa: quem
recebesse o zip (ou baixasse do GitHub, com o repositório público) usaria o
servidor, e ele atendia qualquer pedido, sem limite de salas. Alguém poderia
lotá-lo de salas falsas ou usá-lo para responder a endereços forjados. A ideia
foi mandar aos amigos o programa e uma "chave"; criptografar só o IP não
protegeria nada (o programa precisa do IP para conectar, e ele aparece na
rede).

**Decisão.**
- A chave (`ServerKey`, `FSS1-…`) leva o endereço do servidor e um segredo
  aleatório de 128 bits. Cada pedido ao servidor sai com um HMAC-SHA256 do
  pacote, truncado em 16 bytes, no fim do datagrama.
- O `fluxo-sinal` lê as chaves aceitas de um arquivo e ignora, sem responder,
  o que não vier assinado com uma delas. Sem arquivo, recusa subir (a não ser
  com `--aberto`, para testes). `--nova-chave` gera uma.
- O programa não traz servidor: a chave vai em Configurações > Conexão
  (`--chave` no host e no cliente). Sem chave, só as formas sem servidor.

**Consequências.**
- ➕ Saber o IP não basta: sem chave, o servidor nem responde (não serve de
  amplificador nem de depósito de salas).
- ➕ Quem baixa o projeto roda o próprio servidor, com a própria chave.
- ➕ Chave vazou: gera outra, manda a quem deve ter, apaga a antiga.
- ➖ Quem tem a chave tem o segredo: não dá para saber qual amigo vazou, a não
  ser dando uma chave para cada (o servidor aceita várias).
- ➖ Versões antigas do programa param de achar salas no servidor novo.
- ➖ As respostas do servidor não são assinadas (só dariam um PairInit sem
  resposta; o SPAKE2 protege o pareamento).
- 🔁 Limite de pedidos por IP e de salas, se uma chave vazar e for usada para
  abusar; chave própria do repassador (relay), quando ele existir.

## ADR-020 — Atualizações assinadas e endurecimento geral

**Status:** Aceito (2026-11-12)

**Contexto.** O atualizador (ADR-018) conferia só o SHA-256 publicado na mesma
Release: com a conta do GitHub invadida, uma versão falsa seria instalada em
todos os PCs. Uma revisão de segurança apontou também a chave do servidor na
linha de comando dos programas, o `--sem-cifra` no programa de produção, o UPnP
ligado por padrão, a falta de limite de palpites de código, de um jeito de
tirar alguém da transmissão e IPs no texto de "Copiar detalhes".

**Decisão.**
- Cada versão leva um `MANIFESTO.txt` assinado com Ed25519 (`ed25519-dalek`):
  versão, nome e SHA-256 do zip. A chave privada fica só no PC de quem publica
  (`.secrets\`, `fss-assinatura`); o programa traz a pública. Recusa assinatura
  que não confere, versão diferente da anunciada ou não maior que a sua, zip
  com outro SHA-256 e caminhos perigosos dentro do zip.
- Segredos pela entrada padrão (`--segredos-pela-entrada`); `--sem-cifra` só
  em compilação de teste; UPnP opcional, renovado enquanto o programa roda;
  5 `PairInit` novos por IP a cada 10 s; `expulsar N`; IPs mascarados ao copiar.

**Consequências.**
- ➕ Conta do GitHub invadida não basta para distribuir uma versão.
- ➕ Nenhuma mudança no protocolo nem no caminho do vídeo (custo zero).
- ➖ Perder a chave privada impede publicar versões que os programas aceitem:
  precisa de cópia de segurança. Trocar a chave exige que todos instalem uma
  versão nova à mão.
- ➖ Segredos pela entrada não protegem de um programa que já leia a memória
  dos outros (mesmo usuário); a chave também fica em texto no arquivo de
  configurações.
- ➖ Expulsar bloqueia o IP: atinge todos atrás do mesmo IP.
- 🔁 Assinatura de código do Windows (Authenticode); chave do servidor cifrada
  no disco (DPAPI); trocar o código da sala sem encerrar.

---

## Modelo

```markdown
## ADR-XXX — Título curto

**Status:** Proposto | Aceito | Substituído por ADR-YYY (data)

**Contexto.** Qual problema, quais opções.

**Decisão.** O que foi escolhido.

**Consequências.** ➕ ganhos, ➖ custos, 🔁 quando reavaliar.
```
