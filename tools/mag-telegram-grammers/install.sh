#!/bin/sh
set -eu

cd "$(dirname "$0")"
CARGO_BIN="${CARGO:-$HOME/.cargo/bin/cargo}"
"$CARGO_BIN" build --release
mkdir -p "$HOME/.local/bin"
cp target/release/mag-telegram-grammers "$HOME/.local/bin/mag-telegram-grammers"
echo "installed $HOME/.local/bin/mag-telegram-grammers"
