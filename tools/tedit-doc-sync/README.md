# tedit-doc-sync

`tedit-doc-sync` watches `/home/mag/docs`, renders readable Markdown copies to
`/home/mag/docs/md`, and commits/pushes the original TEdit files plus generated
Markdown to `git@github.com:luciusmagn/tedit-docs`.

The converter is intentionally outside Interlisp. It is a single-file Rust
program that builds with `rustc` and does not need Cargo.

Current parser behavior:

- keeps the original TEdit files as the source of truth
- extracts the readable TEdit text payload before the binary piece section
- decodes TEdit's piece table into byte ranges with character/paragraph looks
- renders inline bold, italic, underline, strike, code, and heading styles to Markdown where those looks are present
- emits Markdown with conversion metadata under `/home/mag/docs/md`
- keeps generated files deterministic by recording source mtimes, not render time

A TEdit piece range is a byte range in the plaintext payload plus references to
the character look and paragraph look that apply to that range. That is what
lets the converter preserve a single bold word inside an otherwise normal
paragraph.

## Build

```sh
./install.sh
```

## Run once

```sh
~/.local/bin/tedit-doc-sync --once
```

## Run as a daemon

```sh
~/.local/bin/tedit-doc-sync
```

The default loop interval is one hour. Use `--interval-seconds N` for testing.

## System Shepherd

`install.sh` only installs the binary wrapper. On this machine the daemon is
managed by root Shepherd from `/etc/config.scm` as a system service:

```sh
sudo herd status tedit-doc-sync
```

The service runs `/home/mag/.local/bin/tedit-doc-sync` as `mag:users`, requires
`user-processes` and `NetworkManager`, respawns on failure, and logs to:

```sh
/var/log/tedit-doc-sync.log
```

The old user Shepherd service file is disabled as
`~/.config/shepherd/init.d/tedit-doc-sync.scm.disabled` to avoid duplicate sync
daemons after graphical login.

The wrapper pins the glibc runtime used by the local Rust compiler and removes
that override again for spawned `git` commands.
