# Stage 12a Tile Implementation Plan

> **For agentic workers:** this plan runs as parallel parts A–D (one Sonnet 5.5 worker each, in its own git worktree) and part F (the coordinator). Each worker reads only its own part plus "Global Constraints" and "Review Focus". Steps use checkbox (`- [ ]`) syntax.

**Goal:** put any window into a layout zone from the island (drag picker, Home tile), Shift+drag zones and Ctrl+Alt hotkeys.

**Architecture:** all decisions and geometry are pure Rust in `dj_core::tile` (layouts, zones, navigation, the drag state machine, restore-size bookkeeping) and `dj_core::island` (the Tile panel state). `dj_win::tile` wraps the Win32 calls (move hook, window queries and placement, monitors, the zone overlay window). `dj_render::tile` paints the island's Tile panel and the zone overlay from plain data. `app/src/tile.rs` (part F) drives it all behind a `WindowOps` trait so it is testable with a fake.

**Tech Stack:** Rust stable (MSVC), `windows` 0.62, `skia-safe` 0.153 (d3d + textlayout), `serde` + `toml`. No new crates.

**Spec:** `task/progress.md` → "Stage 12a: Tile (window zones)", and `task/stage-12a-brief.md`.

## Global Constraints

- Name: "Daijobu Island" (never "Daijōbu"); user-facing text is English.
- `dj-core` is `#![forbid(unsafe_code)]` and has no Win32 or Skia dependency. `unsafe` only in `dj-win`/`dj-render`, each block with a `// SAFETY:` comment.
- No new dependencies. License Apache-2.0.
- Idle CPU ≈ 0: nothing polls outside a window move; render only on change.
- Units: `dj_core::snap::IRect { x, y, w, h }` (i32) for physical desktop px (can be negative); `dj_core::layout::Rect { x, y, w, h }` (f32) for logical px.
- Headless tests only: never run `daijobu.exe`, move real windows, install hooks or register hotkeys in tests.
- Every Bash call: `source C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh && export CARGO_TARGET_DIR=<your worktree>/target CARGO_BUILD_JOBS=4 && cd <your worktree> && …` (the env script points at a shared target; you MUST override it).
- Workers do not commit; the coordinator reviews and commits.
- Gate for every part: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, and the part's own `cargo test -p <crate>`.

## Review Focus

1. **A resize is not a move.** `EVENT_SYSTEM_MOVESIZESTART` also fires when the user drags a window's edge. A resize must never open the picker, never snap and never "restore size" (the user chose that size). → part A: `Drag::poll(.., resizing = true, ..)` keeps the panel closed and `end()` gives `None`; `Placed::after_move` with a changed size forgets the window and returns `None`.
2. **The window goes away mid-way** (closed during a drag, or before a retry). → part F: `WindowOps::info` returns `None`; the controller drops the drag/retry and forgets the window without logging an error loop.
3. **Monitors at negative coordinates and with another DPI.** Zones use that monitor's work area and scale; the overlay sits on that monitor at its own scale. → part A tests zones on a monitor at (-1920, 0) at 1.5 scale; part F test: Shift+drag target on monitor 2.
4. **Hotkey on a window we must not touch** (the desktop, the taskbar, our own island/stack windows, a minimized or cloaked window, a tool window). → part C: `eligible` rules with a pure classifier test; part F test: the fake's ineligible window is left alone.
5. **A broken or missing `layouts.toml`.** Presets still work; each bad layout is skipped with a reason. → part A tests every rejection (overlap, outside the grid, hole, >9 zones, empty/duplicate name, non-positive ratio) and that a syntax error yields zero custom layouts plus one error.

---

## Shared constants (all parts use these exact numbers)

- Tile panel: `400 × 168` logical, radius 32 (island panel size). Panel-local origin = the panel's top-left.
- Panel content: header text baseline row `y 14..30` ("tile · drop on a zone" left, "monitor N · W×H" right, 20 px side padding); thumbnails in up to two rows of six, each `52 × 32`, gap 9 (x) / 10 (y), first row top `y = 42`, the grid centred horizontally; footer line `y 128..146` (status/notice, 20 px side padding). Thumbnail cells use a 2 px inner gap.
- Picker order: the 9 presets in spec order, then up to 3 custom layouts (12 thumbnails max).
- Summon area: below the island's current lower edge, 80 logical px tall, the island's width + 40 logical px on each side (desktop px computed by the app).
- Timers (app): `TIMER_TILE_POLL = 10` (16 ms, only during a move), `TIMER_TILE_RETRY = 11`, `TIMER_TILE_NOTICE = 12`. Hotkey ids: 6–14 (Ctrl+Alt+1‥9), 15–18 (Ctrl+Alt+← → ↑ ↓).
- Gap default 8 logical px (× the target monitor's scale, rounded).

---

## Part A — `dj_core::tile` (Sonnet worker, worktree `12a-core`)

**Files:**
- Create: `crates/dj-core/src/tile.rs` (code + `#[cfg(test)] mod tests`)
- Modify: `crates/dj-core/src/lib.rs` (`pub mod tile;` + crate doc), `crates/dj-core/src/config.rs` (`[tile]`)

**Interfaces — Produces:**

```rust
use crate::layout::Rect;
use crate::snap::IRect;

pub const PANEL_W: f32 = 400.0;          // mirrors layout::TILE_W (part B); the integrator unifies
pub const PANEL_H: f32 = 168.0;
pub const MAX_ZONES: usize = 9;
pub const MAX_CUSTOM: usize = 3;
pub const DEFAULT_LAYOUT: &str = "50/50";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span { pub col: usize, pub row: usize, pub cols: usize, pub rows: usize }

#[derive(Clone, Debug, PartialEq)]
pub struct Layout { pub name: String, pub cols: Vec<f32>, pub rows: Vec<f32>, pub zones: Vec<Span> } // zones in reading order

pub fn presets() -> Vec<Layout>;                                   // 9, spec order and ratios
pub fn parse_custom(toml: &str) -> (Vec<Layout>, Vec<String>);    // valid layouts, one message per rejection
pub fn picker_layouts(custom: &[Layout]) -> Vec<Layout>;          // presets + first MAX_CUSTOM custom
pub fn find(layouts: &[Layout], name: &str) -> Option<usize>;

/// Zone rects in physical px inside `work`, `gap` px between zones and at the edges, reading order.
pub fn zones(layout: &Layout, work: IRect, gap: i32) -> Vec<IRect>;
/// The same in logical px, for thumbnails and the overlay preview.
pub fn zones_f(layout: &Layout, area: Rect, gap: f32) -> Vec<Rect>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir { Left, Right, Up, Down }
pub fn zone_of(zones: &[IRect], frame: IRect, tolerance: i32) -> Option<usize>; // the zone this frame fills
pub fn nearest(zones: &[IRect], frame: IRect) -> Option<usize>;                 // by centre distance
pub fn neighbour(zones: &[IRect], from: usize, dir: Dir) -> Option<usize>;      // nearest centre in that half-plane

/// The rect for SetWindowPos so that the visible frame (DWMWA_EXTENDED_FRAME_BOUNDS) fills `zone`.
pub fn window_rect_for(zone: IRect, window: IRect, frame: IRect) -> IRect;

// --- the island's Tile panel (panel-local logical px) ---
#[derive(Clone, Debug, PartialEq)]
pub struct Thumb { pub layout: usize, pub rect: Rect, pub cells: Vec<Rect> }
pub fn panel_thumbs(layouts: &[Layout]) -> Vec<Thumb>;            // ≤ 12, two rows of six, centred
pub fn panel_hit(thumbs: &[Thumb], x: f32, y: f32) -> Option<(usize, usize)>; // (layout, zone)
pub const PANEL_HEADER: Rect; pub const PANEL_FOOTER: Rect;
pub fn monitor_label(index: usize, rect: IRect) -> String;        // "monitor 1 · 3440×1440"

// --- one window move ---
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor { pub id: String, pub rect: IRect, pub work: IRect, pub scale: f32 }

pub struct Geometry<'a> {
    pub monitors: &'a [Monitor],
    pub layouts: &'a [Layout],
    pub active: &'a [usize],        // active layout per monitor (same order as `monitors`)
    pub island_monitor: usize,
    pub summon: IRect,              // desktop px
    pub panel: IRect,               // desktop px of the panel when open (its final, not animated, rect)
    pub panel_scale: f32,           // island scale (device px per logical px)
    pub gap: f32,                   // logical px; × each monitor's scale, rounded
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target { pub monitor: usize, pub layout: usize, pub zone: usize, pub from_panel: bool }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayView { pub monitor: usize, pub layout: usize, pub highlight: Option<usize> }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DragView { pub panel_open: bool, pub panel_hover: Option<(usize, usize)>, pub overlay: Option<OverlayView>, pub target: Option<Target> }

pub struct Drag { /* window id, panel open flag, last view */ }
impl Drag {
    pub fn new(window: u64) -> Self;
    pub fn window(&self) -> u64;
    /// One cursor sample. `resizing`: the window's size changed since the start (an edge drag).
    pub fn poll(&mut self, cursor: (i32, i32), shift: bool, resizing: bool, geo: &Geometry) -> DragView;
    /// The release: the target of the last poll (None for a resize or nothing under the cursor).
    pub fn end(self) -> Option<Target>;
}

// --- restore size ---
#[derive(Default)]
pub struct Placed { /* window id -> size before the first placement */ }
impl Placed {
    pub fn placed(&mut self, window: u64, before: (i32, i32));   // keeps the first size if already placed
    pub fn is_placed(&self, window: u64) -> bool;
    /// After a move ended outside any zone: the rect to restore (old size at the drop position) and
    /// forget. A changed size (a resize) forgets and returns None.
    pub fn after_move(&mut self, window: u64, start: IRect, end: IRect) -> Option<IRect>;
    pub fn forget(&mut self, window: u64);
}
```

Drag rules (`poll`): if `resizing` → closed panel, no overlay, no target, for the rest of this drag. The panel opens when the cursor is in `summon` and stays open while the cursor is in `summon` ∪ `panel`; closed otherwise. With the panel open and the cursor on a cell → `panel_hover = (layout, zone)`, `overlay = {island_monitor, layout, Some(zone)}`, `target = {island_monitor, layout, zone, from_panel: true}`. Else if `shift` → the monitor containing the cursor (nearest if none), its active layout's zones (`zones(work, round(gap × scale))`), `overlay = {that monitor, active, zone under cursor}`, `target` = that zone if any (`from_panel: false`). Else no overlay, no target.

Config (`config.rs`): `TileConfig { gap: f32 = 8.0, restore_size: bool = true, number_keys: bool = true, arrow_keys: bool = true, monitors: BTreeMap<String, String> }` under `[tile]` (`[tile.monitors]` maps a monitor id to a layout name), lenient parsing like the existing sections (bad values → defaults, the rest of the file survives), `gap` clamped to 0..=48.

`layouts.toml` format:
```toml
[[layout]]
name = "code + docs"
cols = [2, 1]
rows = [1, 1]
zones = [[0, 0, 1, 2], [1, 0, 1, 1], [1, 1, 1, 1]]   # col, row, col_span, row_span
```
Valid: non-empty unique name (not a preset name), 1–6 positive ratios per axis, 1–9 zones, every span inside the grid, no overlaps, every grid cell covered.

- [ ] **A1** Layouts: `Span`, `Layout`, `presets()` (ratios: 60/40 = [3,2]; 70/30 = [7,3]; 1+2 = cols [1.4,1], rows [1,1]; wide center = [1,2,1]; 1+3 = cols [1.6,1], rows [1,1,1]), reading-order normalisation, `parse_custom` + validation, `picker_layouts`, `find`. Tests: each preset's zone count and spans; every rejection reason; syntax error → `([], [one error])`; >3 custom → only 3 in the picker; duplicate of a preset name rejected.
- [ ] **A2** Zone math: `zones`, `zones_f`, `zone_of`, `nearest`, `neighbour`, `window_rect_for`, `monitor_label`. Tests: all presets at 1920×1080 (work area 1920×1032 with a bottom taskbar), 3440×1440 with the 42 px AppBar strip on top, a monitor at (-1920, 0) scale 1.5 with gap 12; zones never overlap, stay inside `work`, edges and gaps exact (remainder px go to the last zone so the far edge is exact); `zone_of` tolerance; `neighbour` in `1+2` and `2x2` for all four directions incl. none; `window_rect_for` with typical Win11 borders (7 px left/right/bottom, 0 top).
- [ ] **A3** Panel geometry: `panel_thumbs`, `panel_hit`, `PANEL_HEADER`, `PANEL_FOOTER`. Tests: 9 and 12 layouts lay out in rows of six, centred, inside 400×168, header/footer don't overlap thumbs; hits on cells, misses in gaps and the header.
- [ ] **A4** Drag state machine (`Geometry`, `Drag`, views) per the rules above. Tests: summon enter → open; leaving both summon and panel → closed; cell hover → target + overlay on the island monitor; release over a gap → None; Shift on monitor 2 (negative origin, 1.5) → overlay there with the right zone; Shift released → overlay gone; resize → never opens, `end()` None even over a cell.
- [ ] **A5** `Placed`: first-size retention across two placements, `after_move` restore rect, resize forgets, `forget`.
- [ ] **A6** `[tile]` config + tests (defaults, values, bad types, clamped gap, `monitors` table round trip with the Stage S writer).
- [ ] **A7** Gate: fmt, `cargo clippy -p dj-core --all-targets -- -D warnings`, `cargo test -p dj-core`, then `cargo clippy --workspace --all-targets -- -D warnings`.

## Part B — island Tile panel (Sonnet worker, worktree `12a-island`)

**Files:** Modify `crates/dj-core/src/island.rs`, `crates/dj-core/src/features.rs`, `crates/dj-core/src/layout.rs` (no other files).

**Interfaces — Produces:**
```rust
// layout.rs
pub const TILE_W: f32 = 400.0; pub const TILE_H: f32 = 168.0; pub const TILE_R: f32 = 32.0;
// island.rs
IslandState::TilePanel                       // new layer; LAYERS 11 → 12; `Layers.tile: f32`
impl Island { pub fn open_tile(&mut self, tokens: &ResolvedTokens); pub fn tile_open(&self) -> bool; }
ClickOutcome::Tile { x: f32, y: f32 }        // panel-local logical px (like `Sweep`), every click inside the panel
// features.rs: `tile` → Available ("Tile & Scenes" name kept); Home's tile click gives the existing ClickOutcome::Open(i)
```
Rules: `open_tile` works from any rest state and from Home; it springs to 400×168 r32 like the other panels; `collapse` returns to the rest state (Music/Timer compact or Idle). A click inside the panel always yields `Tile { x, y }` (the app decides; an empty spot collapses there). The panel ranks like the other panels (the snap thumb and OSD still win). Check every `match` on `IslandState` and the layer count.

- [ ] **B1** Layout constants + state + layer + `open_tile`/`tile_open` + target size. Tests: opening from Idle, Home and Music compact; layer fades; size 400×168; collapse goes back to the right rest state.
- [ ] **B2** Click routing → `ClickOutcome::Tile` with panel-local coordinates (test the four corners and the centre), the snap thumb and OSD still outrank it.
- [ ] **B3** Registry: `tile` Available, the Home grid click on it gives `Open(i)`; update the "N of 12 ready" count/tests.
- [ ] **B4** Gate: fmt, clippy (-p dj-core and workspace), `cargo test -p dj-core`.

## Part C — `dj_win::tile` (Sonnet worker, worktree `12a-win`)

**Files:** Create `crates/dj-win/src/tile.rs`. Modify `crates/dj-win/src/lib.rs`, `crates/dj-win/src/window.rs` (only the `Event::TileMove(crate::tile::MoveEvent)` variant), `crates/dj-win/Cargo.toml` (features as needed), and in `app/src/app.rs` only the placeholder arm `Event::TileMove(_) => {}` so the workspace compiles.

**Interfaces — Produces:**
```rust
pub type Raw = isize;                                         // HWND as a plain value
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveEvent { Start(Raw), End(Raw) }

pub fn install_move_hook() -> bool;   // SetWinEventHook(EVENT_SYSTEM_MOVESIZESTART..END, OUTOFCONTEXT | SKIPOWNPROCESS) → push_event(Event::TileMove)
pub fn uninstall_move_hook();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WinInfo { pub rect: (i32, i32, i32, i32), pub frame: (i32, i32, i32, i32), pub maximized: bool, pub eligible: bool }
pub fn window_info(w: Raw) -> Option<WinInfo>;                // None if gone; frame = DWMWA_EXTENDED_FRAME_BOUNDS (rect if that fails)

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaceError { AccessDenied, Gone, Other(String) }
pub fn place(w: Raw, x: i32, y: i32, cx: i32, cy: i32) -> Result<(), PlaceError>; // SW_RESTORE first if maximized; SWP_NOZORDER | SWP_NOACTIVATE; ERROR_ACCESS_DENIED → AccessDenied (UIPI)
pub fn foreground() -> Raw;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorInfo { pub device: String, pub rect: (i32, i32, i32, i32), pub work: (i32, i32, i32, i32), pub dpi: u32 }
pub fn monitors() -> Vec<MonitorInfo>;                        // primary first, then EnumDisplayMonitors order
pub fn shift_down() -> bool;                                  // GetAsyncKeyState(VK_SHIFT)

// zone overlay window: hidden at creation, WS_POPUP, WS_EX_NOREDIRECTIONBITMAP | TOOLWINDOW | TOPMOST | NOACTIVATE | TRANSPARENT,
// WM_NCHITTEST → HTTRANSPARENT, WM_MOUSEACTIVATE → MA_NOACTIVATE, class "DaijobuTileZones"
pub fn create_overlay(x: i32, y: i32, w: i32, h: i32) -> windows::core::Result<HWND>;
pub fn show_overlay(hwnd: HWND); pub fn hide_overlay(hwnd: HWND); pub fn destroy_overlay(hwnd: HWND);
```
Eligibility (pure helper `fn classify(style, ex_style, visible, iconic, cloaked, own_process, is_root) -> bool`, tested): visible, not iconic, not cloaked, root window, not ours, not `WS_EX_TOOLWINDOW`, has `WS_CAPTION` or `WS_THICKFRAME`, not the shell's desktop/taskbar (`Progman`, `WorkerW`, `Shell_TrayWnd`, `Shell_SecondaryTrayWnd` by class name).

- [ ] **C1** Move hook + `Event::TileMove` + placeholder arm in app.rs. Pure test of the event translation.
- [ ] **C2** `window_info`, `classify`/eligibility, `foreground`. Tests on `classify` for every rule.
- [ ] **C3** `place` with restore-if-maximized and the error mapping (pure helper from the Win32 error code, tested).
- [ ] **C4** `monitors()` (device name, rects, effective DPI; primary first), `shift_down`.
- [ ] **C5** Overlay window functions.
- [ ] **C6** Gate: fmt, `cargo clippy -p dj-win --all-targets -- -D warnings`, `cargo test -p dj-win`, workspace clippy.

## Part D — `dj_render::tile` (Sonnet worker, worktree `12a-render`)

**Files:** Create `crates/dj-render/src/tile.rs`. Modify `crates/dj-render/src/lib.rs` (mod + `pub use tile::{TilePanel, TileThumb, ZoneOverlay, ZoneScene};`). Do **not** touch `paint.rs` (part F hooks the panel into the island painter).

**Interfaces — Produces:**
```rust
pub struct TileThumb<'a> { pub rect: Rect, pub cells: &'a [Rect], pub active: bool, pub hover: Option<usize> }  // panel-local logical px
pub struct TilePanel<'a> { pub title: &'a str, pub monitor: &'a str, pub thumbs: &'a [TileThumb<'a>],
                           pub footer: Option<(&'a str, bool)>,   // (text, warn)
                           pub header: Rect, pub footer_rect: Rect }
/// Draws the panel's content inside `panel` (window-logical px) at `scale`; the island painter has
/// already drawn the pill body. Used by the island painter (part F) and by tests on a CPU surface.
pub fn paint_tile_panel(canvas: &Canvas, fonts: &mut Fonts, tokens: &ResolvedTokens, panel: Rect, scale: f32, p: &TilePanel);

pub struct ZoneScene<'a> { pub tokens: &'a ResolvedTokens, pub scale: f32, pub zones: &'a [Rect], pub highlight: Option<usize> } // client px
pub struct ZoneOverlay { /* own Gpu + Chain like PreviewSurface; drop order */ }
impl ZoneOverlay {
    pub fn new(hwnd: isize, width: u32, height: u32) -> Result<Self, RenderError>;
    pub fn render(&mut self, scene: &ZoneScene) -> Result<(), RenderError>;   // clear transparent, paint, Present(1)
}
```
Look: thumbnails are small rounded rects (r 6) in `text` at low alpha with their cells in `s2`-like tint; the active layout's thumb has an accent outline; the hovered cell is filled with the accent. Zones in the overlay: rounded rects (r 12 × scale) filled with the pill colour at ~35 % alpha, a 2 px text-colour outline at 50 %, the highlighted zone filled with the accent at ~45 % and outlined at 100 %, the zone number (1‥9) centred in the mono font, large (48 px × scale), legible on any wallpaper (outline/halo). Header in `font_label`, mono, 11 px; footer warn text in `warn`.

- [ ] **D1** `paint_tile_panel` + CPU tests: header text inside its rect; an active thumb's outline; a hovered cell filled with accent; the warn footer; nothing drawn outside `panel`.
- [ ] **D2** `ZoneOverlay` + `paint_zones` (CPU-testable function) + tests: zone fill and highlight colours, numbers present, transparent between zones, scale 1.5.
- [ ] **D3** Gate: fmt, `cargo clippy -p dj-render --all-targets -- -D warnings`, `cargo test -p dj-render`, workspace clippy.

## Part F — wiring (coordinator, on `xDaijobu/stage-12a-tile` after A–D merge)

**Files:** Create `app/src/tile.rs`. Modify `app/src/app.rs`, `app/src/storage.rs` (`layouts.toml` path + read), `crates/dj-render/src/paint.rs` (Tile layer → `paint_tile_panel`), `crates/dj-core/src/tile.rs` (`PANEL_W/H` → `layout::TILE_*`), `task/*`.

- `WindowOps` trait (`info`, `place`, `foreground`, `monitors`, `shift_down`) with the real impl over `dj_win::tile` and a fake for tests.
- Controller: `TileMove::Start` → `Drag::new`, arm `TIMER_TILE_POLL`; poll → `Drag::poll` with the island's summon/panel rects → open/close the island panel, set its hover, show/move/hide the zone overlay (one window on the target monitor, recreated when the monitor changes); `End` → `Drag::end` → place (`window_rect_for` on the target zone, restore first if maximized, `Placed::placed` with the pre-placement size, set the active layout and save the config when `from_panel`), or `Placed::after_move` restore; kill the poll timer, close the panel and the overlay.
- Retries: after a placement, re-check at 150/300/500 ms (`TIMER_TILE_RETRY`); place again if the frame drifted more than 4 px; stop after 3.
- Hotkeys 6–18 per `[tile]`; Home tile `Open(tile)` → remember `foreground()` at the click and open the panel; `ClickOutcome::Tile{x,y}` → `panel_hit` → place that window on the island's monitor, else collapse.
- UIPI: `PlaceError::AccessDenied` → the panel footer "can't move an admin window" (opening the panel for 2 s after a hotkey, `TIMER_TILE_NOTICE`).
- Tests with the fake: drop on a panel cell; drop outside → restore size; resize → nothing; Shift target on monitor 2; maximized → restored then placed; retries stop after 3; access denied → footer, nothing moved; window gone → silently dropped; hotkey N beyond the layout → no-op; ineligible foreground → no-op; arrows between `2x2` zones.
- Report `task/stage-12a-report.md`; progress update; full gate (fmt, clippy, workspace tests, release).
