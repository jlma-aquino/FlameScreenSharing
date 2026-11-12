# Publica uma versão no GitHub, que o atualizador do FSS acha sozinho:
#   powershell -NoProfile -ExecutionPolicy Bypass -File pacote\publicar_versao.ps1
#
# Antes: suba o número em Cargo.toml ([workspace.package] version) e escreva o
# que mudou no CHANGELOG.md, numa seção "## [X.Y.Z]". O script:
#   1. gera o pacote (gerar_pacote.ps1);
#   2. grava dist\SHA256SUMS.txt (o atualizador confere o zip com ele);
#   3. com o GitHub CLI (gh) instalado e logado, cria a Release vX.Y.Z com o zip,
#      o SHA256SUMS.txt e as notas do CHANGELOG. Sem o gh, mostra como fazer pelo site.
# O repositório precisa ser público para o atualizador ler as versões sem senha.
$ErrorActionPreference = 'Stop'
$raiz = Split-Path $PSScriptRoot -Parent
Set-Location $raiz

$cargo = Get-Content Cargo.toml -Raw -Encoding UTF8
if ($cargo -notmatch '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') { throw 'não achei a versão em Cargo.toml' }
$versao = $Matches[1]
$tag = "v$versao"
Write-Host "Versão: $versao"

# Notas: a seção "## [X.Y.Z]" do CHANGELOG, até a próxima "## [".
$changelog = Get-Content CHANGELOG.md -Raw -Encoding UTF8
$padrao = '(?ms)^## \[' + [regex]::Escape($versao) + '\][^\n]*\n(.*?)(?=^## \[|\z)'
if ($changelog -notmatch $padrao) { throw "o CHANGELOG.md não tem a seção ## [$versao]" }
$notas = $Matches[1].Trim()

& git rev-parse -q --verify "refs/tags/$tag" | Out-Null
if ($LASTEXITCODE -eq 0) { throw "a tag $tag já existe: suba o número da versão" }
if (& git status --porcelain) { throw 'há mudanças sem commit: faça o commit antes de publicar' }

& powershell -NoProfile -ExecutionPolicy Bypass -File pacote\gerar_pacote.ps1
if ($LASTEXITCODE -ne 0) { throw 'gerar_pacote.ps1 falhou' }

$hash = (Get-FileHash dist\FlameScreenSharing.zip -Algorithm SHA256).Hash.ToLower()
# Sem BOM e com "\n": o formato do sha256sum.
[IO.File]::WriteAllText("$raiz\dist\SHA256SUMS.txt", "$hash  FlameScreenSharing.zip`n")
[IO.File]::WriteAllText("$raiz\dist\NOTAS.md", $notas)
Write-Host "SHA-256: $hash"

if (Get-Command gh -ErrorAction SilentlyContinue) {
    & gh release create $tag dist\FlameScreenSharing.zip dist\SHA256SUMS.txt `
        --title "FlameScreenSharing $versao" --notes-file dist\NOTAS.md --target main
    if ($LASTEXITCODE -ne 0) { throw 'gh release create falhou' }
    Write-Host "Publicada: $tag"
} else {
    Write-Host ''
    Write-Host 'GitHub CLI (gh) não encontrado. Pelo site:'
    Write-Host "  1. GitHub > Releases > Draft a new release; tag $tag (criada em main)."
    Write-Host "  2. Título: FlameScreenSharing $versao; texto: o de dist\NOTAS.md."
    Write-Host '  3. Anexe dist\FlameScreenSharing.zip e dist\SHA256SUMS.txt e publique.'
}
