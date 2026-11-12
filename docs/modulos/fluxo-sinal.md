# fluxo-sinal

**Caminho:** `apps/fluxo-sinal` · **Dependências:** `fluxo-net` · **Plataforma:** qualquer (pensado para um servidor Linux)

Servidor de sinalização: apresenta host e cliente que estão em redes
diferentes. Só troca endereços; vídeo nunca passa por ele, e ele não vê o
código de pareamento. Decisão: ADR-009 em [DECISOES.md](../DECISOES.md).
Formato: [PROTOCOLO.md](../PROTOCOLO.md), seção 2.2. A lógica fica no
`fluxo-net` (`SignalServer`, ver [fluxo-net](fluxo-net.md)); o app só a roda
num socket e mostra o que acontece.

Uma sala guarda até 8 clientes, e cada pedido do host recebe um deles (ver
[PROTOCOLO.md](../PROTOCOLO.md), seção 2.2, e ADR-016): é o que deixa várias
pessoas entrarem com o mesmo código numa sala aberta pelo host.

## Uso

```text
fluxo-sinal [--porta P] [--ip IP]
```

Padrão: UDP 7000 em todas as interfaces. Precisa ficar num endereço que os
dois lados alcancem: IP público com a porta **UDP 7000** liberada (numa VPS,
no firewall dela). Nada é guardado em disco; uma sala some 30 s depois do
último registro do cliente.

```bash
# No servidor (Linux)
cargo build --release -p fluxo-sinal
./target/release/fluxo-sinal
```

```powershell
# No PC que assiste
.\target\release\fluxo-client.exe --sinal meu-servidor.com
#   No PC que compartilha:  fluxo-host --sinal meu-servidor.com --codigo K7M4-QX9P
# No PC que compartilha
.\target\release\fluxo-host.exe --sinal meu-servidor.com --codigo K7M4-QX9P
```

`--sinal` aceita nome, IP ou `endereço:porta` (sem porta: 7000).

## Saída

Uma linha por acontecimento (repetições seguidas iguais são omitidas):

```text
[     1.0 s] sala 3F04B: cliente em 203.0.113.7:41000
[     1.1 s] sala 3F04B: host 198.51.100.4:52311 achou o cliente 203.0.113.7:41000
```

`sala vazia` quando um host procura antes de o cliente abrir (ou com o
código errado, que cai em outra sala); `recusada` quando outro IP tenta uma
sala ocupada (raro: 1 em ~1 milhão por par de códigos).

## Limitações

- Deve ficar **fora** da rede do cliente: na mesma rede, o endereço visto
  é o local, que não serve para um host de fora.
- NAT simétrico dos dois lados não fura; o relay de fallback é o próximo
  item da fase 5.
- Roda na Oracle Cloud em `137.131.152.250:3478` ([SERVIDOR.md](../SERVIDOR.md)).
  Testado com o servidor na internet e os dois PCs na mesma rede; ainda falta
  um teste com os PCs em redes diferentes.
