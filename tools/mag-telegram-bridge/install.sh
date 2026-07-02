#!/bin/sh
set -eu
cd "$(dirname "$0")"
mkdir -p "$HOME/.local/bin"
cc=${CC:-gcc}
$cc -std=c11 -O2 -Wall -Wextra src/mag-telegram-bridge.c -ldl -o "$HOME/.local/bin/mag-telegram-bridge"
echo "installed $HOME/.local/bin/mag-telegram-bridge"
