#!/bin/sh
set -eu
mkdir -p "$HOME/.local/bin"
real_out="$HOME/.local/bin/tedit-doc-sync.real"
wrapper="$HOME/.local/bin/tedit-doc-sync"
rustc --edition=2021 -O "$(dirname "$0")/src/main.rs" -o "$real_out"
interp=$(readelf -l "$real_out" | sed -n 's/.*interpreter: \(.*\)]/\1/p' | head -1)
if [ -z "$interp" ]; then
  echo "could not find Rust binary interpreter with readelf" >&2
  exit 1
fi
glibc_lib=$(dirname "$interp")
cat > "$wrapper" <<EOF2
#!/bin/sh
export LD_LIBRARY_PATH="$glibc_lib:/home/mag/.guix-profile/lib:/run/current-system/profile/lib"
exec "$real_out" "\$@"
EOF2
chmod +x "$wrapper" "$real_out"
