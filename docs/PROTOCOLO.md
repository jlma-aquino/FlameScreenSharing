# Protocolo de rede — versão 11

Especificação do formato dos pacotes UDP do Fluxo. Implementação de
referência: `crates/fluxo-proto`.

**Ao mudar o formato:** incremente `PROTOCOL_VERSION` em
`crates/fluxo-proto/src/packet.rs`, atualize este documento e registre no
CHANGELOG.

## 1. Cabeçalho (28 bytes, big-endian)

| Offset | Tamanho | Campo | Descrição |
|---:|---:|---|---|
| 0 | 2 | `magic` | Sempre `0x464C` ("FL"). Pacote com outro valor é descartado. |
| 2 | 1 | `version` | `11`. Versões diferentes são descartadas (histórico na seção 6). |
| 3 | 1 | `kind` | `1` = Video, `2` = Ping, `3` = Pong, `4` = KeyframeRequest, `5` = Parity, `6` = Feedback, `7` = PairInit, `8` = PairReply, `9` = Signal, `10` = Punch, `11` = Cursor, `13` = Audio (`12` reservado: Input). |
| 4 | 4 | `frame_id` | Número do frame (Video) ou do ping. Cresce de 1 em 1, pode dar a volta. |
| 8 | 2 | `frag_index` | Índice do fragmento, `0 .. frag_count-1`. |
| 10 | 2 | `frag_count` | Total de fragmentos do frame (≥ 1). |
| 12 | 1 | `flags` | Bit 0: keyframe. Bit 1: início de uma onda de intra-refresh. Bit 2: fim da onda. Bit 3: payload cifrado (seção 2.1). Bits 4–7 reservados (enviar 0, ignorar ao ler). No KeyframeRequest, o bit 0 diz que só keyframe serve. |
| 13 | 1 | `fec_group` | Só em Parity: fragmentos por grupo de FEC (≥ 1). Nos outros tipos, 0. |
| 14 | 2 | `payload_len` | Bytes de payload (cifrado: inclui a tag de 16). Deve ser igual a `tamanho do datagrama − 28`. |
| 16 | 4 | `timestamp_us` | Momento da captura em µs: relógio de parede (UTC) truncado para 32 bits, volta a cada ~71 min (`fluxo_net::clock_us`). |
| 20 | 4 | `send_us` | Momento em que **este pacote** saiu, no mesmo relógio. Gravado na hora do envio (`stamp_send_time`), depois do pacing. Ver "Atraso de fila" no Feedback. |
| 24 | 4 | `seq` | Número do pacote no seu sentido (host → cliente ou o contrário), começando em 0: o nonce da cifra. 0 em pacotes em claro. |
| 28 | N | payload | Dados (cifrado: dados cifrados + tag de 16 bytes). |

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
Cliente → host: "perdi a referência, limpe a imagem". `frame_id` = número do
pedido, `timestamp_us` = relógio do cliente, payload vazio. O bit 0 de
`flags` diz o que serve:

- **Bit 0 ligado — só keyframe serve:** largada (o cliente não tem imagem
  nem SPS/PPS) e depois de um erro do decoder.
- **Bit 0 desligado — limpeza depois de perda** (frame com
  `preceded_by_loss`): uma onda de intra-refresh serve, se o host tiver; senão
  o host manda keyframe.

Regras:

- **Cliente:** no máximo um pedido a cada 50 ms (ou 2 RTTs, o que for maior).
  Depois de uma perda, decodifica tudo mas congela a última imagem boa até
  um keyframe, ou até o fim (bit 2) de uma onda cujo início (bit 1) veio
  depois da perda. Esperando há mais de 1 s, pede keyframe.
- **Host:** vários pedidos acumulados contam como um. Com a tela parada,
  recodifica a última imagem enviada (e continua a cada frame até a onda
  acabar).
- O keyframe periódico do host (padrão 10 s) fica só como rede de segurança
  para um pedido que se perca.

### Ondas de intra-refresh (opcional, `fluxo-host --refresh N`)
Em vez de um keyframe, o host redesenha a imagem em faixas ao longo de N
frames (NVENC, `forceIntraRefreshWithFrameCnt`, com 1 quadro de referência).
O primeiro frame da onda leva o bit 1 de `flags`; o último, o bit 2. Um
decoder que perdeu algo antes do início da onda fica, no fim dela, idêntico
pixel a pixel a um que não perdeu nada (medido em 48 de 48 recuperações).

Medido: a onda recupera **mais devagar** que um keyframe (N frames, e
precisa de N frames seguidos sem perda), e os keyframes do NVENC já são
pequenos (VBV de 1 frame). Com 3% de perda de pacotes sem FEC, a imagem ficou
congelada em média 231 ms com ondas de 4, contra 31 ms com keyframe. Por
isso o padrão é keyframe.

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

### PairInit (`kind = 7`) / PairReply (`kind = 8`) — pareamento
Sempre em claro. Transformam o código de pareamento (8 caracteres que o
cliente mostra e quem compartilha digita) numa chave de sessão, com SPAKE2
(Ed25519): quem não sabe o código ganha **um** palpite por tentativa e não
consegue testar códigos depois, gravando a troca.

- **PairInit** (host → cliente): `frame_id` = número da tentativa (o mesmo
  nos reenvios, a cada 250 ms até a resposta); payload de 33 bytes, a
  mensagem SPAKE2 do host (identidade `fluxo-host`).
- **PairReply** (cliente → host): mesmo `frame_id`; payload de 49 bytes, a
  mensagem SPAKE2 do cliente (identidade `fluxo-cliente`) + 16 bytes de
  confirmação (ChaCha20-Poly1305 de um bloco vazio, dado autenticado
  `fluxo: o cliente confirma a chave`, com uma chave derivada só para isso).
  O host confere e, se não bater, para com "o código não confere". Um
  PairInit repetido (mesmo conteúdo, venha de qualquer endereço: o host
  tenta todos os endereços do cliente de uma vez) recebe a mesma resposta.
- **Chaves:** HKDF-SHA256 do segredo do SPAKE2 (sal `fluxo protocolo v8`)
  dá uma chave por sentido (`host -> cliente`, `cliente -> host`) e a de
  confirmação. Cada pareamento gera chaves novas.

## 2.1. Cifra e endereços

Com código de pareamento (o padrão), **todo** pacote depois do pareamento —
vídeo, paridade, pings, pongs, relatórios, pedidos — vai cifrado:

- ChaCha20-Poly1305 com a chave do sentido; nonce = `seq` (12 bytes: 8
  zeros + `seq`). O cabeçalho inteiro é o dado autenticado: visível, mas
  qualquer mudança é detectada. O payload sai cifrado com a tag de 16 bytes
  no fim, e o bit 3 de `flags` ligado.
- **Anti-replay:** o receptor guarda o maior `seq` visto e um mapa dos 64
  anteriores; pacote repetido ou mais de 64 atrás é recusado (como no IPsec
  e no DTLS). Depois de 2³² pacotes num sentido (~10 dias a 5 000/s), é
  preciso parear de novo.
- **Endereços:** o host envia para um `IP:porta` informado à mão ou achado
  pelo servidor de sinalização (seção 2.2) — o endereço que respondeu ao
  pareamento. O cliente só aceita pacotes que decifrem com a sessão; o endereço do host passa a
  ser o do último pacote autenticado (se o NAT mudar a porta, a conexão
  segue). Um PairInit novo cria uma sessão **candidata**, que só substitui
  a atual quando um pacote decifrar com ela — um intruso sem o código não
  derruba a conexão.
- **Sem cifra** (`--sem-cifra` nos dois lados, só para teste): pacotes em
  claro; o cliente aceita o primeiro endereço que mandar um pacote válido e,
  após 2 s sem pacotes dele, aceita um novo. Os dois modos não se misturam:
  cada lado descarta o que vier no modo do outro.

## 2.2. Sinalização e furo de NAT (redes diferentes)

Quando host e cliente estão em redes diferentes, um não sabe o endereço
público do outro, e o NAT de cada um descarta pacotes que não respondem a
algo que saiu. Um **servidor de sinalização** (`fluxo-sinal`, num endereço
público) apresenta os dois; depois eles falam direto. Vídeo nunca passa
pelo servidor.

```text
cliente                       servidor                        host
 Register{sala, locais} ─────►  guarda a sala
 ◄──────── Registered{público}
                                ◄──────────── Lookup{sala, locais}
 ◄──── Found{host: público, locais}  Found{cliente: público, locais} ────►
 Punch ──► cada endereço do host        PairInit ──► cada endereço do cliente
 (abre o NAT do cliente)                (abre o NAT do host)
 ◄─────────────────────────── PairInit ───────────────── (agora passa)
 PairReply ─────────────────────────────────────────────►  sessão; vídeo
```

- **Signal** (`kind = 9`, sempre em claro): `frame_id` = número do pedido,
  ecoado na resposta. Payload:

  | Byte 0 | Mensagem | Sentido | Resto do payload |
  |---:|---|---|---|
  | 1 | Register | cliente → servidor | sala (u32), lista de endereços locais; completado com zeros até 100 bytes |
  | 2 | Lookup | host → servidor | idem |
  | 3 | Registered | servidor → cliente | endereço público do cliente |
  | 4 | Found | servidor → os dois | endereço público do outro lado, lista dos locais dele |
  | 5 | NotFound | servidor → host | — (sala vazia) |
  | 6 | RoomTaken | servidor → cliente | — (outro cliente na sala) |

  Endereço: família (1 byte, 4 ou 6), IP (4 ou 16 bytes), porta (u16).
  Lista: quantidade (1 byte, até 4) e os endereços. Pedidos com menos de 100
  bytes são ignorados; toda resposta é menor, para o servidor não servir de
  amplificador contra um endereço forjado.
- **Sala:** os 20 bits mais altos do SHA-256 de `"fluxo sala v1"` + código.
  O servidor nunca vê o código; vendo a sala, sobram ~20 bits que ele não
  deduz, e o SPAKE2 só aceita um palpite por pareamento (quem controla o
  servidor engana 1 vez em ~800 mil). A sala expira 30 s depois do último
  Register (o cliente renova a cada 5 s, o que também mantém vivo o
  mapeamento do NAT). Outro IP na mesma sala recebe RoomTaken; o mesmo IP
  em outra porta toma o lugar (o mesmo PC reabriu).
- **Endereço público:** o de onde o pedido chegou ao servidor, pelo mesmo
  socket que depois leva o vídeo — o servidor faz o papel de STUN, e o
  mapeamento é exatamente o que interessa.
- **Punch** (`kind = 10`, cliente → host, sem payload, em claro): abre o
  NAT do cliente na direção do host. O cliente manda um a cada endereço do
  host a cada Found (o host repete o Lookup a cada 250 ms até parear). O host
  ignora, mas um Punch vindo de um endereço que ele não conhecia vira
  candidato (NAT que muda a porta conforme o destino).
- **Host:** manda PairInit a todos os candidatos (até 8) a cada 250 ms; o
  primeiro PairReply de um candidato que confere define o destino. Um
  candidato que responde com outra chave (outro cliente Fluxo que calhou de
  estar naquele endereço) sai da lista, e o handshake recomeça com os
  outros.
- **Limites:** NAT simétrico dos dois lados (porta nova a cada destino)
  não fura assim: fica para o relay (fase 5). O servidor deve ficar fora da
  rede do cliente; na mesma rede, o endereço "público" visto é o local.

## 2.3. Sem servidor: código de conexão e STUN público

Quando não há servidor de sinalização, as pessoas levam os endereços pelo
chat que já usam. Nada muda nos pacotes do Fluxo: furo (Punch) e pareamento
são os da seção 2.2; só muda quem entrega os endereços.

```text
cliente: STUN → endereços públicos (IPv4, IPv6) ─► código de conexão ──(chat)──► host
host:    STUN → endereços públicos ─► código de resposta ─(chat)──► cliente
host:    PairInit a cada 250 ms para o cliente (abre o NAT do host)
cliente: Punch a cada 250 ms para o host (abre o NAT do cliente) → PairInit passa
```

- **STUN** (RFC 5389, só o *Binding Request*): o cliente e o host perguntam
  a servidores públicos (`stun.l.google.com:19302`,
  `stun.cloudflare.com:3478`, `stun1.l.google.com:19302`) o endereço
  público **do socket que vai levar o vídeo**, pelo IPv4 e, se a máquina
  tiver, pelo IPv6 (outro socket). Se os servidores veem portas diferentes,
  o NAT é simétrico, e o programa avisa. Pacotes STUN não são do Fluxo:
  quem recebe os ignora.
- **Código de conexão** (cliente → host): 8 símbolos do código de pareamento
  + os endereços + 1 de conferência. **Código de resposta** (host →
  cliente): os endereços + 1 de conferência. Mesmo alfabeto de 31 símbolos
  do código de pareamento. Os endereços são um número em base 31 (símbolo
  mais significativo primeiro), feito dos bytes big-endian:

  | Bytes | Conteúdo | Símbolos |
  |---|---|---|
  | `[marca][IPv4 4][porta 2]` | só IPv4 | 10 (conexão: 19 ao todo; resposta: 11) |
  | `… + [IPv6 16][porta 2]` | IPv4 e IPv6 | 39 (conexão: 48; resposta: 40) |

  A marca é 1 quando a porta IPv4 foi aberta no roteador por UPnP (só no
  código de conexão), 0 nos outros casos; valores maiores são recusados.
  IPv4 `0.0.0.0:0` = sem IPv4. O host só põe IPv6 na resposta se o código de
  conexão tinha IPv6 (senão não serviria, e a resposta fica com 11).
  Conferência: `Σ pᵢ·símbolo_i mod 31` com pesos `pᵢ = (i mod 30) + 1` (de
  1 a 30, nunca 31, que seria 0): pega qualquer símbolo trocado e qualquer
  par de vizinhos invertido, em qualquer tamanho.
- **Host:** abre um socket IPv4 e um IPv6 e manda PairInit a todos os
  endereços do código (IPv6 primeiro) por até 5 min (tempo de o código de
  resposta ir e voltar); o vídeo segue pelo que responder primeiro. No IPv4,
  os pedidos vão com TTL baixo (12; `--ttl-furo`) até o primeiro Punch
  chegar, para não chegarem ao NAT do cliente antes do furo dele (ver
  `modulos/fluxo-net.md`); com a marca de UPnP, vão com TTL normal desde o
  começo. No IPv6 não há NAT para ocupar a porta, e o TTL é sempre o
  normal. Um Punch de um endereço desconhecido vira candidato, como na
  seção 2.2.
- **Cliente:** escuta nos dois sockets até o host aparecer; depois só no da
  família que pareou. Enquanto nenhum host chega, repete o pedido STUN a
  cada 15 s para o NAT não esquecer o mapeamento que está no código.
  Recebido o código de resposta, manda Punch a cada 250 ms para cada
  endereço dele por até 1 min, ou até o host aparecer.
- **UPnP** (IGD; descoberta SSDP em `239.255.255.250:1900`, comandos SOAP):
  o cliente abre a sua porta IPv4 no roteador (`AddPortMapping`, UDP,
  validade de 1 h, apagada ao sair) **só** se o IP externo do roteador for
  o mesmo que o STUN viu; atrás de CGNAT (o roteador vê um IP privado, ex.
  `100.64.x.x`) não pede nada. `--sem-upnp` desliga.
- **Limites:** NAT simétrico dos dois lados no IPv4, sem IPv6 de um dos
  lados, não fura. Com os dois atrás do mesmo roteador, o IPv4 só funciona
  se ele fizer *hairpin* (o IPv6, se houver, passa; na mesma rede, use
  `--destino`).

## 2.4. Cursor (`kind = 11`)

O Desktop Duplication entrega a imagem **sem** o ponteiro do mouse. Em vez
de desenhá-lo na imagem (o que custaria um frame inteiro a cada movimento e
o atrasaria até o próximo encode), o host manda o ponteiro à parte e o
cliente o desenha por cima do vídeo. Cifrado como os outros pacotes.

| Byte 0 | Mensagem | Sentido | Resto do payload |
|---:|---|---|---|
| 1 | Posição | host → cliente | `x` i16, `y` i16 (canto superior esquerdo da imagem do ponteiro, pixels do monitor), visível u8, número da forma u16 (0 = nenhuma) — 8 bytes |
| 2 | Parte da forma | host → cliente | número u16, parte u8, total de partes u8, largura u16, altura u16, ponto ativo x/y u16; depois até 1167 bytes da imagem BGRA (alfa não pré-multiplicado, linha a linha) |
| 3 | Pedido de forma | cliente → host | número u16 |

- **Posição:** sai a cada movimento (o Desktop Duplication avisa até a
  frequência do monitor) e, de segurança, junto com cada Ping (500 ms).
- **Forma:** ganha um número novo a cada troca e sai inteira, em partes
  (lado máximo 256 px). O cliente só monta a mais recente; se a posição
  aponta uma forma que ele não tem, pede de novo (no máximo a cada 200 ms),
  e o host reenvia (no máximo a cada 100 ms).
- **Formas "inverter":** o cursor de texto (I) e outros invertem o que está
  embaixo, o que não dá para fazer desenhando por cima; o host os converte
  em preto com contorno branco.

## 2.5. Áudio (`kind = 13`)

O som que o PC do host toca (todos os programas, capturado por WASAPI
loopback), em Opus 48 kHz estéreo, **um quadro de 10 ms por pacote**,
cifrado como os outros. Sai na hora, fora da fila do vídeo (não espera
pacing nem frames grandes).

- `frame_id` = número do quadro (sobe de 1 em 1; o cliente ordena e vê
  perdas); `timestamp_us` = captura da primeira amostra, no relógio do host.
- Payload = o pacote Opus (VBR, ~160 bytes a 128 kbps; com FEC dentro: um
  quadro perdido pode ser refeito a partir do seguinte).
- Nada tocando no host: quadros de silêncio no mesmo ritmo (poucos bytes).
- Cliente: buffer de jitter (começa em 3 quadros, cresce se esvaziar e
  diminui depois de 10 s estável), FEC do Opus para o perdido quando o
  seguinte já chegou, ocultação de perda (o Opus inventa) quando não.

### Reservado: Input (`kind = 12`)

Para o controle remoto (mouse e teclado do cliente no host, fase 6,
opcional): cliente → host, cifrado, com mouse em coordenadas do vídeo (as
mesmas do cursor) e teclas por *scan code*. Ainda não implementado: o host
de hoje recusa o tipo (desconhecido), e o formato só será fixado quando
houver permissão explícita de quem compartilha.

## 3. Fragmentação (emissor)

- Payload máximo por pacote: **1180 bytes** (`DEFAULT_MAX_PAYLOAD`). Com o
  cabeçalho de 28, a tag de 16 e os cabeçalhos IP/UDP fica abaixo de 1280, o
  MTU mínimo do IPv6, evitando fragmentação IP (ADR-005).
- `frag_count = ceil(tamanho_do_frame / 1180)`, mínimo 1 (frame vazio gera
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

- ~~**FEC mais forte** (Reed-Solomon)~~ — medido e descartado: no Wi-Fi de
  teste as perdas são rajadas longas de fila cheia (ver `modulos/fluxo-net.md`),
  que 2 paridades por grupo não salvam; quem resolve é o bitrate adaptativo.
- ~~**Criptografia** e **handshake**~~ — feitos na versão 8 (seções 2 e 2.1).
  Um ID de sessão no cabeçalho não foi preciso: pacote de outra sessão não
  decifra.
- ~~**Sinalização e furo de NAT**~~ — feitos na versão 9 (seção 2.2).
- **Relay** para quando o furo não passa (NAT simétrico dos dois lados,
  CGNAT restritivo): o servidor repassaria os pacotes já cifrados, sem a
  chave.

## 6. Histórico de versões

| Versão | Mudança |
|---:|---|
| 1 | Video, Ping, Pong. |
| 2 | + KeyframeRequest (`kind = 4`). Pacotes da versão 1 são recusados. |
| 3 | + Parity (`kind = 5`, FEC); byte 13 vira `fec_group`. Pacotes das versões 1 e 2 são recusados. |
| 4 | + Feedback (`kind = 6`), relatório para o bitrate adaptativo. Versões anteriores recusadas. |
| 5 | Pong leva 4 bytes com o relógio de quem responde (sincronização). Versões anteriores recusadas. |
| 6 | Cabeçalho de 24 bytes: + `send_us` (hora de saída de cada pacote), base do atraso de fila. Versões anteriores recusadas. |
| 7 | Flags de onda de intra-refresh no vídeo (bits 1 e 2); KeyframeRequest com bit 0 = "só keyframe serve". Versões anteriores recusadas. |
| 8 | Cabeçalho de 28 bytes (+ `seq`), PairInit e PairReply (pareamento SPAKE2), payload cifrado com ChaCha20-Poly1305 (bit 3 de `flags`) e anti-replay; payload máximo 1180. Versões anteriores recusadas. |
| 9 | + Signal (`kind = 9`, servidor de sinalização) e Punch (`kind = 10`, furo de NAT); um PairInit repetido é reconhecido pelo conteúdo, venha de onde vier. Versões anteriores recusadas. |
| 10 | + Cursor (`kind = 11`): posição e forma do ponteiro à parte do vídeo, pedido de forma perdida. `kind = 12` reservado para o controle remoto. Versões anteriores recusadas. |
| 11 | + Audio (`kind = 13`): o som do host em Opus, um quadro de 10 ms por pacote. Versões anteriores recusadas. |
