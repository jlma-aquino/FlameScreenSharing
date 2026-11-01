# Monta dist\FlameScreenSharing.zip para mandar a quem vai testar:
#   powershell -NoProfile -ExecutionPolicy Bypass -File pacote\gerar_pacote.ps1
# Compila com o runtime do C++ embutido (+crt-static): o outro PC não precisa
# do Visual C++ Redistributable. Pasta própria, para não refazer o target\ normal.
$ErrorActionPreference = 'Stop'
$raiz = Split-Path $PSScriptRoot -Parent
Set-Location $raiz

$env:RUSTFLAGS = '-C target-feature=+crt-static'
$alvo = 'target\pacote'
& cargo build --release --target-dir $alvo -p fluxo -p fluxo-host -p fluxo-client
if ($LASTEXITCODE -ne 0) { throw 'a compilação falhou' }

$pasta = 'dist\FlameScreenSharing'
if (Test-Path dist) { Remove-Item dist -Recurse -Force -Confirm:$false }
New-Item -ItemType Directory $pasta | Out-Null
Copy-Item "$alvo\release\fluxo.exe" "$pasta\FlameScreenSharing.exe"
Copy-Item "$alvo\release\fluxo-host.exe" "$pasta\FSS-host.exe"
Copy-Item "$alvo\release\fluxo-client.exe" "$pasta\FSS-client.exe"
# Com BOM: o Bloco de Notas mostra os acentos em qualquer versão do Windows 10.
Get-Content pacote\LEIA-ME.txt -Encoding UTF8 | Set-Content "$pasta\LEIA-ME.txt" -Encoding UTF8

Compress-Archive -Path $pasta -DestinationPath dist\FlameScreenSharing.zip
Get-ChildItem $pasta, dist\FlameScreenSharing.zip | Select-Object Name, Length
