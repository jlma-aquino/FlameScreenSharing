# fluxo-host

**Caminho:** `apps/fluxo-host` · **Plataforma:** Windows

Lado de quem compartilha a tela: captura (`fluxo-capture`) → encode
(`fluxo-encode`) → rede (`fluxo-net`).

## Uso

```text
fluxo-host (--destino IP:PORTA | --sinal SERVIDOR) --codigo XXXX-XXXX [--monitor N] [--encoder auto|nvenc|mf]
           [--codec auto|h264|hevc|av1] [--resolucao R]
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

## Codec

O encoder só abre depois do pareamento: o cliente diz que codecs
decodifica (junto com os pings) e o host usa o melhor que a GPU também
codifica, na ordem AV1, HEVC, H.264 (ADR-012). A linha do encoder mostra o
escolhido (`NVENC AV1 (...)`); um codec que a GPU recusar aparece como
`(sem AV1 nesta GPU: ...)` antes de tentar o próximo. `--codec h264` (ou
`hevc`, `av1`) força um; se o cliente não o decodifica, o host para e diz o
que ele decodifica.

## Vários espectadores

Com o host rodando, mais gente entra pela entrada padrão (é o que o botão
"Adicionar espectador" da janela faz), até 4 no total:

```text
conexao CÓDIGO-DE-CONEXÃO
destino IP:PORTA XXXX-XXXX
```

Cada um pareia à parte, em sockets próprios. Pela internet, aparece
`Código de resposta (espectador N): …`. Depois, `Espectador N conectou (M no
total)` e `Espectador N saiu (M no total)` (15 s sem nenhum pacote dele). Decisão
e efeitos: ADR-014. Os **Mbps do relatório são de uma cópia**: o upload real é
isso vezes o número de espectadores.

Medido com 4 `fluxo-bench receber --verificar` na mesma máquina, 34 s: 0 frames
perdidos, 1563 de 1563 e 1083 de 1083 decodificados nos dois que chegaram ao
resumo, encode igual (2,2 ms), captura→chegada p50 ~11 ms. Um espectador só com
H.264 entrando obrigou a troca de AV1 para H.264, sem frame perdido para quem já
assistia. O quinto foi recusado, e o que caiu saiu da lista.

## Encoder por software

Sem encoder de hardware (ou com `--encoder software`), o vídeo vai em H.264
pelo OpenH264 na CPU. Os padrões caem para **30 fps** e no máximo **1080
linhas** (uma tela 4K vira 1080p), porque uma VM com poucos núcleos não
acompanha mais que isso; `--fps` e `--resolucao` pedem outros valores. Aqui
(16 núcleos) o frame 1080p leva ~8 ms, o que daria 60 fps.

## Resolução e fps

`--resolucao 720p|1080p|1440p|4k|N` transmite em outra resolução que a da
tela, mantendo a proporção: a conversão BGRA→NV12 do encoder redimensiona
na mesma passada da GPU (`EncoderConfig::input_size`). A linha
"Transmitindo" mostra `1920x1080 → 3840x2160`. Sem `--mbps`, o bitrate
máximo acompanha resolução × fps: 20 Mbps em 1080p60, 40 em 1080p120 ou
1440p60 (35,6), 80 em 4K60, até 100. `--fps 120` só gera 120 frames por
segundo se a tela atualizar nesse ritmo (monitor de 120 Hz+ e conteúdo
rápido).

Medido (mesma máquina, RTX 5050, `receber --verificar`, 0 perdas em todos):

| | Encode por frame | Captura→chegada p50 | Decodificados |
|---|---|---|---|
| 1440p60 AV1 | — | 9,1 ms | 301 de 301 (2560x1440) |
| 4K60 AV1 | 7,0 ms | 15,6 ms | 301 de 301 (3840x2160) |
| 4K60 H.264 | 7,5 ms | 18,3 ms | 301 de 301 |
| Rede a 120 fps (`fluxo-bench enviar`), 40 e 100 Mbps | — | 3–4 ms | 600 de 600 |

Em 4K o encode (~7 ms, preset P4) quase ocupa o intervalo de um frame a
120 fps (8,3 ms): 4K120 fica no limite desta GPU.

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
