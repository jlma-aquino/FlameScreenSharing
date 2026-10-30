# Protocolo de rede — versão 1

Especificação do formato dos pacotes UDP do Fluxo. Implementação de
referência: `crates/fluxo-proto`.

**Ao mudar o formato:** incremente `PROTOCOL_VERSION` em
`crates/fluxo-proto/src/packet.rs`, atualize este documento e registre no
CHANGELOG.

## 1. Cabeçalho (20 bytes, big-endian)

| Offset | Tamanho | Campo | Descrição |
|---:|---:|---|---|
| 0 | 2 | `magic` | Sempre `0x464C` ("FL"). Pacote com outro valor é descartado. |
| 2 | 1 | `version` | `1`. Versões diferentes são descartadas. |
| 3 | 1 | `kind` | `1` = Video, `2` = Ping, `3` = Pong. |
| 4 | 4 | `frame_id` | Número do frame (Video) ou do ping. Cresce de 1 em 1, pode dar a volta. |
| 8 | 2 | `frag_index` | Índice do fragmento, `0 .. frag_count-1`. |
| 10 | 2 | `frag_count` | Total de fragmentos do frame (≥ 1). |
| 12 | 1 | `flags` | Bit 0: keyframe. Bits 1–7 reservados (enviar 0, ignorar ao ler). |
| 13 | 1 | reservado | Enviar 0, ignorar ao ler. |
| 14 | 2 | `payload_len` | Bytes de payload. Deve ser igual a `tamanho do datagrama − 20`. |
| 16 | 4 | `timestamp_us` | Momento da captura em µs: relógio de parede (UTC) truncado para 32 bits, volta a cada ~71 min (`fluxo_net::clock_us`). |
| 20 | N | payload | Dados. |

## 2. Tipos de pacote

### Video (`kind = 1`)
Um pedaço de um frame codificado em **H.264 Annex B** (NALs com prefixo
`00 00 00 01`; keyframes trazem SPS/PPS). Todos os fragmentos de um frame têm
o mesmo `frame_id`, `frag_count`, `flags` e `timestamp_us`.

### Ping (`kind = 2`) / Pong (`kind = 3`)
Medição de RTT, nos **dois sentidos** (host e cliente pingam a cada 500 ms).
Quem recebe um Ping responde com um Pong copiando `frame_id` e
`timestamp_us`. RTT = relógio atual − `timestamp_us` do Pong, com aritmética
de 32 bits (`clock_diff_us`). `frag_index`/`frag_count` = 0, payload vazio.

## 2.1. Endereços (provisório, até o handshake da fase 5)

- O host envia para um `IP:porta` informado à mão.
- O cliente aceita o primeiro endereço que mandar um pacote válido e ignora
  os outros; após 2 s sem pacotes dele, aceita um novo (host reiniciado) e
  zera a remontagem.

## 3. Fragmentação (emissor)

- Payload máximo por pacote: **1200 bytes** (`DEFAULT_MAX_PAYLOAD`). Com
  cabeçalhos IP/UDP fica abaixo de 1280, o MTU mínimo do IPv6, evitando
  fragmentação IP (ADR-005).
- `frag_count = ceil(tamanho_do_frame / 1200)`, mínimo 1 (frame vazio gera
  1 pacote com payload vazio).
- Máximo de 65535 fragmentos por frame (limite do campo).

Tamanhos típicos em 1080p60:

| Bitrate | Frame P médio | Pacotes/frame | Pacotes/s |
|---:|---:|---:|---:|
| 10 Mbps | ~21 KB | ~18 | ~1100 |
| 20 Mbps | ~42 KB | ~35 | ~2100 |
| 40 Mbps | ~83 KB | ~70 | ~4200 |

Keyframes costumam ter 3–10× o tamanho de um frame P.

## 4. Remontagem (receptor)

1. Decodifica o cabeçalho; qualquer erro → descarta e conta.
2. Se `frame_id` ≤ último frame entregue → descarta (atrasado).
3. Guarda o fragmento; duplicado → descarta.
4. Quando todos os fragmentos de um frame chegam → entrega o frame e
   abandona todos os frames incompletos mais antigos.
5. Se houve buraco desde o último frame entregue, o frame sai com
   `preceded_by_loss = true` (o cliente deverá pedir keyframe — fase 2).
6. Mantém no máximo 8 frames incompletos (~133 ms a 60 fps); passou disso,
   descarta o mais antigo.
7. Rejeita frames com mais de 4096 fragmentos (proteção de memória).

A ordem de `frame_id` usa aritmética serial (RFC 1982): `a` é mais novo que
`b` se `(a − b) mod 2³²` < 2³¹.

## 5. Planejado para próximas versões

- `kind = 4` **KeyframeRequest** (cliente → host).
- `kind = 5` **Feedback** (cliente → host): perda, jitter, bitrate recebido.
- **FEC**: pacotes de paridade por grupo de fragmentos (Reed-Solomon ou XOR).
- **Criptografia**: chave por sessão trocada no handshake; payload cifrado
  com AEAD (ChaCha20-Poly1305), cabeçalho como dado autenticado.
- **Handshake/sessão**: identificador de sessão no cabeçalho para ignorar
  pacotes de conexões antigas.
