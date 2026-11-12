# Servidor na Oracle Cloud

Registro do servidor usado como ponto de encontro (e, no futuro, repassador)
do FlameScreenSharing: o que foi feito, em que ordem e como mexer nele.
Nenhum segredo fica aqui.

## Máquina

| | |
|---|---|
| Provedor | Oracle Cloud, plano Always Free |
| Região | Brazil East (São Paulo) |
| Forma | `VM.Standard.A1.Flex` (ARM `aarch64`): 2 núcleos, 11 GB de RAM, 96 GB de disco |
| Sistema | Ubuntu 24.04.5 LTS |
| IP público | `137.131.152.250` |
| Usuário | `ubuntu` (sudo sem senha) |

A Oracle pode recuperar máquinas grátis que ficam ociosas por 7 dias (uso de
CPU, rede e memória abaixo de 20%). Se a máquina sumir, recrie e refaça as
etapas abaixo.

## Acesso

A chave privada fica em `.secrets\` na pasta do projeto, que o `.gitignore`
exclui. **Nunca versionar essa pasta nem mandar a chave para ninguém.** O
SSH do Windows recusa chaves que outros usuários podem ler, então a
permissão foi restrita ao usuário atual:

```powershell
icacls ".secrets\ssh-key-2026-11-12.key" /inheritance:r
icacls ".secrets\ssh-key-2026-11-12.key" /grant:r "${env:USERNAME}:(R)"
```

Entrar:

```powershell
ssh -i ".secrets\ssh-key-2026-11-12.key" ubuntu@137.131.152.250
```

## Arquitetura escolhida

O plano inicial previa LiveKit (SFU), coturn (TURN) e sinalização por
WebSocket com HTTPS. Ficou de fora, porque o FSS não usa WebRTC: tem um
protocolo UDP próprio, e trocar o transporte inteiro arriscaria o delay baixo
que já funciona. No lugar:

| Função | Com o FSS | Estado |
|---|---|---|
| Encontro dos PCs (sem código de resposta) | `fluxo-sinal`, UDP 3478, sem domínio nem HTTPS, só com chave de acesso (ADR-019) | Instalado e testado |
| Sessões de 3 a 5 espectadores e reserva quando o furo do NAT falha | Repassador próprio: o host manda uma vez, o servidor copia para cada espectador | A escrever (precisa de chave de grupo) |

Sem LiveKit e sem coturn, não é preciso domínio.

## Firewall

São duas camadas, e as duas precisam liberar a porta:

1. **Security List da VCN** (painel da Oracle). Liberado em 2026-11-12, de
   qualquer origem: TCP 22, 80, 443, 3478, 5349, 7880-7881; UDP 3478,
   40000-49999, 50000-60000. Várias dessas eram para LiveKit e coturn e
   podem ser fechadas.
2. **iptables da própria máquina.** As imagens Ubuntu da Oracle rejeitam
   tudo que não é a porta 22 (regra `REJECT` no fim da cadeia `INPUT`).
   Regras novas entram antes dela e são gravadas com `netfilter-persistent`
   (já instalado), que as guarda em `/etc/iptables/rules.v4`.

Estado do iptables em 2026-11-12: TCP 22 e UDP 3478 liberadas. Para liberar
mais uma porta (troque a porta e o protocolo):

```bash
n=$(sudo iptables -L INPUT --line-numbers -n | awk '$2=="REJECT"{print $1; exit}')
sudo iptables -I INPUT "$n" -p udp --dport 3478 -m state --state NEW -j ACCEPT
sudo netfilter-persistent save
```

## Etapas

### 2026-11-12: acesso e atualização

1. Permissão da chave restrita (comandos acima) e SSH testado:
   `uname -m` deu `aarch64`.
2. Sistema atualizado:
   ```bash
   sudo apt-get update
   sudo apt-get -y upgrade
   ```
   Não pediu reinício.

### 2026-11-12: `fluxo-sinal`

1. UDP 3478 liberada no iptables e gravada (comando acima). O `fluxo-sinal`
   usa 3478 em vez do padrão 7000 porque a Security List já liberava essa
   porta. Nos apps, o servidor é `137.131.152.250:3478`.
2. Compilador e Rust instalados:
   ```bash
   sudo apt-get install -y build-essential
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
   ```
3. Código copiado para `~/fss` (arquivos versionados ou não ignorados, sem
   `.secrets`) e compilado:
   ```bash
   # No PC, na pasta do projeto (Git Bash)
   git ls-files -z -co --exclude-standard | tar --null -T - -cf - | ssh -i .secrets/ssh-key-2026-11-12.key ubuntu@137.131.152.250 'mkdir -p ~/fss && tar -x -C ~/fss'
   ```
   ```bash
   # No servidor
   cd ~/fss && cargo build --release -p fluxo-sinal
   sudo install -m 755 target/release/fluxo-sinal /usr/local/bin/fluxo-sinal
   ```
4. Serviço do systemd em `/etc/systemd/system/fluxo-sinal.service`: liga com
   a máquina, reinicia 2 s depois de cair e roda com um usuário temporário
   sem permissão de escrita no disco (`DynamicUser`, `ProtectSystem=strict`).
5. Teste do PC de casa com `fluxo-bench receber --sinal` e
   `fluxo-bench enviar --sinal`: o servidor registrou o receber e entregou o
   endereço ao enviar em 0,1 s; 481 frames chegaram, sem perda. Os dois
   estavam na mesma rede, então o vídeo foi pela rede local: o furo de NAT
   entre redes diferentes ainda não foi testado com este servidor.
6. Teste pelo 4G (notebook no celular, `FSS-client --sinal`; PC de casa,
   `FSS-host --sinal --codigo`): o servidor apresentou os dois em 17 s, mas o
   vídeo não passou. O caminho pelo servidor ainda não usava TTL baixo.

### 2026-11-12: salas (ADR-016)

1. Código novo copiado, compilado e testado no servidor (6 testes da
   sinalização), instalado com `sudo install` e serviço reiniciado
   (`sudo systemctl restart fluxo-sinal`, ~2 s fora do ar).
2. Teste do PC de casa pelo servidor: `fluxo-host --sala` e dois
   `fluxo-bench receber --sinal ... --codigo <código da sala>`, entrando com 4 s de diferença. Os dois
   receberam o vídeo, sem perda (604 e 363 frames), e o servidor entregou cada
   um a um socket diferente do host. Mesma rede: o vídeo foi pela rede local.

### 2026-11-12: chave de acesso (ADR-019)

1. Código novo copiado (passo 3 acima), testes da sinalização e da chave
   rodados no servidor, `fluxo-sinal` compilado e instalado.
2. Arquivo de chaves só do root, e a primeira chave:
   ```bash
   sudo mkdir -p /etc/fluxo-sinal
   sudo touch /etc/fluxo-sinal/chaves
   sudo chmod 600 /etc/fluxo-sinal/chaves
   sudo /usr/local/bin/fluxo-sinal --nova-chave 137.131.152.250:3478 --chaves /etc/fluxo-sinal/chaves
   ```
   A chave (id `4c0684dd`) ficou guardada no PC de casa em
   `.secrets\chave-servidor.txt`, fora do git.
3. O serviço recebe o arquivo pelo systemd, sem abrir a permissão dele para o
   usuário temporário (`LoadCredential`):
   ```ini
   LoadCredential=chaves:/etc/fluxo-sinal/chaves
   ExecStart=/usr/local/bin/fluxo-sinal --porta 3478 --chaves ${CREDENTIALS_DIRECTORY}/chaves
   ```
   `daemon-reload` e `restart`: subiu com "1 chave(s) aceita(s): 4c0684dd".
4. Teste do PC de casa: sala com `--chave`; um cliente sem chave (`--sinal`)
   foi ignorado ("pacote sem chave válida … (ignorado)") e não recebeu nada; com
   a chave, o servidor apresentou os dois e o vídeo chegou a 59 fps.

## Chaves

```bash
# Mais uma chave (uma para cada amigo, se quiser saber quem vazou)
sudo /usr/local/bin/fluxo-sinal --nova-chave 137.131.152.250:3478 --chaves /etc/fluxo-sinal/chaves
sudo systemctl restart fluxo-sinal
# Tirar o acesso: apagar a linha da chave (e o comentário acima dela) e reiniciar
sudo nano /etc/fluxo-sinal/chaves
sudo systemctl restart fluxo-sinal
```

O registro mostra só o id de cada chave, nunca a chave.

## Operar o `fluxo-sinal`

```bash
systemctl status fluxo-sinal              # está rodando?
sudo journalctl -u fluxo-sinal -f         # acompanhar o registro (Ctrl+C sai)
sudo systemctl restart fluxo-sinal        # reiniciar
sudo systemctl stop fluxo-sinal           # parar (volta no próximo boot)
sudo systemctl disable --now fluxo-sinal  # parar de vez
```

Atualizar depois de mudar o código: copiar de novo (passo 3), compilar,
`sudo install` e `sudo systemctl restart fluxo-sinal`.

O registro mostra o IP público de quem usa o servidor; ele fica no journal
da máquina.

## Próximos passos

1. Testar a sala (ADR-016) com um amigo em outra rede e de novo pelo 4G,
   agora com TTL baixo.
2. Escrever o repassador.
4. Fechar na Security List as portas que eram de LiveKit e coturn.
