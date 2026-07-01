#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
out="$HOME/.local/bin/tedit-google-sync"

find_lib_dir() {
    lib_name=$1
    shift
    for candidate in "$@"; do
        if [ -n "$candidate" ] && [ -e "$candidate" ]; then
            dir=$(dirname "$(readlink -f "$candidate")")
            if [ -e "$dir/$lib_name" ]; then
                printf '%s\n' "$dir"
                return 0
            fi
        fi
    done
    found=$(find /gnu/store -path "*/lib/$lib_name" -print 2>/dev/null | head -n 1 || true)
    if [ -n "$found" ]; then
        dirname "$(readlink -f "$found")"
        return 0
    fi
    return 1
}

patchelf_bin=$(command -v patchelf || true)
if [ -z "$patchelf_bin" ]; then
    patchelf_store=$(guix build patchelf 2>/dev/null | head -n 1 || true)
    patchelf_bin="$patchelf_store/bin/patchelf"
fi
[ -x "$patchelf_bin" ] || { echo "patchelf not found" >&2; exit 1; }

if command -v rustup-guix-fix >/dev/null 2>&1; then
    rustup-guix-fix >/dev/null
fi

libgcc_file=$(gcc -print-file-name=libgcc_s.so.1 2>/dev/null || true)
libgcc_dir=$(find_lib_dir libgcc_s.so.1 "$libgcc_file")
zlib_store=$(guix build zlib 2>/dev/null | head -n 1 || true)
zlib_dir=$(find_lib_dir libz.so.1 "$zlib_store/lib/libz.so.1")

export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-rpath,$libgcc_dir -C link-arg=-Wl,-rpath,$zlib_dir"
cargo build --release --manifest-path "$root/Cargo.toml"
install -Dm755 "$root/target/release/tedit-google-sync" "$out"
"$patchelf_bin" --set-rpath "$libgcc_dir:$zlib_dir" "$out"

mkdir -p "$HOME/.config/tedit-google-sync" "$HOME/.local/state/tedit-google-sync"
if [ ! -e "$HOME/.config/tedit-google-sync/bitcoin.json" ]; then
    cat > "$HOME/.config/tedit-google-sync/bitcoin.json" <<'JSON'
{
  "markdown": "/home/mag/docs/md/bitcoin.md",
  "title": "Bitcoin",
  "client_secret": "/home/mag/.config/tedit-google-sync/client_secret.json",
  "token_path": "/home/mag/.local/state/tedit-google-sync/token.json",
  "state_path": "/home/mag/.local/state/tedit-google-sync/bitcoin.state.json",
  "open_browser": true,
  "interval_seconds": 0
}
JSON
fi

"$out" --dry-run --once
