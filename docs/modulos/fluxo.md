# fluxo (a janela)

**Caminho:** `apps/fluxo` · **Dependências:** `eframe` (egui), `fluxo-net`, `fluxo-capture`, `fluxo-audio` (só para listar as saídas de som) · **Plataforma:** Windows 10+ (assistir também no Linux)

A forma de usar o FSS sem terminal, no jeito do Meet e do Discord. Por baixo
ela roda o [fluxo-host](fluxo-host.md) e o [fluxo-client](fluxo-client.md) da
**mesma pasta**, sem janela de console.

## Como usar

- **Tela inicial:** dois cartões. **Compartilhar a tela** (escolhe a tela, se
  houver mais de uma, e o som) abre uma sala; **Entrar numa sala** recebe o
  código que quem compartilha mandou. Até 4 pessoas entram com o mesmo código.
- **Durante a transmissão:** no meio, o código da sala em letra grande (com
  Copiar) e o estado; embaixo, a **barra de controles**:
  - quem compartilha: **Tela ⏷** (troca o monitor), **Som** (liga e desliga o
    som do PC), **Pausar/Continuar**, **Qualidade ⏷** (resolução e fps) e
    **Encerrar**;
  - quem assiste não vê esta janela: o vídeo abre **no lugar dela**, na mesma
    posição e tamanho, com a barra de controles por cima do vídeo (mudo,
    volume, estatísticas, tela cheia, Sair; some com o mouse parado, como no
    YouTube). Ao sair, esta janela volta onde o vídeo estava (`src/lugar.rs`;
    ver [fluxo-client](fluxo-client.md#barra-de-controles-sobre-o-vídeo)).
  Nada disso reinicia a transmissão nem derruba quem assiste (ver "Botões ao
  vivo").
- **Outras formas de conectar (sem servidor):** pela internet trocando códigos
  de conexão e de resposta, ou na mesma rede com endereço e código. É o jeito
  antigo, para quando o servidor das salas estiver fora do ar.
- **⚙ Configurações:** página própria, com menu à esquerda (Esc fecha):
  - **Aparência:** escuro, claro ou igual ao sistema (muda na hora) e o tema
    personalizado;
  - **Compartilhar:** tela, resolução, fps, codec, encoder, bitrate máximo, som
    ligado ao começar e de qual saída capturar o som;
  - **Placa de vídeo** (em Compartilhar e em Assistir, a mesma escolha):
    automática (a do Windows) ou uma da lista; num notebook com duas placas,
    a dedicada. Vai como `--gpu N` ao host e ao cliente.
  - **Assistir:** volume inicial, em qual saída tocar, estatísticas sobre o
    vídeo, janela sempre por cima;
  - **Conexão:** a chave do servidor das salas (`FSS1-…`, conferida na hora;
    sem ela, os cartões de sala pedem para colar);
  - **Sobre:** versão, atualizações (ligar ou desligar, Verificar agora).

## Grupo (`src/grupo.rs`, ADR-022)

Configurações > Conexão tem a chave do servidor e, separado, o **convite do
grupo** (`FSSG-…`). Com os dois, uma lista à direita (o botão **Grupo · N**,
em cima, mostra e esconde; some em janelas com menos de 560 px), no jeito do
Discord: o nome do grupo, **ONLINE** (a foto com a bolinha verde, ou vermelha
para quem está ao vivo) e **OFFLINE** (recolhível). A linha de quem está ao
vivo é um botão: entra na sala sem colar código. A tela inicial também lista
quem está ao vivo no cartão Entrar numa sala. Uma
thread fala com o servidor (`fluxo_net::GroupClient`) e conta o apelido e,
compartilhando numa sala, o código dela. O apelido fica em Configurações >
Perfil (vazio: o usuário do Windows), e cada FSS tem um
número sorteado na primeira vez (`membro` nas configurações).

Embaixo de quem está ao vivo aparece quem está assistindo (`assistindo: Ana,
Bia`), pela sala assistida que cada um conta ao grupo. A sala só aceita quem é
do grupo (o servidor separa as salas por chave). Quem compartilha vê, no
cartão Assistindo, o nome de cada espectador (o cliente recebe `--apelido` e
`--membro`) e o menu **Expulsar**: tirar da transmissão, ou tirar e bloquear
por 10 min, 1 hora ou até encerrar. O vídeo de quem sai fecha com o aviso.

Testado com um servidor local e duas janelas com configurações separadas: as
duas entraram no grupo e ficaram online, e uma entrou na sala da outra.

## Perfil (`src/perfil.rs`, ADR-023)

Configurações > **Perfil** (ou o nome e a foto no canto de cima): o nome e a
foto que os outros do grupo veem, ligados ao número de membro deste FSS (não
ao IP). **Escolher foto…** abre a janela do Windows; também dá para arrastar
uma imagem para a janela. A foto é cortada em quadrado, reduzida para 80×80 e
gravada em `foto.jpg` (JPEG de até ~4 KB) na pasta das configurações, tudo
pelo WIC do Windows. Ela sobe ao grupo cifrada com o convite; as dos outros
ficam (cifradas) em `fotos`. Sem foto: as iniciais num círculo de cor fixa
por membro. As fotos aparecem na lista do grupo, no cartão Assistindo de quem
compartilha (o host conta o número de membro de cada espectador) e em cima.

## Primeira vez

Sem chave, a tela inicial mostra um campo só: cole a mensagem que veio com o
programa e ele acha a chave (`FSS1-…`) e o convite (`FSSG-…`), em qualquer
ordem e com outras palavras em volta.

## Durante a transmissão

As abas ficam curtas ("Compartilhando", "Sala XXXX") com a bolinha de cor do
estado; o estado por extenso fica só no cartão do meio. "Copiar código"
mostra "Copiado!". O relatório vira **Conexão boa / instável / ruim** (perda
abaixo de 1% e RTT abaixo de 150 ms; perda de 5% ou mais é ruim), com os
números ao lado. O cartão **Assistindo · N de 4** mostra a foto, o nome e há
quanto tempo cada um está assistindo.

## Várias sessões

Dá para compartilhar enquanto assiste, ou assistir a mais de uma pessoa: cada
sessão vira uma aba no topo ("📡 Compartilhando · …", "▶ Sala ABCD · …"), e
"＋ Começar outra" volta à tela inicial sem fechar nada. No máximo uma
compartilhando. O vídeo abre no lugar desta janela só quando não há outra
sessão; com outra rodando, abre ao lado. A casinha na barra do vídeo traz esta
janela de volta sem fechar o vídeo (o cliente imprime `Mostrar o FSS`).

O som (Windows 10 2004+; ver `fluxo-host`, "De onde vem o som"):
- tela inteira: tudo o que o PC toca **menos** o Discord (Configurações >
  Compartilhar > "Não transmitir o som do Discord", ligado por padrão; vai
  `--com-discord` quando desligado). Quem está na call não ouve a própria voz.
- sem o Discord aberto: tudo menos a árvore de processos desta janela
  (`--som-sem-processo`), então o som de quem você assiste não volta na sua
  transmissão. O Windows só tira um programa por captura: compartilhando,
  assistindo e na call ao mesmo tempo, sai o Discord e o som de quem você
  assiste volta junto.
- uma janela: só o som do programa dela.
- com uma saída de som escolhida nas configurações, na tela inteira vale a
  captura daquela saída.

Testado: dois clientes na mesma sala no mesmo PC, cada um numa porta livre.

## Configurações guardadas

Num arquivo de texto, `chave = valor`, gravado a cada mudança:
`%APPDATA%\FlameScreenSharing\configuracoes.txt` (Linux:
`~/.config/flamescreensharing/configuracoes.txt`). Valor estranho ou chave
desconhecida não impede de abrir: vale o padrão daquela chave.

## Temas

Todas as cores e formas vêm do `Tema` (`src/tema.rs`): duas paletas (clara e
escura) com papéis nomeados (`fundo`, `cartao`, `destaque`, `perigo`…) e o raio
dos cantos. Nenhuma outra parte do código escolhe cor.

Para personalizar, crie `tema.txt` na mesma pasta das configurações (o botão
**Criar tema.txt** em Aparência grava um com as cores atuais) e clique em
**Recarregar**:

```text
nome = Meu tema
raio = 6
escuro.destaque = #3366FF
claro.destaque = #2255DD
```

O que faltar vem do tema padrão. Com erro, a janela avisa a linha e usa o
padrão. Um editor de temas dentro da janela pode vir depois, gravando o mesmo
arquivo.

## Segurança

- **Segredos:** a chave do servidor e os códigos vão ao host e ao cliente pela
  entrada padrão (`--segredos-pela-entrada`, `comandos::separar_segredos`), não
  pela linha de comando, que outros programas do PC conseguem ler.
- **Copiar detalhes** mascara os IPs da internet (`177.73.*.*`, `2804:14c:*`;
  `saida::mascarar_ips`); a chave nunca aparece no registro.
- **Expulsar:** quem compartilha vê quem está assistindo e pode expulsar; o IP
  do expulso é recusado até o fim da transmissão (atinge todos daquele IP,
  como os da mesma casa).
- **UPnP** só com a opção marcada (Conexão).

## Botões ao vivo

Os botões mandam uma linha pela entrada padrão do programa, que muda na hora e
confirma imprimindo uma linha; a janela só mostra o estado novo depois da
confirmação.

| Botão | Linha | Confirmação |
|---|---|---|
| Tela ⏷ | `tela N` (N a partir de 0) ou `janela H` | `Tela: monitor N+1 (LxA)` / `Tela: janela H (LxA)` |
| Som | `som ligado` / `som desligado` | `Som: ligado` / `Som: desligado` |
| Pausar / Continuar | `pausar` / `continuar` | `Vídeo: pausado` / `Vídeo: transmitindo` |
| Qualidade ⏷ | `resolucao R` / `fps F` | `Qualidade: LxA, até F fps` |
| Volume / mudo | `volume N` (0 a 100) | `Volume: N%` |
| Expulsar | `expulsar N` | `Espectador N expulso (M no total)` |

Como cada um funciona por dentro está em [fluxo-host](fluxo-host.md#comandos-ao-vivo)
e [fluxo-client](fluxo-client.md). Decisão: ADR-017.

## Atualizações

Ao abrir, a janela pergunta à API do GitHub qual é a versão mais nova
publicada em Releases (`src/atualizacao.rs`). Sendo mais nova:

1. baixa o `MANIFESTO.txt` com o `curl.exe` do Windows e confere a assinatura
   (Ed25519, com a chave pública embutida; ADR-020), que a versão assinada é a
   anunciada e que é maior que esta. Não conferiu: nem baixa o zip;
2. baixa `FlameScreenSharing.zip` e confere o SHA-256 assinado;
3. lista o zip com o `tar.exe` do Windows e recusa qualquer caminho absoluto,
   com `..`, letra de unidade ou `:`; só então abre numa pasta temporária;
4. sem transmissão em andamento, troca os arquivos da pasta do programa (cada
   antigo vira `*.antigo`, apagado na abertura seguinte; o Windows deixa
   renomear o .exe em uso) e abre a versão nova, que mostra "Atualizado para
   a versão X". Com transmissão, espera ela acabar. Se uma troca falha, as
   anteriores são desfeitas.

Só a cópia empacotada (`FlameScreenSharing.exe`) se atualiza; rodando da
compilação, a janela só avisa. Desliga em Configurações > Sobre. Publicar uma
versão: `pacote\publicar_versao.ps1` (decisão: ADR-018). O repositório precisa
ser público. Para testar sem publicar, `FSS_ATUALIZACOES=file:///.../versao.json`
troca o endereço da API.

Teste local (2026-11-12): cópia instalada numa pasta, "versão 9.9.9" falsa por
`file://`, assinada com a chave de verdade: atualizou e reabriu com
`--atualizado`. Com a tag dizendo 9.9.8 (manifesto da 9.9.9) e com o zip
alterado depois de assinado, nada foi trocado. O manifesto de teste foi apagado.

## Nome e pacote para testes

Para quem usa, o programa se chama **FlameScreenSharing (FSS)**: janela, títulos e
descrição da porta no roteador (`NOME` em `src/main.rs`). Por dentro (crates,
comandos, documentação) continua Fluxo.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File pacote\gerar_pacote.ps1
```

Gera `dist\FlameScreenSharing.zip` com `FlameScreenSharing.exe`, `FSS-host.exe`,
`FSS-client.exe` e o `LEIA-ME.txt`, com o runtime do C++ embutido (o outro PC não
precisa do Visual C++ Redistributable). Não tem assinatura digital: o Windows
avisa ao abrir.

## Arquivos

| Arquivo | Conteúdo |
|---|---|
| `src/main.rs` | Abre a janela |
| `src/janela.rs` | As telas (egui): início, outras formas de conectar, sessão com a barra de controles, configurações |
| `src/tema.rs` | Cores e formas: paletas clara e escura, tema em texto (`tema.txt`) |
| `src/config.rs` | Configurações padrão e o arquivo onde ficam |
| `src/lugar.rs` | O vídeo de quem assiste no lugar desta janela (esconder, voltar) |
| `src/grupo.rs` | A thread que fala com o grupo (presença, lista, fotos) |
| `src/perfil.rs` | Nome e foto: preparar a foto (WIC), iniciais, círculo da foto |
| `src/atualizacao.rs` | Verifica, baixa, confere e instala versões novas |
| `src/comandos.rs` | Monta a linha de comando (escolhas + configurações) e confere os códigos antes de abrir qualquer coisa |
| `src/saida.rs` | Interpreta as linhas que os programas imprimem (códigos, início, confirmações dos botões, erro) |
| `src/processo.rs` | Roda o programa sem console; linhas chegam por um canal; comandos e códigos vão pela entrada padrão |

## Por que rodar os executáveis, em vez de chamar as bibliotecas

A lógica do host e do cliente mora no `main` de cada um (laço de captura,
janela de vídeo, thread de áudio). Rodá-los por baixo reaproveita tudo sem
mexer no caminho de vídeo, que é o que tem medição; e um travamento neles não
derruba a janela. O custo é o texto impresso virar contrato: ao mudar um
`println!` de código ou de confirmação, mude `saida.rs` (os testes de lá
guardam o formato).

## Limitações conhecidas

- O menu de qualidade não marca a opção atual (o estado mostra a resolução e o
  fps confirmados).
- Pausar deixa a última imagem parada para quem assiste, sem aviso de "pausado"
  na janela dele.
- Mudanças nas configurações valem a partir da próxima transmissão (a aparência,
  na hora).
- Encerrar fecha o programa na hora: o resumo final do host não aparece.
- Testar no mesmo PC com som nos dois lados faz eco: o host captura o que o
  cliente toca. Entre dois PCs não acontece.
- Cliques simulados por uma ferramenta de automação não chegam à janela (o
  teclado simulado chega, e cliques de verdade também). Os testes
  automatizados navegam com Tab, Espaço e Enter.

## Teste com a janela aberta

2026-11-12, navegando pelo teclado: sala aberta pela janela, cliente entrando
pelo terminal; Som, Pausar/Continuar e Qualidade mudaram a transmissão sem
derrubar o cliente, e a janela mostrou cada confirmação. Entrar numa sala pela
janela abriu o vídeo; mudo e volume mudaram o estado da barra. Tema claro e
escuro trocando na hora. O volume ao vivo foi conferido também pelo terminal
(`Volume: 30%`, `Volume: 0%`, `volume 200` recusado).

## Testes

```powershell
cargo test -p fluxo
```

27 testes, sem abrir janela: leitura das linhas (inclusive as confirmações dos
botões), montagem dos argumentos com as configurações, arquivo de
configurações e de tema (ida e volta, arquivo estranho), mensagens de código
errado e o processo de verdade (entrada e saída por pipe, erro, fim, soltar
encerra). Telas: só conferindo com a janela aberta.
