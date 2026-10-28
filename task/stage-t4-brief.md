# Stage T4: Terminal shortcuts beside the clock and agent logos

Requested on 2026-10-28. Branch `xDaijobu/stage-t4-terminal-shortcuts`.

## Scope

- Show a compact shortcut beside the idle clock for every running island terminal tab, in display order (up to the existing twelve-tab limit). Plain shells use the terminal icon; Claude and Codex tabs use their agent marks.
- A shortcut selects that exact tab and opens the terminal with keyboard focus, without starting another shell or agent. Closing or exiting a shell removes its shortcut; restored pinned tabs appear only after their sessions start successfully.
- A tab remains a running terminal while its shell stays open, including the prompt after an agent command finishes. This stage does not discover terminals belonging to other applications.
- Widen the resting island only as needed to fit the clock and shortcuts, respecting the configured height and the existing maximum swap-chain size. No shortcuts cover Music, Timer, Snap, expanded panels or the volume OSD.
- Highlight shortcut hover/selection and reuse the existing background-output indicator. No new timer or process polling; session wake events refresh the idle view too.
- Add the same agent logos to Claude/Codex in the new-tab menu, with a matching terminal icon on the plain-shell row. Preserve labels, yolo hints, disabled states and menu hit testing.
- Render local vector assets with Skia, use Apache-compatible sources and record attribution in `NOTICE`.

## Acceptance

- Headless controller tests cover no tabs, live/exited/failed shells, ordering, restoration, exact-id activation and no duplicate spawning.
- Pure geometry/hit tests cover one through twelve shortcuts, minimum notch height/width, resizing, OSD ownership and clicks during transitions.
- Raster checks cover logos, menu text spacing and light/dark theme contrast.
- `cargo fmt --all -- --check`, workspace clippy with warnings denied, workspace tests and a release build pass.
- Run the release build without moving the user's cursor. Record live interaction checks honestly; synthetic input must not launch a paid agent job, change the clipboard or type into another application.
