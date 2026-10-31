# fluxo-crypto

**Caminho:** `crates/fluxo-crypto` · **Dependências:** RustCrypto (`spake2`, `chacha20poly1305`, `hkdf`, `sha2`, `getrandom`) · **Plataforma:** qualquer

Segurança do Fluxo, sem I/O: código de pareamento, handshake e cifra dos
pacotes. Nenhum algoritmo é escrito aqui; só se combinam bibliotecas
prontas e auditadas. Decisão e alternativas: ADR-008 em
[DECISOES.md](../DECISOES.md). Formato dos pacotes:
[PROTOCOLO.md](../PROTOCOLO.md), seções 2 e 2.1.

## Arquivos

| Arquivo | Conteúdo |
|---|---|
| `src/lib.rs` | Visão geral, `CryptoError`, reexportações |
| `src/codigo.rs` | `PairingCode`: sorteio, leitura do que foi digitado, exibição `XXXX-XXXX` |
| `src/handshake.rs` | `HostHandshake` / `respond`: SPAKE2 → chaves de sessão, confirmação |
| `src/cifra.rs` | `Sealer` / `Opener`: ChaCha20-Poly1305 por pacote, janela anti-replay |

## Código de pareamento

8 símbolos de `23456789ABCDEFGHJKMNPQRSTUVWXYZ` (sem 0/O, 1/I/L): ~39,6
bits. Sorteado com o gerador do sistema, com rejeição para não favorecer
nenhum símbolo. Ao ler, aceita minúsculas e ignora hífens e espaços. O
`Debug` não mostra o código, para ele não vazar em logs.

## Handshake

```text
host                                       cliente
 HostHandshake::start(código) ─ PairInit ──► respond(código, init)
                                              → Session (já decifra)
 finish(reply) ◄──────────────── PairReply ─ [msg SPAKE2 | confirmação]
 → Session, ou WrongCode
```

- SPAKE2 com identidades `fluxo-host` e `fluxo-cliente`: chaves iguais só
  com o mesmo código. Quem se mete na conversa sem o código ganha um
  palpite por tentativa; não dá para gravar a troca e testar códigos depois.
- HKDF-SHA256 (sal `fluxo protocolo v8`) deriva a chave host → cliente, a
  cliente → host e a de confirmação.
- O cliente confirma a chave (bloco vazio cifrado com a chave de
  confirmação): o host sabe na hora se o código bate. O cliente confere o
  host implicitamente: só quem tem a chave produz pacotes que decifram.

## Cifra

- `Sealer::next_seq` reserva o número do pacote (que vai no cabeçalho);
  `seal` cifra o payload no lugar com o cabeçalho como dado autenticado e
  acrescenta a tag de 16 bytes. Nonce = `seq`, nunca repetido com a mesma
  chave porque cada sentido tem a sua. Esgota em 2³² pacotes.
- `Opener::open` recusa `seq` repetido ou mais de 64 atrás **antes** de
  decifrar, e só marca o `seq` como visto depois que a tag confere: um pacote
  forjado não mexe na janela.

## Testes

```powershell
cargo test -p fluxo-crypto
```

14 testes: código (sorteio, leitura, recusa de ambíguos, `Debug`
escondido), handshake (mesmo código nos dois sentidos, código errado,
chaves novas a cada pareamento, mensagens malformadas) e cifra (ida e volta,
alteração detectada, chave errada, repetido recusado, fora de ordem aceito,
velho demais recusado, esgotamento do `seq`). Os testes de ponta a ponta
com sockets ficam no `fluxo-net` (`tests/loopback.rs`).
