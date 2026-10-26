# Stage S report: Settings (minimal)

Headless: nothing was run. The exe, the real HKCU Run key, real hotkeys and the clipboard were never touched.

## Checklist
- [x] **Gear icon** in the Home header, left of the clock (`layout::HOME_GEAR`, `icons::draw_gear`). Hover shows a surface behind it and the accent ink. It is not a feature tile; the registry count is unchanged (5 of 12).
- [x] **Settings panel** (`IslandState::Settings`, 400x184, r 32, its own layer). BACK or an empty click returns to Home. Rows: Start with Windows (switch), Snap hotkey chip, Snap alt hotkey chip.
- [x] **Start with Windows** calls `system::set_autostart` (the tray's code), saves `start_with_windows`. The tray menu reads the Run key every time it opens, so its check mark follows.
- [x] **Hotkey chips** cycle as in the contract. The pure cycles live in `dj-core/src/settings.rs`; `cycle_hotkey` also moves the alt off a key the main chip just took.
- [x] **Applied live:** `register_snap_hotkey` now unregisters ids 3 and 5, turns the PrtSc takeover off, then registers the new keys (PrtSc through the hook, the rest through `RegisterHotKey`). The volume-key switch is untouched, and the hook stays installed. A failed key sets `snap_taken`: the chip draws in warn with the `TAKEN BY ANOTHER APP` line, and the reason goes to the log.
- [x] **Persisted:** `storage::save_config` after each change; the config round trip keeps the other sections (test).
- [x] **Idle CPU:** no timers; a click or hover redraws once.
- [x] Tests: hotkey cycles (order, skipping main, off), hit testing (gear, toggle, chips, back, empty), layout fit/overlap, island transitions Home <-> Settings, config round trip.

## Verification
- `cargo fmt --all`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: all pass, 303 tests.
- `cargo build --release`: the default target dir failed with "Access is denied" removing `daijobu.exe` (a running daijobu.exe, pid 42412, holds it; I did not kill it). Built successfully with `CARGO_TARGET_DIR` set to a scratch dir, and `cargo check --release` passes in the shared dir.

## Deviations
- No separate **press** state on the gear (only hover). Other controls (Timer panel buttons, chips) have no press state either.
- The Snap hotkey chip can land on the key the alt chip holds; the alt then moves to its next free value, so it can change as a side effect.
- The switch state is drawn from `config.start_with_windows` (not read from the registry per frame).

## Not verified
- Anything visual (gear glyph, switch, chip placement, the gear's clearance from the clock at other themes' digit fonts), and the live behaviour: the Run key, PrtSc through the hook after a change, `RegisterHotKey` collisions, the warn line, persistence across a restart.

## Click targets (for a live test)
Pill-local logical px, and the offset to the 520-wide island window's client coords at scale 1. Home is 520 wide (x offset 0); the Settings pill is 400 wide, so its x offset is 60. Both have y offset 6 (`TOP_MARGIN`).

| Control | Pill-local rect (x, y, w, h) | Client rect | Client centre |
|---|---|---|---|
| Home gear | 384, 16, 28, 28 | 384, 22 | 398, 36 |
| Back | 304, 8, 84, 24 | 364, 14 | 406, 26 |
| Autostart toggle | 328, 46, 52, 24 | 388, 52 | 414, 64 |
| Snap hotkey chip | 248, 82, 132, 24 | 308, 88 | 374, 100 |
| Snap alt chip | 248, 118, 132, 24 | 308, 124 | 374, 136 |

## Live test (coordinator + user, 2026-10-26)
Release build of this branch, driven by posted clicks.

| Check | Result |
|---|---|
| Home → gear opens the Settings panel (SETTINGS · BACK, the toggle, both chips) | Pass |
| The toggle reflects the real state (on: `DaijobuIsland` is in HKCU Run) | Pass |
| Toggle off → Run entry removed and `start_with_windows = false`; on → restored and `true` | Pass |
| Main chip PrtSc → Ctrl+Space; alt moves off Ctrl+Space to Ctrl+Alt+S; both saved | Pass |
| User: Ctrl+Space and Ctrl+Alt+S open the snap, and PrtSc no longer does | Pass (user) |
| Cycle back to PrtSc and Ctrl+Space through the chips (order and skip rule as in the contract) | Pass |
| Back returns to Home | Pass |

Not verified: a registration failure (the warn chip), and persistence across a restart (the config file is written, so it should hold).
