# Fluxo

Streaming de tela **P2P** de baixa latência para Windows, escrito em Rust.
Meta: **1080p a 60 fps** (com espaço para 1440p/4K e 120 fps) e latência de
ponta a ponta abaixo de **40 ms em rede local**.

> **Status:** Fase 3 em andamento — o pipeline completo funciona: o
> `fluxo-host` captura, codifica (NVENC ou Media Foundation) e transmite por
> UDP; o `fluxo-client` decodifica na GPU e mostra numa janela. Medido na
> mesma máquina: captura → tela em ~3 ms (p50). Veja o [ROADMAP](docs/ROADMAP.md).

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
│   ├── fluxo-encode/       encoder H.264 por hardware: NVENC ou Media Foundation
│   ├── fluxo-net/          transporte UDP: pacing, remontagem, RTT
│   └── fluxo-decode/       decoder H.264 na GPU (Media Foundation + DXVA)
├── apps/                   executáveis
│   ├── fluxo-host/         transmite a tela (captura → encode → rede)
│   ├── fluxo-client/       mostra a tela recebida (rede → decode → janela)
│   └── fluxo-bench/        CLI para testar e medir cada etapa isoladamente
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

Transmitir — no PC que assiste, depois no que compartilha (no mesmo PC, use
`127.0.0.1`):

```powershell
.\target\release\fluxo-client.exe --porta 9000
.\target\release\fluxo-host.exe --destino <IP de quem assiste>:9000
```

Sem janela, só medindo e gravando o que chega: troque o cliente por
`fluxo-bench receber --porta 9000 --segundos 30 --verificar`.

## Documentação de código

Todo item público tem comentário de documentação (`///`). Para ver tudo
navegável no navegador:

```powershell
cargo doc --workspace --no-deps --open
```
