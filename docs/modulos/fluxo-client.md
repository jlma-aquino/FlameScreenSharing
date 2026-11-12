# fluxo-client

**Caminho:** `apps/fluxo-client` · **Plataforma:** Windows 10+

Lado de quem assiste: rede (`fluxo-net`) → decode (`fluxo-decode`) → tela.

## Uso

```text
fluxo-client [--porta P] [--vsync] [--segundos S] [--largura L] [--altura A] [--no-topo]
             [--sem-painel] [--codigo XXXX-XXXX] [--sem-cifra] [--sinal SERVIDOR] [--internet [--sem-upnp]]
```

`--no-topo` mantém a janela sempre por cima das outras: útil para acompanhar
enquanto usa o PC, e necessário quando o cliente é aberto por outro programa
em segundo plano (o Windows não deixa essa janela tomar o foco).

Ao abrir, o cliente sorteia um **código de pareamento** e mostra no
terminal, no título e no painel, junto com o comando pronto para o host (com
o IP provável deste PC, descoberto sem mandar nenhum pacote). Só um host com
esse código consegue transmitir, e tudo vai cifrado; o painel e o título
mostram "cifrado" durante a transmissão. `--codigo` fixa um código (útil em
scripts); `--sem-cifra` desliga o pareamento, só para teste.

Numa sala, `--sala CÓDIGO` usa o código de quem compartilha (`fluxo-host
--sala`) e entra pelo servidor da chave (`--chave FSS1-…`, a mesma do host): nada a mandar de
volta. Pareado, o cliente sai da sala, e a vaga fica para outro.

Em redes diferentes, `--sinal SERVIDOR` registra o código num servidor de
sinalização ([fluxo-sinal](fluxo-sinal.md)) e o comando mostrado vira
`fluxo-host --sinal SERVIDOR --codigo ...`: o host acha este PC pelo
servidor, sem precisar do IP nem de abrir porta no roteador.

Sem servidor nenhum, `--internet` mostra um **código de conexão**
(`K7M4-QX9P-ABCD-EFGH-JK`: pareamento + o endereço público deste PC, visto
por servidores STUN públicos). Mande para quem compartilha; ele roda
`fluxo-host --conexao <código>` e manda de volta um **código de resposta**
(`ABCD-EFGH-JKM`), que você cola no terminal do cliente. Os dois furam os
NATs e o vídeo vai direto. Erro de digitação é avisado na hora. Se este PC
tem IPv6, o código leva o endereço IPv6 também (48 símbolos), e o host
costuma conectar por ele antes mesmo da resposta; se o roteador tem UPnP e
não há CGNAT, a porta é aberta nele (`--sem-upnp` desliga) e basta um
código.

```powershell
# No PC que assiste
.\target\release\fluxo-client.exe --porta 9000
#   Código de pareamento: K7M4-QX9P
#   No PC que compartilha:  fluxo-host --destino 192.168.1.2:9000 --codigo K7M4-QX9P
# No PC que compartilha
.\target\release\fluxo-host.exe --destino 192.168.1.2:9000 --codigo K7M4-QX9P
```

Na janela: **F11** ou **Alt+Enter** alterna tela cheia sem bordas; **Esc**
sai da tela cheia; **F2** mostra/esconde o painel de estatísticas sobre a
imagem (`--sem-painel` começa escondido). Teste na mesma máquina (`--destino 127.0.0.1:9000`)
funciona, mas a janela aparece dentro da própria imagem ("espelho infinito").

Com o vídeo rodando, `volume N` (0 a 100) na entrada padrão muda o volume na
hora e o cliente confirma com `Volume: N%` (é o controle de volume e o mudo da
janela do FSS). Funciona também no cliente portátil.

## No Linux

Com `--features portatil`, `src/portatil/` substitui o caminho do Windows (ADR-015):
`video.rs` (rede → OpenH264), `tela.rs` (janela egui, shader I420 → RGB, cursor e
painel) e `som.rs` (buffer de jitter e Opus, com o PCM indo por pipe ao tocador).
Mesmas opções, teclas e saída no terminal. No Windows, `--portatil` usa este caminho,
para teste.

Notebook Linux (2 núcleos, Intel HD, Wi-Fi, OpenH264 com NASM), vídeo em tela cheia
no host:

| Transmissão | Mostrados | Decode (média) | Captura→tela p50 |
|---|---|---|---|
| 720p60, ~5 Mbps | 32,6 fps | 15,5 ms | 21,0 ms |
| 720p30, ~5 Mbps | 27,9 fps | 18,9 ms | 28,7 ms |
| 720p30, ~18 Mbps | 25,7 fps | 38,8 ms | ~1,3 s: não acompanha |
| 1080p30, ~6 Mbps | 24,4 fps | 34,7 ms | ~0,8 s: não acompanha |
| 1080p60, ~12 Mbps | 20,4 fps | 25,7 ms | ~1,5 s: não acompanha |

Essa tabela é com bitrate escolhido à mão. No uso normal, o cliente mede a carga do
decode e o host se ajusta (ADR-015): 1080p60 com vídeo em tela cheia, 35 s, chegou a
33 fps mostrados, decode de 15 ms e captura→tela p50 de 22,5 ms (p99 292 ms), a 2,5
Mbps, com 1 descarte de fila. A imagem acompanha, mas em bitrate baixo; nesse PC,
720p dá imagem melhor.

O custo do decode segue o bitrate. Sem NASM, 1080p custou 42–59 ms por frame, e com
ele 26–45 ms (25–40% a menos, em conteúdo parecido).

## Arquivos

| Arquivo | Conteúdo |
|---|---|
| `src/main.rs` | Opções e a thread da janela (mensagens, teclas, título) |
| `src/exibir.rs` | Thread de exibição: rede → decode → tela, estatísticas, resumo |
| `src/window.rs` | Janela Win32: redimensionar, tela cheia, fechar, F2 |
| `src/present.rs` | Swapchain e Video Processor (NV12 → tela, escala, barras pretas) |
| `src/overlay.rs` | Painel de estatísticas sobre a imagem (Direct2D + DirectWrite) |

## Threads

```text
janela (main)                          exibição (exibir.rs)
 mensagens do Windows                   recv_frame(5 ms) ──► decodifica TODOS os frames
 F11/Esc → tela cheia                                        (cada P-frame depende do anterior)
 WM_SIZE ──── tamanho novo ──────────►                   ──► mostra só o mais novo
 F2 ───────── painel liga/desliga ───►                       (se atrasou, alcança)
 X ────────── fechar ────────────────►  1×/s: texto do painel e do título
 título ◄──── estatísticas ───────────
```

Quando a janela é arrastada ou redimensionada, o Windows prende a thread
dela num laço interno até soltar o mouse. Com decode e apresentação nessa
thread, a imagem parava e os frames se acumulavam: medido segurando a janela
3 s, um frame esperou **5,6 s** entre chegar e ser processado. Com a thread
de exibição, a maior espera ficou em **22 ms**. Device, decoder e swapchain
são criados e usados só na thread de exibição (o contexto do D3D11 não é
seguro entre threads); a janela só repassa comandos.

Quem decide o que fazer com cada frame é o `fluxo_net::Recovery`:

- Sem imagem decodificável (largada), o cliente pede um keyframe ao host
  (`request_keyframe`) e pula frames até ele chegar.
- **Perda:** um frame que chega com `preceded_by_loss` depende de um que não
  chegou. O cliente **congela a última imagem boa** e pede limpeza
  (`request_refresh`). Continua decodificando, sem mostrar, até chegar um
  keyframe ou o fim de uma onda de intra-refresh (`fluxo-host --refresh N`)
  que começou depois da perda — nada de imagem corrompida na tela. O resumo
  mostra quantas perdas houve e quanto tempo a imagem ficou congelada
  (medido com 1% de perda: 38 ms em média com keyframe).
- Se o decode falhar (bitstream estragado), o decoder é recriado e pede
  keyframe.

## Apresentação de baixa latência

- Swapchain **flip model** (`FLIP_DISCARD`), 2 buffers, no máximo 1 frame na
  fila do driver.
- Sem `--vsync`: `Present` com **tearing permitido** quando o sistema
  suporta — o frame aparece na hora. Com `--vsync`: sem rasgos na imagem, até
  um quadro do monitor a mais de atraso.
- Conversão NV12 (BT.709 limitado) → RGB pelo Video Processor, que também
  faz escala e barras pretas mantendo a proporção.

## Lendo os números

Painel sobre a imagem (F2) e título da janela, a cada segundo. O painel
também mostra a fila (atraso de fila da rede) e os fragmentos reconstruídos
pelo FEC; custa ~0,13 ms de CPU por frame (apresentação média de 0,28 ms com
painel, 0,15 ms sem).

| Campo | Significado | Bom (mesma máquina) |
|---|---|---|
| fps | Frames mostrados | ~60 com vídeo |
| Mbps | Bitrate recebido | ≤ `--mbps` do host |
| captura→tela | Da captura no host ao `Present` aqui (relógios iguais) | < 10 ms |
| RTT | Ida e volta | < 1 ms em LAN cabeada |
| perdidos | Frames que não chegaram inteiros | 0 |

Ao fechar, o terminal mostra o resumo (percentis de captura→tela, decode,
apresentação, perdas, erros). **Espera no cliente** é quanto cada frame
esperou entre chegar pela rede e ser processado: perto de zero no normal;
um valor alto aponta travamento do cliente, não da rede.

## Áudio

O som do host toca numa thread própria ([fluxo-audio](fluxo-audio.md),
`AudioPlayer`): buffer de jitter de ~30 ms, Opus, saída de som padrão, no
ritmo da placa (sem esperar o vídeo). `--volume V` (0 a 100) muda só o
volume do Fluxo; `--sem-audio` não toca. O resumo ao fechar mostra quadros
tocados, perdidos (e quantos o FEC refez) e quantas vezes faltou som.

## Codec

O cliente anuncia ao host os codecs que este Windows decodifica
(`fluxo_decode::decodable_codecs`: H.264 sempre; HEVC e AV1 com as extensões
da Microsoft Store) e troca de decoder quando os frames chegam noutro codec
(na prática, só no primeiro). A linha "vídeo em AV1 | decoder: ..." mostra
o escolhido.

## Cursor

O ponteiro do host é desenhado por cima do vídeo (Direct2D, antes do
painel), na escala da imagem e recortado à área dela. Quando só o cursor
mexe, a thread de exibição redesenha a **última imagem** com ele no lugar
novo, sem esperar frame novo: cada frame mostrado é copiado na GPU (~0,1
ms em 1080p) para isso. O resumo ao fechar conta os pacotes de cursor, as
formas pedidas de novo e os redesenhos sem frame novo (medido na mesma
máquina: 180 redesenhos em 6 s com o mouse em uso).

## Medições (mesma máquina, RTX 5050, 1080p, 20 Mbps)

| | Resultado |
|---|---|
| Frames (30 s, 60 fps) | 1801 recebidos, 1799 mostrados, 0 perdidos, 0 erros de decode |
| Captura → tela (60 fps) | p50 7,4 ms, p99 8,0 ms |
| Frames (12 s, ~170 fps, janela do cliente na tela capturada) | 2035 recebidos e mostrados, 0 descartados |
| Captura → tela (~170 fps) | p50 2,6 ms, p99 7,2 ms |
| Decode (CPU) | ~0,2 ms médio |

Com a janela do cliente dentro da tela capturada, cada frame mostrado muda a
tela e gera nova captura: o host passa a codificar na frequência do monitor
(até ~165 fps), não nos 60 do `--fps`. Ver a pendência de limite de fps no
roadmap.

## Barra de controles sobre o vídeo

Com o mouse sobre o vídeo aparece uma barra embaixo, como a do YouTube:
**mudo**, **volume** (arrastar, roda do mouse ou setas), **estatísticas** (o
painel do F2), **tela cheia** e **Sair**. Com o mouse parado por 2,5 s, ela
some e o cursor some junto; mexer traz de volta. Antes da primeira imagem ela
fica sempre visível (é por ela que dá para sair). Teclas: F11, F, Alt+Enter
ou clique duplo no vídeo alternam a tela cheia; Esc sai dela; M liga e
desliga o som; F2 mostra as estatísticas.

- `src/controles.rs`: onde cada peça fica (`Layout`, em pixels, na escala do
  monitor), o que cada evento do mouse faz (`Controles`) e quando a barra
  some. Sem Windows, com testes.
- `src/window.rs` anota os eventos (mover, apertar, soltar, clique duplo,
  roda, mouse saiu) e esconde o cursor; o laço da janela em `main.rs` aplica
  as ações e manda à exibição um `Desenho` (só quando muda).
- `src/overlay.rs` desenha em Direct2D, depois do painel: degradê escuro,
  ícones da fonte Segoe MDL2 Assets (vem com o Windows), trilho do volume na
  cor de destaque do FSS. A exibição redesenha na hora quando a barra muda,
  mesmo com a tela parada e antes da primeira imagem (é assim que o texto de
  espera aparece agora).

`--janela X,Y,L,A` abre a janela nessa posição e tamanho (externos, em pixels
de tela); ao fechar, o cliente imprime `Janela: X,Y,L,A`. É assim que a
janela do FSS dá o lugar ao vídeo e volta para onde ele estava. O cliente
sabe o DPI de cada monitor (antes, numa tela com escala, o Windows esticava o
vídeo, borrado).

## Limitações conhecidas

- A janela não toma o foco se for aberta por outro programa em segundo
  plano: clique nela na barra de tarefas ou use `--no-topo`.
- Sem mouse ou teclado remotos (fase 6, opcional; o tipo de pacote já está
  reservado).
