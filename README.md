# Fluxo

Streaming de tela **P2P** de baixa latência para Windows, escrito em Rust.
Meta: **1080p a 60 fps** (com espaço para 1440p/4K e 120 fps) e latência de
ponta a ponta abaixo de **40 ms em rede local**.

> **Status:** Fase 6 — o pipeline completo funciona: o `fluxo-host` captura,
> codifica (H.264, HEVC ou AV1, por NVENC, Media Foundation ou na CPU) e
> transmite por UDP, com o som do PC; o `fluxo-client` decodifica na GPU e
> mostra numa janela. Medido na mesma máquina: captura → tela em ~3 ms (p50).
> Pareamento por código, tudo cifrado, e conexão entre redes diferentes sem
> servidor. Veja o [ROADMAP](docs/ROADMAP.md).

## Como funciona (resumo)

```text
 HOST (quem compartilha)                          CLIENTE (quem assiste)
 ┌──────────┐  ┌──────────┐  ┌────────────┐       ┌────────────┐  ┌──────────┐  ┌────────┐
 │ Captura  │─►│ Encoder  │─►│ Protocolo  │─UDP─► │ Protocolo  │─►│ Decoder  │─►│  Tela  │
 │  (DXGI)  │  │ (NVENC   │  │ fragmenta  │  P2P  │  remonta   │  │ (D3D11)  │  │        │
 │   GPU    │  │  ou MF)  │  │            │       │            │  │          │  │        │
 └──────────┘  └──────────┘  └────────────┘       └────────────┘  └──────────┘  └────────┘
   ✅ pronto     ✅ pronto      ✅ pronto            ✅ pronto       ✅ pronto      ✅ pronto
```

O frame nunca sai da GPU no host: a captura entrega uma textura, o encoder de
hardware lê essa textura, e só os bytes comprimidos (~40 KB por frame) vão
para a rede. Detalhes em [ARQUITETURA](docs/ARQUITETURA.md).

## Estrutura do projeto

```text
fluxo/
├── Cargo.toml              workspace (versões de dependências centralizadas)
├── CHANGELOG.md            o que mudou em cada versão
├── crates/                 bibliotecas, uma responsabilidade cada
│   ├── fluxo-proto/        protocolo de rede (puro Rust, sem I/O, 100% testável)
│   ├── fluxo-capture/      captura de tela via DXGI Desktop Duplication
│   ├── fluxo-encode/       encoder H.264/HEVC/AV1: NVENC, Media Foundation ou CPU
│   ├── fluxo-crypto/       código de pareamento, handshake SPAKE2, cifra dos pacotes
│   ├── fluxo-net/          transporte UDP: pacing, remontagem, RTT, sinalização
│   ├── fluxo-decode/       decoder na GPU (Media Foundation + DXVA)
│   └── fluxo-audio/        som do host: WASAPI, Opus, buffer de jitter
├── apps/                   executáveis
│   ├── fluxo/              a janela: compartilhar ou assistir, sem terminal
│   ├── fluxo-host/         transmite a tela (captura → encode → rede)
│   ├── fluxo-client/       mostra a tela recebida (rede → decode → janela)
│   ├── fluxo-bench/        CLI para testar e medir cada etapa isoladamente
│   └── fluxo-sinal/        servidor de sinalização (acha o outro lado pela internet)
└── docs/
    ├── ARQUITETURA.md      desenho do sistema, threads, orçamento de latência
    ├── PROTOCOLO.md        formato dos pacotes byte a byte
    ├── DECISOES.md         registro de decisões técnicas (ADRs)
    ├── ROADMAP.md          fases e próximos passos
    ├── COMPILAR.md         como instalar as ferramentas e compilar
    ├── CONVENCOES.md       padrão de código e de documentação
    └── modulos/            um documento por crate/app
```

## Começo rápido

Pré-requisitos e passo a passo completo: [COMPILAR.md](docs/COMPILAR.md).

```powershell
cargo build --release
.\target\release\fluxo-bench.exe listar
.\target\release\fluxo-bench.exe bench --segundos 10
.\target\release\fluxo-bench.exe screenshot --saida teste.bmp
.\target\release\fluxo-bench.exe encoders
.\target\release\fluxo-bench.exe encode --segundos 10 --verificar
cargo test --workspace
```

O jeito mais simples de usar é a janela, nos dois PCs (precisa do
`fluxo-host.exe` e do `fluxo-client.exe` na mesma pasta); veja
[fluxo](docs/modulos/fluxo.md):

```powershell
.\target\release\fluxo.exe
```

Pela linha de comando: transmitir — no PC que assiste, depois no que compartilha (no mesmo PC, use
`127.0.0.1`). O cliente mostra um código de pareamento (ex.: `K7M4-QX9P`) e
o comando pronto para o host; só quem digita esse código consegue
transmitir, e tudo vai cifrado:

```powershell
.\target\release\fluxo-client.exe --porta 9000
.\target\release\fluxo-host.exe --destino <IP de quem assiste>:9000 --codigo <código do cliente>
```

Em redes diferentes, **sem servidor nenhum**: o cliente mostra um código de
conexão, que vai pelo chat para quem compartilha; o host responde com um
código de resposta, que o cliente cola no terminal (ADR-010):

```powershell
.\target\release\fluxo-client.exe --internet
.\target\release\fluxo-host.exe --conexao <código de conexão do cliente>
```

Ou numa sala, por um servidor [fluxo-sinal](docs/modulos/fluxo-sinal.md) com a chave
dele (`FSS1-…`, gerada por quem roda o servidor): quem compartilha passa um
código, e quem assiste entra com ele, sem resposta:

```powershell
.\target\release\fluxo-host.exe --sala --chave FSS1-…
.\target\release\fluxo-client.exe --sala <código da sala> --chave FSS1-…
```

Sem janela, só medindo e gravando o que chega: troque o cliente por
`fluxo-bench receber --porta 9000 --segundos 30 --verificar` (que também
mostra um código).

## Documentação de código

Todo item público tem comentário de documentação (`///`). Para ver tudo
navegável no navegador:

```powershell
cargo doc --workspace --no-deps --open
```
