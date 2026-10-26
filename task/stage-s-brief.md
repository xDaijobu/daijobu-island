# Stage S brief: Settings (minimal), for the worktree agent

Your worktree starts from `master`, which has Stages 1–7a and the Snap hotkey fix. You are adding a gear icon on Home and a small Settings panel in the island: a *Start with Windows* toggle and two Snap hotkey chips.

## HEADLESS
Don't run `daijobu.exe`. Don't touch the real HKCU Run key, real hotkeys or the real clipboard in tests; put them behind traits or fakes. Before every cargo command, run `source C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh`. Verify only with `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` and `cargo build --release`. There is no python on this machine; use the Edit/Write tools.

## Read first
1. `AGENTS.md`.
2. The contract: `task/progress.md`, section "Stage S: Settings (minimal)". Build exactly that, and don't edit progress.md.
3. How a panel is wired: the Timer panel (`dj-core/src/timer.rs`, `island.rs`, `layout.rs`, `dj-render/src/timer.rs`, `app/src/timer.rs`).
4. The autostart code (`dj-win`, which the tray's *Start with Windows* uses), the tray menu sync in `app/src/app.rs`, config saving (`app/src/storage.rs`, `dj-core/src/config.rs`), and the Snap hotkey setup in `app.rs` (`register_snap_hotkey`, `register_snap_key`, `hook::set_print_screen`, and `HOTKEY_SNAP`/`HOTKEY_SNAP_ALT` in `app/src/snap.rs`).

## Guidance
- `dj-core`: a pure `settings` module with the chip cycles (`next_hotkey`, and `next_alt_hotkey` that skips the main key and includes "off" as `"none"`) plus the panel's hit testing. Add a `Settings` island state with its own layer, opened from a gear rect in the Home header. Add a gear icon in `dj-render/src/icons.rs`.
- App: a hotkey change re-runs the Snap hotkey setup. Unregister ids 3 and 5 and turn the PrtSc hook takeover off first, then register the new keys. Keep the volume-key switch untouched. Save the config, and show a warn status if registration fails.
- Toggling autostart calls the existing autostart code, saves `start_with_windows`, and updates the tray check mark.
- Match the existing style, naming and comment density. No new dependencies.

## Finish
Commit in small steps, and don't push. Write `task/stage-s-report.md` with: a checklist, test counts, deviations, what wasn't verified, and the pill-local rects of the gear, the toggle, both chips and back, so they can be clicked in a live test. Then stop.
