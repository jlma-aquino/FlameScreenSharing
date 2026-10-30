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

## Testes

```powershell
cargo test -p fluxo-net
```

Ponta a ponta com captura e encode: [fluxo-host](fluxo-host.md).
