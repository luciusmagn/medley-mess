# Mag Low-Level Drawing Notes

This is a local note for the Medley/Maiko Mag terminal and Gopher work. It documents the drawing primitives currently used by `greetfiles/MAG-*` and the performance direction for later C/Maiko work.

## Files

- `greetfiles/MAG-EXTRAS`: thin loader only.
- `greetfiles/MAG-COMMON`: shared window rows, clipboard, prompt helpers.
- `greetfiles/MAG-VTERM`: Mag Shell/Codex terminal orchestration and Ghostty-backed render copyout.
- `greetfiles/MAG-DEBUG`: wrappers for Maiko debug status reporting.
- `greetfiles/MAG-GOPHER`: Medley-rendered Gopher UI.
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
- Keep slow Lisp loops out of hot terminal paths. Prefer Maiko/C for terminal parse/render state, dirty row tracking, row copyout, cursor drawing, and key encoding.
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

There is now a limited direct agent-facing protocol into the running Medley instance. It is intentionally not arbitrary eval.

Current native commands:

- `38`: copy a compact debug report into a VM page buffer.
- `39`: read and consume `/tmp/medley-mag-request` into a VM page buffer.
- `40`: copy a compact per-job Mag terminal state report into a VM page buffer.
- `41`: copy a compact runtime configuration report into a VM page buffer.
- `42`: reset Ghostty timing/copyout counters for one shell job or all jobs.

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
- `write-debug-report`
- `battery`
- `who-line-battery`
- `reload-mag`
- `open-shell`
- `restart-rpc`
- `open-gopher`
- `shell-self-test`
- `shell-render-stats`
- `shell-reset-render-stats`
- `shell-box-test`
- `shell-reset-native-stats`
- `shell-reset-all-native-stats`
- `mag-self-test`
- `keys-test`
- `gopher-keys`
- `gopher-state`
- `gopher-draw-stats`
- `gopher-reset-draw-stats`
- `gopher-test-page`
- `gopher-self-test`
- `gopher-key-up`
- `gopher-key-down`
- `gopher-key-left`
- `gopher-key-right`
- `shell-state`
- `keys-help`

`performance-report` verifies that the local faster launch configuration is
active: 256 MB VM, 10 ms timer, and Maiko `--noscroll`.

`status-report` is the preferred one-shot snapshot when diagnosing a running
instance. It combines debug counters, performance config, battery/who-line
state, key decoding, active shell state, and active Gopher state without
arbitrary evaluation.

`mag-self-test` is the preferred post-reload smoke test. It checks Maiko's Unix
helper, the performance-oriented runtime config, the Ghostty-backed Mag
terminal path, the verified arrow decoder table, Gopher's self-test, and the
who-line battery hook.

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

`/home/mag/.local/bin/medley-interlisp` is mirrored as `scripts/mag-medley-interlisp`. It starts `apps.sysout` with `--greet -` and uses Maiko's `MAIKO_STARTUP_TYPEAHEAD_FILE` hook to type `(IL:LOAD ".../MAG-NOGREET" T)` into the initial exec after a short delay. This avoids depending on EXWM/emacsclient to synthesize the startup load. The launcher must not pass `--nofork`/`-NF`, because that disables Maiko's Unix helper and makes `FORK-SHELL`/Mag Shell fail before a PTY job is created.

The launcher explicitly passes `--mem "$MAG_MEDLEY_MEMORY_MB"`, defaulting to
256 MB. The stock loadup sysouts are 64 MB but expandable; this Maiko build
reports 256 MB VM support. Use `debug-report` after restart to verify
`vmem-process-mb`, `timer-interval-us`, `window`, `screen`, and `noscroll`.
