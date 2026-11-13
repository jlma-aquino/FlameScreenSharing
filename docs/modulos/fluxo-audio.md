# fluxo-audio

**Caminho:** `crates/fluxo-audio` · **Dependências:** `unsafe-libopus` (e `windows`, para o WASAPI) · **Plataforma:** Opus, jitter e conversão em qualquer uma; captura e reprodução no Windows

O som do PC que compartilha, levado ao PC que assiste. Decisão do codec:
ADR-011 em [DECISOES.md](../DECISOES.md). Formato na rede:
[PROTOCOLO.md](../PROTOCOLO.md), seção 2.5.

```text
HOST                                              CLIENTE
WASAPI loopback → Converter → Encoder ─ rede ─► JitterBuffer → Decoder → Converter → WASAPI
 (o que toca)      48k estéreo  Opus 10 ms       (~30 ms)      (FEC/PLC)   taxa da placa
```

## Arquivos

| Arquivo | Conteúdo |
|---|---|
| `src/opus.rs` | `Encoder` / `Decoder`: Opus 48 kHz estéreo, quadros de 10 ms, FEC e ocultação de perda. Todo o `unsafe` do crate |
| `src/jitter.rs` | `JitterBuffer`: segura alguns quadros e entrega em ritmo fixo; o alvo acompanha a rede |
| `src/resample.rs` | `Converter`: taxa (interpolação linear) e canais (mono, 5.1, 7.1) ↔ 48 kHz estéreo |
| `src/wasapi.rs` | `LoopbackCapture` (o que o PC toca), `AudioOutput` (tocar, com volume próprio), `output_devices` |
| `src/player.rs` | `AudioPlayer`: jitter + decoder + saída, no ritmo da placa (usado pelo cliente e pelo `fluxo-bench receber --tocar`) |

## Detalhes

- **Opus:** aplicação "baixo atraso restrito" (sem o modo de voz), sinal de
  música, 128 kbps, FEC dentro do pacote para ~5% de perda. Encode de um
  quadro: ~0,2–0,3 ms.
- **Captura:** WASAPI loopback da saída padrão (ou a escolhida), no formato
  que a placa usa (float, 16, 24 ou 32 bits; qualquer taxa e número de
  canais). Nada tocando = o Windows não entrega nada: `read_frames`
  completa com silêncio pelo relógio, para o fluxo não parar.
- **Saídas:** `output_devices()` lista as saídas de som com o nível agora:
  mostra onde o som está saindo (no teste, o jogo tocava num headset que
  era a saída padrão; nos primeiros segundos, nada tocava).
- **Reprodução:** `AudioPlayer::pump` enche a fila da placa até 20 ms,
  pedindo um quadro por vez ao buffer de jitter (começa em 30 ms). Volume
  próprio do Fluxo (`ISimpleAudioVolume`), sem mexer no do sistema.

## Medições

| Teste | Resultado |
|---|---|
| `fluxo-bench audio` (captura → Opus → decode, 2 s de som) | 100 quadros/s, 130 kbps, encode até 0,33 ms |
| Host → `receber --audio` na mesma máquina (10 s) | 767 quadros, 0 perdidos, captura→chegada ~10 ms |
| Host → `receber --tocar 0` na mesma máquina (10 s) | 754 quadros tocados, 0 perdidos, 0 vezes sem som, buffer 30 ms + ~10 ms na placa |
| Host → notebook pelo Wi-Fi, `receber --audio` (12 s, com vídeo) | 1168 quadros, 0 perdidos, captura→chegada p50 10,7 ms |

Latência estimada até o som sair: ~10 ms do quadro + rede + 30 ms de
jitter + ~20 ms na placa ≈ 60–70 ms numa LAN, uns 40–50 ms atrás da imagem.
O buffer de jitter começa em 3 quadros e diminui sozinho se a rede deixar
(mínimo 2).

## Testes

```powershell
cargo test -p fluxo-audio
.\target\release\fluxo-bench.exe audio --segundos 5 --volume 0
```

10 testes: Opus de ida e volta (energia preservada, bitrate), quadro
perdido refeito pelo FEC ou inventado, buffer de jitter (ordem, perda com
FEC, esvaziar aumenta o alvo e estabilidade diminui, excesso descartado),
conversão de taxa e canais (em blocos, sem saltos nas emendas). Mais o
teste de ponta a ponta do áudio cifrado em `fluxo-net/tests/loopback.rs`.

## Captura sem o som do FSS

`LoopbackCapture::excluding_process(pid)` usa a captura por processo do Windows
10 2004+ (`ActivateAudioInterfaceAsync` com
`PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE`): tudo o que o PC toca menos
o processo dado e os filhos dele, em 48 kHz estéreo float. O host usa com o
número da janela do FSS, para o som de quem você assiste não voltar na sua
transmissão. Cuidado ao mexer: o `PROPVARIANT` do crate `windows` se limpa
sozinho e liberaria o bloco de parâmetros (que é nosso, na pilha): ele fica em
`ManuallyDrop` (sem isso, heap corrompido, `0xC0000374`).
