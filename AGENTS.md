# Local Mag Notes

This tree has local, uncommitted Medley/Maiko integration work. Do not discard it.

## Mag file layout

`greetfiles/MAG-EXTRAS` is intentionally a thin loader. The implementation is split across:

- `greetfiles/MAG-COMMON`: shared row drawing, clipboard, prompt helpers.
- `greetfiles/MAG-VTERM`: Mag Shell/Codex terminal orchestration and Ghostty-backed rendering.
- `greetfiles/MAG-DEBUG`: Maiko/Medley debug status wrappers.
- `greetfiles/MAG-GOPHER`: Medley-rendered Gopher browser.
- `greetfiles/MAG-STOCK`: battery who-line and stock Rooms/Notecards/documentation buttons.

Low-level drawing notes live in `docs/mag-low-level-drawing.md`.

## Mag terminal and gopher arrows

`greetfiles/MAG-EXTRAS` has two separate arrow decoders:

- Mag Shell/terminal uses `MAG-VTERM-SPECIAL-KEYID` and low-level write helpers.
- Mag Gopher uses `MAG-GOPHER-SPECIAL-KEYID` and should stay in sync with the terminal mapping, but must be fixed independently when only Gopher is wrong.

Current key-id convention:

- `1` up
- `2` down
- `3` right / open
- `4` left / back
- `5` home
- `7` page up
- `8` page down

Direct special key codes:

- `57346` up
- `57347` down
- `57345` right
- `57344` left

High-byte fallback `(LRSH CH 8) = 1`, using `(LOGAND CH 255)`:

- `82` or `130` up
- `69` or `131` down
- `87` or `132` right
- `84` or `129` left

If Mag Shell arrows work but Gopher arrows are wrong, fix `MAG-GOPHER-SPECIAL-KEYID`; do not perturb the terminal path.

Gopher should delegate key decoding to `MAG-VTERM-SPECIAL-KEYID`. It should
also avoid `TTYDISPLAYSTREAM` in `MAG-GOPHER-TYPEIN`; the working Mag
Shell/Ghostty path reads raw keys without it, and `TTYDISPLAYSTREAM` changes
arrow translation in a way that has made Gopher disagree with Mag Shell.

Concrete failure mode seen on this machine: Mag Shell can have correct arrows
while Gopher reports up correctly but treats left as down. That means Gopher is
on the wrong input/translation path. Do not "fix" this by rotating
`MAG-VTERM-SPECIAL-KEYID` or changing the terminal CSI output; that re-breaks
Fish autosuggestions and Codex selection UIs. Keep Mag Shell as the source of
truth, then make Gopher use the same raw key-id decoder.

When the arrows seem rotated in bizarre ways, the usual cause is patching the
wrong layer or mixing raw Medley key codes with terminal CSI semantics. Do not
change terminal and Gopher mappings together. Stabilize Mag Shell first:
terminal arrows must emit CSI `A/B/C/D` for up/down/right/left respectively.
Only then fix Gopher to interpret the same key ids as selection movement,
open/right, and back/left.

Future-agent warning: this mapping has been repeatedly broken by trying to
"rotate" the arrow table. Do not do that. The direct Medley key codes are not
laid out in visual arrow order:

- `57344` is left and must send CSI `D`.
- `57345` is right and must send CSI `C`.
- `57346` is up and must send CSI `A`.
- `57347` is down and must send CSI `B`.

The terminal key-id abstraction is the source of truth:

- key-id `1` sends CSI `A` / up.
- key-id `2` sends CSI `B` / down.
- key-id `3` sends CSI `C` / right.
- key-id `4` sends CSI `D` / left.

When debugging, test Mag Shell first with Fish autosuggestions and Codex
selection UIs. Right arrow should accept a Fish autosuggestion; left arrow
should move backward. Up/down must move Codex selections without producing
literal `[[[[` spam. Only after Mag Shell is correct should Gopher be changed
to match it.

For live evidence, use the Medley RPC bridge:

- `keys-test` reports the currently loaded decoder table for direct and
  high-byte fallback arrow codes.
- `gopher-keys` reports recent raw keys actually received by
  `MAG-GOPHER-HANDLE-KEY`; use this after pressing arrows inside Mag Gopher to
  see whether Gopher is receiving a different translated stream than Mag Shell.
- `open-gopher` opens a normal Mag Gopher window through the same async path as
  the UI button.
- `shell-self-test` creates a short-lived PTY shell, initializes the native
  Ghostty path, reads output, scans changed rows, reports command 40, and
  closes the test job. Use it to prove terminal creation and native counters
  without depending on the async window-opening path.
- `gopher-state` reports the remembered live Gopher window's host, top,
  selected row, visible row count, entry count, and status.
- `gopher-key-up`, `gopher-key-down`, `gopher-key-left`, and
  `gopher-key-right` inject direct Medley arrow key codes through
  `MAG-GOPHER-HANDLE-KEY` against the remembered live Gopher window.
- `gopher-test-page` replaces the live Gopher window contents with a local
  60-entry test page. On a 31-row viewport, repeated `gopher-key-down` should
  show `top=0 selected=31` followed by `top=10 selected=32`; this verifies the
  viewport jumps by `MAG-GOPHER-VIEW-JUMP` instead of crawling by one line.
- `gopher-self-test` creates the local 60-entry test page, checks the shared
  arrow decoder table, drives raw down/up keys through `MAG-GOPHER-HANDLE-KEY`,
  and reports whether the viewport jump and basic movement semantics pass.
- `restart-rpc` restarts the Medley-side poller after the current response is
  written. Use it after changing the poll loop itself; ordinary `reload-mag`
  updates dispatch handlers but may not replace an already-running loop frame.

## Checks

Run the Interlisp structure checker after edits:

```sh
/home/mag/src/medley/scripts/mag-check-interlisp.js \
  /home/mag/src/medley/greetfiles/MAG-EXTRAS \
  /home/mag/src/medley/greetfiles/MAG-COMMON \
  /home/mag/src/medley/greetfiles/MAG-VTERM \
  /home/mag/src/medley/greetfiles/MAG-DEBUG \
  /home/mag/src/medley/greetfiles/MAG-GOPHER \
  /home/mag/src/medley/greetfiles/MAG-STOCK \
  /home/mag/src/medley/greetfiles/MAG-NOGREET
```

A real load test is still required for semantic errors; the checker only catches structural problems and likely undefined local functions.

## Terminal/rendering direction

Keep Mag Shell (`MAG-VTERM-*`) fast enough to run Codex, but avoid moving terminal logic back into slow Interlisp drawing loops. Prefer Maiko/C-backed operations for:

- dirty row discovery and copyout
- box drawing and glyph fallback
- cursor painting
- key encoding to the PTY
- future debug/inspection commands callable through `UNIX-HANDLECOMM`

The current Lisp side should mostly orchestrate windows, menus, and redraw scheduling.

`MAG-GHOSTTY-REFRESH` uses Maiko command 37 for hash-based changed-row
detection when the refresh is not forced. Forced refreshes still repaint every
row. Keep the row-list VM page separate from the row-content VM page; reusing
one page corrupts the changed-row list while rows are being redrawn.

## Maiko debug command

Local Maiko command `UNIX-HANDLECOMM 38` writes a compact text status report into a caller-provided buffer. In Medley use `MAG-DEBUG-REPORT`, `MAG-DEBUG-SHOW`, or `MAG-DEBUG-WRITE-REPORT`.

If `MAG-DEBUG-REPORT` says command 38 is unavailable, rebuild/restart Maiko; the Lisp side can be newer than the running `lde` binary.

The local command 38 report also includes compact Ghostty/Mag terminal metrics
after restarting into the rebuilt Maiko binary:

- `gt-write-calls`
- `gt-write-bytes`
- `gt-render-updates`
- `gt-change-scans`
- `gt-changed-rows`
- `gt-write-us`
- `gt-update-us`
- `gt-scan-us`
- `gt-last-update-us`
- `gt-last-scan-us`
- `gt-last-changed`
- `gt-hash-rows`

Local Maiko command `UNIX-HANDLECOMM 39` reads and consumes `/tmp/medley-mag-request` into a caller-provided buffer. Local Maiko command `UNIX-HANDLECOMM 40` reports a single Mag terminal job's native state into a caller-provided buffer. `MAG-DEBUG-RPC-START` runs a safe Medley-side poller that dispatches only these commands:

- `ping`
- `debug-report`
- `write-debug-report`
- `battery`
- `who-line-battery`
- `reload-mag`
- `restart-rpc`
- `open-shell`
- `shell-self-test`
- `shell-state`
- `open-gopher`
- `keys-test`
- `gopher-keys`
- `gopher-state`
- `gopher-test-page`
- `gopher-self-test`
- `gopher-key-up`
- `gopher-key-down`
- `gopher-key-left`
- `gopher-key-right`
- `keys-help`

Keep `reload-mag` asynchronous. Loading `MAG-EXTRAS` inside the RPC poller
itself can redefine/reset the code that is currently handling the request and
has blocked the bridge. The poller should spawn `MAG-RPC-RELOAD` and return.

## MCP bridge

`scripts/mag-medley-mcp.js` is registered in `/home/mag/.codex/config.toml` as `medley_mag`. It exposes process/log/status tools and `medley_request`, which writes `/tmp/medley-mag-request`, waits for `/tmp/medley-mag-response`, and returns the live Medley response.

Smoke test:

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"medley_request","arguments":{"command":"ping","timeout_ms":5000}}}' \
  | /home/mag/src/medley/scripts/mag-medley-mcp.js
```

Normal launch path: `/home/mag/.local/bin/medley-interlisp`, mirrored as `scripts/mag-medley-interlisp`, starts `apps.sysout` with `--greet -` and sets `MAIKO_STARTUP_TYPEAHEAD_FILE` to a tiny file containing `(IL:LOAD ".../MAG-NOGREET" T)`. Maiko's local X11 startup typeahead hook injects that form after `MAIKO_STARTUP_TYPEAHEAD_DELAY` seconds. This replaced the old EXWM/emacsclient synthetic loader, which was fragile because EXWM does not always expose a live Medley buffer during startup. Do not pass `--nofork`/`-NF` here: Maiko uses that flag to skip `fork_Unix`, and Mag Shell/`FORK-SHELL` need the Unix communication helper.
