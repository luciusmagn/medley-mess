# mag-telegram-bridge

`mag-telegram-bridge` is the host-side foundation for a text-only Telegram
client in Medley. It keeps Telegram protocol and TDLib state outside Interlisp
and exposes a tiny local command protocol that Medley can render.

The intended live backend is Telegram TDLib through `libtdjson.so`. The bridge
loads TDLib dynamically with `dlopen`, so it still builds on machines where the
Guix `tdlib` package is not installed yet. Without TDLib or credentials it runs
in mock mode, which is useful for proving the Medley UI and request protocol.

Authoritative TDLib references:

- https://core.telegram.org/tdlib/getting-started
- https://core.telegram.org/tdlib/docs/td__json__client_8h.html
- https://core.telegram.org/api/obtaining_api_id

## Build and Install

```sh
./install.sh
```

This installs `~/.local/bin/mag-telegram-bridge`.

## Run

Start the daemon:

```sh
mag-telegram-bridge --daemon
```

Query it from another process:

```sh
mag-telegram-bridge status
mag-telegram-bridge chats
mag-telegram-bridge messages 1001
mag-telegram-bridge send 1001 'hello from Medley'
```

Send an exact request from a file, useful for tests and non-shell request
construction:

```sh
printf 'send 1001 hello with spaces\n' >/tmp/mag-telegram-request.txt
mag-telegram-bridge --request-file /tmp/mag-telegram-request.txt
```

Stop it:

```sh
mag-telegram-bridge quit
```

## Live TDLib Mode

Install TDLib and provide Telegram application credentials. On this machine Guix
currently advertises package `tdlib`.

```sh
guix install tdlib
mkdir -p ~/.config/mag-telegram
chmod 700 ~/.config/mag-telegram
cat >~/.config/mag-telegram/config <<'EOF'
api_id=123456
api_hash=your-api-hash
encryption_key=local database key
# Optional exact TDLib database directory:
# data_dir=/home/mag/.local/share/mag-telegram/tdlib
# Optional explicit library path:
# tdlib_library=/gnu/store/...-tdlib.../lib/libtdjson.so
EOF
chmod 600 ~/.config/mag-telegram/config
mag-telegram-bridge --daemon
```

Environment variables override the config file, which is useful for one-off
tests:

```sh
export MAG_TELEGRAM_API_ID=123456
export MAG_TELEGRAM_API_HASH=your-api-hash
export MAG_TELEGRAM_ENCRYPTION_KEY='local database key'
mag-telegram-bridge --daemon
```

Authorization is driven through bridge commands:

```sh
mag-telegram-bridge auth-phone +420...
mag-telegram-bridge auth-code 12345
mag-telegram-bridge auth-password '2fa-password'
```

The bridge stores TDLib database files below
`~/.local/share/mag-telegram/tdlib` by default. Override with
`MAG_TELEGRAM_DATA_DIR` if needed. Set `MAG_TELEGRAM_TDLIB_LIBRARY` to an
explicit `libtdjson.so` path if it is not discoverable by the dynamic linker.
Set `MAG_TELEGRAM_CONFIG` to use a non-default config file.

## Protocol Files

The daemon reads `/tmp/mag-telegram-request` and writes
`/tmp/mag-telegram-response`. This mirrors the existing Medley debug bridge and
keeps the first Medley UI simple. The Medley UI writes this request file
directly and decodes the UTF-8 response bytes itself, avoiding shell quoting and
Medley `ShellCommand` character translation. Normal Telegram UI requests do not
run `ShellCommand`; on Maiko builds with `UNIX-HANDLECOMM 57`, Medley also
starts the daemon through native host process launch and only falls back to
`ShellCommand` on older binaries. The protocol is intentionally text-oriented:
Medley should render compact lines, not raw TDLib JSON.

Supported requests:

- `status`
- `chats`
- `messages CHAT_ID [FROM_MESSAGE_ID]`
- `older CHAT_ID FROM_MESSAGE_ID`
- `send CHAT_ID TEXT`
- `auth-phone PHONE`
- `auth-code CODE`
- `auth-password PASSWORD`
- `raw JSON`
- `quit`

## Medley UI

Open the Medley dashboard with background menu item `Mag Telegram` or MCP
request `open-telegram`.

Keys:

- `c`: show chats
- `Up` / `Down`: select a chat
- `Enter` / `Right`: open selected chat messages
- `Left`: return to chats/dashboard
- `s`: start bridge on dashboard/chats; send a text message in message view
- `o`: load the next older message page in message view
- `p`: submit phone number for TDLib auth
- `v`: submit login verification code
- `w`: submit 2FA password
- `r`: refresh current view
- `q`: close the window

## Current Scope

Implemented now:

- mock backend with private/group/channel sample data
- daemon request/response protocol
- exact `--request-file` CLI path for file-originated text requests
- persistent config file at `~/.config/mag-telegram/config`, with environment
  variables still available as overrides
- direct Medley request-file path for normal UI requests; shell use is limited
  to fallback daemon startup on older Maiko binaries
- dynamic TDLib loading and basic authorization-state command emission
- TDLib main chat-list ordering from chat position updates
- live update cache for `updateNewChat`, `updateNewMessage`,
  `updateMessageSendSucceeded`, and `messages` history responses using a small
  purpose-built JSON extractor
- compact user-name cache from TDLib `user` / `updateUser` objects so message
  views show names when available and ids only as fallback
- recent TDLib chat-history fetch on `messages CHAT_ID`
- older-page chat-history fetch through `older CHAT_ID FROM_MESSAGE_ID`, using
  bridge-returned `oldest-id` metadata
- send text message request construction with complete `formattedText`
- Medley dashboard, chat list selection, cached message viewing, text send, and
  basic auth prompts

Still intentionally missing:

- robust JSON parser for all TDLib entities
- richer message viewport behavior beyond page-at-a-time backscroll
- contact search and channel joining
- media rendering, reactions, edits, read receipts
- a permanent service definition
