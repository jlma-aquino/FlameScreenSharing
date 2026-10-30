# Protocolo de rede — versão 6

Especificação do formato dos pacotes UDP do Fluxo. Implementação de
referência: `crates/fluxo-proto`.

**Ao mudar o formato:** incremente `PROTOCOL_VERSION` em
`crates/fluxo-proto/src/packet.rs`, atualize este documento e registre no
CHANGELOG.

## 1. Cabeçalho (24 bytes, big-endian)

| Offset | Tamanho | Campo | Descrição |
|---:|---:|---|---|
| 0 | 2 | `magic` | Sempre `0x464C` ("FL"). Pacote com outro valor é descartado. |
| 2 | 1 | `version` | `6`. Versões diferentes são descartadas (histórico na seção 6). |
| 3 | 1 | `kind` | `1` = Video, `2` = Ping, `3` = Pong, `4` = KeyframeRequest, `5` = Parity, `6` = Feedback. |
| 4 | 4 | `frame_id` | Número do frame (Video) ou do ping. Cresce de 1 em 1, pode dar a volta. |
| 8 | 2 | `frag_index` | Índice do fragmento, `0 .. frag_count-1`. |
| 10 | 2 | `frag_count` | Total de fragmentos do frame (≥ 1). |
| 12 | 1 | `flags` | Bit 0: keyframe. Bits 1–7 reservados (enviar 0, ignorar ao ler). |
| 13 | 1 | `fec_group` | Só em Parity: fragmentos por grupo de FEC (≥ 1). Nos outros tipos, 0. |
| 14 | 2 | `payload_len` | Bytes de payload. Deve ser igual a `tamanho do datagrama − 24`. |
| 16 | 4 | `timestamp_us` | Momento da captura em µs: relógio de parede (UTC) truncado para 32 bits, volta a cada ~71 min (`fluxo_net::clock_us`). |
| 20 | 4 | `send_us` | Momento em que **este pacote** saiu, no mesmo relógio. Gravado na hora do envio (`stamp_send_time`), depois do pacing. Ver "Atraso de fila" no Feedback. |
| 24 | N | payload | Dados. |

## 2. Tipos de pacote

### Video (`kind = 1`)
Um pedaço de um frame codificado em **H.264 Annex B** (NALs com prefixo
`00 00 00 01`; keyframes trazem SPS/PPS). Todos os fragmentos de um frame têm
o mesmo `frame_id`, `frag_count`, `flags` e `timestamp_us`.

### Ping (`kind = 2`) / Pong (`kind = 3`)
Medição de RTT e **sincronização de relógio**, nos dois sentidos (host e
cliente pingam a cada 500 ms). `frag_index`/`frag_count` = 0.

- **Ping:** `timestamp_us` = relógio de quem pinga (`t0`); payload vazio.
- **Pong:** copia `frame_id` e `timestamp_us` do Ping; payload de **4 bytes**
  (u32 big-endian) com o relógio de quem responde, na hora de responder (`t1`).
- Quem recebe o Pong no instante `t3` calcula `RTT = t3 − t0` e a diferença
  entre os relógios `t1 − (t0 + RTT/2)` (estilo NTP), usando entre as
  últimas 16 medições a de menor RTT. Erro máximo: metade desse RTT (~0,5 ms
  em rede local). Toda a aritmética é de 32 bits com volta (`clock_diff_us`).

### KeyframeRequest (`kind = 4`)
Cliente → host: "perdi a referência, mande um keyframe". Enviado quando um
frame chega com `preceded_by_loss`, quando o decode falha, e enquanto o
cliente não tem imagem (largada). `frame_id` = número do pedido,
`timestamp_us` = relógio do cliente, payload vazio.

- **Cliente:** no máximo um pedido a cada 50 ms (ou 2 RTTs, o que for maior),
  para dar tempo ao keyframe de chegar antes de pedir de novo.
- **Host:** vários pedidos acumulados contam como um. Força keyframe no
  próximo encode; com a tela parada, recodifica a última imagem enviada.
- O keyframe periódico do host (padrão 10 s) fica só como rede de segurança
  para um pedido que se perca.

### Feedback (`kind = 6`) — relatório para o bitrate adaptativo
Cliente → host a cada 250 ms. Payload de 16 bytes (big-endian):
`packets_received` (u32, acumulado: vídeo + paridade que chegaram),
`queue_delay_us` (u32), `frames_lost` (u32, acumulado), `fec_recovered`
(u32, acumulado). Os contadores são acumulados para que um relatório
perdido não estrague a conta.

- **Perda:** o host compara `packets_received` com quantos pacotes tentou
  mandar entre dois relatórios: `perda = 1 − recebidos / tentados`. Com
  menos de 30 pacotes no intervalo (tela parada) a conta não é feita.
- **Atraso de fila:** quando um frame se completa, o cliente calcula
  `chegada − send_us` do pacote que o completou e compara com o menor valor
  dos últimos 10 s (a rede vazia). A defasagem entre os relógios se cancela,
  então vale entre PCs diferentes. Usar `send_us`, e não `timestamp_us`,
  tira da conta o encode, a espera do limite de fps e o pacing (que crescem
  com o tamanho do frame e pareciam fila — versão 6).

### Parity (`kind = 5`) — FEC
Pacote de paridade XOR de um grupo de fragmentos de vídeo. Os fragmentos de
um frame são divididos em grupos de `fec_group` (padrão 10); depois do
último fragmento de cada grupo sai a sua paridade.

- `frame_id`, `frag_count`, `flags` e `timestamp_us`: iguais aos do frame.
- `frag_index`: número do grupo, `0 .. ceil(frag_count / fec_group) - 1`.
- Payload: `[len_xor: u16][xor]`, onde `xor` é o XOR dos payloads do grupo
  (os mais curtos completados com zeros até o maior) e `len_xor` o XOR dos
  seus comprimentos.
- Receptor: se o grupo tem **exatamente um** fragmento faltando e a paridade
  chegou, `faltante = xor ⊕ (todos os outros)`, cortado em
  `len_xor ⊕ (comprimentos dos outros)`. Com 2+ faltando, o grupo não se
  recupera (aí entra o KeyframeRequest).
- Custo: `1/fec_group` a mais de pacotes. Medido com 1% de perda: 98,7% dos
  frames inteiros com grupos de 10, contra 68% sem FEC.

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
   `preceded_by_loss = true`: o cliente congela a última imagem boa e pede
   um keyframe (KeyframeRequest).
6. Mantém no máximo 8 frames incompletos (~133 ms a 60 fps); passou disso,
   descarta o mais antigo.
7. Rejeita frames com mais de 4096 fragmentos (proteção de memória).
8. Guarda as paridades por grupo e reconstrói um fragmento quando o grupo tem
   exatamente um faltando. Paridade de frame já entregue é ignorada (não
   conta como pacote atrasado).

A ordem de `frame_id` usa aritmética serial (RFC 1982): `a` é mais novo que
`b` se `(a − b) mod 2³²` < 2³¹.

## 5. Planejado para próximas versões

- **FEC mais forte** (Reed-Solomon) se a perda em rajada se mostrar comum:
  o XOR atual só recupera uma perda por grupo.
- **Criptografia**: chave por sessão trocada no handshake; payload cifrado
  com AEAD (ChaCha20-Poly1305), cabeçalho como dado autenticado.
- **Handshake/sessão**: identificador de sessão no cabeçalho para ignorar
  pacotes de conexões antigas.

## 6. Histórico de versões

| Versão | Mudança |
|---:|---|
| 1 | Video, Ping, Pong. |
| 2 | + KeyframeRequest (`kind = 4`). Pacotes da versão 1 são recusados. |
| 3 | + Parity (`kind = 5`, FEC); byte 13 vira `fec_group`. Pacotes das versões 1 e 2 são recusados. |
| 4 | + Feedback (`kind = 6`), relatório para o bitrate adaptativo. Versões anteriores recusadas. |
| 5 | Pong leva 4 bytes com o relógio de quem responde (sincronização). Versões anteriores recusadas. |
| 6 | Cabeçalho de 24 bytes: + `send_us` (hora de saída de cada pacote), base do atraso de fila. Versões anteriores recusadas. |
