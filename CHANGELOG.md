# Changelog

Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/).
Versões seguem [SemVer](https://semver.org/lang/pt-BR/).

## [Não lançado]

## [0.1.0] — 2026-10-29

### Adicionado
- Workspace Rust com crates `fluxo-proto`, `fluxo-capture` e app `fluxo-bench`.
- `fluxo-proto`: cabeçalho de 20 bytes (versão de protocolo 1), tipos Video,
  Ping e Pong; `Fragmenter`; `Reassembler` com descarte de frames atrasados,
  limite de memória e detecção de perda; comparação de sequência com
  wrap-around. 20 testes unitários + 3 doctests.
- `fluxo-proto/examples/loopback.rs`: teste de carga UDP local com perda simulada.
- `fluxo-capture`: captura via DXGI Desktop Duplication com device D3D11 na
  GPU do monitor, recuperação automática de `ACCESS_LOST`, cópia opcional
  para RAM, suporte a DPI alto. Stub para sistemas não-Windows.
- `fluxo-bench`: comandos `listar`, `bench` e `screenshot` (BMP).
- Documentação: README, arquitetura, protocolo, ADRs 001–005, roadmap,
  guia de compilação, convenções e documentos por módulo.

### Medido
- Loopback 1080p60 a 20 Mbps: 180/180 frames, ~2200 pacotes/s.
- Com 0,5% de perda de pacotes: ~18% de frames perdidos → motiva FEC (fase 4).
