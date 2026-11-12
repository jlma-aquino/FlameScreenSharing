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
fluxo-sinal --chaves ARQUIVO [--porta P] [--ip IP]
fluxo-sinal --nova-chave ENDEREÇO [--chaves ARQUIVO]
fluxo-sinal --aberto [--porta P]          # só para testes
```

Padrão: UDP 7000 em todas as interfaces. Precisa ficar num endereço que os
dois lados alcancem: IP público com a porta UDP liberada (numa VPS, no
firewall dela). Nada de salas é guardado em disco; uma sala some 30 s depois
do último registro do cliente.

**Chaves de acesso** (ADR-019). O servidor só atende quem tem uma chave
(`FSS1-…`, 52 caracteres): o endereço dele e um segredo de 128 bits. Cada
pedido sai assinado com o segredo (HMAC-SHA256, 16 bytes no fim do
datagrama); pedido sem assinatura de uma chave do arquivo é ignorado, sem
resposta. Saber o IP não basta para usar o servidor nem para fazê-lo
responder a terceiros.

```bash
# Cria uma chave para este servidor e guarda no arquivo
fluxo-sinal --nova-chave 203.0.113.5:3478 --chaves chaves.txt
# Atende quem tem uma das chaves do arquivo
fluxo-sinal --chaves chaves.txt --porta 3478
```

O arquivo tem uma chave por linha (`#` começa comentário). Para tirar o
acesso de alguém, apague a linha e reinicie. Sem `--chaves` o servidor
recusa subir; `--aberto` atende qualquer um (testes locais, `fluxo-bench`).

Quem usa cola a chave nas configurações do FSS (Conexão), ou passa
`--chave` aos programas:

```powershell
.\target\release\fluxo-host.exe --sala --chave FSS1-…
.\target\release\fluxo-client.exe --sala K7M4-QX9P --chave FSS1-…
```

`--sinal ENDEREÇO` (sem chave) continua valendo para servidores `--aberto`.

## Saída

Uma linha por acontecimento (repetições seguidas iguais são omitidas):

```text
[     1.0 s] sala 3F04B: cliente em 203.0.113.7:41000
[     1.1 s] sala 3F04B: host 198.51.100.4:52311 achou o cliente 203.0.113.7:41000
```

`pacote sem chave válida de IP (ignorado)` aparece no máximo uma vez por
minuto por IP.

`sala vazia` quando um host procura antes de o cliente abrir (ou com o
código errado, que cai em outra sala); `recusada` quando outro IP tenta uma
sala ocupada (raro: 1 em ~1 milhão por par de códigos).

## Limitações

- Deve ficar **fora** da rede do cliente: na mesma rede, o endereço visto
  é o local, que não serve para um host de fora.
- NAT simétrico dos dois lados não fura; o relay de fallback é o próximo
  item da fase 5.
- O do projeto roda na Oracle Cloud ([SERVIDOR.md](../SERVIDOR.md)); o
  programa não traz nenhum servidor: sem chave, as salas não funcionam.
  Testado com o servidor na internet e os dois PCs na mesma rede; ainda falta
  um teste com os PCs em redes diferentes.
