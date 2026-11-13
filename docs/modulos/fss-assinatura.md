# fss-assinatura

**Caminho:** `apps/fss-assinatura` · **Dependências:** `fluxo-crypto`, `sha2` · **Plataforma:** qualquer

A chave que assina as atualizações (ADR-020). Só quem publica as versões usa,
no próprio PC; os programas instalados trazem a chave pública
(`apps/fluxo/src/atualizacao.rs`) e só aceitam versão assinada com a privada.

```text
fss-assinatura gerar ARQUIVO        cria a chave privada (não sobrescreve) e mostra a pública
fss-assinatura publica ARQUIVO      mostra a pública de uma privada
fss-assinatura assinar --chave ARQUIVO --versao X.Y.Z --arquivo ZIP --saida MANIFESTO.txt
```

A chave do projeto está em `.secrets\assinatura-atualizacoes.txt`, fora do git.
**Faça cópia de segurança**: sem ela, não há como publicar uma versão que os
programas já instalados aceitem (trocar a chave exige instalar uma versão nova
à mão em todos os PCs). `pacote\publicar_versao.ps1` assina sozinho.

Formato do manifesto: `fluxo-crypto`, `manifesto.rs`.
