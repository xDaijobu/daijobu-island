# Stage 7c plan: parallel split and interface contract

Coordinator: Claude Opus 5.5. Workers: Claude Sonnet 5.5, one git worktree each, branched from
`xDaijobu/stage-7c-snap-stack` at the commit that adds this file. The contract and acceptance
are in `task/progress.md` (Stage 7c) and `task/stage-7c-brief.md`; this file only fixes the
seams between the parallel parts so they merge without rework.

| Part | Branch / worktree | Owns (only these files) |
|---|---|---|
| A core | `xDaijobu/7c-core` → `workspaces/daijobu-island/7c-core` | `crates/dj-core/src/snap_stack.rs` (new), `crates/dj-core/src/lib.rs` (one `pub mod` line + doc), `crates/dj-core/src/config.rs` (`preview_seconds`) |
| B native | `xDaijobu/7c-win` → `workspaces/daijobu-island/7c-win` | `crates/dj-win/src/previews.rs` (new), `crates/dj-win/src/lib.rs`, `crates/dj-win/src/window.rs` (one `Event` variant only), `crates/dj-win/src/tray.rs`, `crates/dj-win/Cargo.toml` (`Win32_Graphics_Dwm`) |
| C render | `xDaijobu/7c-render` → `workspaces/daijobu-island/7c-render` | `crates/dj-render/src/previews.rs` (new), `crates/dj-render/src/lib.rs` (mod + exports) |
| D store | `xDaijobu/7c-store` → `workspaces/daijobu-island/7c-store` | `app/src/preview_store.rs` (new), `app/src/snap.rs`, `app/src/main.rs` (one `mod` line) |
| F wiring | `xDaijobu/stage-7c-snap-stack` (after A–D merge) | `app/src/previews.rs` (new controller), `app/src/app.rs`, report |

Parts A–D must not depend on each other's new code: each builds against the baseline of the
other crates. Types cross the seams only in part F, which maps between them.

Units everywhere: **logical px** (`f32`, 96-DPI units) inside the stack window unless named
`*_px` / "physical". Window client origin (0, 0) = top-left of the stack window.

---

## A. `dj_core::snap_stack` (pure; `#![forbid(unsafe_code)]` crate; time injected)

```rust
pub const MAX_ITEMS: usize = 5;
pub const MAX_BYTES: u64 = 256 * 1024 * 1024;
pub const CARD_W: f32 = 124.0;
pub const CARD_H: f32 = 80.0;
pub const CARD_R: f32 = 12.0;
pub const GAP: f32 = 10.0;
pub const RING: f32 = 16.0;
/// Gap between the island's lower edge and the stack window's content top.
pub const BELOW_ISLAND: f32 = 12.0;
/// Transparent margin around the content inside the window (shadow + lift room).
pub const PAD: f32 = 16.0;
/// "Preview stack full" stays this long.
pub const NOTICE: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u64);              // monotonic from 1, never reused

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardStatus { Ready, Copied, CopyFailed, Saving, Saved, SaveFailed, Missing }
impl CardStatus { pub fn label(self) -> Option<&'static str>; pub fn is_error(self) -> bool; }
// labels: Ready → None, Copied "copied", CopyFailed "copy failed", Saving "saving…",
// Saved "saved", SaveFailed "save failed", Missing "file missing"

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardAction { Copy, Save /* reads "open" once saved */, Pin /* "unpin" when pinned */, Dismiss }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarAction { SelectAll, Clear, SaveSelected, DismissSelected }

pub struct Item {                        // read via `Stack::items()`; fields pub, countdown private
    pub id: ItemId,
    pub width: u32, pub height: u32,      // image px
    pub bytes: u64,                       // retained BGRA + PNG bytes for this payload
    pub pinned: bool, pub selected: bool, pub hovered: bool, pub focused: bool, pub busy: bool,
    pub status: CardStatus,
    pub saved: Option<std::path::PathBuf>,
    /* countdown: private */
}

pub enum Admit { Added { id: ItemId, evicted: Vec<ItemId> }, Full }

impl Stack {
    pub fn new(lifetime: Duration) -> Self;                      // MAX_ITEMS / MAX_BYTES
    pub fn with_limits(lifetime: Duration, max_items: usize, max_bytes: u64) -> Self;
    pub fn admit(&mut self, width: u32, height: u32, bytes: u64, now: Instant) -> Admit;
    pub fn remove(&mut self, id: ItemId, now: Instant) -> bool;  // dismiss (pinned too)
    pub fn remove_selected(&mut self, now: Instant) -> Vec<ItemId>;
    pub fn tick(&mut self, now: Instant) -> Vec<ItemId>;         // removes + returns expired
    pub fn items(&self) -> &[Item];                              // newest first
    pub fn get(&self, id: ItemId) -> Option<&Item>;
    pub fn len(&self) -> usize; pub fn is_empty(&self) -> bool; pub fn bytes(&self) -> u64;
    pub fn set_hidden(&mut self, hidden: bool, now: Instant);   // stack hidden or capture up
    pub fn set_hover(&mut self, id: Option<ItemId>, now: Instant);
    pub fn toggle_select(&mut self, id: ItemId, now: Instant);
    pub fn select_all(&mut self, now: Instant);
    pub fn clear_selection(&mut self, now: Instant);
    pub fn selected(&self) -> Vec<ItemId>;                       // newest first
    pub fn toggle_pin(&mut self, id: ItemId, now: Instant);
    pub fn set_busy(&mut self, id: ItemId, busy: bool, now: Instant) -> bool;   // false = stale id
    pub fn set_status(&mut self, id: ItemId, status: CardStatus) -> bool;      // false = stale id
    pub fn set_saved(&mut self, id: ItemId, path: Option<PathBuf>) -> bool;   // false = stale id
    pub fn fraction(&self, id: ItemId, now: Instant) -> Option<f32>;  // remaining / lifetime, 0..=1
    pub fn remaining(&self, id: ItemId, now: Instant) -> Option<Duration>;
    pub fn is_running(&self, id: ItemId) -> bool;
    pub fn any_running(&self) -> bool;
    pub fn next_change(&self, now: Instant) -> Option<Duration>; // soonest expiry or notice end
    pub fn notice(&self, now: Instant) -> Option<&'static str>;  // "Preview stack full" for NOTICE
    // keyboard focus (see below)
    pub fn focus(&self) -> Option<Focus>;
    pub fn set_focus(&mut self, f: Option<Focus>, now: Instant);
    pub fn key(&mut self, key: StackKey, now: Instant) -> KeyCmd;
}
```

Rules (one predicate, re-evaluated after every mutation, so nothing drifts):
`running(item) = !hidden && no item selected && !item.hovered && !item.focused && !item.pinned && !item.busy`.
Remaining time only decreases while running; pausing stores the remainder; resuming continues
it (never restarts). Expired = running and remaining == 0 → removed by `tick`.

Admission: `bytes > max_bytes` → `Full`. Otherwise plan evictions of the **oldest unprotected**
items (protected = pinned | selected | hovered | focused | busy) until `len < max_items` and
`total + bytes <= max_bytes`. If the plan cannot reach that, return `Full` and change nothing
(except starting the notice). Never evict protected items. `Full` starts `notice()` for
`NOTICE`. A new item starts with the full lifetime and `CardStatus::Ready`.

Keyboard model (deliberate access via the tray; no global shortcuts):

```rust
pub enum Focus { Card(ItemId), Bar(BarAction) }
pub enum StackKey { Left, Right, Tab, ShiftTab, Space, Enter, Escape, Delete, C, S, P, CtrlA }
pub enum KeyCmd { None, Redraw, Card(ItemId, CardAction), ToggleSelect(ItemId), Bar(BarAction), Release }
```
Left/Right move between cards (newest is leftmost). Tab/ShiftTab cycle cards then visible bar
buttons. Space/Enter on a card → `ToggleSelect`; Enter on a bar button → `Bar(..)`. C/S/P/Delete
→ `Card(focused, Copy/Save/Pin/Dismiss)`. CtrlA → `Bar(SelectAll)`. Escape → `Release`.
Focus moves to a neighbour when its card is removed; `None` when empty.

Layout + hit testing (logical px, window client space):

```rust
pub struct CardLayout {
    pub id: ItemId,
    pub rect: Rect,                              // dj_core::layout::Rect
    pub check: Rect,                             // 18×18 at (6, 6) inside the card
    pub ring: Rect,                              // 16×16 at top-right, 5 px inset
    pub status: Rect,                            // status chip between check and ring
    pub actions: [(CardAction, Rect); 4],        // bottom row, 5 px inset, 16 px high
}
pub struct BarLayout { pub rect: Rect, pub count: Rect, pub buttons: Vec<(BarAction, Rect)> }
pub struct StackLayout { pub cards: Vec<CardLayout>, pub bar: Option<BarLayout>, pub notice: Option<Rect> }
pub enum Hit { Card(ItemId), Action(ItemId, CardAction), Bar(BarAction) }

pub fn window_size() -> (f32, f32);              // constant: fits MAX_ITEMS cards + bar row + notice + PAD
pub fn layout(stack: &Stack, now: Instant) -> StackLayout;
pub fn hit(layout: &StackLayout, x: f32, y: f32) -> Option<Hit>;      // actions win over card body
pub fn input_rects(layout: &StackLayout) -> Vec<Rect>;                // cards + bar only (click-through elsewhere)
```
Cards in one row, newest first (left), centred horizontally in the window, top at `PAD`; no
fan in this stage (the contract makes it optional). Bar row (only when ≥ 1 selected) under the
cards with `GAP`, centred: count text, then `Select all`, `Clear`, `Save selected`,
`Dismiss selected` (button widths fixed by A, ≥ 24 px high). The notice row sits where the bar
would be (bar wins if both; then the notice goes under the bar). Action widths in the 114 px
row: Copy 27, Save/Open 27, Pin/Unpin 33, Dismiss 18, gaps 3.

Motion (pure springs from `dj_core::spring`, stepped by the app's frame loop):

```rust
pub struct MotionCard { pub id: ItemId, pub rect: Rect, pub alpha: f32, pub scale: f32, pub exiting: bool }
impl Motion {
    pub fn new() -> Self;
    pub fn sync(&mut self, layout: &StackLayout);   // new ids pop in (from y-30, scale .6, alpha 0);
                                                    // moved ids slide; removed ids fade/lift out as ghosts
    pub fn step(&mut self, dt: f32, spec: SpringSpec);
    pub fn animating(&self) -> bool;
    pub fn cards(&self) -> Vec<MotionCard>;          // draw order back→front, ghosts included
    pub fn take_finished(&mut self) -> Vec<ItemId>; // ghosts that finished exiting (free their images)
}
```

Config (`[snap]`): `preview_seconds` integer, default 10; `SnapConfig::preview_secs() -> u64`
returns it when `3..=60`, else 10. Absent → default; keeps the Stage S writer round trip.

## B. `dj_win::previews` (native companion window)

```rust
pub enum PreviewInput {                 // client px (physical)
    Move { x: i32, y: i32 }, Press { x: i32, y: i32 }, Release { x: i32, y: i32 },
    Leave,                                // WM_MOUSELEAVE, or capture lost / WM_CANCELMODE
    Key { vk: u16, ctrl: bool, shift: bool },
    FocusLost,                            // WM_KILLFOCUS / WM_ACTIVATE inactive
}
// added to `window::Event`: `Previews(crate::previews::PreviewInput)`

pub fn create(x: i32, y: i32, w: i32, h: i32) -> windows::core::Result<HWND>;
    // hidden; WS_POPUP; WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
    // class "DaijobuSnapPreviews", title "Daijobu Island Snap previews"
pub fn show(hwnd: HWND);                 // SW_SHOWNOACTIVATE + HWND_TOPMOST, never activates
pub fn hide(hwnd: HWND);
pub fn place(hwnd: HWND, x: i32, y: i32, w: i32, h: i32);   // SWP_NOACTIVATE
pub fn focus(hwnd: HWND);                // deliberate (tray command only): SetForegroundWindow + SetFocus
pub fn destroy(hwnd: HWND);
pub fn set_regions(hwnd: HWND, rects: &[(i32, i32, i32, i32)], radius: i32);
    // union of rounded rects (client px, l/t/r/b); EMPTY slice = empty region (no input at all)
pub fn flush_composition();              // DwmFlush(): returns after the next composition pass
pub fn work_area_at(x: i32, y: i32) -> (i32, i32, i32, i32);   // rcWork (l, t, r, b) of the nearest monitor
pub fn dpi_at(x: i32, y: i32) -> u32;   // effective DPI of the nearest monitor, ≥ 96
```
`WM_MOUSEACTIVATE` → `MA_NOACTIVATE`. Press → `SetCapture`; release → `ReleaseCapture`.
`WM_KEYDOWN` → `Key` (ctrl false while Alt is down, as in `overlay.rs`). `WM_CLOSE` is ignored
(the app owns the lifetime). `WM_DPICHANGED` returns 0 (the app relayouts from the island's
event). Tray: `MenuCommand::SnapPreviews`, `MenuSpec.previews: bool` (item
"Focus Snap previews", greyed when false), placed above "Quit" with the other app items.

## C. `dj_render::previews` (Skia on its own D3D12 + DComp chain)

```rust
pub struct PreviewSurface { /* own Gpu + Chain, like SnapOverlay; drop order matters */ }
impl PreviewSurface {
    pub fn new(hwnd: isize, width: u32, height: u32) -> Result<Self, RenderError>;  // device px, sized once
    pub fn size(&self) -> (u32, u32);
    pub fn set_image(&mut self, id: u64, bitmap: &dj_core::snap::Bitmap, scale: f32) -> bool;
        // downscale ONCE to the card's aspect-fit box at `scale`, upload, cache by id; false if Skia refused
    pub fn has_image(&self, id: u64) -> bool;
    pub fn drop_image(&mut self, id: u64);
    pub fn clear_images(&mut self);
    pub fn render(&mut self, scene: &PreviewScene) -> Result<(), RenderError>;   // clear transparent, draw, Present(1)
}
pub struct PreviewScene<'a> {
    pub tokens: &'a dj_core::theme::ResolvedTokens,
    pub scale: f32,                               // device px per logical px
    pub cards: &'a [PreviewCard<'a>],             // back → front
    pub bar: Option<PreviewBar<'a>>,
    pub notice: Option<(Rect, &'a str)>,
}
pub struct PreviewCard<'a> {
    pub id: u64,
    pub rect: Rect,                               // logical, window space, animated
    pub alpha: f32, pub scale: f32,               // pop-in/out, scale about the card centre
    pub remaining: Option<f32>,                   // ring fraction 0..=1; None = no ring (pinned)
    pub selected: bool, pub pinned: bool, pub hovered: bool, pub focused: bool, pub busy: bool,
    pub show_check: bool, pub show_actions: bool,
    pub status: Option<(&'a str, bool)>,          // (label, is_error)
    pub check: Rect, pub ring: Rect, pub status_rect: Rect,       // RELATIVE to the card's top-left
    pub actions: [PreviewButton<'a>; 4],          // rects RELATIVE to the card's top-left
}
pub struct PreviewButton<'a> { pub rect: Rect, pub label: &'a str, pub focused: bool, pub primary: bool }
pub struct PreviewBar<'a> { pub rect: Rect, pub count: (Rect, &'a str), pub buttons: &'a [PreviewButton<'a>] } // absolute
```
Selected/pinned/focused/error must differ by **shape or text**, not colour alone (check glyph,
"PIN" badge instead of the ring, focus outline drawn as a second ring, error text). Transparent
pixels outside cards/bar/notice stay alpha 0. Cards use theme tokens (pill/text/accent/
highlight), mono font from the existing `Fonts`. The paint code runs on any canvas so tests
draw a synthetic scene on a CPU raster surface.

## D. app payload + IO worker (`app/src/preview_store.rs`, `app/src/snap.rs`)

- `Shot` keeps its image as `Arc<Bitmap>` and PNG as `Option<Arc<[u8]>>`; behaviour of
  copy/save/thumb unchanged. `Snapper::payload(&self) -> Option<Payload>` clones the Arcs of the
  current shot (the one `end` just made).
- `pub struct Payload { pub bitmap: Arc<Bitmap>, pub png: Option<Arc<[u8]>> }` with
  `fn bytes(&self) -> u64` (= pixels + PNG; shared with the thumb, so nothing is copied).
- `pub struct Store` — payloads by `u64` id + one IO thread (lazy, bounded queue):
  ```rust
  pub fn new(wake: Box<dyn Fn() + Send>) -> Store;          // wake = dj_win::window::wake(hwnd)
  pub fn insert(&mut self, id: u64, p: Payload);
  pub fn remove(&mut self, id: u64);                           // releases the Arcs; no disk/clipboard effect
  pub fn get(&self, id: u64) -> Option<&Payload>;
  pub fn copy(&mut self, id: u64, clip: &mut dyn Clipboard) -> CopyStart;  // Done(Ok/Err) when the PNG exists; Pending if it must be encoded first (worker)
  pub fn save(&mut self, id: u64, dir: PathBuf, stamp: Stamp) -> bool;    // queued; false if no payload or queue full
  pub fn drain(&mut self) -> Vec<Reply>;                      // Reply { id, kind: Saved(PathBuf) | SaveFailed(String) | Encoded | EncodeFailed(String) }
  pub fn finish_copy(&mut self, id: u64, clip: &mut dyn Clipboard) -> Result<(), String>; // after Encoded
  pub fn reveal(path: &Path) -> Result<(), RevealError>;      // Missing if the file is gone; else dj_sweep reveal
  ```
  Saves reuse `write_new` (no-overwrite naming); PNG encode on the worker. Replies carry the id
  only; the caller ignores ids that no longer exist. Tests: temp dirs, collisions, missing
  file, partial batch failures, stale ids, shared-byte accounting, fake clipboard.

## F. wiring (after merge)

`app/src/previews.rs` owns `Stack`, `Motion`, `Store`, the window and the `PreviewSurface`, and
maps core layout → render scene. `app.rs`: add a card in `end_snap` when `taken` (copy/save
results as initial status); hide + `flush_composition` + `set_hidden(true)` at the start of
`begin_snap` (covers the direct and the delayed path), restore in `end_snap`; follow
`set_visible`; place under the island's current lower edge each frame it moves; timer
`TIMER_SNAP_STACK = 9` at 100 ms only while a visible countdown runs (else one-shot to
`next_change`); `Event::Wake` also drains `Store`; tray `SnapPreviews` → `previews::focus`;
DPI/display change → recreate surface; quit/empty → free window, surface, images, timer.
