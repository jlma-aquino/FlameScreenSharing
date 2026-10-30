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
| fps | Frames codificados e enviados | ~60 com vídeo/jogo |
| Mbps | Bitrate real | ≤ `--mbps` |
| encode | Tempo médio de conversão + encode | < 5 ms |
| envio | Tempo do primeiro ao último pacote do último frame (pacing) | < 10 ms |
| fila | Frames esperando a rede | 0–1; crescendo = rede não dá conta |
| RTT | Ida e volta medida por ping | LAN cabeada < 1 ms; Wi-Fi 2–10 ms |

## Decisões provisórias

- **Destino manual.** Sem descoberta nem NAT (fase 5).
- **Keyframe periódico** a cada 2 s (`--keyframe-a-cada`), para um receptor
  que entrar depois conseguir começar. Vira pedido do cliente na fase 4.
- **Sem limite de fps.** Num monitor de 144/165 Hz, animações geram mais de
  60 frames por segundo e o host codifica todos. Limitar exige guardar o
  último frame para não perder a imagem final de uma animação (no roadmap).
- **Captura e encode na mesma thread.** O encode é síncrono e leva ~2,5 ms,
  então separar ainda não compensa; a rede já tem as suas threads.
- **Mudança de resolução** recria o encoder (o novo começa com keyframe).
