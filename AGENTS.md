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

Maiko command 25 intentionally writes unmodified arrow key ids 1-4 as explicit
CSI bytes instead of routing them through libghostty's key encoder. If Mag
Shell arrows are swapped while `keys-test` is correct, inspect that direct CSI
fast path before changing any Lisp decoder table.

`MAG-VTERM-SEND-KEY` must not contain a second raw arrow table. It should map
raw Medley key codes through `MAG-VTERM-SPECIAL-KEYID` and then write through
command 25 or `MAG-VTERM-WRITE-KEYID-FALLBACK`. Reintroducing inline
`57344`/`57345`/`57346`/`57347` CSI writes makes future rotations easy.

When debugging, test Mag Shell first with Fish autosuggestions and Codex
selection UIs. Right arrow should accept a Fish autosuggestion; left arrow
should move backward. Up/down must move Codex selections without producing
literal `[[[[` spam. Only after Mag Shell is correct should Gopher be changed
to match it.

For live evidence, use the Medley RPC bridge:

- `keys-test` reports the currently loaded decoder table for direct and
  high-byte fallback arrow codes.
- `key-encode-test` reports the native Maiko bytes for terminal key ids 1-4
  and verifies they are CSI `A/B/C/D` for up/down/right/left.
- `shell-key-probe` starts a short-lived raw PTY reader, sends Mag terminal
  key ids 1-4 through command 25, and verifies the actual shell receives
  `ESC [ A`, `ESC [ B`, `ESC [ C`, `ESC [ D`.
- `config-report` reports Maiko runtime configuration such as VM size, timer
  interval, no-scroll state, and window/screen dimensions.
- `performance-report` verifies the local performance-oriented launch config:
  256 MB VM, 10 ms timer, and Maiko `--noscroll`.
- `status-report` returns a single combined snapshot with debug, performance,
  battery, key decoder, active shell, and active Gopher state.
- `process-status` returns a bounded process snapshot for the RPC poller,
  active Mag Shell window, active Mag Gopher window, and known Mag worker
  names. It does not run arbitrary eval or list every process.
- `native-jobs` returns Maiko's bounded native job list from
  `UNIX-HANDLECOMM 46`: aggregate job counts plus compact live job lines that
  fit in one VM page.
- `goal-status` returns a non-mutating summary of the original Mag integration
  goal evidence: split modules loaded, baseline pad, performance config,
  battery who-line, native terminal path, and key decoder/encoder stability.
- `gopher-keys` reports recent raw keys actually received by
  `MAG-GOPHER-HANDLE-KEY`; use this after pressing arrows inside Mag Gopher to
  see whether Gopher is receiving a different translated stream than Mag Shell.
- `open-gopher` opens a normal Mag Gopher window through the same async path as
  the UI button.
- `close-gopher` closes the remembered active Mag Gopher window. Use it to
  clean up windows opened by `open-gopher`, `gopher-test-page`, or
  `gopher-self-test` during diagnostics.
- `close-shell` closes the remembered active Mag Shell window. Use it to clean
  up windows opened by `open-shell` during diagnostics.
- `shell-self-test` creates a short-lived PTY shell, initializes the native
  Ghostty path, reads output, scans changed rows, reports command 40, and
  closes the test job. Use it to prove terminal creation and native counters
  without depending on the async window-opening path.
- `shell-state` reports the active Mag Shell native job status plus Lisp-side
  render counters.
- `shell-render-stats` reports only those active Mag Shell render counters.
- `shell-reset-render-stats` resets those counters before a live performance
  probe.
- `shell-box-test` sends a fixed UTF-8 box-drawing probe to the active Mag
  Shell. Use it after `shell-reset-render-stats`; `shell-render-stats` should
  then report a nonzero `box-cells` count.
- `shell-reset-native-stats` resets Maiko/Ghostty counters for the active Mag
  Shell. `shell-reset-all-native-stats` resets them for all live Ghostty-backed
  shell jobs. These require `UNIX-HANDLECOMM 42`, so restart into the rebuilt
  Maiko binary after changing this path.
- `mag-self-test` runs the integrated smoke test for Maiko helper status,
  Ghostty/Mag terminal status, the verified arrow decoder table, Gopher
  viewport behavior, and the who-line battery hook.
- `gopher-state` reports the remembered live Gopher window's host, top,
  selected row, visible row count, entry count, and status.
- `gopher-draw-stats` reports Gopher repaint, entry-row draw, draw-index, and
  chrome draw counters for the remembered live Gopher window.
- `gopher-reset-draw-stats` resets those counters. Use it before injected
  movement tests to prove same-viewport selection movement redraws only the old
  and new rows, not the whole window.
- `gopher-key-up`, `gopher-key-down`, `gopher-key-left`, and
  `gopher-key-right` inject direct Medley arrow key codes through
  `MAG-GOPHER-HANDLE-KEY` against the remembered live Gopher window.
- `gopher-test-page` replaces the live Gopher window contents with a local
  60-entry test page. On a 31-row viewport, repeated `gopher-key-down` should
  show `top=0 selected=31` followed by `top=10 selected=32`; this verifies the
  viewport jumps by `MAG-GOPHER-VIEW-JUMP` instead of crawling by one line.
- `gopher-viewport-status` reports the native C-side Gopher viewport self-test
  without opening or changing a Gopher window. It should show `status=ok`.
- `gopher-label-status` reports the native C-side Gopher quick-label self-test
  for labels such as `aa`, `zz`, and `aaa`. It should show `status=ok`.
- `gopher-type-status` reports the native C-side Gopher type-tag self-test for
  tags such as `TEXT`, `DIR `, `HTML`, and `????`. It should show `status=ok`.
- `gopher-self-test` creates the local 60-entry test page, checks the shared
  arrow decoder table, drives raw down/up keys through `MAG-GOPHER-HANDLE-KEY`,
  and reports whether the viewport jump, native labels, native type tags, and
  basic movement semantics pass.
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

Maiko tags terminal box-drawing cells in the upper nibble of the per-cell
flags byte during row copyout: left `0x10`, right `0x20`, up `0x40`, down
`0x80`. The lower nibble remains terminal style state. Lisp must use those
flags for hot-path `DRAWLINE` rendering and should not redo Unicode box range
classification per cell.

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

- `unix-helper`
- `unix-pipes`
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

Local Maiko command `UNIX-HANDLECOMM 39` reads and consumes `/tmp/medley-mag-request` into a caller-provided buffer. Local Maiko command `UNIX-HANDLECOMM 40` reports a single Mag terminal job's native state into a caller-provided buffer. Local Maiko command `UNIX-HANDLECOMM 41` reports runtime configuration such as VM size, timer interval, and screen/window dimensions. Local Maiko command `UNIX-HANDLECOMM 42` resets Ghostty timing/copyout counters for one shell job or all shell jobs. `MAG-DEBUG-RPC-START` runs a safe Medley-side poller that dispatches only these commands:
Local Maiko command `UNIX-HANDLECOMM 43` computes Mag Gopher viewport
transitions from `top selected delta count visible jump` and returns
`new-top new-selected repaint-needed` in a compact buffer. Lisp should use
this through `MAG-GOPHER-VIEWPORT-STEP` rather than duplicating the movement
math in draw/input hot paths.
Local Maiko command `UNIX-HANDLECOMM 44` reports the native terminal arrow
encoding bytes used by command 25. `key-encode-test` must show key ids 1-4 as
CSI `A/B/C/D` before changing terminal arrow handling.
Local Maiko command `UNIX-HANDLECOMM 45` reports a native C-side Mag Gopher
viewport self-test. `gopher-viewport-status` must show `status=ok`.
Local Maiko command `UNIX-HANDLECOMM 46` reports a bounded native job list for
Maiko shell/process/socket slots. `native-jobs` must fit in one VM page and
report omitted jobs instead of overflowing.
Local Maiko command `UNIX-HANDLECOMM 47` computes a Mag Gopher quick-label for
a zero-based item index. `gopher-label-status` must show `status=ok`.
Local Maiko command `UNIX-HANDLECOMM 48` computes a fixed-width Mag Gopher
type tag for a Gopher type byte. `gopher-type-status` must show `status=ok`.

- `ping`
- `debug-report`
- `config-report`
- `performance-report`
- `status-report`
- `process-status`
- `native-jobs`
- `goal-status`
- `write-debug-report`
- `battery`
- `who-line-battery`
- `reload-mag`
- `restart-rpc`
- `open-shell`
- `close-shell`
- `shell-self-test`
- `shell-key-probe`
- `shell-state`
- `shell-render-stats`
- `shell-reset-render-stats`
- `shell-reset-native-stats`
- `shell-reset-all-native-stats`
- `mag-self-test`
- `open-gopher`
- `close-gopher`
- `keys-test`
- `key-encode-test`
- `gopher-keys`
- `gopher-state`
- `gopher-draw-stats`
- `gopher-reset-draw-stats`
- `gopher-test-page`
- `gopher-self-test`
- `gopher-viewport-status`
- `gopher-label-status`
- `gopher-type-status`
- `gopher-key-up`
- `gopher-key-down`
- `gopher-key-left`
- `gopher-key-right`
- `keys-help`

Do not add arbitrary `eval FORM` to this poller without isolating it from the
RPC loop. Direct eval attempts have wedged the poller even on `eval 42`.
A later file-backed Lisp worker attempt also made the fixed RPC stop answering
on `(IPLUS 2 3)`. The failed patch is saved as
`/tmp/mag-eval-worker-failed-20260630.patch` on this machine for reference.
Future remote eval should be implemented below the cooperative Lisp process
layer, or with a proven abortable worker, before it is exposed through MCP.

Keep `reload-mag` asynchronous. Loading `MAG-EXTRAS` inside the RPC poller
itself can redefine/reset the code that is currently handling the request and
has blocked the bridge. The poller should spawn `MAG-RPC-RELOAD` and return.

## MCP bridge

`scripts/mag-medley-mcp.js` is registered in `/home/mag/.codex/config.toml` as `medley_mag`. The primary Codex transport is streamable HTTP at `http://127.0.0.1:8765/mcp`; `scripts/medley-mag-mcp-ensure` starts the persistent daemon with `setsid` so it survives the shell that launched it. The same JS file still supports stdio for direct protocol smoke tests.

Autostart hooks on this machine:

- `/home/mag/.config/fish/conf.d/medley_mcp.fish` starts `medley-mag-mcp-ensure` for interactive fish shells.
- `/home/mag/.emacs.d/init.el` starts `medley-mag-mcp-ensure` from `emacs-startup-hook` for EXWM login.

The bridge exposes process/log/status tools and `medley_request`, which writes `/tmp/medley-mag-request`, waits for `/tmp/medley-mag-response`, and returns the live Medley response.

HTTP smoke test:

```sh
/home/mag/.local/bin/medley-mag-mcp-ensure
/home/mag/.guix-profile/bin/node -e '
const http=require("http");
const body=JSON.stringify({jsonrpc:"2.0",id:1,method:"tools/call",params:{name:"medley_request",arguments:{command:"ping",timeout_ms:5000}}});
const req=http.request({host:"127.0.0.1",port:8765,path:"/mcp",method:"POST",headers:{"content-type":"application/json","content-length":Buffer.byteLength(body)}},res=>{let d="";res.on("data",c=>d+=c);res.on("end",()=>console.log(res.statusCode,d));});
req.end(body);
'
```

Stdio fallback smoke test:

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"medley_request","arguments":{"command":"ping","timeout_ms":5000}}}' \
  | /home/mag/src/medley/scripts/mag-medley-mcp.js
```

Normal launch path: `/home/mag/.local/bin/medley-interlisp`, mirrored as `scripts/mag-medley-interlisp`, starts `apps.sysout` with `--greet -` and sets `MAIKO_STARTUP_TYPEAHEAD_FILE` to a tiny file containing `(IL:LOAD ".../MAG-NOGREET" T)`. Maiko's local X11 startup typeahead hook injects that form after `MAIKO_STARTUP_TYPEAHEAD_DELAY` seconds. This replaced the old EXWM/emacsclient synthetic loader, which was fragile because EXWM does not always expose a live Medley buffer during startup. Do not pass `--nofork`/`-NF` here: Maiko uses that flag to skip `fork_Unix`, and Mag Shell/`FORK-SHELL` need the Unix communication helper.

The Mag launcher defaults `MAG_MEDLEY_MEMORY_MB` to 256, matching this Maiko
build's 256 MB VM support and avoiding the stock 64 MB apps.sysout ceiling.
Override `MAG_MEDLEY_GEOMETRY`, `MAG_MEDLEY_SCREENSIZE`,
`MAG_MEDLEY_MEMORY_MB`, or `MAG_MEDLEY_TITLE` in the environment when testing
different display or VM settings.
