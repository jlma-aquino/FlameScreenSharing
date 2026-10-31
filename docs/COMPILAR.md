# Como compilar e rodar

## 1. Instalar as ferramentas (uma vez)

1. **Visual Studio Build Tools** (compilador e linker da Microsoft):
   - Baixe em <https://visualstudio.microsoft.com/visual-cpp-build-tools/>
   - Marque **"Desenvolvimento para desktop com C++"** e instale.
2. **Rust**: baixe e rode o `rustup-init.exe` de <https://rustup.rs>,
   aceitando as opções padrão.
3. Feche e reabra o terminal. Confira:
   ```powershell
   rustc --version
   cargo --version
   ```
4. *(Opcional)* Editor: VS Code com a extensão **rust-analyzer** — mostra
   erros, tipos e a documentação ao passar o mouse.

## 2. Compilar

Na pasta do projeto (onde está o `Cargo.toml` principal):

```powershell
cargo build --release
```

A primeira vez baixa as dependências e demora alguns minutos. Os executáveis
ficam em `target\release\`.

## 3. Rodar os testes

```powershell
cargo test --workspace
```

Esperado: todos `ok` (protocolo, BMP e exemplos da documentação).

## 4. Testar a captura

```powershell
.\target\release\fluxo-bench.exe listar
.\target\release\fluxo-bench.exe screenshot --saida teste.bmp
.\target\release\fluxo-bench.exe bench --segundos 10
.\target\release\fluxo-bench.exe bench --segundos 10 --copiar-cpu
```

Durante o `bench`, deixe um vídeo do YouTube em 60 fps ou um jogo rodando no
monitor capturado. Como interpretar a saída:
[modulos/fluxo-bench.md](modulos/fluxo-bench.md).

## 5. Testar o encoder

```powershell
.\target\release\fluxo-bench.exe encoders
.\target\release\fluxo-bench.exe encode --segundos 10
.\target\release\fluxo-bench.exe encode --segundos 10 --encoder mf
.\target\release\fluxo-bench.exe encode --segundos 5 --verificar
```

`encoders` mostra quais backends funcionam no PC. `encode` grava
`captura.h264` (abre no VLC). `--verificar` decodifica tudo com o decoder do
Windows e mede a fidelidade. Como interpretar:
[modulos/fluxo-bench.md](modulos/fluxo-bench.md).

## 6. Testar a transmissão

Em dois terminais (ou dois PCs, trocando `127.0.0.1` pelo IP de quem recebe).
Primeiro quem assiste, depois quem compartilha. O cliente mostra um código de
pareamento (ex.: `K7M4-QX9P`) e o comando pronto para o host:

```powershell
.\target\release\fluxo-client.exe --porta 9000
.\target\release\fluxo-host.exe --destino 127.0.0.1:9000 --codigo <código mostrado pelo cliente>
```

Sem janela, só medindo e gravando o que chega (`--codigo` fixa o código nos
dois lados, útil em scripts):

```powershell
.\target\release\fluxo-bench.exe receber --porta 9000 --segundos 30 --verificar --codigo K7M4-QX9P
.\target\release\fluxo-host.exe --destino 127.0.0.1:9000 --segundos 35 --codigo K7M4-QX9P
```

Na primeira vez o Windows pode pedir para liberar o `fluxo-client` (ou o
`fluxo-bench`) no firewall: para receber de outro PC, permita em "Redes
privadas". Como ler a saída: [modulos/fluxo-client.md](modulos/fluxo-client.md)
e [modulos/fluxo-host.md](modulos/fluxo-host.md).

Só o decoder, sem rede:

```powershell
.\target\release\fluxo-bench.exe decode --segundos 10
```

### Receptor em Linux (segundo PC sem Windows)

O `fluxo-bench receber` compila e roda em Linux. Serve para medir a rede de
verdade (perdas, RTT, bitrate) com o host no Windows.

1. **Copiar o projeto** sem a pasta `target` (gigante e só serve no Windows).
   No Windows, com o pendrive em `E:`:
   ```powershell
   git clone "C:\caminho\do\ScreenSharing Project" E:\fluxo
   ```
2. **No Linux**, instalar compilador C e Rust (uma vez):
   ```bash
   sudo apt install build-essential        # Debian/Ubuntu/Mint
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source "$HOME/.cargo/env"
   ```
3. Copiar `fluxo` do pendrive para a pasta pessoal e compilar só o bench:
   ```bash
   cp -r /media/$USER/<pendrive>/fluxo ~/fluxo && cd ~/fluxo
   cargo build --release -p fluxo-bench
   ```
4. Descobrir o IP do notebook (`hostname -I`) e, se o firewall estiver
   ativo, liberar a porta: `sudo ufw allow 9000/udp`.
5. Receber (Linux) e transmitir (Windows):
   ```bash
   ./target/release/fluxo-bench receber --porta 9000 --segundos 30   # mostra o código
   ```
   ```powershell
   .\target\release\fluxo-host.exe --destino <IP do notebook>:9000 --segundos 35 --codigo <código>
   ```
6. Conferir a imagem: `ffplay recebido.h264` (ou abrir no VLC).

O "trânsito" medido entre dois PCs depende dos relógios estarem
sincronizados (NTP); o RTT e as perdas são confiáveis sempre.

### Redes diferentes (servidor de sinalização)

O `fluxo-sinal` compila em qualquer sistema e só troca endereços. Rode num
PC com endereço público e a porta **UDP 7000** liberada (uma VPS Linux
barata serve), e use `--sinal` nos dois lados no lugar do IP:

```bash
cargo build --release -p fluxo-sinal
./target/release/fluxo-sinal
```

```powershell
.\target\release\fluxo-client.exe --sinal <endereço do servidor>
.\target\release\fluxo-host.exe --sinal <endereço do servidor> --codigo <código mostrado pelo cliente>
```

Para testar o fluxo numa rede só, o servidor pode rodar no notebook Linux
(o host acha o cliente pelo endereço local). Detalhes e limitações:
[modulos/fluxo-sinal.md](modulos/fluxo-sinal.md).

## 7. Testar o protocolo de rede

```powershell
cargo run -p fluxo-proto --release --example loopback -- --segundos 5 --mbps 20
cargo run -p fluxo-proto --release --example loopback -- --segundos 5 --mbps 20 --perda 0.5
```

## 8. Ver a documentação do código

```powershell
cargo doc --workspace --no-deps --open
```

## Problemas comuns

| Sintoma | Causa provável |
|---|---|
| `link.exe not found` | Build Tools sem o pacote "Desenvolvimento para desktop com C++". |
| `Desktop Duplication indisponível: o driver não suporta` | Notebook com duas GPUs: no Painel NVIDIA/Configurações de gráficos do Windows, force o `fluxo-bench.exe` na GPU ligada à tela. |
| `bench` mostra ~0–5 fps | Tela parada. Abra um vídeo/jogo. |
| `acesso à captura perdido` aparecendo | Normal ao trocar resolução, abrir UAC ou alternar jogo em tela cheia exclusiva; a captura se recria sozinha. |
| Erro de compilação dentro de `fluxo-capture/src/dxgi.rs` | Possível diferença de versão do crate `windows`. Copie a mensagem de erro completa para corrigirmos. |
| `encoders` mostra `[--] nvenc ... driver suporta a API NVENC até X` | Driver NVIDIA antigo: atualize para 531 ou mais novo (ou use `--encoder mf`). |
| `encoders` mostra `[--] mf ... nenhum encoder H.264 de hardware` | Driver de vídeo sem encoder (VM, driver genérico da Microsoft): instale o driver do fabricante. |
