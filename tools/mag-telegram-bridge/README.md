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
mag-telegram-bridge --doctor
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
mag-telegram-bridge --request-file-ascii /tmp/mag-telegram-request.txt
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

`encryption_key` is written as a normal local string in the config file. The
bridge base64-encodes it when sending TDLib JSON, because TDLib represents
`bytes` fields as base64 strings in JSON mode.

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
mag-telegram-bridge auth-register First Last
```

The bridge stores TDLib database files below
`~/.local/share/mag-telegram/tdlib` by default. Override with
`MAG_TELEGRAM_DATA_DIR` if needed. Set `MAG_TELEGRAM_TDLIB_LIBRARY` to an
explicit `libtdjson.so` path if it is not discoverable by the dynamic linker.
Set `MAG_TELEGRAM_CONFIG` to use a non-default config file.

## SHODAN MTProto Session

`~/shodan-telegram-session` contains a grammers JSON MTProto session plus API
credentials copied from SHODAN. The API id/hash are usable by the current TDLib
bridge config, but the session file itself is not a TDLib database and cannot
be imported by this C/TDLib backend.

`tools/mag-telegram-grammers` is the Rust/grammers backend for that session.
It converts the source JSON session into
`~/.local/share/mag-telegram/grammers/session.sqlite` and has been verified to
report `authorized=yes`, list dialogs, and fetch redacted message ids through
the existing session. Do not let two clients write the same source session file
concurrently; the Rust tool writes its own backend-owned SQLite copy instead.

The Medley UI still talks to this C bridge. Set `backend=grammers` and
`grammers_command=/home/mag/.local/bin/mag-telegram-grammers` in
`~/.config/mag-telegram/config` to make the bridge delegate normal requests to
the Rust backend while keeping the same `/tmp/mag-telegram-request` protocol.
The bridge starts and reuses a persistent `mag-telegram-grammers --daemon`
process, forwarding requests through `/tmp/mag-telegram-grammers-request` and
`/tmp/mag-telegram-grammers-response`. The Rust daemon holds the MTProto client
open and bounds each request at 20s so a slow Telegram operation cannot wedge
the C bridge indefinitely. It also caches dialog rows and message pages for
normal Medley navigation, so moving around the chat list and reopening a recent
message page does not force a Telegram fetch each time.

## Protocol Files

The C bridge daemon reads `/tmp/mag-telegram-request` and writes
`/tmp/mag-telegram-response`. Its client path waits for the request file to be
consumed before accepting a response, then deletes the response after reading
it; this avoids reading stale replies under fast MCP/Medley request loops.

Medley itself does not write the daemon request file directly anymore. It writes
the exact request to `/tmp/mag-telegram-medley-request` and invokes
`mag-telegram-bridge --request-file-ascii /tmp/mag-telegram-medley-request`.
The ASCII mode keeps host CLI Unicode behavior intact while protecting
Interlisp `ShellCommand` stream reading from Telegram names/messages that
contain non-ASCII bytes. The protocol is intentionally text-oriented: Medley
should render compact lines, not raw TDLib JSON.

Supported requests:

- `status`
- `doctor`
- `auth-status`
- `chats`
- `chats-view SELECTED TOP VISIBLE`
- `chat-at INDEX`
- `messages CHAT_ID [FROM_MESSAGE_ID]`
- `older CHAT_ID FROM_MESSAGE_ID`
- `send CHAT_ID TEXT`
- `mark-read CHAT_ID [MESSAGE_ID]`
- `auth-phone PHONE`
- `auth-code CODE`
- `auth-password PASSWORD`
- `auth-register FIRST [LAST]`
- `raw JSON`
- `quit`

For agent-side automation, the Medley MCP server also exposes
`mag_telegram_request`. It writes the same request/response files, accepts the
normal safe commands above, and intentionally excludes `raw` and `quit` from
the schema. Use it for parameterized auth and message operations instead of
adding new Lisp or shell-quoted request paths.

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
- `n`: submit first/last name if TDLib asks for registration
- `d`: show bridge doctor output
- `r`: refresh current view
- `q`: close the window

## Current Scope

Implemented now:

- mock backend with private/group/channel sample data
- daemon request/response protocol
- exact `--request-file` CLI path for file-originated text requests
- ASCII-safe `--request-file-ascii` CLI path used by Medley
- persistent config file at `~/.config/mag-telegram/config`, with environment
  variables still available as overrides
- `backend=grammers` config and environment selection, with
  `grammers_command` override for the Rust helper
- secret-safe `doctor` diagnostics for config, credential presence, TDLib
  library/symbol loading, and grammers backend reachability
- private Medley request-file path through `--request-file-ascii`; direct
  daemon request/response files are owned by the C bridge client/daemon pair
- grammers delegation for `status`, `auth-status`, `chats`,
  `chats-view`, `chat-at`, `messages`, `older`, `send`, and `mark-read`
  through a persistent Rust `mag-telegram-grammers --daemon`
- short-lived dialog/message caches in the Rust daemon, with message-cache
  invalidation after sending
- dynamic TDLib loading and authorization-state command emission for
  phone/code/password/registration flows
- TDLib main chat-list ordering from chat position updates
- C-side chat kind classification for private chats, basic groups,
  supergroups, channels, and secret chats
- compact chat previews from TDLib unread counts and last-message updates,
  rendered as `unread=N :: sender: text` in chat listings
- C-rendered chat selection viewports and selected-chat lookup through
  `chats-view` and `chat-at`, keeping row formatting out of Interlisp
- `messages CHAT_ID` and `mark-read CHAT_ID [MESSAGE_ID]` use TDLib
  `viewMessages` to mark displayed text as viewed/read in live mode
- live update cache for `updateNewChat`, `updateNewMessage`,
  `updateMessageSendSucceeded`, and `messages` history responses using a small
  purpose-built JSON extractor
- caption text from non-text message contents is preserved as `[caption] ...`;
  media itself is intentionally not rendered
- compact user-name cache from TDLib `user` / `updateUser` objects so message
  views show names when available and ids only as fallback
- recent TDLib chat-history fetch on `messages CHAT_ID`
- older-page chat-history fetch through `older CHAT_ID FROM_MESSAGE_ID`, using
  bridge-returned `oldest-id` metadata
- send text message request construction with complete `formattedText`
- Medley dashboard, chat list selection, cached message viewing, text send, and
  basic auth prompts
- Medley-side request wait increased to 25s so grammers' bounded network
  timeout does not trigger false daemon restart attempts

Still intentionally missing:

- robust JSON parser for all TDLib entities
- richer message viewport behavior beyond page-at-a-time backscroll
- contact search and channel joining
- media rendering, reactions, edits, read receipts
- a permanent service definition
