# fss-icone

**Caminho:** `apps/fss-icone` · **Dependências:** nenhuma · **Plataforma:** qualquer

Desenha o ícone do FlameScreenSharing (um monitor com uma chama na tela e o
sinal de Wi-Fi no canto de cima, à direita) e grava `pacote/icone/fss.ico`:

```text
cargo run --release -p fss-icone             grava pacote/icone/fss.ico
cargo run --release -p fss-icone -- X.ico    grava em outro lugar
```

O desenho está no código, em coordenadas de 0 a 256 (camadas de formas:
retângulos arredondados, curvas de Bézier da chama e arcos do Wi-Fi). Para
mudar cores ou formas, mexa em `camadas()` e rode de novo. Cada tamanho do .ico
(16, 24, 32, 48, 64, 128 e 256) é rasterizado direto, com 8x8 amostras por
pixel; o .ico leva bitmaps BGRA de 32 bits com transparência.

## Onde o ícone entra

- **Executáveis** (`FlameScreenSharing.exe`, `FSS-host.exe`, `FSS-client.exe`):
  o `build.rs` de cada um inclui `pacote/icone/recurso.rs`, que compila um
  recurso com o `rc.exe` do Windows SDK e o passa ao linker. Sem o `rc.exe`,
  o programa compila sem ícone (o cargo avisa).
- **Janela do FSS:** `apps/fluxo/src/main.rs` lê o bitmap de 256 do mesmo
  .ico (`include_bytes!`) para a barra de título e a barra de tarefas.
- **Janela do vídeo:** o `fluxo-client` usa o recurso 1 do próprio executável
  (`LoadIconW`).
