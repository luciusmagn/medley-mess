# mag-telegram-grammers

`mag-telegram-grammers` is a Rust/grammers MTProto backend for the Medley
Telegram work. It exists because `~/shodan-telegram-session` contains a
grammers JSON session, not a TDLib database. The C bridge can use the copied
API id/hash, but TDLib cannot import that existing login session.

This tool keeps Telegram protocol state outside Medley Lisp and proves the
existing SHODAN session can be used without phone/code auth. The C bridge
delegates to this helper when `~/.config/mag-telegram/config` contains
`backend=grammers`.

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
mag-telegram-grammers --request-file /tmp/request
mag-telegram-grammers --daemon
```

Use `--show-names` only when you explicitly want chat names and message text in
stdout. Redacted mode hides names and message bodies.

`--request-file` accepts the bridge protocol commands used by the Medley UI:
`status`, `auth-status`, `chats`, `chats-view`, `chat-at`, `messages`, `older`,
`send`, and `mark-read`. Each request has a bounded 20s Telegram timeout and
aborts the grammers runner task before process exit; this avoids wedging the C
bridge if MTProto stalls.

`--daemon` keeps one grammers MTProto client open and serves the same protocol
through `/tmp/mag-telegram-grammers-request` and
`/tmp/mag-telegram-grammers-response`. This is the normal backend used by
`mag-telegram-bridge` when `backend=grammers` is configured. Daemon `status`
is intentionally cheap and reports `chats=ondemand`; chat counts are fetched by
`chats` and `chats-view`.

## Verified On This Machine

- The SHODAN session parses as grammers JSON.
- Import creates `~/.local/share/mag-telegram/grammers/session.sqlite`.
- `online-status` reports `authorized=yes`.
- `chats --limit 3 --redact` returns live dialogs.
- `messages PEER_DIALOG_ID --redact` returns live message ids without names or
  message bodies.
- `mag-telegram-bridge` reports `backend=grammers`, `auth=ready`, and serves
  `chats-view`/`messages` through the existing Medley-facing protocol.
- 50 repeated `mag-telegram-bridge status` calls leave only one persistent
  `mag-telegram-grammers --daemon`, with no per-request helper processes.

## Next Integration Step

The next useful step is richer message viewport behavior and better cached
dialog/message state in the daemon, so common navigation does not need a fresh
Telegram fetch each time.
