# Stage T: Terminal — implementation plan

> Written by the coordinator (Claude Opus 5.5). Parts A–D run in parallel, one Claude Sonnet 5.5 worker each, in its own git worktree. Part F (wiring, docs, live test, report) is the coordinator's. Each worker reads its own part plus "Global Constraints", "Shared types" and "Review Focus".

**Goal:** a real shell inside the island. `Ctrl+Alt+`` drops a 760 × 420 terminal panel from the notch; the session keeps running when it is closed.

**Architecture:**
- `alacritty_terminal` 0.26 (Apache-2.0) does the emulation and the ConPTY, in a new crate `dj-term`.
- `dj-render` draws its grid with Skia inside the island's panel layer.
- `dj-win` lets the island window take the keyboard while the panel is open.
- `dj-core` holds the pure parts: state, layout, key mapping, paste, palette and config.

**Spec:** `task/progress.md` → "Stage T: Terminal".

## Global Constraints

- Rust stable (MSVC), `windows` 0.62, Apache-2.0. **The only new third-party dependency is `alacritty_terminal = "0.26"`** (in `dj-term`; its tree is MIT/Apache). No tokio, winit, webview or egui.
- `dj-core` stays `#![forbid(unsafe_code)]` with no Win32 or Skia. `unsafe` only in `dj-win`, `dj-render` and `dj-term`, each block with `// SAFETY:`.
- User-facing text is English. The name is "Daijobu Island".
- Idle CPU ≈ 0: no polling, no cursor blink. The PTY thread blocks; the UI wakes only on output, coalesced.
- Headless tests only: no exe, no windows shown, and no hooks. The real clipboard is never touched in tests (it is the user's). The one ConPTY test runs `cmd.exe /c echo …` hidden.
- Every Bash call: `source C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh && export CARGO_TARGET_DIR=<your worktree>/target CARGO_BUILD_JOBS=4 && cd <your worktree> && …` (this overrides the shared target).
- Workers do not commit. Gate: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test -p <your crate>`.
- Don't edit files outside your part's list. If a `match` elsewhere breaks, make the smallest compile fix and report it.

## Shared types (already committed — do not change them)

`crates/dj-core/src/terminal.rs` (skeleton) has `Style`, `Cell` (`ch`, `fg: Rgb`, `bg: Option<Rgb>` = default background, `style`), `CursorShape` (Block, HollowBlock, Underline, Beam), `Cursor`, `SelSpan { row, start, end }` (half-open), `Screen { cols, rows, cells (row-major), cursor: Option<Cursor>, selection, offset, history }` (+ `Screen::blank`, `Screen::cell`), `Palette { fg, bg, cursor, selection, ansi: [Rgb; 16] }`, `SELECTION_ALPHA = 0.35`, `Mods { ctrl, shift, alt }`, `Modes { app_cursor, app_keypad, bracketed_paste }`. Part A adds functions to this file; nobody else edits it.

`crates/dj-core/src/layout.rs` (skeleton) has `TERM_W = 760`, `TERM_H = 420`, `TERM_R = 22`, `TERM_HEADER_H = 36`, `TERM_CLOSE` (24 × 24 at the header's right, 14 px in) and `TERM_TEXT` (14, 40, 732, 368): where the cells go, panel-local logical px.

Colours: `dj_core::color::Rgb` (f32 0..1, `Rgb::new(r, g, b)`, `to_u8()`).

## Review Focus

These are the inputs most likely to hurt someone using it and that no single part's tests cover end to end. Each part pins its share:
1. **AltGr layouts** (German `@` = Ctrl+Alt+Q, Polish, French): a character typed with Ctrl+Alt must arrive as that character, never as an ESC-prefixed or control byte. (A: `on_char`; D: `ctrl`/`alt` read at WM_CHAR time.)
2. **Alt+F4 / F10 / Alt+Space while the panel has the keyboard** must not close the island or open its system menu. (D)
3. **Pasting multi-line or hostile text**: lines must not run one by one before the user presses Enter, and `ESC[201~` inside the clipboard must not end bracketed paste early. (A: `paste_bytes`)
4. **Wide and missing glyphs** (CJK, emoji, Nerd-font prompt icons): no overlap into the next cell, no box tofu where a system font has the glyph, and the cursor stays on the right cell. (B: wide/spacer; C: per-cell placement and fallback)
5. **The shell exits or never starts** (wrong `shell` path, `exit`): no crash or hang, a readable line in the panel, and Enter restarts. (B: spawn error and `Exited`; F: notice)

---

## Part A — `dj-core` (worktree `t-core`)

**Files:** `crates/dj-core/src/{terminal.rs, layout.rs, island.rs, features.rs, config.rs, theme.rs}`, plus the theme TOMLs only if a test needs one.

1. **Layout:** `window_size()` must hold the terminal: width `max(HOME_W, TERM_W) + 2 * NOTCH_SHOULDER`, height `TOP_MARGIN + max(HOME_H, TERM_H) + BOTTOM_SLACK`. Update the layout tests that list panel sizes.
2. **Island:**
   - `IslandState::Terminal` gets its own layer (index 12, `LAYERS = 13`) and `Layers.terminal: f32`; its size is `(TERM_W, TERM_H, TERM_R)`. It is not a rest state, and facts wait for the collapse as with the other panels.
   - `pub fn open_terminal(&mut self, tokens)` from any state; it takes over an open panel. `pub fn terminal_open(&self) -> bool`.
   - A click inside the panel returns `ClickOutcome::Terminal { x, y }` (panel-local, like `Tile`), and the island does nothing else; the app decides.
   - `collapse()` from Terminal goes to the rest state.
   - Tests: open from every rest state and from Home, size, layer, click routing, collapse, facts waiting, and the window size.
3. **Features:** the locked `("cli", "dj CLI", Icon::Cli)` becomes `available("terminal", "Terminal", Icon::Cli)`. Update the registry test.
4. **Config:**
   - `TerminalConfig` is lenient like `TileConfig` (a bad value falls back without costing the file):
     - `shell: String` (`""` auto), `args: Vec<String>`, `cwd: String` (`""` home)
     - `font: String` (`"Cascadia Mono"`), `font_size: f32` (12, clamped 8–24)
     - `hide_on_blur: bool` (true), `scrollback: u32` (5000, clamped 0–50 000)
   - It is `Config.terminal` after `tile`. Tests: defaults, clamps, bad types and a round trip.
5. **Theme:**
   - An optional top-level `terminal_ansi = ["#rrggbb", … 16]` in a theme file becomes `ResolvedTokens.terminal_ansi: Option<[Rgb; 16]>`. `lerp` picks a side at the midpoint.
   - A wrong count or a bad colour rejects the theme with a clear reason. The built-in themes don't set it.
   - Tests: absent, valid, wrong count, bad hex.
6. **`terminal.rs` logic** (pure; `vk` are Win32 virtual-key codes, constants defined here):
   - `pub fn palette(t: &ResolvedTokens) -> Palette`:
     - `fg = t.text`, `bg = t.pill`, `cursor = t.accent`, `selection = t.accent`.
     - `ansi = t.terminal_ansi` or a built-in set for `t.mode`: one tuned for dark pills, one for light pills. On light pills, white/bright white must stay visible: use greys that reach ≥ 3:1 against `bg`, and normal colours ≥ 3:1 against `bg` where possible.
     - Test the contrast for all four built-in themes.
   - `pub enum KeyAction { Send(Vec<u8>), SendSkipChar(Vec<u8>), Copy, Paste, PassToChar }` and `pub fn on_key(vk: u16, mods: Mods, modes: Modes, has_selection: bool) -> KeyAction`:
     - Arrows: `CSI A..D`, or `SS3` with `app_cursor` and no modifiers. With modifiers, `CSI 1;m A` where `m = 1 + shift + 2·alt + 4·ctrl`.
     - Home/End: `CSI H`/`CSI F` (SS3 in app_cursor), modified as `CSI 1;m H`.
     - Ins/Del/PgUp/PgDn: `CSI 2~/3~/5~/6~`, modified as `CSI n;m ~`.
     - F1–F4: `SS3 P..S` (modified: `CSI 1;m P..S`). F5–F12: `CSI 15~ 17~ 18~ 19~ 20~ 21~ 23~ 24~`.
     - Shift+Tab: `CSI Z` (skip char). Backspace: `0x7f`, Ctrl+Backspace `0x08`, Alt+Backspace `ESC 0x7f` (skip char). Ctrl+Space: `0x00` (skip char).
     - Copy: Ctrl+C with a selection, or Ctrl+Shift+C. Paste: Ctrl+V, Ctrl+Shift+V or Shift+Insert.
     - Everything else: `PassToChar`.
     - AltGr (ctrl and alt both down) is never a shortcut.
   - `pub fn on_char(ch: char, mods: Mods) -> Vec<u8>`: UTF-8 of `ch`; with Alt and without Ctrl, prefixed by ESC. Ctrl+Alt is AltGr, so plain. `'\n'` becomes `'\r'`.
   - `pub fn paste_bytes(text: &str, modes: Modes) -> Vec<u8>`:
     - CRLF and LF become CR. C0 controls other than `\t` and `\r` and DEL are dropped, so ESC is dropped and a pasted `ESC[201~` can't end the paste.
     - With `bracketed_paste`: wrapped in `ESC[200~` … `ESC[201~`.
     - Without it, a multi-line paste is still sent as one write; the shell decides. Document that.
   - `pub fn grid_fit(cell_w_px: u16, cell_h_px: u16, scale: f32) -> (u16, u16)`: cols × rows of whole cells in `TERM_TEXT` at `scale`, at least 20 × 4.
   - `pub fn cell_at(x: f32, y: f32, cell_w_px: u16, cell_h_px: u16, scale: f32, cols: u16, rows: u16) -> Option<(usize, usize, bool)>`: panel-local logical point → (col, row, right half). Points outside `TERM_TEXT` clamp to the nearest cell when a drag leaves it. Document the choice: `None` only when the point is in the header.
   - `pub fn wheel_lines(delta: i32) -> i32`: 3 lines per 120, with the sign kept; positive means into history.
   - Tests: every key row above, AltGr, the paste cases (including a pasted `ESC[201~`), grid_fit at 1, 1.25, 1.5 and 2, cell_at edges, wheel.

## Part B — crate `dj-term` (worktree `t-term`)

**Files:** create `crates/dj-term/` (`Cargo.toml` with `dj-core` and `alacritty_terminal = "0.26"`; `src/lib.rs` plus modules as you like). Add `"crates/dj-term"` to the root `Cargo.toml` members. `Cargo.lock` changes with it. Nothing in `app/`.

```rust
pub struct SpawnOptions { pub program: String, pub args: Vec<String>, pub cwd: Option<PathBuf>,
                          pub cols: u16, pub rows: u16, pub cell_w: u16, pub cell_h: u16, pub scrollback: usize }
/// "" → pwsh.exe if found on PATH, else powershell.exe (System32); "pwsh"/"powershell"/"cmd"/"wsl"
/// → that program; anything else is a path or program name used as given. `args` pass through
/// ("" gets `-NoLogo` for PowerShell when no args are given).
pub fn resolve_shell(shell: &str, args: &[String]) -> (String, Vec<String>);
pub enum TermEvent { Title(String), ResetTitle, Bell, Exited(Option<i32>) }
pub struct Session { /* Arc<FairMutex<Term<Listener>>>, Notifier, loop thread */ }
impl Session {
    /// Starts the shell in a ConPTY. `wake` is called (from the PTY thread) when there is
    /// something new, at most once until the next `take_events`.
    pub fn spawn(opts: &SpawnOptions, wake: Box<dyn Fn() + Send + Sync>) -> Result<Session, String>;
    pub fn write(&self, bytes: Vec<u8>);
    pub fn resize(&mut self, cols: u16, rows: u16, cell_w: u16, cell_h: u16);
    pub fn scroll(&mut self, lines: i32);          // + = back into history
    pub fn scroll_to_bottom(&mut self);
    pub fn select_start(&mut self, col: usize, row: usize, right_half: bool, word: bool);
    pub fn select_update(&mut self, col: usize, row: usize, right_half: bool);
    pub fn select_clear(&mut self);
    pub fn has_selection(&self) -> bool;
    pub fn selection_text(&self) -> Option<String>;
    pub fn modes(&self) -> Modes;
    /// The visible grid with colours resolved through `palette`. `focused: false` turns a
    /// block cursor into `HollowBlock`.
    pub fn screen(&self, palette: &Palette, focused: bool) -> Screen;
    /// Title / bell / exit since the last call; also re-arms `wake`.
    pub fn take_events(&mut self) -> Vec<TermEvent>;
    pub fn exited(&self) -> bool;
}
impl Drop for Session { /* Msg::Shutdown; join with a bound; the child is ended */ }
```
- **Colour resolution** goes in a pure function over a `Term` (fed bytes in tests with `vte::ansi::Processor`):
  - Named 0–15 → `ansi`; Foreground/Background → `fg` / default bg (`None`).
  - Indexed 16–231 → the xterm 6×6×6 cube (0, 95, 135, 175, 215, 255); 232–255 → grey ramp (8 + 10·n).
  - Truecolor as given; OSC 4 overrides (`term.colors()`) win.
  - Bold with ansi 0–7 → bright 8–15. Dim → fg mixed 66 % toward bg. Inverse swaps fg/bg, and a swapped default bg becomes `palette.bg`. Hidden → fg = bg.
  - Wide char → `wide` on its cell and `spacer` on the next. Zero-width combining characters are dropped.
- **Wake coalescing:** the listener sets an `AtomicBool` and calls `wake` only when it flips false → true. `take_events` clears it first, then drains.
- **OSC 52 clipboard writes are ignored** (`Osc52::Disabled`): a program can't set the user's clipboard.
- **Tests:**
  - Fed bytes: SGR 31/91/1;31, 38;5;n (cube and grey), 38;2;r;g;b, 7 inverse, 2 dim, 8 hidden, a wide char, the cursor position and hidden cursor (`ESC[?25l`), app-cursor and bracketed-paste modes, scrollback offset after scrolling, selection spans and `selection_text`, a title event.
  - `resolve_shell` cases.
  - One real ConPTY round trip: `cmd.exe /c echo dj-term-ok` shows in `screen()` and ends with `Exited`, with a timeout of 15 s.
  - Dropping a running session (`cmd.exe` with `/k`) returns within 3 s.

## Part C — `dj-render` (worktree `t-render`)

**Files:** create `crates/dj-render/src/terminal.rs`; `crates/dj-render/src/lib.rs` (mod and `pub use terminal::TerminalPanel`); `crates/dj-render/src/gpu.rs` only to add `Renderer::terminal_cell`; `fonts.rs` only if you need italic or per-character fallback (keep the existing API). **Not `paint.rs`**: the coordinator hooks the layer.

```rust
pub struct TerminalPanel<'a> {
    pub title: &'a str,            // header text, already chosen by the app
    pub screen: &'a Screen,
    pub palette: &'a Palette,
    pub font: &'a str,             // family; Geist Mono if missing
    pub font_size: f32,            // logical px
    pub notice: Option<&'a str>,   // e.g. "process exited · press Enter to restart"
    pub close_hover: bool,
}
/// The whole panel content inside `at` (the panel rect at its final size, logical px,
/// anchored like the Tile panel); clipped to `at`.
pub(crate) fn paint_terminal(canvas: &Canvas, fonts: &mut Fonts, t: &ResolvedTokens, at: LRect, scale: f32, p: &TerminalPanel);
/// Cell size in whole device px for `font` at `size` logical px × `scale` (advance of 'M'
/// rounded, line height = ascent + descent + leading rounded).
pub(crate) fn cell_size(fonts: &mut Fonts, font: &str, size: f32, scale: f32) -> (u16, u16);
impl Renderer { pub fn terminal_cell(&mut self, font: &str, size: f32, scale: f32) -> (u16, u16); }
```
- **Header:** the theme lamp (accent, with halo, as on the idle pill), `title` in `font_label` 11 cut with `..`, and ✕ drawn as two strokes in `TERM_CLOSE`, brighter on `close_hover`. A hairline divider sits under the header.
- **Cells** go in `TERM_TEXT` from its top-left, each glyph at its own cell's x. Runs may batch, but there is no drift.
  - Backgrounds: only `Some` cells, merged per run.
  - Bold uses a heavier face (or fake bold); italic uses the italic face or a skew.
  - Underline and strike are 1 device px lines.
  - Wide glyphs span two cells; spacers draw nothing.
  - A glyph the font lacks falls back to a system font that has it (`FontMgr::match_family_style_character`), never tofu when one exists.
  - Everything is clipped to `TERM_TEXT`.
- **Cursor:** Block = a cursor-colour box with the glyph redrawn in `palette.bg`; HollowBlock = a 1 px outline; Underline = 2 px; Beam = 2 px. It covers 2 cells on a wide char.
- **Selection:** `palette.selection` at `SELECTION_ALPHA` over the spans.
- **Scroll indicator:** while `screen.offset > 0`, a thin bar at the right edge of `TERM_TEXT` showing position and length.
- **Notice:** one line over the bottom of the text area on a `palette.bg` band.
- **Tests (CPU raster):**
  - Cells land at col·cw / row·ch; default background is unpainted and non-default painted.
  - Bold, underline and strike pixels; a wide char with its spacer.
  - Every cursor shape, including on a wide char; the selection fill; the header lamp and ✕ (and hover); the scroll indicator only when scrolled.
  - Nothing is drawn outside `at` or outside `TERM_TEXT` (except the header); scale 1, 1.5 and 2.
  - `cell_size` is whole pixels and the same for repeated calls; fallback for a CJK char when a system CJK font exists (skip if none).

## Part D — `dj-win` (worktree `t-win`)

**Files:** `crates/dj-win/src/window.rs`, a new `crates/dj-win/src/clipboard.rs`, `crates/dj-win/src/lib.rs`, `crates/dj-win/Cargo.toml` (windows features only).

- **Keyboard mode:**
  - `pub fn set_keyboard(hwnd: HWND, on: bool)`: on removes `WS_EX_NOACTIVATE` and makes `WM_MOUSEACTIVATE` answer `MA_ACTIVATE`; off restores both. Keep the state in a thread-local.
  - `pub fn take_keyboard(hwnd: HWND) -> isize`: remembers `GetForegroundWindow()` (returned raw), then `SetForegroundWindow` and `SetFocus`. Callers use it only right after the user's hotkey or click, which grants the foreground.
  - `pub fn give_back_keyboard(prev: isize)`: activates `prev` if it is still a window and visible.
- **New `Event`s** (plain fields; dj-win doesn't depend on dj-core):
  - `Key { vk: u16, ctrl: bool, shift: bool, alt: bool }` from `WM_KEYDOWN` / `WM_SYSKEYDOWN`.
  - `Char { ch: char, ctrl: bool, alt: bool }` from `WM_CHAR` / `WM_SYSCHAR`, with surrogate pairs joined and modifiers read with `GetKeyState` at that moment.
  - `Wheel { delta: i32, x: i32, y: i32 }` (client coordinates), `RightClick { x, y }`, `DoubleClick { x, y }`, `FocusLost`.
  - All of these are sent **only in keyboard mode**, except `DoubleClick`.
- **Message handling in keyboard mode:**
  - `WM_KEYDOWN`/`WM_SYSKEYDOWN` return 0 without `DefWindowProc`, so Alt+F4 / F10 / Alt+Space never reach the system menu. `TranslateMessage` still makes the chars.
  - `WM_KILLFOCUS`, or `WM_ACTIVATE` with `WA_INACTIVE`, → `FocusLost` (idempotent use is the app's).
- **Double click:** add `CS_DBLCLKS` to the class. `WM_LBUTTONDBLCLK` pushes `Click` (as a second press did before, with capture) **and** `DoubleClick`, so no existing feature changes.
- **`clipboard`:** `pub fn get_text(owner: isize) -> Option<String>` and `pub fn set_text(owner: isize, text: &str) -> bool` (CF_UNICODETEXT). `OpenClipboard` is retried up to 5 × 10 ms; the memory is freed on every path.
- `pub const VK_OEM_3: u32 = 0xC0;` (the backtick key on US layouts).
- **Tests:**
  - A pure `translate(msg, wp, lp, keyboard_on, mods) -> Option<Event>` covers every message above in both modes, the AltGr flags, the surrogate join, `WA_INACTIVE` vs active and wheel delta signs.
  - Clipboard functions are **not** run in tests (the user's clipboard); the coordinator checks them live.

## Part F — wiring (coordinator)

The coordinator owns:
- `app/src/terminal.rs`: a controller with an injected `TerminalBackend` trait so it can be tested without a PTY. It handles spawn on first open, restart on Enter after exit, title, notice, cell metrics per scale and font, resize on DPI or font change, selection with the mouse, wheel, copy/paste through `dj_win::clipboard`, and the key flow (`on_key` → `SendSkipChar` swallows the next `Char`).
- The hotkey `Ctrl+Alt+`` (id 19), the Home tile `Open("terminal")`, `ClickOutcome::Terminal` (✕ / text area), and the island's keyboard mode on open/close.
- `FocusLost` → hide when `hide_on_blur`; give the keyboard back on close; `Event::Wake` → `take_events` + redraw.
- `Frame.terminal` plus the layer in `paint.rs`.
- `tile_geo` treats Terminal as not resting. Tile hotkeys must not move the island itself.
- Snap collapses the panel. `quit` drops the session.
- Docs (README, AGENTS.md layout line, NOTICE), a Sonnet review of the wiring, the live test of acceptance 1–7, the report and the PR.
