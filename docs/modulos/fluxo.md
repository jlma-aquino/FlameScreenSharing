# fluxo (a janela)

**Caminho:** `apps/fluxo` · **Dependências:** `eframe` (egui), `fluxo-net`, `fluxo-capture` · **Plataforma:** Windows 10+

A forma de usar o Fluxo sem terminal: `fluxo.exe` abre uma janela pequena com
**Compartilhar minha tela** e **Assistir**. Por baixo ela roda o
[fluxo-host](fluxo-host.md) e o [fluxo-client](fluxo-client.md) da **mesma
pasta**, sem janela de console, e mostra os códigos com um botão de copiar.

## Como usar

Pela internet (redes diferentes), sem servidor:

1. Quem vai **assistir** clica em Assistir → Começar e manda o **código de
   conexão** para quem vai compartilhar. A janela de vídeo abre esperando.
2. Quem vai **compartilhar** clica em Compartilhar minha tela, cola o código,
   escolhe resolução e fps e clica em Começar.
3. Se aparecer um **código de resposta**, quem compartilha o manda de volta, e
   quem assiste cola no campo da janela e clica em Enviar.

Na mesma rede é igual, com endereço + código no lugar do código de conexão.

## Nome e pacote para testes

Para quem usa, o programa se chama **FlameScreenSharing (FSS)**: janela, títulos e
descrição da porta no roteador (`NOME` em `src/main.rs`). Por dentro (crates,
comandos, documentação) continua Fluxo.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File pacotegerar_pacote.ps1
```

Gera `distlamescreensharing.zip` com `flamescreensharing.exe`, `fss-host.exe`,
`fss-client.exe` e o `leia-me.txt`, com o runtime do c++ embutido (o outro pc não
precisa do visual c++ redistributable). não tem assinatura digital: o windows avisa
ao abrir.

## arquivos

| Arquivo | Conteúdo |
|---|---|
| `src/main.rs` | Abre a janela |
| `src/janela.rs` | As telas (egui): início, formulários, sessão em andamento |
| `src/comandos.rs` | Monta a linha de comando e confere os códigos antes de abrir qualquer coisa |
| `src/saida.rs` | Interpreta as linhas que os programas imprimem (códigos, início da transmissão, erro) |
| `src/processo.rs` | Roda o programa sem console; linhas chegam por um canal; o código de resposta vai pela entrada padrão |

## Por que rodar os executáveis, em vez de chamar as bibliotecas

A lógica do host e do cliente mora no `main` de cada um (laço de captura,
janela de vídeo, thread de áudio). Rodá-los por baixo reaproveita tudo sem
mexer no caminho de vídeo, que é o que tem medição; e um travamento neles não
derruba a janela. O custo é o texto impresso virar contrato: ao mudar um
`println!` de código, mude `saida.rs` (os testes de lá guardam o formato).

## Limitações conhecidas

- Parar encerra o programa na hora: o resumo final do host não aparece.
- As opções não ficam salvas entre uma abertura e outra.
- Só as opções mais comuns; o resto (`--codec`, `--mbps`, `--fec`…) continua
  na linha de comando.
- Testar no mesmo PC com som nos dois lados faz eco: o host captura o que o
  cliente toca. Entre dois PCs não acontece.
- Cliques simulados por uma ferramenta de automação não foram aceitos pela
  janela (cliques de verdade, sim). Causa não encontrada; não é o nível de
  permissão do processo.

## Teste com a janela aberta

Código de conexão copiado e colado entre duas janelas no mesmo PC, com cliques
de verdade: transmissão a 60 fps, 15,5 Mbps, captura→tela 11,6 ms, 0 perdidos,
cifrada. Esse teste mostrou que a janela de quem assiste ficava em "Esperando"
com o vídeo já chegando; agora o cliente avisa no primeiro frame.

## Testes

```powershell
cargo test -p fluxo
```

16 testes, sem abrir janela: leitura das linhas, montagem dos argumentos,
mensagens de código errado, e o processo de verdade (entrada e saída por pipe,
erro, fim, soltar encerra). Telas: só conferindo com a janela aberta.
