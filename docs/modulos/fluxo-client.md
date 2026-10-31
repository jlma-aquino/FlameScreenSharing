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

## Limitações conhecidas

- A janela não toma o foco se for aberta por outro programa em segundo
  plano: clique nela na barra de tarefas ou use `--no-topo`.
- Sem áudio, mouse ou teclado remotos (fase 6).
