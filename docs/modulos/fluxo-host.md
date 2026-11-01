# fluxo-host

**Caminho:** `apps/fluxo-host` · **Plataforma:** Windows

Lado de quem compartilha a tela: captura (`fluxo-capture`) → encode
(`fluxo-encode`) → rede (`fluxo-net`).

## Uso

```text
fluxo-host (--destino IP:PORTA | --sinal SERVIDOR) --codigo XXXX-XXXX [--monitor N] [--encoder auto|nvenc|mf]
fluxo-host --conexao CÓDIGO-DE-CONEXÃO [opções]
           [--mbps M] [--fps F] [--pacing-mbps P] [--keyframe-a-cada S] [--refresh N] [--fec K]
           [--segundos S] [--porta-local P] [--sem-cifra]
```

O receptor ([fluxo-client](fluxo-client.md), ou o `fluxo-bench receber`
sem janela) mostra um **código de pareamento** e o comando pronto:

```powershell
# No PC que vai assistir (ou outro terminal, para teste local)
.\target\release\fluxo-client.exe --porta 9000
# No PC que compartilha, com o código que apareceu lá
.\target\release\fluxo-host.exe --destino 192.168.0.20:9000 --codigo K7M4-QX9P
```

O host espera até 30 s pelo pareamento; com o código errado, para na hora
("o código de pareamento não confere"). Pareado, só esse cliente recebe e
tudo vai cifrado (ver [fluxo-net](fluxo-net.md), "Pareamento e cifra").
`--sem-cifra` (nos dois lados) desliga tudo isso, só para teste. Na mesma
máquina, use `--destino 127.0.0.1:9000`.

Em redes diferentes, os dois usam um servidor de sinalização
([fluxo-sinal](fluxo-sinal.md)) no lugar do endereço:

```powershell
.\target\release\fluxo-client.exe --sinal meu-servidor.com
.\target\release\fluxo-host.exe --sinal meu-servidor.com --codigo K7M4-QX9P
```

O host acha o cliente pela sala do código, os dois furam os NATs e o vídeo
vai direto; a saída mostra "Procurando o cliente pelo servidor…" e depois o
endereço que respondeu. Se não der, o erro diz onde parou (servidor mudo,
nenhum cliente com esse código, ou pacotes barrados pelos NATs).

**Sem servidor nenhum** (ADR-010): o cliente roda `fluxo-client --internet`
e manda o código de conexão pelo chat; aqui:

```powershell
.\target\release\fluxo-host.exe --conexao K7M4-QX9P-ABCD-EFGH-JK
#   Código de resposta: ABCD-EFGH-JKM   ← mande de volta; ele cola no terminal do cliente
```

O código de conexão já traz o código de pareamento (sem `--codigo`). O host
espera até 5 min pelo código de resposta ir e voltar, e avisa se esta rede
tem NAT simétrico. Se os dois lados têm IPv6, o código de conexão já traz o
endereço IPv6 do cliente, e o host costuma conectar por ele na hora, antes
da resposta (no teste com o notebook na rede do celular: tela real a 60 fps
e 9 Mbps, sem perda, RTT 58 ms).

## Firewall

Na primeira execução do `fluxo-bench receber` (ou de qualquer programa que
escute numa porta), o Windows pode perguntar se libera o acesso à rede. Para
receber de **outro PC** é preciso permitir em "Redes privadas". Na mesma
máquina (127.0.0.1) funciona mesmo sem permitir.

## Lendo a saída (a cada segundo)

| Campo | Significado | Bom |
|---|---|---|
| fps | Frames codificados e enviados | ~60 com vídeo/jogo; nunca acima de `--fps` |
| capturas adiadas | Capturas que chegaram antes do horário (limite de fps) | 0 com fonte de 60 Hz; ~100/s com animação a 165 Hz |
| Mbps | Bitrate real | ≤ `--mbps` |
| encode | Tempo médio de conversão + encode | < 5 ms |
| envio | Tempo do primeiro ao último pacote do último frame (pacing) | < 10 ms |
| fila | Frames esperando a rede | 0–1; crescendo = rede não dá conta |
| RTT | Ida e volta medida por ping | LAN cabeada < 1 ms; Wi-Fi 2–10 ms |

## Limite de fps (`src/limite.rs`)

Em monitores de 144/165 Hz a captura entrega mais frames que o `--fps`.
Codificar todos multiplica o bitrate (medido: 162 fps e ~60 Mbps com 20 de
alvo); descartar os excedentes perderia a imagem final de uma animação.

- Os envios seguem uma grade de horários a cada `1/fps`. Captura no horário
  (ou até 10% de intervalo antes) sai na hora.
- Captura adiantada vira **pendente**: uma cópia na GPU (microssegundos).
  Uma captura mais nova substitui a pendente; no horário seguinte, a
  pendente sai — a última imagem sempre chega.
- A espera pela captura seguinte é limitada ao horário da pendente.

Custo: quando a tela muda mais rápido que o limite, a imagem espera em média
meio intervalo (~8 ms a 60 fps). Com conteúdo a 60 fps nada é adiado.

Medido (cliente na tela capturada, monitor de 165 Hz): 59,8 fps médio,
15 Mbps, 1372 capturas adiadas em 14 s, 0 frames perdidos.

## Decisões provisórias

- **Áudio** (padrão ligado): uma thread captura o que o PC toca (WASAPI
  loopback da saída padrão, ou `--audio-dispositivo N`) e manda em Opus,
  128 kbps, um quadro de 10 ms por pacote, pelo mesmo `VideoSender`
  ([fluxo-audio](fluxo-audio.md)). `--sem-audio` desliga. O resumo mostra
  quantos segundos vieram da placa (o resto é silêncio completado).
- **Destino manual ou pela sinalização.** `--destino` serve na mesma rede;
  em redes diferentes, `--sinal SERVIDOR` acha o cliente pelo
  [fluxo-sinal](fluxo-sinal.md) e fura os NATs (ver [fluxo-net](fluxo-net.md)).
  NAT simétrico dos dois lados ainda não passa (falta o relay).
- **Keyframes sob pedido.** O cliente pede um keyframe quando perde algo
  (ou ao começar); o host força keyframe no próximo encode e, com a tela
  parada, recodifica a última imagem enviada (guardada como cópia na GPU).
  O keyframe periódico (`--keyframe-a-cada`, padrão 10 s) fica só como rede
  de segurança para um pedido que se perca.
- **`--refresh N`** (opcional, NVENC): responde a um pedido de limpeza
  depois de perda com uma onda de intra-refresh de N frames em vez de
  keyframe; com a tela parada, recodifica a última imagem a cada frame até
  a onda acabar. Desligado por padrão: no NVENC os keyframes já são pequenos
  e a onda recupera bem mais devagar (3% de perda sem FEC: 231 ms congelado
  em média com ondas de 4, contra 31 ms com keyframe).
- **Largada aquecida.** O primeiro ping sai antes do primeiro frame, e o
  envio espera até 50 ms pela resposta, para o sistema resolver o endereço
  do cliente (ARP) antes da primeira rajada.
- **Bitrate adaptativo** (padrão ligado): `--mbps` vira o teto, `--mbps-min`
  o piso (padrão teto/8). Os relatórios do cliente decidem; quedas aparecem
  na saída (`↓ bitrate 9.8 Mbps (perda 54%)`), e o relatório por segundo
  mostra alvo, perda e fila. `--bitrate-fixo` desliga.
- **`--simular-banda M`** (teste): simula um link de M Mbps com fila de ~4
  frames e descarte quando enche.
- **`--simular-atraso MS` e `--simular-jitter MS`** (teste): somam MS de
  atraso fixo e um extra sorteado entre 0 e MS (trocado a cada 20 ms) em
  **cada sentido**. Com `--simular-atraso 15 --simular-jitter 20` na mesma
  máquina: RTT ~52 ms, latência p50 36,6 ms, fila medida 7–12 ms, sem
  perdas e sem queda de bitrate.
- **FEC** (`--fec K`, padrão 10): 1 pacote de paridade a cada K de dados;
  o cliente reconstrói uma perda por grupo sozinho. 0 desliga.
- **`--simular-perda P`** (teste): descarta P% dos pacotes de vídeo antes de
  enviar, para testar a recuperação sem precisar de uma rede ruim.
- **Captura e encode na mesma thread.** O encode é síncrono e leva ~2,5 ms,
  então separar ainda não compensa; a rede já tem as suas threads.
- **Mudança de resolução** recria o encoder (o novo começa com keyframe).
