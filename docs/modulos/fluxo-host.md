# fluxo-host

**Caminho:** `apps/fluxo-host` · **Plataforma:** Windows

Lado de quem compartilha a tela: captura (`fluxo-capture`) → encode
(`fluxo-encode`) → rede (`fluxo-net`).

## Uso

```text
fluxo-host --destino IP:PORTA [--monitor N] [--encoder auto|nvenc|mf]
           [--mbps M] [--fps F] [--pacing-mbps P] [--keyframe-a-cada S]
           [--segundos S] [--porta-local P]
```

Enquanto o cliente de verdade não existe (fase 3), o receptor é o
`fluxo-bench receber`:

```powershell
# No PC que vai assistir (ou outro terminal, para teste local)
.\target\release\fluxo-bench.exe receber --porta 9000 --segundos 30 --verificar
# No PC que compartilha
.\target\release\fluxo-host.exe --destino 192.168.0.20:9000
```

Para teste na mesma máquina, use `--destino 127.0.0.1:9000`.

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

- **Destino manual.** Sem descoberta nem NAT (fase 5).
- **Keyframes sob pedido.** O cliente pede um keyframe quando perde algo
  (ou ao começar); o host força keyframe no próximo encode e, com a tela
  parada, recodifica a última imagem enviada (guardada como cópia na GPU).
  O keyframe periódico (`--keyframe-a-cada`, padrão 10 s) fica só como rede
  de segurança para um pedido que se perca.
- **Largada aquecida.** O primeiro ping sai antes do primeiro frame, e o
  envio espera até 50 ms pela resposta, para o sistema resolver o endereço
  do cliente (ARP) antes da primeira rajada.
- **`--simular-perda P`** (teste): descarta P% dos pacotes de vídeo antes de
  enviar, para testar a recuperação sem precisar de uma rede ruim.
- **Captura e encode na mesma thread.** O encode é síncrono e leva ~2,5 ms,
  então separar ainda não compensa; a rede já tem as suas threads.
- **Mudança de resolução** recria o encoder (o novo começa com keyframe).
