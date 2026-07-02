# mag-telegram-grammers

`mag-telegram-grammers` is a Rust/grammers MTProto backend scaffold for the
Medley Telegram work. It exists because `~/shodan-telegram-session` contains a
grammers JSON session, not a TDLib database. The current C/TDLib bridge can use
the copied API id/hash, but it cannot import that existing login session.

This tool keeps Telegram protocol state outside Medley Lisp and proves the
existing SHODAN session can be used without phone/code auth.

## Build

This crate tracks grammers from its upstream git repository. The current
dependency set requires a newer Rust than Guix `rust` 1.85. Use the rustup
toolchain installed in `~/.cargo/bin`.

```sh
./install.sh
```

The script installs `~/.local/bin/mag-telegram-grammers`.

## State

Defaults:

- source session: `~/shodan-telegram-session/telegram-user.session`
- API credentials: `~/.config/mag-telegram/config`, falling back to
  `~/shodan-telegram-session/telegram-credentials.env`
- runtime session: `~/.local/share/mag-telegram/grammers/session.sqlite`

The source JSON session is never written in-place. `import-session` converts it
into grammers' SQLite storage and writes the runtime session with `0600`
permissions.

## Commands

```sh
mag-telegram-grammers doctor
mag-telegram-grammers session-info
mag-telegram-grammers import-session --force
mag-telegram-grammers online-status
mag-telegram-grammers chats --limit 20 --redact
mag-telegram-grammers messages PEER_DIALOG_ID --limit 20 --redact
mag-telegram-grammers send PEER_DIALOG_ID "text"
```

Use `--show-names` only when you explicitly want chat names and message text in
stdout. Redacted mode hides names and message bodies.

## Verified On This Machine

- The SHODAN session parses as grammers JSON.
- Import creates `~/.local/share/mag-telegram/grammers/session.sqlite`.
- `online-status` reports `authorized=yes`.
- `chats --limit 3 --redact` returns live dialogs.
- `messages PEER_DIALOG_ID --redact` returns live message ids without names or
  message bodies.

## Next Integration Step

The Medley UI still talks to `mag-telegram-bridge` and its
`/tmp/mag-telegram-request` daemon protocol. The next useful step is to make
that bridge delegate to this Rust backend, or to promote this tool into a
persistent daemon that implements the same request/response protocol.
