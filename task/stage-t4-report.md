# Stage T4 report: Terminal shortcuts and agent logos

Brief: [`stage-t4-brief.md`](stage-t4-brief.md). Branch `xDaijobu/stage-t4-terminal-shortcuts`.

## Delivered

- [x] One icon shortcut per running island terminal tab beside the idle clock, in tab display order. Claude uses its starburst, Codex the OpenAI mark, and plain shells the terminal icon.
- [x] A click selects the existing session by stable tab ID and opens the terminal with its existing keyboard-focus path. It does not spawn, restart or duplicate a session; stale IDs are ignored.
- [x] Failed starts and exited/closed shells are excluded. Restored pinned tabs appear after their shells start. A shell remains a running terminal after its agent returns to the prompt.
- [x] The configured width stays the minimum; the idle island grows only enough to fit its clock and up to twelve shortcuts. Height follows Settings. All twelve fit the existing swap chain. Music, Timer, Snap, panels and the OSD retain their geometry and input ownership.
- [x] Hover and active-tab plates plus the existing unread-output indicator. Session wake events update the shortcut model even with the panel hidden. Unchanged output does not repeatedly redraw the idle strip.
- [x] Matching logos in the new-tab menu, with labels, yolo hints, disabled styling and click targets retained.
- [x] Local Skia vector rendering with no new dependency. Simple Icons v15.0.0 assets and CC0 license are bundled under `assets/icons/` and attributed in `NOTICE`.
- [x] README usage updated.

## Verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test --workspace --locked`: **1207 passed**, 0 failed (11 new tests).
- `cargo build --release --locked --target-dir C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4`: pass. Build output: `target/stage-t4/release/daijobu.exe` in the main checkout.
- Controller fakes cover exact-tab activation without spawning, pinned ordering/restoration, output activity, failed starts, exits, retries, closes and stale IDs.
- Geometry/state tests cover one through twelve tabs, all configured notch size limits, gaps, clipping during morphs, hover, removing sessions, returning to the configured size, and OSD/content-transition ownership.
- CPU-raster tests cover both logo assets, the shell icon, all four themes, menu icon/label/hint spacing and DPI scales 100%, 150% and 200%. A scratch raster preview was inspected; no real agents were launched by tests.
- The design HTML was opened in a headless browser. Existing spring/theme/swap-chain tokens were retained.

## Run and live checks

Quit the current island from the tray, then run the build above. Open the terminal and create tabs through **+**. Close the panel or click away; while the idle clock is shown, click a shortcut to return to that same tab. Close a tab or exit its shell to remove its shortcut. Check Claude/Codex logos in **+**.

- [ ] Updated release executable started and inspected live.
- [ ] Shortcut clicks and new-tab menu visuals verified on the real desktop.
- The running island currently owns PowerShell terminal sessions. It was left running to preserve those sessions; restart approval was requested separately. No tests sent input into those sessions or started a Claude/Codex job.

## Limits

- Shortcuts cover island terminal tabs, not terminals owned by Orca, Windows Terminal or other applications.
- Shortcuts appear in Idle beside the clock; other compact content retains its existing controls.
- Agent identity belongs to the tab's launch mode. Typing an agent command in a plain shell does not change its terminal icon. An agent tab that has returned to its shell prompt keeps its agent logo until the shell exits or the tab closes.
