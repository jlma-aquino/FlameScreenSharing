# fluxo-proto

**Caminho:** `crates/fluxo-proto` · **Dependências:** nenhuma · **Plataforma:** qualquer

Protocolo de rede: transforma frames em pacotes e pacotes em frames. Não abre
sockets — isso é da camada de rede. Formato byte a byte em
[PROTOCOLO.md](../PROTOCOLO.md).

## Arquivos

| Arquivo | Conteúdo |
|---|---|
| `src/lib.rs` | Visão geral, reexportações, `DEFAULT_MAX_PAYLOAD` |
| `src/packet.rs` | `PacketHeader` (encode/decode), `PacketKind`, `ProtoError`, constantes |
| `src/fragment.rs` | `Fragmenter`: frame → pacotes, numeração de frames |
| `src/reassembly.rs` | `Reassembler`: pacotes → frames, `ReassemblerStats`, reconstrução por FEC |
| `src/fec.rs` | Paridade XOR: `parity`, `recover`, `group_count`, `group_range` |
| `src/seq.rs` | `is_newer` / `is_newer_or_equal` com wrap-around |
| `src/signal.rs` | `SignalMessage`: mensagens do servidor de sinalização (PROTOCOLO.md, seção 2.2) |
| `src/stun.rs` | STUN mínimo: `binding_request` e `mapped_address` (PROTOCOLO.md, seção 2.3) |
| `src/cursor.rs` | `CursorMessage`, `CursorShape`, `ShapeAssembler`: cursor à parte do vídeo (PROTOCOLO.md, seção 2.4) |
| `examples/loopback.rs` | Teste de carga via UDP local |

## Uso típico

```rust
// Host
let mut frag = Fragmenter::default();
let (frame_id, pacotes) = frag.fragment(&bytes_codificados, eh_keyframe, timestamp_us)?;
for p in &pacotes { socket.send(p)?; }

// Cliente
let mut reasm = Reassembler::default();
let n = socket.recv(&mut buf)?;
if let Ok((header, payload)) = PacketHeader::decode(&buf[..n]) {
    if let Some(frame) = reasm.push(&header, payload) {
        if frame.preceded_by_loss { /* pedir keyframe (fase 4) */ }
        decoder.decode(&frame.data);
    }
}
```

## Pontos de atenção ao alterar

- **Mudou o cabeçalho?** Incremente `PROTOCOL_VERSION`, atualize
  `PROTOCOLO.md`, ajuste `HEADER_LEN` e os testes de `packet.rs`.
- `Reassembler` assume que frames mais novos invalidam os mais velhos. Se um
  dia houver retransmissão (NACK), revise a regra 4 de `reassembly.rs`.
- Os limites `max_pending_frames` e `max_fragments_per_frame` protegem
  memória contra pacotes maliciosos; não remova.

## Testes

```powershell
cargo test -p fluxo-proto
cargo run -p fluxo-proto --release --example loopback -- --perda 1
```
