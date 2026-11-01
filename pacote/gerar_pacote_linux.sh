#!/bin/sh
# Monta dist/FlameScreenSharing-linux.tar.gz, para quem vai assistir no Linux.
# Rode no próprio Linux, na pasta do projeto (precisa de Rust e g++):
#   sh pacote/gerar_pacote_linux.sh
# O executável depende da glibc do PC onde foi compilado: compile na distribuição
# mais antiga que for usar.
set -e
cargo build --release -p fluxo -p fluxo-client --features fluxo-client/portatil

pasta=dist/FlameScreenSharing
rm -rf dist
mkdir -p "$pasta"
cp target/release/fluxo "$pasta/FlameScreenSharing"
cp target/release/fluxo-client "$pasta/FSS-client"
strip "$pasta/FlameScreenSharing" "$pasta/FSS-client"
cp pacote/LEIA-ME-LINUX.txt "$pasta/LEIA-ME.txt"
tar -C dist -czf dist/FlameScreenSharing-linux.tar.gz FlameScreenSharing
ls -l "$pasta" dist/FlameScreenSharing-linux.tar.gz
