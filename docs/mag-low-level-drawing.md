# Mag Low-Level Drawing Notes

This is a local note for the Medley/Maiko Mag terminal and Gopher work. It documents the drawing primitives currently used by `greetfiles/MAG-*` and the performance direction for later C/Maiko work.

## Files

- `greetfiles/MAG-EXTRAS`: thin loader only.
- `greetfiles/MAG-COMMON`: shared window rows, clipboard, prompt helpers.
- `greetfiles/MAG-VTERM`: Mag Shell/Codex terminal orchestration and Ghostty-backed render copyout.
- `greetfiles/MAG-DEBUG`: wrappers for Maiko debug status reporting.
- `greetfiles/MAG-GOPHER`: Medley-rendered Gopher UI.
- `greetfiles/MAG-TELEGRAM`: text-only Medley UI for the host-side Telegram bridge.
- `greetfiles/MAG-STOCK`: battery who-line, Rooms/Notecards/doc buttons.

## Coordinate Model Used By Mag Rows

The Mag row helpers draw from the bottom of a Medley window upward.

- `MAG-WINDOW-ROW-BOTTOM` computes the bottom pixel of a logical row.
- `MAG-WINDOW-ROW-Y` returns the text baseline by adding font descent to the row bottom.
- `MAG-WINDOW-CLEAR-LINE` clears using the row bottom, not the baseline.
- `MAG-WINDOW-BASELINE-PAD`, currently `1`, adds one pixel of leading and
  moves baselines up by one pixel. This avoids descender clipping with fonts
  whose actual raster descent is slightly larger than the reported descent.

This distinction matters. Passing the row bottom directly to `MOVETO` clips
glyph descenders by about a pixel; text should use the padded baseline.

## Primitives

- `MOVETO x y window`: move the text cursor/baseline in the destination stream/window.
- `PRIN1 text window`: draw text at the current cursor position.
- `DSPFONT font window`: set the active font for subsequent text drawing.
- `FONTPROP font 'HEIGHT`: total row height for the active font.
- `FONTPROP font 'DESCENT`: descent below baseline; use this when computing `MOVETO` Y.
- `BLTSHADE shade window x y width height op`: fill/erase rectangular regions. Current code uses `WHITESHADE` for row clear and `BLACKSHADE` for divider/cursor work.
- `DRAWLINE x1 y1 x2 y2 width op dsp`: draw vector lines. Current terminal code uses it for box-drawing fallback.
- `BITBLT src sx sy dst dx dy width height op`: copy pixels; useful for cursor inversion and cached who-line/temp-stream redraws.
- `CLEARW window`: clear an entire window. Avoid this for selection moves; prefer row-level redraw.
- `DSPCREATE` and `BITMAPCREATE`: create off-screen drawing targets for cached redraws.

## Performance Rules

- Avoid full-window `CLEARW`/repaint on small state changes. Gopher selection movement should redraw old/new rows when the viewport does not change.
- Use `gopher-reset-draw-stats` followed by one `gopher-key-down` to verify
  that same-viewport movement reports `repaints=0`, `entry-draws=2`, and
  `draw-index=2`.
- When the Gopher viewport does change, jump from the current top by a chunk, currently up to 10 rows, then clamp so the selected row remains visible. Do not compute bottom-edge scroll as `selected - visible + jump`; that can overshoot.
- `MAG-GOPHER-VIEWPORT-STEP` delegates that movement/clamping decision to
  Maiko command 43 when available, keeping Lisp responsible for state storage
  and redraw dispatch.
- `MAG-GOPHER-LABEL-FOR-N` delegates quick-label generation to Maiko command
  47 when available and falls back to the Lisp implementation on older Maiko
  binaries.
- `MAG-GOPHER-TYPE-TAG` delegates fixed-width type-tag generation to Maiko
  command 48 when available and falls back to the Lisp implementation on older
  Maiko binaries.
- Keep slow Lisp loops out of hot terminal paths. Prefer Maiko/C for terminal parse/render state, dirty row tracking, row copyout, cursor drawing, and key encoding.
- Do not call raw interpreted `(SUBRCALL UNIX-HANDLECOMM ...)` in MAG hot
  paths. Interpreted `SUBRCALL` macroexpands through runtime `CL:COMPILE` and
  `GENSYM`; repeated calls intern retained `A####` atoms and can overflow
  `HTCOLL`, disabling GC while Vmem still looks low. Use the cached fixed-arity
  wrappers `MAG-UNIX-HANDLECOMM1` through `MAG-UNIX-HANDLECOMM8` from
  `MAG-COMMON`.
- Maiko command 25 sends unmodified terminal arrow key ids 1-4 as direct CSI
  bytes: up `ESC [ A`, down `ESC [ B`, right `ESC [ C`, left `ESC [ D`.
  Do not rotate Lisp's raw-key decoder to compensate for libghostty encoder
  behavior.
- Physical X arrow keys should enter Mag raw `\GETKEY` windows as keypad-style
  codes: `82` up, `69` down, `87` right, `84` left.  The Lisp decoder also
  accepts dedicated `129..132` codes for direct/synthetic compatibility, but
  using those as the physical-only path has made Mag Shell and Mag Keys stop
  seeing arrow input while stock Medley apps still worked.
- Keep `MAG-VTERM-SEND-KEY` on the same `MAG-VTERM-SPECIAL-KEYID` path used
  by the table tests. It should not carry a duplicate raw `57344`-style arrow
  mapping.
- Lisp should orchestrate windows, menus, process lifecycle, and high-level UI state.
- Use `UNIX-HANDLECOMM` as the integration boundary for C-backed operations until a better debug/control protocol exists.
- `MAG-GHOSTTY-REFRESH` now uses `UNIX-HANDLECOMM 37` to get a C-computed list of rendered rows whose hashes changed, except for forced refreshes, which still repaint every row.
- Maiko tags Unicode box-drawing cells in the upper nibble of the per-cell
  flags byte during row copyout: left `0x10`, right `0x20`, up `0x40`, down
  `0x80`. Lisp uses those flags for low-level `DRAWLINE` rendering; do not
  put per-cell box range classification back into the hot Lisp draw loop.
- `UNIX-HANDLECOMM 38` is the native Mag debug report command.
- Command 38 includes compact terminal/helper counters after restart: `unix-helper`, `unix-pipes`, `gt-write-calls`, `gt-write-bytes`, `gt-render-updates`, `gt-change-scans`, `gt-changed-rows`, `gt-write-us`, `gt-update-us`, `gt-scan-us`, `gt-last-update-us`, `gt-last-scan-us`, `gt-last-changed`, and `gt-hash-rows`.
- `UNIX-HANDLECOMM 39` reads and consumes `/tmp/medley-mag-request` for the safe Medley-side RPC poller.
- `UNIX-HANDLECOMM 40` reports a single Mag terminal job's native state.
- `UNIX-HANDLECOMM 41` reports runtime configuration: VM size, timer interval,
  no-scroll state, window dimensions, and screen dimensions.
- `UNIX-HANDLECOMM 42` resets Ghostty timing/copyout counters for one shell
  job, or all Ghostty-backed shell jobs when called with `-1`.
- `UNIX-HANDLECOMM 43` computes Mag Gopher viewport transitions from `top
  selected delta count visible jump` and returns `new-top new-selected
  repaint-needed` in a compact buffer.
- `UNIX-HANDLECOMM 44` reports the native terminal arrow key encoding bytes
  used by command 25.
- `UNIX-HANDLECOMM 45` reports a native Mag Gopher viewport self-test, proving
  the C-side jump/clamp behavior without driving the UI.
- `UNIX-HANDLECOMM 50` reports GC table pressure in one VM page: `HTCOLL`
  high-water/free/live collision-link counts, `HTBIGCOUNT` occupancy,
  GC-disabled state, and reclaim countdown/min values.
- `UNIX-HANDLECOMM 51` reports whether `/tmp/medley-mag-request` exists without
  taking a Lisp VM page buffer. It is useful for diagnostics, but it does not
  by itself prevent GC table pressure if invoked through interpreted raw
  `SUBRCALL`; call it through `MAG-UNIX-HANDLECOMM1` if needed.
- `UNIX-HANDLECOMM 52` reports whether a Mag shell/process descriptor is
  readable without consuming bytes. Mag Shell uses it before drain attempts so
  idle typeout loops avoid unnecessary Lisp/native buffer traffic.
- `UNIX-HANDLECOMM 53` drains PTY bytes directly into Ghostty's terminal state
  without copying those raw bytes through a Lisp VMEMPAGEP.
- `UNIX-HANDLECOMM 54` copies a Ghostty-rendered row as simple ASCII display
  bytes. This is the current highest stable Mag Shell render path.
- `UNIX-HANDLECOMM 55` writes `/tmp/medley-mag-response` directly from a Lisp
  string using native file I/O. The RPC loop should prefer this over
  `MAG-WRITE-UTF8-TEXT-FILE` so every control-plane response does not allocate
  Medley file streams. Do not reuse command 55 for direct `newbltchar` drawing;
  that experiment crashed because `newbltchar` can punt into Lisp from an
  invalid subr stack frame.
- `UNIX-HANDLECOMM 56` drains multiple PTY chunks directly into Ghostty in one
  native call. Lisp gates it with `MAG-GHOSTTY-DRAIN-MANY-ENABLED`; disabling
  that flag falls back to command 53 and is useful for GC-pressure A/B tests.
- `shell-render-stats` and `shell-reset-render-stats` expose Lisp-side Mag
  Shell draw counters for refreshes, changed-row refreshes, full-refresh
  fallbacks, forced refreshes, row draws, cursor inversions, and box-cell
  `DRAWLINE` rendering. Use `shell-box-test` after a render-stat reset to
  prove the active Mag Shell is taking the native box flag path.

## Good C/Maiko Candidates

- Gopher row cache/copyout for formatted entries, if the Lisp UI remains too slow.
- Terminal glyph fallback and box drawing.
- Dirty row list computation.
- Cursor shape and cursor inversion.
- Runtime debug snapshots: window ids, active terminal channel, dirty rows, dimensions, and last native command error.

## Debugging Gap

There is now a limited direct agent-facing protocol into the running Medley instance. MCP eval is available, but it is deliberately routed through native typeahead into the live Exec rather than through `PROCESS.EVAL` or the RPC poller process.

Current native commands:

- `38`: copy a compact debug report into a VM page buffer.
- `39`: read and consume `/tmp/medley-mag-request` into a VM page buffer.
- `40`: copy a compact per-job Mag terminal state report into a VM page buffer.
- `41`: copy a compact runtime configuration report into a VM page buffer.
- `42`: reset Ghostty timing/copyout counters for one shell job or all jobs.
- `43`: compute Mag Gopher viewport transitions for selection movement.
- `44`: report native terminal arrow key encoding bytes.
- `45`: report native Mag Gopher viewport self-test results.
- `46`: report a bounded native job list for Maiko shell/process/socket slots.
- `47`: compute a Mag Gopher quick-label for a zero-based item index.
- `48`: compute a fixed-width Mag Gopher type tag for a Gopher type byte.
- `49`: read and consume `/tmp/medley-mag-typeahead`, injecting it through the
  same X key event path as startup typeahead.
- `50`: report bounded GC table pressure for `HTCOLL`, `HTBIGCOUNT`,
  `GCDISABLED`, and reclaim countdown/min values.
- `51`: report whether `/tmp/medley-mag-request` exists without taking a Lisp
  VM page buffer.
- `52`: report whether a Mag shell/process descriptor is readable.
- `53`: drain PTY bytes directly into Ghostty without a Lisp VM page copy.
- `54`: copy a Ghostty-rendered row as simple ASCII display bytes.
- `55`: write `/tmp/medley-mag-response` through native file I/O.
- `56`: drain many PTY chunks directly into Ghostty without Lisp/native
  round-trips.

Current Lisp wrappers:

- `MAG-DEBUG-REPORT`
- `MAG-DEBUG-SHOW`
- `MAG-DEBUG-WRITE-REPORT`
- `MAG-DEBUG-RPC-START`

Current safe request commands:

- `ping`
- `debug-report`
- `config-report`
- `performance-report`
- `status-report`
- `process-status`
- `shell-root-report`
- `native-jobs`
- `gc-report`
- `goal-status`
- `write-debug-report`
- `battery`
- `who-line-battery`
- `eval-status`
- `eval-reset`
- `reload-mag`
- `open-shell`
- `shell-load-test`
- `close-shell`
- `restart-rpc`
- `open-gopher`
- `open-telegram`
- `telegram-status`
- `telegram-chats`
- `close-gopher`
- `shell-self-test`
- `shell-key-probe`
- `shell-render-stats`
- `shell-drain-once`
- `shell-reset-render-stats`
- `shell-box-test`
- `shell-reset-native-stats`
- `shell-reset-all-native-stats`
- `mag-self-test`
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
- `shell-state`
- `keys-help`

`shell-drain-once` is diagnostic, not the normal render path. It directly
drains the remembered active Mag Shell channel through command 9 and refreshes
once if bytes arrived. If this reports bytes while `MAG-VTERM-TYPEOUT` counters
stay flat, the bug is in Lisp process scheduling/typeout orchestration rather
than the PTY/Ghostty native path.

For GC root tracing, use `scripts/mag-gc-scan-live.py <ldex-pid> --top N` to
pick sample pointer values from HTCOLL, then
`scripts/mag-gc-refscan.py <ldex-pid> <ptr>...` to scan the mapped Lisp address
space for raw referrers. This is read-only. Treat unboxed array/data matches as
noisy until the chain reaches an obvious Lisp root.

## Telegram Bridge

`tools/mag-telegram-bridge` is the host-side foundation for a text-only
Telegram client. It keeps TDLib, authentication state, local Telegram storage,
and JSON update processing outside Interlisp. The Medley module
`greetfiles/MAG-TELEGRAM` renders compact text returned by the bridge and
offers a small dashboard window.

The bridge intentionally loads `libtdjson.so` with `dlopen` so it can build even
before Guix `tdlib` is installed. Without TDLib or `MAG_TELEGRAM_API_ID` /
`MAG_TELEGRAM_API_HASH`, it runs in mock mode. Live mode should use Telegram's
TDLib JSON interface and keep session data under
`~/.local/share/mag-telegram/tdlib` unless `MAG_TELEGRAM_DATA_DIR` overrides it.

Medley-facing commands:

- `MAG-TELEGRAM` opens the dashboard.
- `open-telegram` opens it through the safe debug RPC path.
- `telegram-status` and `telegram-chats` return compact bridge output.

Do not make `MAG-DEBUG-RPC-LOOP` automatically call terminal pump helpers.
The RPC loop must stay control-plane only. A reproduced GC-table pressure test
showed automatic RPC pumping raised `HTCOLL` high-water from 2624 to 8136 after
three Mag Shell load cycles; with the pump removed, the same cleaned build
stayed at `hi-links=2624` through three cycles. Use `shell-pump-once` only as
a manual diagnostic.

`medley_eval` is an MCP tool, not a raw safe request. The daemon writes the
source form to `/tmp/medley-mag-eval/<id>.lisp` for diagnostics, writes a helper
form to `/tmp/medley-mag-typeahead`, sends internal request `eval <id>`, and
waits for `/tmp/medley-mag-eval/<id>.out`. The Medley poller only validates the
id, focuses the `EXEC` process, and calls command 49. Verified smoke forms:
`(+ 2 3)`, `(CL:LIST 1 2 3)`, and `(IL:IPLUS 2 3)`.

Do not implement MCP eval with `ADD.PROCESS`, `PROCESS.EVAL`, unqualified
`EVAL`, or `CL:EVAL`. Those approaches caused stack overflow or wedged the live
Medley process on this machine.

Do not use `medley_eval` for UI/window-opening forms such as `(IL:MAG-SHELL)`.
That route has wedged the request poller/Exec path. Prefer fixed safe request
commands that spawn asynchronous workers for UI actions.

`performance-report` verifies that the local faster launch configuration is
active: 256 MB VM, 10 ms timer, and Maiko `--noscroll`.

`status-report` is the preferred one-shot snapshot when diagnosing a running
instance. It combines debug counters, performance config, battery/who-line
state, key decoding, active shell state, and active Gopher state without
arbitrary evaluation.

`process-status` is a bounded fixed diagnostic for process/RPC liveness. It
reports the current RPC process, the process named `MAG-DEBUG-RPC`, active Mag
Shell input/typeout processes, active Gopher input/load processes, and selected
known Mag worker names. It intentionally does not expose arbitrary eval or an
unbounded all-process dump.

`close-gopher` closes the remembered active Mag Gopher window and lets the
normal Gopher close hook dispose of channels and processes. Use it after
`gopher-test-page` or `gopher-self-test` leaves a diagnostic window open.

`native-jobs` reports Maiko's bounded native job list from `UNIX-HANDLECOMM
46`: aggregate live job counts and compact per-job lines. The C side truncates
by counting omitted jobs rather than overflowing the 512-byte VM page.

`goal-status` is a lighter, non-mutating progress report for the Mag
integration objective. It verifies split module loading, baseline padding,
performance config, battery who-line installation, native terminal enablement,
and terminal key decoder/encoder stability without opening Gopher pages or
spawning test shells.

`mag-self-test` is the preferred post-reload smoke test. It checks Maiko's Unix
helper, the performance-oriented runtime config, the Ghostty-backed Mag
terminal path, the verified arrow decoder table, Gopher's self-test, and the
who-line battery hook.

`gopher-viewport-status` is the non-mutating C-side Gopher viewport diagnostic.
It should report `ok jump-down top=10 selected=30 repaint=1` and `status=ok`.

`gopher-label-status` is the non-mutating C-side Gopher quick-label diagnostic.
It verifies labels such as `aa`, `az`, `zz`, and `aaa` through
`UNIX-HANDLECOMM 47`.

`gopher-type-status` is the non-mutating C-side Gopher type-tag diagnostic. It
verifies fixed-width tags such as `TEXT`, `DIR `, `HTML`, and `????` through
`UNIX-HANDLECOMM 48`.

`shell-key-probe` is the stronger terminal-arrow check: it opens a temporary
raw PTY reader, sends key ids 1-4 through command 25, and verifies the shell
receives `ESC [ A/B/C/D`.

`reload-mag` must stay asynchronous. It should spawn the reload worker and
return immediately; doing `LOAD MAG-EXTRAS` in the RPC poller process can block
future requests.

## MCP Bridge

`scripts/mag-medley-mcp.js` is a local stdio MCP server registered as `medley_mag` in `/home/mag/.codex/config.toml`.

Current tools:

- `medley_processes`: list running `run-medley`, `ldex`, and `lde` processes.
- `medley_log_tail`: read the Medley launcher log tail.
- `medley_debug_report_file`: read `/tmp/medley-mag-debug.txt`, written from inside Medley by `(MAG-DEBUG-WRITE-REPORT NIL)`.
- `medley_worktree_status`: show Medley and Maiko git status.
- `medley_request`: send a safe request to the running Medley RPC poller and return `/tmp/medley-mag-response`.
- `medley_eval`: evaluate one physical-line expression through the native
  typeahead/Exec helper path.
- `medley_eval_status`: report whether command 49 and Exec are available.
- `medley_eval_reset`: report reset status for the eval backend.

`/home/mag/.local/bin/medley-interlisp` is mirrored as `scripts/mag-medley-interlisp`. It starts `apps.sysout` with `--greet -` and uses Maiko's `MAIKO_STARTUP_TYPEAHEAD_FILE` hook to type `(IL:LOAD ".../MAG-NOGREET" T)` into the initial Exec after a short delay. Directly using `MAG-NOGREET` as the actual greetfile has stalled before the RPC poller starts. The launcher must not pass `--nofork`/`-NF`, because that disables Maiko's Unix helper and makes `FORK-SHELL`/Mag Shell fail before a PTY job is created. Idle RPC polling must keep using command 51 before command 39; otherwise the typeahead startup path regrows `HTCOLL` links.

The launcher explicitly passes `--mem "$MAG_MEDLEY_MEMORY_MB"`, defaulting to
256 MB. The stock loadup sysouts are 64 MB but expandable; this Maiko build
reports 256 MB VM support. Use `debug-report` after restart to verify
`vmem-process-mb`, `timer-interval-us`, `window`, `screen`, and `noscroll`.
