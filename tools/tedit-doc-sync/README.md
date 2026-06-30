# tedit-doc-sync

`tedit-doc-sync` watches `/home/mag/docs`, renders readable Markdown copies to
`/home/mag/docs/md`, and commits/pushes the original TEdit files plus generated
Markdown to `git@github.com:luciusmagn/tedit-docs`.

The converter is intentionally outside Interlisp. It is a single-file Rust
program that builds with `rustc` and does not need Cargo.

Current parser behavior:

- keeps the original TEdit files as the source of truth
- extracts the readable TEdit text payload before the binary/trailer section
- detects Mag style metadata such as `MAG-TEDIT HEADING1` in the TEdit trailer
- emits Markdown with conversion metadata under `/home/mag/docs/md`

The next parser step is full TEdit piece-range decoding, so headings/bold/italic
can be reconstructed by range rather than noted as detected metadata.

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

## User Shepherd

`install.sh` only installs the binary wrapper. The matching Shepherd service is
`tedit-doc-sync.scm`; on this machine it is installed as:

```sh
~/.config/shepherd/init.d/tedit-doc-sync.scm
```

Start it with:

```sh
herd start tedit-doc-sync
```

The wrapper pins the glibc runtime used by the local Rust compiler and removes
that override again for spawned `git` commands.
