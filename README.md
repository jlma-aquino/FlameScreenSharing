<p align="center">
  <img src="docs/imagens/icone.png" width="96" alt="Ícone do FlameScreenSharing">
</p>

<h1 align="center">FlameScreenSharing</h1>

<p align="center">
  Compartilhamento de tela entre amigos, com pouco atraso e o vídeo indo direto de um PC para o outro.<br>
  Para Windows, escrito em Rust.
</p>

---

O FlameScreenSharing (FSS) nasceu para substituir o compartilhamento de tela do
Discord num grupo de amigos: mais qualidade (até 1440p ou 4K, 60 ou 120 fps),
menos atraso e sem precisar de conta. Quem compartilha clica em
**Compartilhar** e manda um código; quem assiste cola o código e o vídeo abre
em alguns segundos.

- **Pouco atraso:** da captura até a imagem no PC de quem assiste, cerca de 5 ms
  na mesma rede (medido: 5,1 ms p50 entre dois PCs no Wi-Fi; veja
  [Resultados](#resultados)). Pela internet,
  soma-se a metade do ping.
- **Direto entre os PCs:** o vídeo não passa por servidor nenhum quando a rede
  deixa; o servidor só apresenta os dois lados.
- **Tudo cifrado:** vídeo, som e comandos, de ponta a ponta.

## Funcionalidades

**Compartilhar**
- Tela inteira (qualquer monitor) ou **uma janela só**.
- **Som do PC** junto, em Opus. Na tela inteira, o som do **Discord fica de fora**
  (quem está na call não ouve a própria voz); numa janela, vai **só o som do
  programa dela**.
- Até **4 pessoas** assistindo ao mesmo tempo, cada uma entrando sozinha com o
  código.
- Durante a transmissão, sem derrubar ninguém: trocar de tela ou janela, ligar e
  desligar o som, pausar, mudar resolução e fps, tirar alguém da sala.
- Codecs **AV1, HEVC e H.264**, no encoder da placa (NVIDIA NVENC, AMD e Intel
  pelo Media Foundation) ou na CPU. O codec é negociado: vale o melhor que a
  placa de quem assiste decodifica.
- Qualidade que se ajusta à rede sozinha (bitrate adaptativo, correção de
  pacotes perdidos).

**Assistir**
- O vídeo abre no lugar da janela do FSS, com uma barra de controles por cima
  que some com o mouse parado (mudo, volume, estatísticas, tela cheia, sair).
- Decodificação na placa de vídeo (D3D11/DXVA).
- Dá para assistir a mais de uma pessoa e compartilhar ao mesmo tempo, cada
  sessão numa aba.

**Conexão**
- **Salas:** o servidor de sinalização apresenta os dois lados e eles abrem o
  caminho direto (furo de NAT).
- **Repassador:** quando o caminho direto não abre (CGNAT, internet de celular,
  roteadores restritivos), o vídeo passa pelo servidor, ainda cifrado.
- **Sem servidor:** trocando códigos de conexão e de resposta pelo chat, ou na
  mesma rede pelo endereço.

**Programa**
- Atualiza sozinho pelas versões publicadas aqui no GitHub: baixa, confere a
  assinatura (Ed25519), instala e abre de novo. Nunca no meio de uma transmissão.
- Tema claro e escuro (ou igual ao sistema) e cores personalizáveis.
- Configurações guardadas: qualidade, codec, encoder, de onde vem e onde toca o
  som, **qual placa de vídeo usar** (notebooks com duas placas), entre outras.

## Como usar

1. Baixe o `FlameScreenSharing.zip` da [versão mais recente](../../releases/latest),
   extraia a pasta inteira e abra o `FlameScreenSharing.exe`.
2. Na primeira vez, cole a **chave do servidor** (`FSS1-…`) em Configurações >
   Conexão. Ela vem de quem roda o servidor (veja [Servidor](#servidor)).
3. Quem compartilha: **Compartilhar** > **Copiar código** e manda o código.
4. Quem assiste: cola o código em **Entrar numa sala** > **Entrar**.

Requisitos: Windows 10 (versão 2004 ou mais nova) ou Windows 11, com placa de
vídeo que codifique em hardware (recomendado). O programa não tem assinatura
digital: o Windows pode avisar na primeira vez. O passo a passo para quem vai
usar está no [LEIA-ME](pacote/LEIA-ME.txt) que vai junto no zip.

## Como funciona

```text
 QUEM COMPARTILHA (fluxo-host)                       QUEM ASSISTE (fluxo-client)
 ┌──────────┐  ┌───────────┐  ┌────────────┐          ┌────────────┐  ┌──────────┐  ┌────────┐
 │ Captura  │─►│  Encoder  │─►│ Protocolo  │─ UDP ──► │ Protocolo  │─►│ Decoder  │─►│  Tela  │
 │ DXGI/WGC │  │ NVENC, MF │  │ cifra,     │  cifrado │ remonta,   │  │ D3D11    │  │        │
 │ (na GPU) │  │ ou CPU    │  │ fragmenta  │          │ recupera   │  │ (DXVA)   │  │        │
 └──────────┘  └───────────┘  └────────────┘          └────────────┘  └──────────┘  └────────┘
 ┌──────────┐  ┌───────────┐         ▲                                 ┌──────────┐  ┌────────┐
 │ Som WASAPI│─►│   Opus    │─────────┘                                 │   Opus   │─►│  Som   │
 └──────────┘  └───────────┘                                            └──────────┘  └────────┘

            servidor (fluxo-sinal): apresenta os dois lados; repassa só se o caminho direto não abrir
```

- O quadro nunca sai da placa de vídeo no PC de quem compartilha: a captura
  entrega uma textura, o encoder lê dela, e só os bytes comprimidos vão para a
  rede.
- O transporte é UDP com protocolo próprio ([PROTOCOLO](docs/PROTOCOLO.md)):
  um pacote perdido não trava os seguintes, e o quadro quebrado é recuperado
  pela correção de erros ou pulado.
- O pareamento usa SPAKE2 sobre o código da sala: só quem tem o código entra,
  e cada sessão tem chaves próprias.
- A janela (`fluxo`) roda o `fluxo-host` e o `fluxo-client` por baixo e
  conversa com eles pela entrada e saída padrão.

Detalhes: [ARQUITETURA](docs/ARQUITETURA.md) e as decisões técnicas em
[DECISOES](docs/DECISOES.md).

## Resultados

Medições feitas durante o desenvolvimento, quase todas com uma RTX 5050 em
1080p. O registro completo de cada teste está nos documentos dos módulos.

**Atraso (captura até o quadro completo em quem assiste)**

| Cenário | p50 | p99 |
|---|---|---|
| Mesma máquina, área de trabalho | 2,9 ms | 7,1 ms |
| Mesma máquina, vídeo de jogo a 60 fps | 5,5 ms | 7,6 ms |
| Entre dois PCs no Wi-Fi | 5,1 ms | 29,9 ms |
| Até o `Present` na janela de quem assiste (área de trabalho) | 3,1 ms | 9,3 ms |

O som chega cerca de 10 ms depois de capturado, na mesma rede, e sai pela
caixa de som uns 60–70 ms depois (com o buffer contra variação da rede).

**Cada etapa**

| Etapa | Resultado |
|---|---|
| Encode NVENC, 1080p a 20 Mbps | 2,3 ms p50, 2,8 ms p99 por quadro; PSNR de luminância 48,8 dB |
| Decode na GPU (D3D11) | 0,17 ms de CPU por quadro (p50), 255 de 255 quadros |
| Pela internet, PC de casa para um notebook no 4G do celular | 59,9 fps, 0% de perda, RTT 58 ms, encode 2,4 ms |
| 4 pessoas assistindo ao mesmo tempo (34 s) | 0 quadros perdidos; a quinta foi recusada |
| Entre dois PCs no Wi-Fi (30 s) | 40 085 pacotes, 0 quadros perdidos durante o vídeo |

**Rede ruim** (link simulado de 8 Mbps, com o host pedindo 20):

| | Bitrate fixo | Bitrate adaptativo do FSS |
|---|---|---|
| Quadros mostrados | 31 fps | 51 fps |
| Atraso p99 | 223 ms | 95 ms |
| Perda depois de estabilizar | 45–65% | 0% |

Com 3% de pacotes perdidos, a imagem estragada se recupera em média em 31 ms.

**No dia a dia:** o grupo usa o FSS entre casas diferentes pelo servidor de
salas, no lugar do compartilhamento do Discord. Também foram testados um
notebook Linux assistindo (pela linha de comando, decodificando na CPU) e a
escolha da placa de vídeo (60 fps, sem perda).

## Servidor

As salas usam um servidor de sinalização próprio, o
[fluxo-sinal](docs/modulos/fluxo-sinal.md): leve, só UDP, roda numa VM
pequena (Linux, inclusive ARM). Cada servidor tem as suas **chaves de acesso**
(`FSS1-…`, com o endereço do servidor e um segredo): quem baixa o FSS daqui não
usa o servidor de ninguém sem a chave. Para um grupo novo, rode o seu:

```bash
cargo build --release -p fluxo-sinal
fluxo-sinal --nova-chave <IP público>:3478 --chaves chaves.txt
fluxo-sinal --porta 3478 --chaves chaves.txt
```

Como o servidor deste projeto foi montado: [SERVIDOR](docs/SERVIDOR.md).

## Segurança

- Vídeo, som e comandos cifrados com chaves que só os dois lados têm; o
  servidor (inclusive no repasse) não consegue ler nada.
- Atualizações só são instaladas com a assinatura de quem publica e nunca
  voltam para uma versão mais antiga.
- Quem compartilha pode tirar alguém da sala; os detalhes copiados para
  suporte escondem os IPs e nunca levam a chave do servidor.
- Limite de tentativas de pareamento por endereço, e o UPnP do roteador só é
  usado se a opção estiver ligada.

Quem tem o código vê a tela e ouve o som de quem compartilha: mande só para
quem você quer.

## Compilar

Pré-requisitos e passo a passo: [COMPILAR](docs/COMPILAR.md). Em resumo
(Rust estável e as ferramentas de C++ do Visual Studio):

```powershell
cargo build --release
cargo test --workspace
.\target\release\fluxo.exe
```

O pacote para distribuir (com o runtime do C++ embutido) sai de
`pacote\gerar_pacote.ps1`, e `pacote\publicar_versao.ps1` assina e publica uma
versão.

Cada etapa também roda e se mede isolada, pela linha de comando
([fluxo-bench](docs/modulos/fluxo-bench.md)):

```powershell
.\target\release\fluxo-bench.exe listar      # monitores e placas de vídeo
.\target\release\fluxo-bench.exe bench --segundos 10
.\target\release\fluxo-client.exe --sala <código> --chave FSS1-…
.\target\release\fluxo-host.exe --sala --chave FSS1-…
```

## Estrutura do projeto

```text
├── crates/                 bibliotecas, uma responsabilidade cada
│   ├── fluxo-proto/        protocolo de rede (puro Rust, sem I/O)
│   ├── fluxo-capture/      captura da tela e de janelas (DXGI, Windows.Graphics.Capture)
│   ├── fluxo-encode/       encoder AV1/HEVC/H.264: NVENC, Media Foundation ou CPU
│   ├── fluxo-decode/       decoder na GPU (Media Foundation + DXVA)
│   ├── fluxo-audio/        som: captura WASAPI (por programa), Opus, buffer de jitter
│   ├── fluxo-crypto/       código de pareamento, SPAKE2, cifra, assinatura das versões
│   └── fluxo-net/          transporte UDP: pacing, FEC, RTT, salas, furo de NAT, repasse
├── apps/                   executáveis
│   ├── fluxo/              a janela (FlameScreenSharing.exe)
│   ├── fluxo-host/         transmite (FSS-host.exe)
│   ├── fluxo-client/       mostra (FSS-client.exe)
│   ├── fluxo-sinal/        servidor de sinalização e repassador
│   ├── fluxo-bench/        testa e mede cada etapa isolada
│   ├── fss-assinatura/     assina as versões publicadas
│   └── fss-icone/          desenha o ícone
├── pacote/                 scripts do pacote e da publicação, LEIA-ME, ícone
└── docs/                   arquitetura, protocolo, decisões (ADRs), roteiro,
                            servidor e um documento por módulo (docs/modulos)
```

O que mudou em cada versão: [CHANGELOG](CHANGELOG.md). Próximos passos:
[ROADMAP](docs/ROADMAP.md).
