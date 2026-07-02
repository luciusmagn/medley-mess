# Local Mag Notes

This tree has local, uncommitted Medley/Maiko integration work. Do not discard it.

## Mag file layout

`greetfiles/MAG-EXTRAS` is intentionally a thin loader. The implementation is split across:

- `greetfiles/MAG-COMMON`: shared row drawing, clipboard, prompt helpers.
- `greetfiles/MAG-VTERM`: Mag Shell/Codex terminal orchestration and Ghostty-backed rendering.
- `greetfiles/MAG-DEBUG`: Maiko/Medley debug status wrappers.
- `greetfiles/MAG-GOPHER`: Medley-rendered Gopher browser.
- `greetfiles/MAG-TELEGRAM`: Medley UI for the host-side Telegram bridge.
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

Maiko's X input layer should make physical arrow keys produce keypad-style
codes so raw `\GETKEY` consumers such as Mag Shell and Mag Keys receive
buffered input:

- `84` left
- `82` up
- `69` down
- `87` right

The dedicated `129..132` arrow codes remain accepted by
`MAG-VTERM-SPECIAL-KEYID` for direct/synthetic compatibility, but they are not
the preferred physical X event representation for Mag raw input windows. If
TEdit/File Browser/Notecards/Exec arrows work but Mag Shell and Mag Keys see no
arrow input, suspect that Maiko was changed back to dedicated-only physical
arrow codes.

If Mag Shell arrows work but Gopher arrows are wrong, fix `MAG-GOPHER-SPECIAL-KEYID`; do not perturb the terminal path.

## Medley GC Disabled Warning

The warning

```text
GC Disabled Warning: Internal garbage collector tables have overflowed, due
to too many pointers with reference count greater than 1.
```

is not ordinary VM heap exhaustion. The user may see low Vmem usage when this
happens. In Maiko this is triggered by the GC reference-count hash/collision
tables overflowing and `disablegc1` marking all type entries `NOREF`; the live
image is not recoverable in-place. The immediate action is to save TEdit work
and restart Medley.

Do not respond to this failure by only increasing `MAG_MEDLEY_MEMORY_MB`; the
current `RELEASE=351` build is already the 256 MB layout, and the overflowing
structure is separate from the main VM heap. After restarting into a Maiko
binary with command 50, use `gc-report` to inspect `HTCOLL`
collision-link high-water/free/live counts, `HTBIGCOUNT` occupancy, the
GC-disabled flag, and reclaim countdown/min values.

Current diagnosis as of 2026-07-02:

- Plain apps.sysout launched without `MAG-NOGREET` stayed stable over repeated
  samples: `hi-links=2624`, with live links only bouncing normally.
- Normal `MAG-NOGREET` startup through the real typeahead path leaked generated
  `A####` atoms when the RPC poller called interpreted `SUBRCALL
  UNIX-HANDLECOMM` in its idle loop, even with no Mag Shell window active.
- Reference tracing showed the retained type-45 chunks were pname storage for
  interned `GENSYM` atoms, retained through the atom/package hash tables, so
  `RECLAIM` cannot recover them.
- Root cause: interpreted `SUBRCALL` is a macro that expands to a runtime
  `CL:COMPILE` of a throwaway lambda using generated argument names. Every hot
  interpreted call can intern more `A####` symbols.
- Fix: live MAG greetfiles must not call raw `(SUBRCALL UNIX-HANDLECOMM ...)`
  from interpreted hot paths. Use `MAG-UNIX-HANDLECOMM1` through
  `MAG-UNIX-HANDLECOMM8` from `MAG-COMMON`; they compile direct opcode wrappers
  once per arity and reuse them.
- Verified after the wrapper conversion: full `MAG-NOGREET` startup plus RPC
  `ping` stayed at `A#=0` through samples 0-5; `shell-load-test` stayed at
  `A#=0` through samples 0-12 while Mag Shell rendered output.
- Use `scripts/mag-gc-scan-live.py` to measure generated atom counts and
  `scripts/mag-gc-refscan.py <ldex-pid> <ptr>...` for raw referrer tracing.

Do not put automatic Mag Shell draining/rendering in `MAG-DEBUG-RPC-LOOP`.
That loop is control-plane only. A reproduced failure showed that polling
`MAG-VTERM-PUMP-ACTIVE` from the RPC loop made repeated Mag Shell load tests
raise `HTCOLL` high-water from 2624 to 8136 after three open/render/close
cycles. With the RPC pump removed, the same cleaned build stayed at
`hi-links=2624` for all three cycles. `shell-pump-once` is diagnostic only.

RPC responses should use Maiko command 55 when available. It writes
`/tmp/medley-mag-response` with native file I/O and avoids allocating a Medley
file stream for every control-plane reply. Falling back to
`MAG-WRITE-UTF8-TEXT-FILE` is only for older binaries.

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
  GC pressure, battery, key decoder, active shell, and active Gopher state.
- `process-status` returns a bounded process snapshot for the RPC poller,
  active Mag Shell window, active Mag Gopher window, and known Mag worker
  names. It does not run arbitrary eval or list every process.
- `shell-root-report` reports bounded Mag Shell root evidence: last active
  shell window, `\LastInWindow`, open Mag vterm windows, and live Mag shell
  processes.
- `eval-status` reports whether the native typeahead eval bridge is available.
  `medley_eval` is exposed at the MCP layer, not as a raw `medley_request`
  command; it writes a typeahead file and then sends an internal `eval <id>`
  request to the poller.
- `native-jobs` returns Maiko's bounded native job list from
  `UNIX-HANDLECOMM 46`: aggregate job counts plus compact live job lines that
  fit in one VM page.
- `gc-report` returns Maiko's bounded GC table status from `UNIX-HANDLECOMM
  50`: `HTCOLL` high-water/free/live link counts, `HTBIGCOUNT` occupancy,
  GC-disabled state, and reclaim countdown/min values.
- `goal-status` returns a non-mutating summary of the original Mag integration
  goal evidence: split modules loaded, baseline pad, performance config,
  battery who-line, native terminal path, and key decoder/encoder stability.
- `gopher-keys` reports recent raw keys actually received by
  `MAG-GOPHER-HANDLE-KEY`; use this after pressing arrows inside Mag Gopher to
  see whether Gopher is receiving a different translated stream than Mag Shell.
- `open-gopher` opens a normal Mag Gopher window through the same async path as
  the UI button.
- `open-telegram` opens the text-only Mag Telegram dashboard backed by
  `tools/mag-telegram-bridge`.
- `telegram-status`, `telegram-auth`, `telegram-doctor`, and
  `telegram-chats` proxy compact bridge diagnostics/listings without exposing
  raw TDLib JSON.
- `close-gopher` closes the remembered active Mag Gopher window. Use it to
  clean up windows opened by `open-gopher`, `gopher-test-page`, or
  `gopher-self-test` during diagnostics.
- `close-shell` closes the remembered active Mag Shell window. Use it to clean
  up windows opened by `open-shell` during diagnostics.
- `shell-load-test` opens a Mag terminal running a bounded line-output command.
  Use it for repeatable `HTCOLL` pressure tests of the terminal render path.
- `shell-self-test` creates a short-lived PTY shell, initializes the native
  Ghostty path, reads output, scans changed rows, reports command 40, and
  closes the test job. Use it to prove terminal creation and native counters
  without depending on the async window-opening path.
- `shell-state` reports the active Mag Shell native job status plus Lisp-side
  render counters.
- `shell-drain-once` drains the remembered active Mag Shell channel once from
  the RPC side, refreshes if bytes were read, and reports native/render
  counters. Use it to distinguish PTY output from a stalled typeout process.
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
Local Maiko command `UNIX-HANDLECOMM 49` reads and consumes
`/tmp/medley-mag-typeahead`, injecting its contents through the same X key
event path used by startup typeahead. MCP eval uses this to type a bounded
helper form into the live Exec without calling `PROCESS.EVAL` or `EVAL` from
the RPC poller.
Local Maiko command `UNIX-HANDLECOMM 50` reports GC table pressure in one VM
page: `HTCOLL` collision-link high-water/free/live counts, `HTBIGCOUNT`
occupancy, `GCDISABLED`, and reclaim countdown/min values. The safe RPC command
is `gc-report`, and `status-report` includes it as the `gc` section.
Local Maiko command `UNIX-HANDLECOMM 51` checks whether
`/tmp/medley-mag-request` exists without taking a Lisp VM page buffer argument.
`MAG-DEBUG-RPC-READ` must use command 51 before command 39 so idle polling does
not hand a VM buffer to native code on every tick.
Local Maiko command `UNIX-HANDLECOMM 52` checks whether a Mag shell/process fd
is readable without consuming bytes. Mag Shell uses it to avoid pointless
native/Lisp buffer traffic while idle.
Local Maiko command `UNIX-HANDLECOMM 53` drains PTY bytes directly into
Ghostty without copying raw bytes through a Lisp VM page.
Local Maiko command `UNIX-HANDLECOMM 54` copies a Ghostty-rendered row as
simple ASCII display text.
Local Maiko command `UNIX-HANDLECOMM 55` writes `/tmp/medley-mag-response`
through native file I/O from a Lisp string.
Local Maiko command `UNIX-HANDLECOMM 56` drains multiple PTY chunks directly
into Ghostty in one native call.
Local Maiko command `UNIX-HANDLECOMM 57` starts
`/home/mag/.local/bin/mag-telegram-bridge --daemon` as a detached host process.
`MAG-TELEGRAM` should prefer this native start path and keep `ShellCommand`
only as compatibility fallback for older Maiko binaries.

- `ping`
- `debug-report`
- `config-report`
- `performance-report`
- `status-report`
- `process-status`
- `native-jobs`
- `gc-report`
- `goal-status`
- `write-debug-report`
- `battery`
- `who-line-battery`
- `eval-status`
- `eval-reset`
- `reload-mag`
- `restart-rpc`
- `open-shell`
- `shell-load-test`
- `close-shell`
- `shell-self-test`
- `shell-key-probe`
- `shell-state`
- `shell-drain-once`
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

MCP eval is deliberately indirect. The JS daemon writes the user's one-line
expression to `/tmp/medley-mag-eval/<id>.lisp` for diagnostics, writes a helper
form to `/tmp/medley-mag-typeahead`, and sends internal request `eval <id>`.
Medley focuses the `EXEC` process and command 49 types that helper form. The
helper calls a zero-argument thunk and writes `/tmp/medley-mag-eval/<id>.out`.
Verified smoke forms: `(+ 2 3)`, `(CL:LIST 1 2 3)`, and `(IL:IPLUS 2 3)`.
Do not replace this with `ADD.PROCESS`, `PROCESS.EVAL`, unqualified `EVAL`, or
`CL:EVAL`; those attempts caused stack overflow, wedged the live process, or
left Exec in an error prompt.

Do not use `medley_eval` for UI/window-opening forms such as `(IL:MAG-SHELL)`.
That route has wedged the request poller/Exec path. Add fixed safe requests
that spawn asynchronous workers for UI actions instead.

Keep `reload-mag` asynchronous. Loading `MAG-EXTRAS` inside the RPC poller
itself can redefine/reset the code that is currently handling the request and
has blocked the bridge. The poller should spawn `MAG-RPC-RELOAD` and return.

## MCP bridge

`scripts/mag-medley-mcp.js` is registered in `/home/mag/.codex/config.toml` as `medley_mag`. The primary Codex transport is streamable HTTP at `http://127.0.0.1:8765/mcp`; `scripts/medley-mag-mcp-ensure` starts the persistent daemon with `setsid` so it survives the shell that launched it. The same JS file still supports stdio for direct protocol smoke tests.

Autostart hooks on this machine:

- `/home/mag/.config/fish/conf.d/medley_mcp.fish` starts `medley-mag-mcp-ensure` for interactive fish shells.
- `/home/mag/.emacs.d/init.el` starts `medley-mag-mcp-ensure` from `emacs-startup-hook` for EXWM login.

The bridge exposes process/log/status tools, `medley_request`, and `medley_eval`.
`medley_request` writes `/tmp/medley-mag-request`, waits for
`/tmp/medley-mag-response`, and returns the live Medley response. `medley_eval`
uses the command 49 typeahead route described above; check `medley_eval_status`
before relying on it after a rebuild.

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

Normal launch path: `/home/mag/.local/bin/medley-interlisp`, mirrored as `scripts/mag-medley-interlisp`, starts `apps.sysout` with `--greet -` and uses Maiko startup typeahead to load `/home/mag/src/medley/greetfiles/MAG-NOGREET` after the initial Exec exists. Directly passing `MAG-NOGREET` as the actual greetfile has stalled before the RPC poller starts. The old idle `HTCOLL` growth from this typeahead path was fixed by Maiko command 51 plus the guarded `MAG-DEBUG-RPC-READ`; do not remove that guard. Do not pass `--nofork`/`-NF` here: Maiko uses that flag to skip `fork_Unix`, and Mag Shell/`FORK-SHELL` need the Unix communication helper.

The Mag launcher defaults `MAG_MEDLEY_MEMORY_MB` to 256, matching this Maiko
build's 256 MB VM support and avoiding the stock 64 MB apps.sysout ceiling.
Override `MAG_MEDLEY_GEOMETRY`, `MAG_MEDLEY_SCREENSIZE`,
`MAG_MEDLEY_MEMORY_MB`, or `MAG_MEDLEY_TITLE` in the environment when testing
different display or VM settings.
