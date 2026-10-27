# Stage 12a report: Tile (window zones)

Status: **built and verified headless** on `xDaijobu/stage-12a-tile`. **No live test yet.**

Built 2026-10-27 by the coordinator (Claude Opus 5.5) with four parallel Claude Sonnet 5.5 workers in separate worktrees, from the plan in [`stage-12a-plan.md`](stage-12a-plan.md).

| Part | What | By |
|---|---|---|
| A core | `dj_core::tile`: 9 presets + custom `layouts.toml` with validation, zone math, reading order, `zone_of` / `nearest` / `neighbour`, border correction, the panel's thumbnail grid and hit test, the drag state machine, restore-size bookkeeping; `[tile]` config | Sonnet worker |
| B island | `IslandState::TilePanel` (400 × 168, own layer), `open_tile`, panel-local `ClickOutcome::Tile`, `tile` Available on Home | Sonnet worker |
| C native | `dj_win::tile`: the out-of-context move hook, window info/eligibility, placement with UIPI → `AccessDenied`, monitors, Shift, the zone overlay window | Sonnet worker |
| D render | `dj_render::tile`: the panel's content and the `ZoneOverlay` surface | Sonnet worker |
| F wiring | `app/src/tile.rs` (`Tiler` behind `WindowOps`), `app.rs`, the panel hook in `paint.rs`, `layouts.toml` loading | coordinator |

## Checklist (Stage 12a scope in `progress.md`)

Legend: ✅ done and covered by headless tests · 🟡 done, needs the live test to confirm

- ✅ **Layouts.** The 9 presets in spec order; custom layouts from `~/.daijobu/layouts.toml` (bad ones skipped and logged with the reason, one syntax error → none); up to 3 custom in the picker; reading-order numbering; active layout per monitor in `[tile.monitors]` (by `\\.\DISPLAYn`), default `50/50`.
- ✅ **Zone rects** from the work area (taskbar and AppBar strip respected), gap 8 × scale between zones and at the edges, exact far edges, negative coordinates.
- 🟡 **Picker.** The move hook arms a 16 ms cursor poll only during a move. The summon area is under the island's current lower edge (80 px tall, island width + 40 each side); the panel stays open over the summon area or the panel; a cell hover lights it and previews the zone on screen; release on a cell places the window and makes that layout active. The picker targets the island's (primary) monitor. No summon area while the island is hidden or a snap is up.
- 🟡 **Shift + drag.** The active layout's zones on the monitor under the cursor (its own scale), the hovered one lit, release inside → placed. A panel cell wins over Shift.
- 🟡 **Hotkeys** Ctrl+Alt+1‥9 and Ctrl+Alt+arrows (ids 6–18), each group switchable in `[tile]`; a key held by another app is logged. Numbers beyond the layout do nothing; arrows go to the nearest zone first, then to neighbours.
- 🟡 **Home tile.** Opens the panel for the app in front ("tile · pick a zone"); hover lights cells; a click on a cell places it, anywhere else closes the panel.
- 🟡 **Placement.** Eligible windows only; maximized ones are restored by `place`; the visible frame (DWM extended bounds) fills the zone; re-checked at 150/300/500 ms and placed again if it drifted more than 4 px; restore size when a placed window is dragged out (a resize forgets it; a maximized window isn't remembered).
- 🟡 **Admin windows.** `AccessDenied` → nothing moves and the panel shows "can't move an admin window" for 2 s.
- 🟡 **Zone overlay.** One topmost, no-activate window on the target monitor, drawn with its own DComp surface, placed under the island so it never covers the panel, redrawn only when the zones or the lit one change, destroyed at the end of the move.
- ✅ **Idle.** Nothing polls outside a move; the hook is the only always-on part; the WinEvent callback posts a `WM_NULL` so `GetMessageW` returns and the queued move event is handled at once.
- ✅ **Config** `[tile]`: `gap`, `restore_size`, `number_keys`, `arrow_keys`, `[tile.monitors]`; lenient parsing; written only when an active layout changes.
- ✅ **Tests.** See below.

## Commands and results

Worktree-local target (`CARGO_TARGET_DIR=…/stage-12a-tile/target`), env from `dj-build-env.sh`:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | FMT |
| `cargo clippy --workspace --all-targets -- -D warnings` | CLIPPY |
| `cargo test --workspace` | TESTS |
| `cargo build --release` | RELEASE |

New tests: core 49 (`tile`) + 3 config, island 13, win 15, render 17, controller 14.

## Known issues and decisions

- The picker always targets the island's monitor; other monitors use Shift+drag or the hotkeys (agreed in the contract).
- Monitors are keyed by `\\.\DISPLAYn`, which can change when displays are reconnected; the active layout then falls back to `50/50`.
- `SetWindowPos` is synchronous: a hung target window can stall the island briefly.
- A maximized window placed in a zone has no remembered size (its old size would be the monitor's).
- Ctrl+Alt+arrows rotate the screen on some old Intel drivers, and Ctrl+Alt+digits are AltGr characters on some European layouts; both groups can be turned off in `[tile]`.
- The part C worker ran a short read-only live probe (monitors, window info, hook install/uninstall, overlay create/destroy) outside the tests; no window was moved.

## Not verified (needs the live test)

The seven acceptance steps in `progress.md`: the panel opening under the notch before Windows' snap bar, a drop on `1+2` filling the zone without a gap, restore size on drag-out, Shift+drag on any monitor, Ctrl+Alt+1‥3 and arrows, the Home tile with Notepad, Task Manager's notice, 100 %/150 % and the ultrawide, and idle CPU ≈ 0. Also first-run checks: the GPU path of `ZoneOverlay`, the move hook's timing, and the overlay's z-order against the island.
