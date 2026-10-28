# Stage T2: Terminal tabs — implementation plan

> Written by the coordinator (Claude Opus 5.5). Parts A and B run in parallel, one Claude Sonnet 5.5 worker each, in their own git worktree. Part F (multi-session controller, middle click, the pinned-tabs file, wiring, live test) is the coordinator's. Read your part plus "Global Constraints", "Shared types" and "Review Focus". The terminal itself (Stage T) is already built: see `task/stage-t-plan.md` and `task/stage-t-report.md` for how it works.

**Goal:** tabs in the terminal panel's header: several shells, pin/unpin (pinned tabs on the left, kept by "Close others", restored after an island restart), close / close others / close to the right from an island-styled menu, and drag-and-drop to reorder.

**Spec:** `task/progress.md` → "Stage T: Terminal" → "Tabs (T2)".

## Global Constraints

- The Stage T rules apply: Rust stable, no new dependencies, `dj-core` is `#![forbid(unsafe_code)]` with no Win32 or Skia, user-facing text is English, idle CPU ≈ 0, headless tests only.
- Every Bash call: `source C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh && export CARGO_TARGET_DIR=<your worktree>/target CARGO_BUILD_JOBS=4 && cd <your worktree> && …`
- Workers don't commit. Gate: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test -p <your crate>`.
- Only edit your part's files. If a struct literal elsewhere breaks, make the smallest compile fix and report it.

## Shared types (already committed in `crates/dj-core/src/term_tabs.rs` — do not change them)

- `MAX_TABS = 12`; the header strip `TABS` (30, 6, 684, 24); `PINNED_W = 76`, `TAB_MAX_W = 168`, `TAB_MIN_W = 64`, `TAB_GAP = 2`, `PLUS_W = 24`, `DRAG_SLOP = 4`, `MENU_W = 168`, `MENU_ROW_H = 26`. All are panel-local logical px (the panel is `layout::TERM_W` × `TERM_H`, its header `TERM_HEADER_H` = 36, and the panel close button `TERM_CLOSE` sits right of `TABS`).
- `TabId = u64`, `BarHit { Tab(i), Close(i), Plus }` (display index), `TabSlot { id, rect, title, pinned, active, activity, exited, close: Option<Rect>, dragging }`, `MenuAction { Pin, Unpin, Close, CloseOthers, CloseRight }`, `MenuItemView { action, label, rect, enabled }`, `MenuView { rect, items, hover }`, `TabBarView { tabs, plus, hover, menu }` (Default), `TabKey { New, Close, Next, Prev, Jump(usize) }`.

## Review Focus

1. **Drag across the pinned/regular boundary:** a regular tab must never land among pinned ones or the reverse, whatever the pointer does. (A)
2. **Closing the active tab, or many at once** (close others, close right, with pinned in between): the active tab after the close must be a tab that still exists, and its neighbour the user expects (the right one, else the left). (A)
3. **A press that turns into a drag must not also act as a click.** It neither closes a tab (on its ✕) nor opens the menu, and a drag released outside the bar still ends cleanly. (A)
4. **Many tabs and long titles:** 12 tabs fit the strip without overlapping "+" or the panel ✕, and titles are cut, never spilling into the next tab. (A layout, B drawing)
5. **The menu near the panel's right edge** stays inside the panel, and a click outside it closes it without acting on what is under it. (A)

---

## Part A — the tab model (worktree `t2-core`)

**Files:** `crates/dj-core/src/term_tabs.rs` only (add below the shared types).

```rust
pub struct Tabs { /* tabs in display order (pinned first), active, next id, hover, drag, menu */ }
impl Tabs {
    pub fn new() -> Self;
    pub fn len(&self) -> usize;  pub fn is_empty(&self) -> bool;
    pub fn ids(&self) -> Vec<TabId>;                 // display order
    pub fn active(&self) -> Option<TabId>;
    pub fn is_pinned(&self, id: TabId) -> bool;
    /// A new tab, made active: a regular one at the end, a pinned one at the end of the
    /// pinned group. None at MAX_TABS.
    pub fn add(&mut self, title: String, pinned: bool) -> Option<TabId>;
    /// Removes it; if it was active, the tab to its right becomes active, else the one to
    /// its left. False if unknown.
    pub fn close(&mut self, id: TabId) -> bool;
    /// Every regular tab except `id` (pinned ones stay); returns the closed ids. `id`
    /// becomes active.
    pub fn close_others(&mut self, id: TabId) -> Vec<TabId>;
    /// Every regular tab to the right of `id`; returns the closed ids. The active tab stays
    /// if it survives, else `id` becomes active.
    pub fn close_right(&mut self, id: TabId) -> Vec<TabId>;
    /// Pin: moves to the end of the pinned group. Unpin: to the start of the regular group.
    pub fn set_pinned(&mut self, id: TabId, pinned: bool);
    pub fn activate(&mut self, id: TabId);           // clears its activity
    pub fn next(&mut self);  pub fn prev(&mut self); // wrap around
    pub fn jump(&mut self, index: usize);            // ignored when out of range
    pub fn set_title(&mut self, id: TabId, title: String);
    pub fn set_activity(&mut self, id: TabId);       // ignored for the active tab
    pub fn set_exited(&mut self, id: TabId, exited: bool);

    /// Tab rects in display order (pinned PINNED_W, regular ones shrunk to fit between
    /// TAB_MIN_W and TAB_MAX_W, TAB_GAP apart, inside TABS), and "+" right after the last
    /// tab (None at MAX_TABS).
    pub fn layout(&self) -> (Vec<Rect>, Option<Rect>);
    pub fn hit(&self, x: f32, y: f32) -> Option<BarHit>;
    /// Pointer moved over the panel (panel-local). True if what is hovered changed (a tab,
    /// a ✕, "+", or a menu row).
    pub fn hover(&mut self, x: f32, y: f32) -> bool;
    pub fn clear_hover(&mut self) -> bool;

    /// Left press on the header. A tab is activated at once and may become a drag; its ✕
    /// waits for the release (so a drag never closes). With the menu open, a press anywhere
    /// closes it, and acts only on a menu row.
    pub fn press(&mut self, x: f32, y: f32) -> Press;
    /// Pointer moved with the button down: past DRAG_SLOP the pressed tab follows the
    /// pointer, and the order changes when its centre crosses a neighbour's centre, never
    /// across the pinned / regular boundary. True if the view changed.
    pub fn drag(&mut self, x: f32, y: f32) -> bool;
    /// Button up: ends a drag, or completes a ✕ press released on the same ✕.
    pub fn release(&mut self, x: f32, y: f32) -> Release;
    /// Middle click: the regular tab under the pointer to close (pinned tabs need the menu).
    pub fn middle_click(&self, x: f32, y: f32) -> Option<TabId>;
    /// Right click on a tab opens its menu below it (kept inside the panel). True if opened.
    pub fn open_menu(&mut self, x: f32, y: f32) -> bool;
    pub fn close_menu(&mut self) -> bool;
    pub fn menu_open(&self) -> bool;
    pub fn view(&self) -> TabBarView;
}
pub enum Press { None, Activated(TabId), Plus, Menu(MenuAction, TabId), MenuDismissed }
pub enum Release { None, Close(TabId), Reordered }
/// Ctrl+Shift+T / W, Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+Shift+1..9 (vk '1'..'9'); None for
/// anything else (Alt held: None, so AltGr keys are never shortcuts).
pub fn shortcut(vk: u16, ctrl: bool, shift: bool, alt: bool) -> Option<TabKey>;

/// A pinned tab as saved in `~/.daijobu/terminal-tabs.toml` (`[[pinned]]` tables).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedTab { pub title: String, pub shell: String, pub args: Vec<String>, pub cwd: String }
/// Lenient: a bad entry is skipped with a message, the rest kept; at most MAX_TABS.
pub fn parse_pinned(src: &str) -> (Vec<PinnedTab>, Vec<String>);
pub fn pinned_to_toml(tabs: &[PinnedTab]) -> String;
```
- **Menu items:** *Pin tab* or *Unpin tab*, *Close*, *Close others* (enabled when another regular tab exists), *Close to the right* (enabled when a regular tab sits right of it). The menu is `MENU_W` wide, one `MENU_ROW_H` per item, just below the tab, and shifted left to stay inside `TERM_W`. A disabled row does nothing when pressed (the menu still closes).
- **✕ on a regular tab:** shown on the active tab and the hovered one. A pinned tab has no ✕.
- **Tests:**
  - Every operation above: active-tab rules, pin/unpin moves, close others/right with pinned tabs mixed in, and `MAX_TABS`.
  - Layout: widths at 1, 4 and 12 tabs, nothing past `TABS`, "+" placement and its absence at the limit.
  - Hit and hover, and drag reorder: crossing centres, slop, the group boundary, releasing outside.
  - A press on ✕ then a drag cancels the close.
  - The menu: items, enabled rules, placement at the right edge, outside press, disabled row.
  - Shortcuts, including AltGr; `parse_pinned` with good, bad and over-limit input, and a round trip.
  - Pin each Review Focus item.

## Part B — drawing the tabs (worktree `t2-render`)

**Files:** `crates/dj-render/src/terminal.rs` only (and `lib.rs` if a re-export is needed).

- `TerminalPanel` gets `pub tabs: &'a TabBarView`. When `tabs.tabs` is not empty, the header draws the tab bar in place of the title. When it is empty, it draws the title as today.
  - The app's `TerminalPanel { … }` literal in `app/src/app.rs` will break. Fix it with the smallest change (`tabs: &TabBarView::default()` through a local) and report it; the coordinator wires the real one.
- **Tab:** a rounded rect (r 6) with the title in `font_label` 11, cut with `..` inside its rect minus padding (and minus the ✕ when shown).
  - Active: `t.surface2`-ish fill (choose what reads on all four themes) and `t.text`.
  - Inactive: no fill and `t.text_dim`; hovered: a faint fill.
  - Pinned: a small drawn pin mark (a shape, not a glyph) before a shorter title.
  - `activity`: a small accent dot at the right. `exited`: the title in `t.text_dim` with a hollow dot.
  - `close`: a small ✕ of two strokes, brighter when `hover == Close(i)`.
  - `dragging`: drawn last, with a faint shadow or outline so it reads as lifted.
- **"+":** two strokes in `PLUS_W`, brighter on hover.
- **Menu:** drawn after everything else in the panel, inside `at`:
  - A rounded rect (r 8) in the pill colour with a hairline border and a soft shadow.
  - Rows with labels in `font_label` 11, the hovered enabled row with a faint fill, disabled rows in `t.text_dim` at reduced opacity.
- Every glyph used is in the embedded fonts (use shapes for pin, ✕ and +). Nothing is drawn outside `at`, and tabs never outside `TABS`.
- **Tests (CPU raster, scales 1 / 1.5 / 2):**
  - Active vs inactive fill; a cut title stays inside its tab; the pin mark; the activity dot; ✕ only where `close` is set; the lifted dragged tab.
  - "+" present and absent; the menu rect, a hovered row and a disabled row.
  - The title-only header when `tabs` is empty; nothing outside `at`.
  - Build the `TabBarView` by hand (Part A is separate).

## Part F — coordinator

- **`dj-win`:** `Event::MiddleClick { x, y }` in keyboard mode.
- **`dj-term`:** `Session::drain()` returns whether there was output since the last call, together with the events, for the activity dot.
- **`app/src/terminal.rs`:**
  - A session per `TabId`, each keeping its own program, title, exit state and notice.
  - Tab shortcuts are checked before `on_key`, and the chars they make are skipped.
  - "+" and Ctrl+Shift+T open a new tab; closing the last tab closes the panel.
  - Menu actions, middle click, drag and release are routed to `Tabs`.
- **Pinned tabs:**
  - Loaded at start from `~/.daijobu/terminal-tabs.toml`. Their shells start on first open, not at island start.
  - Saved on pin/unpin, on closing a pinned tab, and on a pinned reorder. The folder saved is the one the tab started in.
- **Rest:** the paint hook, review, live test, report and progress.
