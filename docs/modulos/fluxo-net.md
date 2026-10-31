# fluxo-net

**Caminho:** `crates/fluxo-net` · **Dependências:** `fluxo-proto`, `fluxo-crypto` · **Plataforma:** qualquer

Transporte UDP: faz o I/O que o `fluxo-proto` deliberadamente não faz. Só
biblioteca padrão — roda e é testado em qualquer sistema.

## Arquivos

| Arquivo | Conteúdo |
|---|---|
| `src/lib.rs` | Visão geral, `NetError`, reexportações |
| `src/sender.rs` | `VideoSender`, `SenderConfig`, `SenderStats` (host) |
| `src/receiver.rs` | `VideoReceiver`, `ReceiverConfig`, `ReceivedFrame`, `ReceiverStats` (cliente) |
| `src/pacer.rs` | Token bucket do pacing (lógica pura, testada com tempo simulado) |
| `src/rtt.rs` | `RttEstimator` / `RttStats` (média suavizada estilo TCP) |
| `src/clock.rs` | `clock_us`, `clock_diff_us` — relógio de 32 bits dos pacotes |
| `src/sync.rs` | `ClockSync` — diferença entre os relógios pelos Ping/Pong |
| `src/sim.rs` | Atraso e jitter simulados (só testes): `DelayModel`, `DelayLine` |
| `src/recovery.rs` | `Recovery` — quando mostrar a imagem e o que pedir ao host depois de perdas (cliente) |
| `src/signal.rs` | `SignalServer` — servidor de sinalização (salas, endereços; lógica testável sem rede) |
| `src/stun.rs` | `discover_public_addr` — endereço público pelo STUN, com detecção de NAT simétrico |
| `src/conexao.rs` | `ConnectionCode` / `ReplyCode` — os códigos que vão pelo chat, sem servidor |
| `tests/loopback.rs` | Testes com sockets reais em 127.0.0.1 |

## Threads

```text
HOST (VideoSender)                              CLIENTE (VideoReceiver)
 send_frame() ──fila──► [envio]                  [recepção] ──fila──► recv_frame()
                          Fragmenter                remonta frames
                          pacing (token bucket)     responde Ping, mede Pong
                          Ping a cada 500 ms        Ping a cada 500 ms
                        [pings]
                          responde Ping, mede Pong
```

- `send_frame` só enfileira: captura e encode nunca esperam a rede.
- A recepção lê o socket sem parar, para o buffer do sistema não transbordar
  enquanto o cliente decodifica.
- A fila de frames prontos guarda até 120 (`ReceiverConfig::queue_frames`,
  ~2 s a 60 fps): descartar um frame quebra a cadeia de P-frames, então é
  melhor o cliente atrasar um instante e alcançar do que perder imagem.
- Os dois lados medem RTT: o host para o futuro controle de bitrate, o
  cliente para o overlay.

## Pacing

Token bucket: taxa padrão de **2,5× o bitrate** do vídeo, rajadas de até
**10 pacotes**. Um frame de 40 KB a 50 Mbps leva ~6 ms para sair inteiro.
`VideoSender::set_pacing_bps` muda a taxa em tempo real (para o controle de
bitrate da fase 4); 0 desliga.

O modelo é de "dívida": o pacote sai se o saldo não está negativo e então
desconta seu tamanho. Assim pacotes maiores que o balde nunca travam e a taxa
média é exata.

## FEC

`SenderConfig::fec_group` (padrão 10) liga a paridade XOR do `fluxo-proto`:
1 pacote extra a cada 10, e o `Reassembler` do cliente reconstrói uma perda
por grupo sem pedir nada ao host. `ReassemblerStats::fec_recovered` conta as
reconstruções. O pacing já leva em conta os pacotes extras.

Medido com o exemplo `loopback` (300 frames, 20 Mbps):

| Perda | Sem FEC | Grupos de 10 (+10%) | Grupos de 5 (+20%) |
|---|---|---|---|
| 1% | 68% dos frames inteiros | 98,7% | — |
| 3% | 35% | 85% | 91% |

Medido no Wi-Fi (Windows → notebook Linux, `fluxo-bench enviar`, 20 s por
carga, FEC de 10): até 40 Mbps, **nenhum pacote perdido**. Acima da
capacidade (~45 Mbps) as perdas vêm em rajada:

| Carga | Frames perdidos | Grupos falhos com 2 faltando | com 3 ou mais |
|---|---|---|---|
| 50 Mbps | 551 | 208 | 1.783 |
| 60 Mbps | 894 | 309 | 3.348 |
| 80 Mbps | 562 | 432 | 7.745 |

Um código com 2 paridades por grupo (Reed-Solomon) salvaria só a minoria de
2; o frame cairia pelos outros grupos. Perda por fila cheia se resolve
baixando o bitrate (adaptativo), não com mais FEC. `ReassemblerStats`
conta os grupos que falharam (`fec_groups_failed_2`, `_3plus`).

## Bitrate adaptativo

- **Cliente** (`VideoReceiver`, automático): a cada 250 ms manda um
  `Feedback` com os pacotes de vídeo/paridade recebidos (acumulado) e o
  atraso de fila (`delay.rs`: `chegada − send_us` do pacote que completou
  cada frame, menos o menor dos últimos 10 s, suavizado).
  `ReceiverStats::queue_delay_ms` mostra o valor. Medir desde a **saída**
  (`send_us`, protocolo v6) e não desde a captura é essencial: encode,
  espera do limite de fps e pacing crescem com o tamanho do frame e
  pareciam 15 ms de fila a 7 Mbps num Wi-Fi que aguenta 30+.
- **Host** (`VideoSender`): compara os recebidos com os pacotes que tentou
  mandar e calcula a perda; `take_report()` entrega um
  `NetworkReport { loss, queue_delay_ms, sent_kbps }`. A conta (`history.rs`) evita
  "perda fantasma", que no primeiro teste no Wi-Fi derrubou o bitrate de 20
  para 2,5 Mbps com zero pacotes perdidos:
  - cada pacote conta como enviado **na hora em que sai** (não no início do
    frame, que leva milissegundos no pacing);
  - compara com o que tinha saído **um RTT antes** do relatório chegar (o
    resto ainda estava no caminho ou na fila do Wi-Fi);
  - usa uma janela de **1 s** de relatórios, não 250 ms;
  - só conta perda que o cliente **confirmou** (fragmento reconstruído pelo
    FEC ou frame perdido): o Wi-Fi segura pacotes por alguns ms e os entrega
    em lote, o que sozinho parecia 3–6% de perda.
- **Decisão** (`BitrateController`, lógica pura em `bitrate.rs`):

| Situação | Ação |
|---|---|
| perda ≥ 10% ou fila ≥ 2,4 × limite (36 ms) | × 0,7 |
| perda ≥ 3% ou fila ≥ limite (15 ms) | × 0,85 (no máximo 1 queda a cada 500 ms) |
| perda < 1%, fila < 0,4 × limite (6 ms), envio ≥ 50% do alvo, 2 s sem quedas | + 8% (no máximo 1×/s) |

  - O limite de fila é `BitrateController::with_queue_limit` (host:
    `--fila-max`, padrão 15 ms).
  - Começa em no máximo 20 Mbps (`START_KBPS`) e sobe se a rede deixar:
    começar direto em 60 deixava a largada ~2 s com 100–200 ms de atraso.
  - Só sobe se o envio real (`sent_kbps`) usar pelo menos metade do alvo:
    com a tela parada a rede parece folgada, o alvo ia a 60 Mbps e, quando o
    movimento voltava, a rajada enchia a fila.

  Perda abaixo de ~2% fica para o FEC. O host aplica a decisão no encoder
  (`Encoder::set_bitrate`, sem keyframe) e no pacing (2,5× o bitrate).

Medido no Wi-Fi (host Windows → notebook Linux, vídeo, `--mbps 60`, 30 s),
já medindo a fila pelo `send_us`:

| Limite de fila | Bitrate médio | Latência p50 | Fila em regime |
|---|---|---|---|
| 25 ms | 32,7 Mbps | 22,5 ms | 4–12 ms, picos de 24–26 |
| 15 ms | 27,1 Mbps | 25,3 ms | 2–7 ms |
| 10 ms | 23,6 Mbps | 20,5 ms | 2–6 ms |

O conteúdo do vídeo pesa mais que o limite; a latência em regime acompanha
o bitrate (~13–16 ms a 15 Mbps, ~22–25 ms a 35–40 Mbps), pelo tempo do
frame atravessar o Wi-Fi. O p99 dessas rodadas (~180–210 ms) vinha todo da
largada a 60 Mbps; começando em 20 Mbps, a largada ficou em ~27 ms e o p99
caiu para 70 ms.
- **Banda simulada** (`SenderConfig::simulated_bandwidth_bps`, só testes):
  envia no ritmo do "link"; com mais de 4 frames na fila, descarta o frame
  inteiro — como um roteador congestionado.

Medido (link simulado de 8 Mbps, host querendo 20, 15 s):

| | Bitrate fixo | Adaptativo |
|---|---|---|
| Frames perdidos | 255 | 44 (quase todos nos 2 s iniciais) |
| Frames mostrados | 470 de 593 (31 fps) | 762 de 873 (51 fps) |
| Captura → tela p99 | 223 ms | 95 ms |
| Regime estável | perda 45–65%, fila 60–125 ms | perda 0%, fila 5–10 ms |

## Recuperação de perdas

- Cliente: `Recovery` (lógica pura, usada pelo `fluxo-client` e pelo
  `fluxo-bench receber`) decide, frame a frame, se decodifica, se mostra e o
  que pedir: `request_keyframe()` na largada e depois de erro do decoder;
  `request_refresh()` depois de uma perda. Depois da perda continua
  decodificando sem mostrar, até um keyframe ou o fim de uma onda de
  intra-refresh que começou depois dela; esperando há mais de 1 s, pede
  keyframe. Os pedidos podem ser feitos a cada frame: só saem se o último
  tiver mais de 50 ms (ou 2 RTTs).
- Host: `take_keyframe_request()` (só keyframe serve) e
  `take_refresh_request()` (uma onda serve; sem ela, keyframe), uma vez por
  pedido. `send_frame_with_flags` leva as marcas da onda
  (`FLAG_REFRESH_START`/`FLAG_REFRESH_END`).
- Medido na mesma máquina, 3% de perda de pacotes sem FEC: congelamento
  médio de 31 ms com keyframe, 231 ms com ondas de 4 frames (a onda precisa
  de 4 frames seguidos sem perda). Ver `PROTOCOLO.md`.
- No Wi-Fi (Windows → notebook Linux, 2% de perda simulada sem FEC, 20 s):
  keyframe limpou a imagem em média em 22 ms (máx 135 ms, 70 recuperações);
  ondas de 4 em 152 ms (máx 470 ms; 78 ondas, só 33 terminaram sem outra
  perda no meio). A latência máxima foi menor com ondas (24 contra 165 ms),
  mas o conteúdo da tela variou entre as rodadas.

## Largada e perda simulada

- O primeiro ping sai antes do primeiro frame, e o envio espera até 50 ms
  pela resposta: sem isso, a primeira rajada para um IP novo pode ser
  descartada enquanto o sistema resolve o endereço físico (ARP).
- `SenderConfig::simulated_loss` (só para testes) descarta uma fração dos
  pacotes de vídeo antes do envio; o pacote "perdido" ainda ocupa seu tempo
  no pacing, como numa rede real.
- `SenderConfig::simulated_delay` / `simulated_jitter` (só para testes):
  tudo o que o host manda passa por uma linha de atraso (thread própria), e
  tudo o que ele recebe espera a sua vez antes de ser tratado — os dois
  sentidos atrasam igual, então o RTT e a sincronização dos relógios
  continuam coerentes. O jitter sorteia um extra em `[0, jitter]` a cada
  20 ms; quando o extra cai, os pacotes segurados saem juntos (rajada, como
  no Wi-Fi). Os pacotes nunca se ultrapassam.

## Pareamento e cifra

Com `SenderConfig`/`ReceiverConfig::pairing_code` (o padrão nos apps), o
`fluxo-crypto` entra em cena:

- **Host** (`VideoSender::connect`): antes de criar as threads, manda
  `PairInit` a cada 250 ms até chegar o `PairReply` (prazo
  `pairing_timeout`; os apps usam 30 s). Código errado: erro na hora
  (`NetError::Pairing`, "o código de pareamento não confere"). Depois, todo
  pacote passa pela `Saida`, que cifra por último — com `send_us` já
  gravado, o cabeçalho inteiro é autenticado. A chave de envio é uma só para
  as duas threads (o `seq` também).
- **Cliente** (`VideoReceiver`): responde a cada `PairInit` (o mesmo de
  novo recebe a mesma resposta) com uma sessão **candidata**, que vira a
  ativa no primeiro pacote que ela decifrar; até lá, a sessão antiga segue
  valendo. Só aceita o que decifrar; o endereço do host passa a ser o do
  último pacote autenticado (o NAT pode mudar a porta). Tudo o que manda
  (pings, pongs, relatórios, pedidos) vai cifrado pela mesma chave.
- Estatísticas: `encrypted` nos dois lados, `pairings` e
  `invalid_packets` no cliente, `rejected_packets` no host.
- Medido no Wi-Fi (Windows → notebook Linux, 30 Mbps sintéticos, 20 s):
  latência p50 13,7 ms cifrado contra 13,2 ms em claro; nenhuma perda.

## Sinalização e furo de NAT (redes diferentes)

Formato e sequência: [PROTOCOLO.md](../PROTOCOLO.md), seção 2.2. Servidor:
[fluxo-sinal](fluxo-sinal.md).

- **Cliente** (`ReceiverConfig::signal_server`, exige código): manda
  `Register` com a sala do código (`PairingCode::room`) e o próprio
  endereço local a cada 1 s até o servidor confirmar, depois a cada 5 s
  (mantém a sala e o mapeamento do NAT). A cada `Found` (um host
  procurando), manda um `Punch` a cada endereço do host. Estatísticas:
  `public_addr` (como a internet nos vê), `room_taken`, `punches_sent`.
- **Host** (`VideoSender::connect_via_signal`): o socket fica solto durante
  o pareamento. A cada 250 ms manda `Lookup` ao servidor e `PairInit` a
  todos os candidatos (público e locais do cliente, mais quem mandou furo;
  até 8). O primeiro `PairReply` válido de um candidato define o destino, e
  só então o socket se prende a ele (`connect`). Um candidato que responde
  com outra chave sai da lista e o handshake recomeça. Se nada der certo, o
  erro diz onde parou: servidor mudo, sala vazia, ou cliente achado mas
  pacotes barrados (NAT restritivo nos dois lados).
- `local_ip()` descobre o IP da rede local sem mandar pacote (rota até um
  IP público); `resolve_signal_server` aceita `host`, `host:porta` ou IP,
  com a porta padrão 7000.

Medido com o servidor no notebook, nos dois sentidos, pelo Wi-Fi (sem NAT
entre os dois: prova o fluxo, não o furo de um NAT de verdade):

| Teste | Resultado |
|---|---|
| `enviar` (Windows) acha o `receber` (notebook), 30 Mbps, 5 s | pareado pelo endereço local do cliente (registrado pelo 127.0.0.1); 0 perdas, latência 15–17 ms |
| `enviar` (notebook) acha o `receber` (Windows), 30 Mbps, 8 s | 0 perdas, p50 17,5 ms |
| `fluxo-host` (tela real, 20 Mbps) acha o `receber` (notebook), 10 s | 0 perdas, 1 keyframe pedido na largada, p50 18,1 ms |

O servidor só aparece no começo: depois do `Found`, nada mais passa por ele.

## Sem servidor: código de conexão (STUN público)

Formato e sequência: [PROTOCOLO.md](../PROTOCOLO.md), seção 2.3; decisão:
ADR-010. O furo e o pareamento são os da sinalização; só os endereços
chegam de outro jeito.

- `discover_public_addr(&socket, STUN_SERVERS)`: pergunta aos servidores
  STUN públicos o endereço do socket (até ~1,5 s, com reenvio). Dois
  servidores vendo portas diferentes = NAT simétrico (`PublicAddr::warning`).
  O socket depois vai para `VideoReceiver::with_socket` ou
  `VideoSender::connect_with_socket`: o endereço só vale para ele.
- **Cliente:** `ReceiverConfig::keepalive_to` (o servidor STUN que
  respondeu) repete o pedido a cada 15 s enquanto nenhum host chega, para o
  NAT não esquecer o mapeamento. `punch(&[endereço do host])` — ou
  `puncher()`, de outra thread — manda Punch a cada 250 ms por até 1 min;
  para sozinho quando o host aparece.
- **Host:** `connect_with_socket(socket, endereço do cliente, cfg)` manda
  PairInit até o prazo (os apps usam 5 min nesse modo). Furo vindo de outra
  porta vira candidato: funciona com NAT simétrico **de um lado só**
  (testado em `tests/loopback.rs` com o endereço do código errado de
  propósito).
- `ConnectionCode` / `ReplyCode`: ver o arquivo; erros de digitação são
  pegos pela conferência (`CodeError::Check`).

Medido nesta rede: os dois servidores STUN responderam a mesma porta (NAT
de casa não é simétrico). Com host e cliente atrás do mesmo roteador o furo
não fecha (o roteador não faz *hairpin*); o teste de verdade é com o
notebook na rede do celular.

**Diagnóstico.** `probe_nat` devolve o que cada servidor STUN viu (base do
`fluxo-bench nat`); `PublicAddr::summary` resume numa linha. Com
`SenderConfig::on_pairing` / `ReceiverConfig::on_pairing` (um
`PairingObserver`), cada datagrama que chega antes da sessão vira
`PairingEvent::Received` ("furo", "pedido de pareamento", "lixo"...), e a
cada 5 s sai um `PairingEvent::Waiting` com quantos pedidos/furos saíram e
para onde. `PairingObserver::print_to_terminal()` mostra isso como linhas
`[conexão]`; os apps ligam no modo de código de conexão.

Primeiro teste com o notebook na rede do celular (2026-10-31): o host
mandou pedidos ao endereço do código de conexão por minutos e nada voltou;
o NAT de casa deu a mesma porta para três servidores STUN. Sem o
diagnóstico acima, não dava para saber em qual lado os pacotes paravam.

## Host único e reinício (sem cifra)

Com `--sem-cifra` (teste), o receptor aprende o endereço do host pelo
primeiro pacote válido e ignora os outros. Se o host ficar 2 s sem mandar
nada (`ReceiverConfig::peer_timeout`), um endereço novo é aceito e a
remontagem recomeça do zero — a numeração de frames do host novo recomeça e,
sem isso, pareceria "atrasada". Com cifra, o host novo é o que parear.

## Relógio e latência

`timestamp_us` = relógio de parede em µs, truncado para 32 bits
(`clock_us`). Entre dois PCs os relógios diferem (no teste com o notebook,
837 ms), então o Fluxo os **sincroniza pelos Ping/Pong** (`sync.rs`, estilo
NTP): o Pong leva o relógio de quem respondeu, e a diferença é estimada pela
medição de menor RTT entre as últimas 16 — erro máximo de meio RTT (~0,5 ms
numa LAN).

- `ReceivedFrame::latency_us`: latência real captura → frame completo, com a
  diferença descontada (`None` nos ~500 ms antes da 1ª medição).
- `ReceivedFrame::transit_us`: o valor bruto, comparando os relógios como
  estão (usado pelo atraso de fila, onde a diferença se cancela).
- `ReceiverStats::clock_offset_ms` / `clock_error_ms` e
  `SenderStats::clock_offset_ms`: a diferença medida de cada lado.

Medido (host Windows → notebook Linux no Wi-Fi, vídeo, 30 s): latência de ida
p50 **5,1 ms**, p99 29,9 ms, máx 85 ms; relógios a +837,4 ms ± 0,5 ms.

## Erros

- `connection reset` em UDP é normal no Windows: aparece quando um datagrama
  nosso encontrou a porta do outro lado fechada (cliente ainda não abriu). É
  ignorado.
- Pacotes inválidos ou de outro endereço são descartados e contados
  (`ReceiverStats::invalid_packets`), nunca derrubam a recepção.

## Medições (mesma máquina, 1080p, NVENC 20 Mbps)

| | Área de trabalho | Vídeo de jogo a 60 fps |
|---|---|---|
| Captura → frame completo no receptor | p50 2,9 ms, p99 7,1 ms | p50 5,5 ms, p99 7,6 ms |
| Frames perdidos / decodificados | 0 / 488 de 488 | 0 / 900 de 900 |
| Maior tempo de envio de um frame (pacing 50 Mbps) | 5,0 ms | 6,5 ms |
| Bitrate médio | 6,3 Mbps | 13,3 Mbps |

## Medições entre dois PCs (host Windows → notebook Linux no Wi-Fi, 30 s)

| | Resultado |
|---|---|
| Pacotes | 40 085 recebidos, 0 duplicados, 0 atrasados, 0 inválidos |
| Frames perdidos | 1, na largada (provável descarte da 1ª rajada enquanto o Windows resolvia o ARP); 0 durante o vídeo |
| RTT | 1,0 ms mínimo, 2,9 ms suavizado; sobe de ~1,2 para 2,5–4,6 ms transmitindo a ~17 Mbps (fila no caminho) |
| Trânsito | Relógios defasados ~0,7 s, então só a variação vale: p99 +17 ms e pico +56 ms acima da mediana (na mesma máquina, ~2 ms) |

## Testes

```powershell
cargo test -p fluxo-net
```

Ponta a ponta com captura e encode: [fluxo-host](fluxo-host.md).
