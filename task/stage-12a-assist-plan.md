# Stage 12a window picker (assist) — implementation plan

> Parallel parts A–C (one Sonnet 5.5 worker each, own git worktree), then F (coordinator). Each worker reads only its part plus "Global Constraints" and "Shared constants".

**Goal:** from the Home-opened Tile panel, a click on a cell fills that layout zone by zone: a picker covering the lit zone shows the open windows as cards with live DWM thumbnails; pick one, it fills the zone, the picker moves to the next zone.

**Spec:** `task/progress.md` → Stage 12a → "Window picker (assist)" (and the rest of Stage 12a for context).

## Global Constraints

Same as `task/stage-12a-plan.md`: Rust stable (MSVC), `windows` 0.62, `skia-safe` 0.153; no new dependencies; `dj-core` is `forbid(unsafe)` with no Win32/Skia; every `unsafe` has a `// SAFETY:` comment; English UI text; idle CPU ≈ 0 (the picker renders only on input); headless tests only (no exe, no real windows moved, no hooks, no thumbnails registered in tests); every Bash call sources `dj-build-env.sh` and overrides `CARGO_TARGET_DIR` to the worktree's own `target`; workers don't commit; gate = `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, the part's `cargo test -p <crate>`.

## Shared constants (logical px; the picker window is exactly the zone rect, its client origin = the zone's top-left)

- Zone padding 24; header row at the top `24..56` (title left, "1 / 2" step right); footer row at the bottom (last 28 px above the padding); cards in between.
- Card: an icon + title row 28 tall above a thumbnail area with aspect 16:10; card width between 120 and 320; gap 16 between cards; the grid is centred in the space between header and footer. Icons are drawn at 20 px.
- More windows than fit at 120 px wide: the first ones that fit are shown and the footer says "N more not shown".

---

## Part A — `dj_core::tile_assist` (worktree `12a-assist-core`)

**Files:** create `crates/dj-core/src/tile_assist.rs`; `crates/dj-core/src/lib.rs` (one `pub mod tile_assist;` line + doc). Nothing else.

```rust
use crate::layout::Rect;
use crate::snap::IRect;

pub const PAD: f32 = 24.0;
pub const HEADER_H: f32 = 32.0;          // the header row is PAD..PAD+HEADER_H
pub const FOOTER_H: f32 = 28.0;          // the footer row ends PAD above the bottom
pub const TITLE_H: f32 = 28.0;
pub const ICON: f32 = 20.0;
pub const CARD_GAP: f32 = 16.0;
pub const CARD_MIN_W: f32 = 120.0;
pub const CARD_MAX_W: f32 = 320.0;
pub const THUMB_ASPECT: f32 = 1.6;       // width / height of the thumbnail area

/// One "fill every zone" run.
pub struct Assist { /* zones, fill order, step, chosen windows, keyboard selection */ }
impl Assist {
    /// `zones` in reading order (physical px); the run starts at `first`, then the others in reading order.
    pub fn new(zones: Vec<IRect>, first: usize) -> Self;
    /// The zone being filled: (its index in the layout, its rect). None once every zone is done.
    pub fn zone(&self) -> Option<(usize, IRect)>;
    /// (1-based step, number of zones), for "1 / 2".
    pub fn progress(&self) -> (usize, usize);
    /// `windows` (newest first) minus the ones already picked in this run.
    pub fn candidates(&self, windows: &[u64]) -> Vec<u64>;
    /// `window` filled the current zone: remember it, go to the next zone, reset the selection.
    pub fn chose(&mut self, window: u64);
    pub fn done(&self) -> bool;
    /// The keyboard selection (index into the current candidates).
    pub fn selected(&self) -> usize;
    pub fn set_selected(&mut self, i: usize);
    /// A key with `cols` columns and `count` cards on screen.
    pub fn key(&mut self, key: AssistKey, cols: usize, count: usize) -> AssistCmd;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum AssistKey { Left, Right, Up, Down, Home, End, Enter, Escape }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum AssistCmd { None, Redraw, Pick(usize), Cancel }

#[derive(Clone, Debug, PartialEq)] pub struct Card { pub rect: Rect, pub icon: Rect, pub title: Rect, pub thumb: Rect }
#[derive(Clone, Debug, PartialEq)] pub struct AssistLayout { pub header: Rect, pub footer: Rect, pub cards: Vec<Card>, pub cols: usize, pub hidden: usize }
/// The card grid for a zone of `w` × `h` logical px and `count` windows (zone-local logical px).
pub fn layout(w: f32, h: f32, count: usize) -> AssistLayout;
pub fn hit(l: &AssistLayout, x: f32, y: f32) -> Option<usize>;
/// `src` (w, h) aspect-fitted and centred in `area`.
pub fn fit(area: Rect, src: (f32, f32)) -> Rect;
```
Rules: the largest card width in `[CARD_MIN_W, CARD_MAX_W]` whose grid (cols × rows, card height = `TITLE_H + width / THUMB_ASPECT`) fits the area between header and footer; when even 120 px doesn't fit all, show as many as fit (`hidden` = the rest). Keys: arrows move by 1 / by `cols`, clamped (no wrap); Home/End; Enter → `Pick(selected)`; Escape → `Cancel`; `count == 0` → arrows do nothing, Enter `None`. Tests: order (first zone, then the rest), candidates without picked ones, progress, done; layout for 1, 2, 7, 30 windows in a half of 1920×1080 and of 3440×1440 and in a third; never outside the zone, never overlapping, header/footer clear; `hidden`; hit; fit; every key incl. clamps.

## Part B — native picker support (worktree `12a-assist-win`)

**Files:** create `crates/dj-win/src/assist.rs`; `crates/dj-win/src/lib.rs`; `crates/dj-win/src/window.rs` (ONLY `Event::Assist(crate::assist::AssistInput)`); `crates/dj-win/Cargo.toml` features; in `app/src/app.rs` ONLY the placeholder arm `Event::Assist(_) => {}`.

```rust
pub type Raw = crate::tile::Raw;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowEntry { pub w: Raw, pub title: String, pub minimized: bool, pub elevated: bool }
/// Top-level windows a user would switch to, newest (top of the z-order) first: visible, titled, not
/// cloaked (other virtual desktops), not tool windows, not ours, not the shell (Progman/WorkerW/tray),
/// root owner only (Alt-Tab rules: WS_EX_APPWINDOW or no owner). Minimized ones included.
pub fn list_windows() -> Vec<WindowEntry>;
/// `elevated`: the owning process runs elevated (TokenElevation); a token we may not open counts as elevated.

/// The window's icon (WM_GETICON small/big, else the class icon) drawn into `size`×`size` premultiplied BGRA.
pub fn icon_bgra(w: Raw, size: u32) -> Option<Vec<u8>>;

/// A live DWM thumbnail of `src` drawn on `dest` (unregistered on drop).
pub struct Thumbnail { /* HTHUMBNAIL */ }
impl Thumbnail {
    pub fn new(dest: HWND, src: Raw) -> Option<Thumbnail>;
    pub fn source_size(&self) -> Option<(i32, i32)>;
    /// Destination rect in `dest`'s client px (x, y, w, h); `visible` false hides it.
    pub fn show(&self, rect: (i32, i32, i32, i32), visible: bool);
}

/// Un-minimizes and un-maximizes a window before it is placed.
pub fn restore_any(w: Raw);
/// Brings our own picked windows to the front at the end (SetForegroundWindow, best effort).
pub fn activate(w: Raw);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssistInput { Move { x: i32, y: i32 }, Press { x: i32, y: i32 }, Release { x: i32, y: i32 }, Leave, Key { vk: u16, ctrl: bool, shift: bool }, FocusLost }
/// The picker window: WS_POPUP, WS_EX_NOREDIRECTIONBITMAP | TOOLWINDOW | TOPMOST (activatable: it needs the
/// keyboard), class "DaijobuTileAssist", hidden at creation; messages → push_event(Event::Assist(..)) like previews.rs.
pub fn create(x: i32, y: i32, w: i32, h: i32) -> windows::core::Result<HWND>;
pub fn place(hwnd: HWND, x: i32, y: i32, w: i32, h: i32);
/// Shows it on top and gives it the keyboard (SetForegroundWindow + SetFocus).
pub fn show(hwnd: HWND);
pub fn destroy(hwnd: HWND);
```
Tests: pure helpers only (the Alt-Tab filter as a pure classifier over style/ex-style/owner/class/cloaked/visible/title; icon pixel conversion on a synthetic DIB if you factor it; input translation). No thumbnails registered, no windows created in tests.

## Part C — `dj_render::assist` (worktree `12a-assist-render`)

**Files:** create `crates/dj-render/src/assist.rs`; `crates/dj-render/src/lib.rs` (mod + `pub use assist::{AssistCard, AssistScene, AssistSurface};`). Not `paint.rs`.

```rust
pub struct AssistCard<'a> {
    pub rect: Rect, pub icon_rect: Rect, pub title_rect: Rect, pub thumb_rect: Rect,   // window logical px
    pub title: &'a str, pub icon: Option<u64>,       // key into the surface's icon cache
    pub hovered: bool, pub selected: bool, pub elevated: bool,
    /// No live thumbnail (minimized or refused): draw the icon large in the thumb area instead.
    pub no_thumb: bool,
}
pub struct AssistScene<'a> {
    pub tokens: &'a ResolvedTokens, pub scale: f32,
    pub size: (f32, f32),                             // the window in logical px (the zone)
    pub header: Rect, pub title: &'a str, pub step: &'a str,
    pub footer: Rect, pub footer_text: &'a str, pub warn: bool,
    pub cards: &'a [AssistCard<'a>],
}
pub struct AssistSurface { /* own Gpu + Chain like ZoneOverlay; icon cache */ }
impl AssistSurface {
    pub fn new(hwnd: isize, width: u32, height: u32) -> Result<Self, RenderError>;
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), RenderError>;   // the next zone may differ in size
    pub fn set_icon(&mut self, key: u64, bgra: &[u8], size: u32) -> bool;
    pub fn clear_icons(&mut self);
    pub fn render(&mut self, scene: &AssistScene) -> Result<(), RenderError>;
}
pub(crate) fn paint_assist(canvas, fonts, icons, scene);   // CPU-testable
```
Look: the whole zone is a rounded rect (r 16 × scale) of the pill colour at ~88 % with a 2 px accent outline (the lit zone); header title in `font_label` mono 12 px `text`, step right-aligned in accent; cards r 10 in a `surface1`-like tint, title row with the icon (20 px) and the title in `font_title` 12 px, ellipsized; the thumbnail area a darker plate (the DWM thumbnail is composed **on top** of it by Windows, so never draw over the thumb rect except when `no_thumb`); hover: accent outline 2 px; keyboard selection: a second outline in `text` (differs by shape from hover); `elevated`: a small "admin" chip in `warn` at the title row's right; footer text dim, or `warn` when `warn`. `resize` recreates the swap chain at the new size (allowed here: it happens once per zone, not per frame). Tests on CPU raster: zone outline/fill, card hover vs selected, admin chip, no_thumb icon, text clipped to its rect, nothing drawn inside a thumb rect when a thumbnail will cover it, transparent outside the rounded zone.

## Part F — wiring (coordinator)

`Tiler`: `WindowOps::list_windows`, `start_assist(layout, zone)` on a Home-panel cell click (instead of placing the app in front; sets the active layout), `assist_view`, `assist_pick(i)` (restore minimized/maximized → `place_in`; admin → footer notice, the run stays on that zone), `assist_cancel`, `assist_key`, and at the end activate the picked windows in order. `App`: the picker window over the current zone (moved/resized per zone), its `AssistSurface`, one `Thumbnail` per visible card (re-made when the zone or the list changes), icons cached per window, input events, cancel on `FocusLost`/hide/snap; tests with the fake `WindowOps`; report; progress.
