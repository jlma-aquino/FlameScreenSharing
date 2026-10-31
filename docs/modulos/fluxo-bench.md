# fluxo-bench

**Caminho:** `apps/fluxo-bench` · **Plataforma:** Windows

Ferramenta de linha de comando para validar captura, encode e rede isoladamente.

## Arquivos

| Arquivo | Conteúdo |
|---|---|
| `src/main.rs` | Parsing de argumentos e os comandos `listar`, `bench`, `screenshot` |
| `src/encode.rs` | Comandos `encoders`, `encode` e `decode` |
| `src/receber.rs` | Comando `receber` (receptor provisório do fluxo-host) |
| `src/verificar.rs` | `--verificar`: decoder H.264 do Windows, PSNR, NV12→BGRA |
| `src/bmp.rs` | Gravação de BMP 32 bits sem dependências |

## Comandos

```text
fluxo-bench listar
fluxo-bench bench      [--monitor N] [--segundos S] [--copiar-cpu]
fluxo-bench screenshot [--monitor N] [--saida arquivo.bmp]
fluxo-bench encoders   [--monitor N]
fluxo-bench encode     [--monitor N] [--segundos S] [--encoder auto|nvenc|mf]
                       [--mbps M] [--fps F] [--keyframe-a-cada S]
                       [--saida arquivo.h264] [--verificar] [--refresh N] [--perder-a-cada S]
fluxo-bench decode     [--monitor N] [--segundos S] [--encoder auto|nvenc|mf] [--mbps M]
fluxo-bench receber    [--porta P] [--segundos S] [--saida arquivo.h264] [--verificar] [--codigo C] [--sem-cifra] [--sinal SERVIDOR]
fluxo-bench enviar     (--destino IP:PORTA | --sinal SERVIDOR) (--codigo C | --sem-cifra) [--mbps M] [--fps F] [--segundos S] [--fec K]
```

## Lendo o resultado do `bench`

| Linha | Significado | Esperado com vídeo/jogo a 60 fps |
|---|---|---|
| frames com imagem nova | Frames em que a tela mudou | ~60 fps médio |
| updates só de cursor | Só o mouse mexeu | Qualquer valor |
| esperas sem mudança | Blocos de 100 ms sem nenhuma mudança | ~0 |
| frames perdidos (Windows acumulou) | O Windows desenhou e não pegamos | 0 ou perto disso |
| perdas de acesso | ACCESS_LOST (recuperado sozinho) | 0, salvo troca de resolução/UAC |
| intervalo entre frames p50 / p99 | Regularidade | p50 ≈ 16,7 ms; p99 < 25 ms |
| cópia GPU→RAM (com `--copiar-cpu`) | Custo de trazer o frame para a RAM | 1–5 ms em 1080p |

O FPS medido é limitado pelo que está **na tela**: um vídeo de 30 fps dá 30;
um jogo a 144 fps num monitor de 144 Hz pode dar 144.

**O que é ruim:** "frames perdidos" crescendo, ou p99 muito acima do
intervalo do monitor — sinal de que algo (CPU ocupada, `--copiar-cpu`) está
atrasando a captura.

## `encoders`

Tenta abrir cada backend no device do monitor e mostra `[ok]` ou o motivo
da falha, e qual deles o modo `auto` escolheria. É o primeiro comando a rodar
num PC novo.

## Lendo o resultado do `encode`

| Linha | Significado | Esperado (1080p, 20 Mbps) |
|---|---|---|
| tempo de encode (P) | Conversão NV12 + encode de um frame comum, medido na CPU (inclui esperar a GPU) | p99 < 5 ms |
| tempo de encode (keyframe) | Idem para keyframes (forçados a cada `--keyframe-a-cada` s) | < 8 ms |
| tamanho médio (P) | Bytes por frame comum vs. o que o bitrate permite | ≤ alvo; bem menor com tela parada |
| tamanho médio (keyframe) / maior frame | Picos que atravessam a rede | perto do alvo/frame com VBV de 1 frame |
| bitrate real | Bytes gravados / duração | ≤ alvo |

O arquivo `.h264` abre no VLC (arraste o arquivo) ou com
`ffplay -f h264 captura.h264`.

### `--verificar`

Decodifica cada frame com o decoder H.264 do próprio Windows e compara com a
imagem capturada:

| Linha | Significado | Esperado |
|---|---|---|
| frames decodificados | Quantos o decoder aceitou | todos |
| PSNR vs. original | Fidelidade em RGB | > 30 dB; ~32 em área de trabalho com texto colorido (limite do 4:2:0), mais em vídeo |

Também salva o último frame como `<saida>.original.bmp` e
`<saida>.decodificado.bmp` para comparar no olho. A cópia para a RAM deixa
o loop mais lento: use `--verificar` para conferir qualidade, não para medir
tempo.

### `--perder-a-cada S` (com `--verificar`)

Descarta um frame codificado a cada S segundos (o decoder não o recebe, como
se a rede o perdesse) e pede a recuperação: uma onda de intra-refresh com
`--refresh N`, senão um keyframe. Um segundo decoder recebe **todos** os
frames, e o resultado compara os dois:

| Linha | O que é | Esperado |
|---|---|---|
| frames até a imagem limpa | 1 com keyframe, N com onda | — |
| PSNR do frame que limpou | Qualidade da imagem logo depois da recuperação | perto do normal |
| vs. decoder sem perda | Frame que limpou igual ao de um decoder que não perdeu nada (99 dB = idêntico) | todas idênticas |
| N frames seguintes | O mesmo, nos frames depois (estrago que sobrou e se propaga apareceria aqui) | 99 dB |

Use `--keyframe-a-cada 0` para os keyframes periódicos não esconderem o
resultado. Foi assim que apareceu (e foi corrigido) o estrago residual de
ondas com mais de um quadro de referência.

## `receber`

Receptor sem janela: escuta numa porta UDP (`--porta`, padrão 9000), recebe
o vídeo do [fluxo-host](fluxo-host.md), grava em `recebido.h264` a partir do
primeiro keyframe e mostra estatísticas. `--segundos` conta a partir do
primeiro frame recebido (0 = até Ctrl+C). Como o cliente, mostra um código de
pareamento (ou usa `--codigo`); o host precisa dele. `--sem-cifra` só para
teste.

É o único comando que **roda também em Linux/macOS** (usa só o `fluxo-net`):
serve para testar a rede com um segundo PC que não seja Windows. Lá,
`--verificar` não existe; abra o `.h264` gravado com `ffplay` ou VLC. Como
compilar: seção "Receptor em Linux" de [COMPILAR.md](../COMPILAR.md).

| Linha | Significado | Esperado |
|---|---|---|
| trânsito captura→chegada | Do momento da captura no host até o frame completo aqui | Mesma máquina: p50 < 5 ms. Entre PCs: só confiável com relógios sincronizados |
| frames perdidos na rede | Frames que nunca chegaram inteiros | 0 em rede cabeada |
| descartados na fila | O próprio receptor não leu a tempo | 0 |
| RTT | Ida e volta medida por ping | LAN cabeada < 1 ms |
| decodificados (com `--verificar`) | Frames aceitos pelo decoder H.264 do Windows | todos |
| grupos de FEC que falharam | Nos frames perdidos: grupos com 2 pacotes faltando (Reed-Solomon de 2 paridades salvaria) e com 3+ (rajada) | — |

## `enviar`

Manda frames sintéticos ao `receber`, a um bitrate fixo, sem captura nem
encode: mede a rede com carga controlada (com a tela parada o host real
quase não gera tráfego). Usa o mesmo `VideoSender` do host (pacing a 2,5× o
bitrate, FEC `--fec`), sem bitrate adaptativo. Todo frame vai marcado como
keyframe, para o receptor nunca pular frames. Também roda em Linux.

```powershell
# No PC que recebe (Linux: --saida /dev/null)
fluxo-bench receber --porta 9000 --segundos 20 --codigo K7M4-QX9P
# No PC que envia
fluxo-bench enviar --destino <IP>:9000 --mbps 40 --segundos 21 --codigo K7M4-QX9P
```

Foi assim que se mediu que o Wi-Fi de teste não perde nada até ~40 Mbps e
que, acima da capacidade, perde em rajadas (ver `fluxo-net.md`, FEC).

Pelo servidor de sinalização ([fluxo-sinal](fluxo-sinal.md)), sem saber o
IP de quem recebe: os dois usam `--sinal` no lugar de `--porta`/`--destino`.
O `receber` avisa quando o servidor confirma o registro (e o endereço
público com que ele o viu).

```bash
fluxo-bench receber --segundos 20 --sinal meu-servidor.com --codigo K7M4-QX9P
fluxo-bench enviar --sinal meu-servidor.com --mbps 30 --segundos 21 --codigo K7M4-QX9P
```

## `decode`

Captura → encode → decode na mesma máquina, sem rede nem janela, com o
decoder num device separado (como no cliente). Mostra se o decoder está na
GPU (DXVA), quantos frames saíram e o tempo de CPU por frame.

| Linha | Esperado |
|---|---|
| Decode | "acelerado pela GPU (DXVA)" |
| frames | decodificados = codificados (nenhum retido) |
| tempo de decode (CPU) | < 1 ms (a GPU termina o trabalho em paralelo) |
