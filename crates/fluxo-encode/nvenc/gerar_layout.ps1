# Regenera crates/fluxo-encode/src/nvenc/layout.rs a partir de nvEncodeAPI.h.
# Uso (de qualquer pasta):  powershell -File crates\fluxo-encode\nvenc\gerar_layout.ps1
# Precisa do Visual Studio Build Tools com "Desenvolvimento para desktop com C++".

$ErrorActionPreference = 'Stop'
$aqui = $PSScriptRoot
$saida = Join-Path $aqui '..\src\nvenc\layout.rs'

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw 'Visual Studio Build Tools (C++) não encontrado.' }
$vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'

$tmp = Join-Path $env:TEMP 'fluxo-nvenc-layout'
New-Item -ItemType Directory -Force $tmp | Out-Null
$exe = Join-Path $tmp 'gen_layout.exe'

# /utf-8: o fonte tem acentos nas mensagens geradas.
cmd /c "`"$vcvars`" >nul && cl /nologo /utf-8 /W3 /I `"$aqui`" `"$aqui\gen_layout.c`" /Fo`"$tmp\\`" /Fe`"$exe`""
if ($LASTEXITCODE -ne 0) { throw 'Falha ao compilar gen_layout.c' }

$texto = & $exe
if ($LASTEXITCODE -ne 0) { throw 'gen_layout.exe falhou' }
[System.IO.File]::WriteAllLines((Resolve-Path (Split-Path $saida)).Path + '\layout.rs', $texto, (New-Object System.Text.UTF8Encoding $false))
Write-Host "Gerado $saida"
