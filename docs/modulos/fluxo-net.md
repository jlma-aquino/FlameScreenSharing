# fluxo-net

**Caminho:** `crates/fluxo-net` · **Dependências:** `fluxo-proto` · **Plataforma:** qualquer

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

## Bitrate adaptativo

- **Cliente** (`VideoReceiver`, automático): a cada 250 ms manda um
  `Feedback` com os pacotes de vídeo/paridade recebidos (acumulado) e o
  atraso de fila (`delay.rs`: trânsito de cada frame menos o menor dos
  últimos 10 s, suavizado). `ReceiverStats::queue_delay_ms` mostra o valor.
- **Host** (`VideoSender`): compara os recebidos com os pacotes que tentou
  mandar no intervalo e calcula a perda; `take_report()` entrega um
  `NetworkReport { loss, queue_delay_ms }`.
- **Decisão** (`BitrateController`, lógica pura em `bitrate.rs`):

| Situação | Ação |
|---|---|
| perda ≥ 10% ou fila ≥ 60 ms | × 0,7 |
| perda ≥ 3% ou fila ≥ 25 ms | × 0,85 (no máximo 1 queda a cada 500 ms) |
| perda < 1% e fila < 10 ms, 2 s sem quedas | + 8% (no máximo 1×/s) |

  Perda abaixo de ~2% fica para o FEC. O host aplica a decisão no encoder
  (`Encoder::set_bitrate`, sem keyframe) e no pacing (2,5× o bitrate).
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

## Pedido de keyframe

- Cliente: `VideoReceiver::request_keyframe()` manda um `KeyframeRequest` ao
  host. Pode ser chamado a cada frame: só envia se o último pedido tiver mais
  de 50 ms (ou 2 RTTs), tempo para o keyframe chegar antes de pedir de novo.
- Host: `VideoSender::take_keyframe_request()` devolve `true` uma vez por
  pedido (vários acumulados contam como um).

## Largada e perda simulada

- O primeiro ping sai antes do primeiro frame, e o envio espera até 50 ms
  pela resposta: sem isso, a primeira rajada para um IP novo pode ser
  descartada enquanto o sistema resolve o endereço físico (ARP).
- `SenderConfig::simulated_loss` (só para testes) descarta uma fração dos
  pacotes de vídeo antes do envio; o pacote "perdido" ainda ocupa seu tempo
  no pacing, como numa rede real.

## Host único e reinício

O receptor aprende o endereço do host pelo primeiro pacote válido e ignora
os outros. Se o host ficar 2 s sem mandar nada
(`ReceiverConfig::peer_timeout`), um endereço novo é aceito e a remontagem
recomeça do zero — a numeração de frames do host novo recomeça e, sem isso,
pareceria "atrasada". Isso é provisório: a fase 5 traz handshake e ID de
sessão.

## Relógio e latência

`timestamp_us` = relógio de parede em µs, truncado para 32 bits
(`clock_us`). Na mesma máquina, ou com relógios sincronizados por NTP,
`ReceivedFrame::transit_us` dá a latência real captura → frame completo. Em
máquinas diferentes sem sincronia, o número tem o erro da diferença entre os
relógios; o RTT não é afetado.

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
