# Fluxo

Streaming de tela **P2P** de baixa latência para Windows, escrito em Rust.
Meta: **1080p a 60 fps** (com espaço para 1440p/4K e 120 fps) e latência de
ponta a ponta abaixo de **40 ms em rede local**.

> **Status:** Fase 1 — captura de tela e protocolo de rede prontos e testados.
> Ainda não transmite vídeo. Veja o [ROADMAP](docs/ROADMAP.md).

## Como funciona (resumo)

```text
 HOST (quem compartilha)                          CLIENTE (quem assiste)
 ┌──────────┐  ┌──────────┐  ┌────────────┐       ┌────────────┐  ┌──────────┐  ┌────────┐
 │ Captura  │─►│ Encoder  │─►│ Protocolo  │─UDP─► │ Protocolo  │─►│ Decoder  │─►│  Tela  │
 │  (DXGI)  │  │ (NVENC/  │  │ fragmenta  │  P2P  │  remonta   │  │ (D3D11)  │  │        │
 │   GPU    │  │ AMF/QSV) │  │            │       │            │  │          │  │        │
 └──────────┘  └──────────┘  └────────────┘       └────────────┘  └──────────┘  └────────┘
   ✅ pronto     ⏳ fase 2      ✅ pronto            ✅ pronto       ⏳ fase 3      ⏳ fase 3
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
│   └── fluxo-capture/      captura de tela via DXGI Desktop Duplication
├── apps/                   executáveis
│   └── fluxo-bench/        CLI para testar e medir a captura
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
cargo test --workspace
```

## Documentação de código

Todo item público tem comentário de documentação (`///`). Para ver tudo
navegável no navegador:

```powershell
cargo doc --workspace --no-deps --open
```
