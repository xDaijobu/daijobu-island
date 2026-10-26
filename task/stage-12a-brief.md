# Stage 12a brief: Tile (window zones)

Status: contract written with the user on 2026-10-27, waiting for the user's review. Worktree `stage-12a-tile` starts at master `5ad1f8d` (Stage 7c merged). Implementation starts only after the review and the interface plan (`stage-12a-plan.md`).

## Contract and scope

Read `AGENTS.md`, then `task/progress.md` section **Stage 12a** (scope and acceptance are authoritative), and in `docs/plan.html` the Tile card (F3), the `tile` mockup panel (`L` layouts, 400 × 176) and the desktop zones demo. The HTML shows the eventual product, including Scenes and Boot Scene, which are later stages.

Deliver layouts and zone math, the island's Tile panel (drag picker and Home tile), the Shift+drag zone overlay, Ctrl+Alt+1‥9 / arrow hotkeys, placement with border correction, retries and restore size, and the admin-window notice. Do not implement scenes, boot scenes, elevation, a layout editor, zone spanning or per-app rules. Do not change Windows' own Snap settings.

## Architecture

```
WinEvent MOVESIZESTART/END ─┐                 ┌─ island Tile panel (dj-render paint)
cursor poll (only in a move) ├─► app tile ────┼─ zone overlay window (dj-win) + surface (dj-render)
RegisterHotKey Ctrl+Alt+…  ──┤   controller    └─ window ops (dj-win): rects, frame bounds, place
Home tile click ─────────────┘   (state machine and math in dj-core::tile)
```

- **`crates/dj-core/src/tile.rs`** (pure, `forbid(unsafe)`, time and geometry injected): `Layout` (presets + custom from TOML, validation), `zones(layout, work_area, gap_px) -> Vec<IRect>` in physical px, reading-order numbering, `neighbour(zones, from, dir)` / `nearest`, frame-border correction (`window_rect_for(zone, window_rect, frame_rect)`), the Tile panel's geometry and hit testing (layout thumbnails and their cells), the **drag state machine** (`Drag::start(hwnd, monitor)`, `poll(cursor, shift, island_rect) -> DragView`, `end() -> Option<Drop>`), restore-size bookkeeping, and `[tile]` config + `layouts.toml` parsing.
- **`crates/dj-core/src/island.rs` + `features.rs` + `layout.rs`**: `IslandState::TilePanel` (opened by the Home tile or by the drag picker, closed like the other panels), its size, `ClickOutcome::Tile { x, y }` (panel-local, like Sweep), and `tile` → `Available`.
- **`crates/dj-win/src/tile.rs`**: the out-of-context `SetWinEventHook(EVENT_SYSTEM_MOVESIZESTART..END)` → `Event::TileMove { start/end, hwnd }`; window queries and placement for other processes' windows (eligibility, `GetWindowRect`, `DWMWA_EXTENDED_FRAME_BOUNDS`, `IsZoomed`/restore, `SetWindowPos` with an access-denied result for UIPI, `GetForegroundWindow`); monitors (`EnumDisplayMonitors` → device name, rect, work area, DPI); Shift state; the zone overlay window (topmost, `WS_EX_NOACTIVATE | TOOLWINDOW | TRANSPARENT`, no input).
- **`crates/dj-render`**: the Tile panel drawn in the island painter from a `TileView` (thumbnails, highlighted cell, header line), and `ZoneOverlay` (own D3D12 + DComp chain like `PreviewSurface`, sized to one monitor, drawn from a `ZoneScene`).
- **`app`**: `app/src/tile.rs` controller behind a `WindowOps` trait (real one wraps `dj_win::tile`; tests use a fake): move events → drag state → island panel / overlay → placement, retries (timer), restore size, hotkeys, notices, config writes. Timer ids: next free after 9.

## Parallel split (same method as 7c)

| Part | Owner | Files |
|---|---|---|
| A core | Sonnet worker | `dj-core/src/tile.rs`, `lib.rs` line, `config.rs` (`[tile]`) |
| B island | Sonnet worker | `dj-core/src/island.rs`, `features.rs`, `layout.rs` (panel state, size, click outcome, unlock) |
| C native | Sonnet worker | `dj-win/src/tile.rs`, `lib.rs`, `window.rs` (one `Event` variant), `Cargo.toml` features |
| D render | Sonnet worker | `dj-render/src/tile.rs` (panel paint + `ZoneOverlay`), `paint.rs` hook-up, `lib.rs` |
| F wiring | coordinator | `app/src/tile.rs`, `app.rs`, report; review and fixes across A–D |

Exact types at the seams go in `task/stage-12a-plan.md` before the workers start; parts don't depend on each other's new code (part F maps between them).

## Headless verification

Never run `daijobu.exe`, move real windows, install hooks or register hotkeys in tests. Use a worktree-local `CARGO_TARGET_DIR`. Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo build --release`.

## Live acceptance and handoff

The coordinator and the user run the seven acceptance steps in `progress.md`. The report (`task/stage-12a-report.md`) lists every live check not performed, exact commands and results, and known issues.
