//! The terminal panel: a real shell inside the island, like VS Code's integrated terminal.
//!
//! This module is pure. It holds the data that crosses crates: `dj-term` (the emulator and
//! the ConPTY) fills a [`Screen`] using a [`Palette`]; `dj-render` draws it; the app maps
//! keys and pastes to bytes with the helpers here. Nothing here touches Win32 or Skia.
//!
//! The helpers: [`palette`] (the theme's colours), [`on_key`], [`on_char`] and
//! [`paste_bytes`] (what to send the shell), and [`grid_fit`], [`cell_at`] and
//! [`wheel_lines`] (the grid and the mouse).

use crate::color::Rgb;
use crate::layout::{TERM_HEADER_H, TERM_TEXT};
use crate::theme::{Mode, ResolvedTokens};

/// How a cell's glyph is drawn. Colours are already resolved into [`Cell::fg`] /
/// [`Cell::bg`] (inverse, dim and bold-as-bright are applied by `dj-term`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// The first half of a double-width character: the glyph spans this cell and the next.
    pub wide: bool,
    /// The second half of a double-width character: draw nothing (the background only).
    pub spacer: bool,
}

/// One cell of the visible grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    /// `' '` for an empty cell.
    pub ch: char,
    pub fg: Rgb,
    /// `None`: the default background (the panel shows through, nothing is painted).
    pub bg: Option<Rgb>,
    pub style: Style,
}

impl Cell {
    /// An empty cell in the default colours.
    pub fn blank(palette: &Palette) -> Self {
        Cell {
            ch: ' ',
            fg: palette.fg,
            bg: None,
            style: Style::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    #[default]
    Block,
    /// A block outline: the shape while the panel doesn't have the keyboard.
    HollowBlock,
    Underline,
    Beam,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub col: usize,
    pub row: usize,
    pub shape: CursorShape,
}

/// Selected columns `start..end` (half-open) of one visible row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelSpan {
    pub row: usize,
    pub start: usize,
    pub end: usize,
}

/// What the panel shows: the visible rows of the grid (scrolled back by `offset`).
#[derive(Clone, Debug, PartialEq)]
pub struct Screen {
    pub cols: usize,
    pub rows: usize,
    /// `rows * cols` cells, row-major, the top visible row first.
    pub cells: Vec<Cell>,
    /// `None` while the cursor is hidden or scrolled out of view.
    pub cursor: Option<Cursor>,
    pub selection: Vec<SelSpan>,
    /// Lines scrolled back from the bottom (0: following the output) and how many lines
    /// of history there are, for the scroll indicator.
    pub offset: usize,
    pub history: usize,
}

impl Screen {
    /// A blank screen of `cols` × `rows` (before the shell has drawn anything).
    pub fn blank(cols: usize, rows: usize, palette: &Palette) -> Self {
        Screen {
            cols,
            rows,
            cells: vec![Cell::blank(palette); cols * rows],
            cursor: Some(Cursor {
                col: 0,
                row: 0,
                shape: CursorShape::Block,
            }),
            selection: Vec::new(),
            offset: 0,
            history: 0,
        }
    }

    pub fn cell(&self, col: usize, row: usize) -> Option<&Cell> {
        (col < self.cols && row < self.rows).then(|| &self.cells[row * self.cols + col])
    }
}

/// The terminal's colours for the current island theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Default text and background (the panel: the theme's pill colour).
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Rgb,
    /// The selection's fill (drawn at `SELECTION_ALPHA` over the cells).
    pub selection: Rgb,
    /// ANSI 0–15: black, red, green, yellow, blue, magenta, cyan, white, then the bright
    /// eight in the same order.
    pub ansi: [Rgb; 16],
}

/// Opacity of the selection fill over the cells.
pub const SELECTION_ALPHA: f32 = 0.35;

/// Keyboard modifiers held with a key or a character.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// The emulator's modes that change what a key or a paste sends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    /// DECCKM: arrows send `ESC O A` instead of `ESC [ A`.
    pub app_cursor: bool,
    /// DECKPAM: the numeric keypad sends application sequences.
    pub app_keypad: bool,
    /// The shell asked for bracketed paste (`ESC [ 200 ~` … `ESC [ 201 ~`).
    pub bracketed_paste: bool,
}

// ---- Colours ----

/// A colour from `0xRRGGBB`.
const fn hex(v: u32) -> Rgb {
    Rgb::new(
        ((v >> 16) & 0xFF) as f32 / 255.0,
        ((v >> 8) & 0xFF) as f32 / 255.0,
        (v & 0xFF) as f32 / 255.0,
    )
}

/// The built-in ANSI set for dark pills: black, red, green, yellow, blue, magenta, cyan,
/// white, then the bright eight. Everything but black reaches 4.5:1 against the dark
/// built-in pills; black stays a dark grey so it still works as a background (and as the
/// text on a coloured one), and bright black is the "dim text" grey.
const DARK_ANSI: [Rgb; 16] = [
    hex(0x3A3633),
    hex(0xE06C5F),
    hex(0x8FBF6A),
    hex(0xE0B45A),
    hex(0x6F9FE8),
    hex(0xC586D6),
    hex(0x5FBFB8),
    hex(0xCFC8BA),
    hex(0x857E73),
    hex(0xF58B7E),
    hex(0xA9D985),
    hex(0xF2CB7C),
    hex(0x8FB8F5),
    hex(0xDDA0EA),
    hex(0x7FD6CF),
    hex(0xF5F0E6),
];

/// The built-in ANSI set for light pills. Light text on a light pill would vanish, so
/// the colours are the dark, saturated ones: every colour reaches 3:1 against the light
/// built-in pill and most reach 4:1. "White" is a mid grey (3:1), and bright white is a
/// darker grey than white, so bold white text stands out more instead of disappearing.
const LIGHT_ANSI: [Rgb; 16] = [
    hex(0x26231E),
    hex(0xB3261E),
    hex(0x2E7D32),
    hex(0x8A6A00),
    hex(0x1F5FBF),
    hex(0x8E3FA8),
    hex(0x0E7C86),
    hex(0x858074),
    hex(0x6A655A),
    hex(0xC93A2F),
    hex(0x3B8A3F),
    hex(0x9A7400),
    hex(0x2F6FD0),
    hex(0xA44CBF),
    hex(0x13808A),
    hex(0x5A554B),
];

/// The terminal's colours for a theme: text on the pill, the accent for the cursor and
/// the selection, and the 16 ANSI colours from the theme's `terminal_ansi` or else the
/// built-in set for its mode (dark or light).
pub fn palette(t: &ResolvedTokens) -> Palette {
    Palette {
        fg: t.text,
        bg: t.pill,
        cursor: t.accent,
        selection: t.accent,
        ansi: t.terminal_ansi.unwrap_or(match t.mode {
            Mode::Dark => DARK_ANSI,
            Mode::Light => LIGHT_ANSI,
        }),
    }
}

// ---- Keys ----

/// Win32 virtual-key codes the terminal maps (the `wParam` of `WM_KEYDOWN`).
pub mod vk {
    pub const BACK: u16 = 0x08;
    pub const TAB: u16 = 0x09;
    pub const SPACE: u16 = 0x20;
    /// `VK_PRIOR`.
    pub const PAGE_UP: u16 = 0x21;
    /// `VK_NEXT`.
    pub const PAGE_DOWN: u16 = 0x22;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const INSERT: u16 = 0x2D;
    pub const DELETE: u16 = 0x2E;
    pub const C: u16 = 0x43;
    pub const V: u16 = 0x56;
    pub const F1: u16 = 0x70;
    pub const F4: u16 = 0x73;
    pub const F5: u16 = 0x74;
    pub const F12: u16 = 0x7B;
}

/// What a key press does. The press comes first (`WM_KEYDOWN`); Windows may then send the
/// character it makes (`WM_CHAR`), which is what [`on_char`] is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// Send these bytes. The key makes no character, so nothing else follows.
    Send(Vec<u8>),
    /// Send these bytes and drop the `WM_CHAR` that follows (Backspace, Shift+Tab and
    /// Ctrl+Space make one of their own, which would be a second, wrong, keystroke).
    SendSkipChar(Vec<u8>),
    /// Copy the selection to the clipboard.
    Copy,
    /// Paste the clipboard (see [`paste_bytes`]).
    Paste,
    /// Not ours: let `WM_CHAR` deliver the character through [`on_char`].
    PassToChar,
}

impl KeyAction {
    /// True when the caller must also drop the `WM_CHAR` that Windows sends after this key
    /// press `vk`. That is [`KeyAction::SendSkipChar`], and also Copy and Paste: Ctrl+C
    /// makes a `0x03` and Ctrl+V a `0x16`, and sent to the shell those would interrupt it
    /// and print a stray byte. Shift+Insert makes no character, so it skips nothing (the
    /// next typed letter must not be eaten).
    pub fn skips_char(&self, vk: u16) -> bool {
        match self {
            KeyAction::SendSkipChar(_) => true,
            KeyAction::Copy | KeyAction::Paste => vk != self::vk::INSERT,
            KeyAction::Send(_) | KeyAction::PassToChar => false,
        }
    }
}

/// The xterm modifier parameter: `1 + shift + 2·alt + 4·ctrl` (1 means none).
fn modifier_param(mods: Mods) -> u8 {
    1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.ctrl)
}

/// Arrows, Home and End: `CSI 1;m X` with modifiers, else `SS3 X` in application cursor
/// mode, else `CSI X`.
fn cursor_seq(fin: u8, m: u8, app_cursor: bool) -> Vec<u8> {
    if m > 1 {
        format!("\x1b[1;{m}{}", fin as char).into_bytes()
    } else if app_cursor {
        vec![0x1b, b'O', fin]
    } else {
        vec![0x1b, b'[', fin]
    }
}

/// F1 to F4: `SS3 X`, or `CSI 1;m X` with modifiers (whatever the cursor mode).
fn ss3_seq(fin: u8, m: u8) -> Vec<u8> {
    if m > 1 {
        format!("\x1b[1;{m}{}", fin as char).into_bytes()
    } else {
        vec![0x1b, b'O', fin]
    }
}

/// Insert, Delete, Page Up and Down, F5 to F12: `CSI n ~`, or `CSI n;m ~` with modifiers.
fn tilde_seq(n: u8, m: u8) -> Vec<u8> {
    if m > 1 {
        format!("\x1b[{n};{m}~").into_bytes()
    } else {
        format!("\x1b[{n}~").into_bytes()
    }
}

/// What the key press `vk` (a Win32 virtual-key code) does, with the modifiers held.
///
/// Arrows, Home/End, Insert/Delete, Page Up/Down and F1 to F12 become the xterm escape
/// sequences (application cursor mode changes the plain arrows and Home/End). Backspace,
/// Shift+Tab and Ctrl+Space are sent here too, and their own character is skipped.
/// Ctrl+C with a selection, or Ctrl+Shift+C, copies; Ctrl+V, Ctrl+Shift+V and Shift+Insert
/// paste. Everything else is `PassToChar`: letters, digits, Enter, Esc, Tab, Ctrl letters
/// all arrive as a character.
///
/// Ctrl and Alt held together is AltGr on many layouts (German `@` is Ctrl+Alt+Q), so it is
/// never a shortcut: those keys are `PassToChar` and the layout's character gets through.
/// Application keypad mode (`Modes::app_keypad`) is not used: the keypad sends its digits.
pub fn on_key(vk: u16, mods: Mods, modes: Modes, has_selection: bool) -> KeyAction {
    let altgr = mods.ctrl && mods.alt;
    // Copy and paste. Ctrl+C is the shell's interrupt unless something is selected.
    if mods.ctrl && !mods.alt {
        match vk {
            vk::C if mods.shift || has_selection => return KeyAction::Copy,
            vk::V => return KeyAction::Paste,
            _ => {}
        }
    }
    if vk == vk::INSERT && mods.shift && !mods.ctrl && !mods.alt {
        return KeyAction::Paste;
    }
    // Keys that also make a character: send our bytes, skip theirs.
    if !altgr {
        match vk {
            vk::BACK => {
                return KeyAction::SendSkipChar(if mods.ctrl {
                    vec![0x08]
                } else if mods.alt {
                    vec![0x1b, 0x7f]
                } else {
                    vec![0x7f]
                });
            }
            vk::TAB if mods.shift && !mods.ctrl && !mods.alt => {
                return KeyAction::SendSkipChar(b"\x1b[Z".to_vec());
            }
            vk::SPACE if mods.ctrl && !mods.shift && !mods.alt => {
                return KeyAction::SendSkipChar(vec![0x00]);
            }
            _ => {}
        }
    }
    let m = modifier_param(mods);
    let app = modes.app_cursor;
    let seq = match vk {
        vk::UP => cursor_seq(b'A', m, app),
        vk::DOWN => cursor_seq(b'B', m, app),
        vk::RIGHT => cursor_seq(b'C', m, app),
        vk::LEFT => cursor_seq(b'D', m, app),
        vk::HOME => cursor_seq(b'H', m, app),
        vk::END => cursor_seq(b'F', m, app),
        vk::INSERT => tilde_seq(2, m),
        vk::DELETE => tilde_seq(3, m),
        vk::PAGE_UP => tilde_seq(5, m),
        vk::PAGE_DOWN => tilde_seq(6, m),
        f @ vk::F1..=vk::F4 => ss3_seq(b"PQRS"[usize::from(f - vk::F1)], m),
        f @ vk::F5..=vk::F12 => {
            tilde_seq([15, 17, 18, 19, 20, 21, 23, 24][usize::from(f - vk::F5)], m)
        }
        _ => return KeyAction::PassToChar,
    };
    KeyAction::Send(seq)
}

/// The bytes for a character Windows delivered (`WM_CHAR`): its UTF-8, with Alt held (and
/// no Ctrl) prefixed by ESC, the way a terminal sends Alt+x. Enter's `\n` (Ctrl+Enter)
/// becomes `\r`.
///
/// Ctrl and Alt together is AltGr (`@` on a German layout is Ctrl+Alt+Q, `ł` on a Polish
/// one is Ctrl+Alt+L), so that character is sent as it is, never ESC-prefixed. The cost is
/// that a real Ctrl+Alt+letter on a layout without AltGr arrives as the plain control byte.
pub fn on_char(ch: char, mods: Mods) -> Vec<u8> {
    let ch = if ch == '\n' { '\r' } else { ch };
    let mut out = Vec::with_capacity(5);
    if mods.alt && !mods.ctrl {
        out.push(0x1b);
    }
    let mut buf = [0u8; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    out
}

/// A control the shell must not get from a paste: C0 (but not Tab, and CR is made below),
/// DEL, and C1 (U+0080 to U+009F, whose U+009B is an 8-bit CSI).
fn is_blocked_in_paste(c: char) -> bool {
    c <= '\u{1f}' || ('\u{7f}'..='\u{9f}').contains(&c)
}

/// The bytes to send for a paste of `text`.
///
/// Line breaks (CRLF, LF) become CR, the Enter key. Control characters other than Tab and
/// CR are dropped, so a pasted ESC, and with it a `ESC [ 201 ~`, can't end a bracketed
/// paste early and run the rest as typed commands. DEL and the C1 controls are dropped
/// too (U+009B is another way to write CSI). Nothing is left to send for an empty result.
///
/// With `modes.bracketed_paste` the text is wrapped in `ESC [ 200 ~` and `ESC [ 201 ~`, so
/// the shell takes it as one block and doesn't run its lines. Without it, a multi-line
/// paste is still one write: whether the lines run one by one is the shell's call (a
/// shell that doesn't ask for bracketed paste can't tell a paste from typing).
pub fn paste_bytes(text: &str, modes: Modes) -> Vec<u8> {
    let mut body = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                body.push('\r');
            }
            '\n' => body.push('\r'),
            '\t' => body.push('\t'),
            c if is_blocked_in_paste(c) => {}
            c => body.push(c),
        }
    }
    if body.is_empty() {
        return Vec::new();
    }
    if modes.bracketed_paste {
        let mut out = Vec::with_capacity(body.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(body.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        body.into_bytes()
    }
}

// ---- Grid and mouse ----

/// The smallest grid `grid_fit` returns.
pub const MIN_COLS: u16 = 20;
pub const MIN_ROWS: u16 = 4;

/// Scale as the maths wants it: a positive finite number, else 1.
fn sane_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// The grid that fits `TERM_TEXT` at `scale`, as whole cells of `cell_w_px` × `cell_h_px`
/// device px: `(cols, rows)`, at least 20 × 4. What is left over stays empty on the right
/// and at the bottom.
pub fn grid_fit(cell_w_px: u16, cell_h_px: u16, scale: f32) -> (u16, u16) {
    // A hair of slack so an exact fit isn't lost to rounding (732 × 1.25 and the like).
    const SLACK: f32 = 1e-3;
    let scale = sane_scale(scale);
    let fit = |extent: f32, cell: u16| {
        (((extent * scale + SLACK) / f32::from(cell.max(1))).floor() as u16).max(1)
    };
    (
        fit(TERM_TEXT.w, cell_w_px).max(MIN_COLS),
        fit(TERM_TEXT.h, cell_h_px).max(MIN_ROWS),
    )
}

/// The cell under a panel-local logical point `(x, y)` (the panel's top-left is the
/// origin, as in `ClickOutcome::Terminal`): `(col, row, right_half)`, where `right_half`
/// is whether the point is in the cell's right half (a selection edge rounds to the
/// nearest cell boundary with it).
///
/// A point that leaves the grid, in a drag, clamps to the nearest cell: left of it is the
/// first column (left half), right of it the last (right half), and above or below it the
/// first or last row. The one `None` is a point in the header (`y < TERM_HEADER_H`: the
/// lamp, the title and the ✕, which are not text), and also a grid with no cells.
pub fn cell_at(
    x: f32,
    y: f32,
    cell_w_px: u16,
    cell_h_px: u16,
    scale: f32,
    cols: u16,
    rows: u16,
) -> Option<(usize, usize, bool)> {
    if y < TERM_HEADER_H || cols == 0 || rows == 0 || cell_w_px == 0 || cell_h_px == 0 {
        return None;
    }
    let scale = sane_scale(scale);
    let (cw, ch) = (f32::from(cell_w_px), f32::from(cell_h_px));
    // Device px from the text area's top-left.
    let (px, py) = ((x - TERM_TEXT.x) * scale, (y - TERM_TEXT.y) * scale);
    let col_f = (px / cw).floor();
    let last_col = f32::from(cols - 1);
    let col = col_f.clamp(0.0, last_col);
    let right = if col_f < 0.0 {
        false
    } else if col_f > last_col {
        true
    } else {
        (px - col * cw) * 2.0 >= cw
    };
    let row = (py / ch).floor().clamp(0.0, f32::from(rows - 1));
    Some((col as usize, row as usize, right))
}

/// Lines to scroll for a wheel `delta` (`WHEEL_DELTA` is 120 per notch): 3 per notch,
/// the sign kept, a part of a notch cut to whole lines. Positive (wheel up, away from
/// you) is back into the history. A wheel that sends less than a notch at a time needs
/// [`WheelAccum`], or its small steps add up to nothing.
pub fn wheel_lines(delta: i32) -> i32 {
    (i64::from(delta) * 3 / 120) as i32
}

/// [`wheel_lines`] for wheels and touchpads that send part of a notch: the part that
/// doesn't make a whole line yet is kept for the next event. A change of direction drops it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WheelAccum {
    /// Left over, in 120ths of a line (the same sign as the last delta).
    rest: i32,
}

impl WheelAccum {
    /// Whole lines for this `delta`, counting what the earlier ones left.
    pub fn feed(&mut self, delta: i32) -> i32 {
        if delta == 0 {
            return 0;
        }
        if (delta < 0) != (self.rest < 0) && self.rest != 0 {
            self.rest = 0;
        }
        let total = i64::from(self.rest) + i64::from(delta) * 3;
        self.rest = (total % 120) as i32;
        (total / 120) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ThemeRegistry, MIN_TEXT_CONTRAST};

    // ---- helpers ----

    const NONE: Mods = Mods {
        ctrl: false,
        shift: false,
        alt: false,
    };
    const CTRL: Mods = Mods { ctrl: true, ..NONE };
    const SHIFT: Mods = Mods {
        shift: true,
        ..NONE
    };
    const ALT: Mods = Mods { alt: true, ..NONE };
    const ALTGR: Mods = Mods {
        ctrl: true,
        alt: true,
        shift: false,
    };
    const NORMAL: Modes = Modes {
        app_cursor: false,
        app_keypad: false,
        bracketed_paste: false,
    };
    const APP: Modes = Modes {
        app_cursor: true,
        ..NORMAL
    };
    const BRACKETED: Modes = Modes {
        bracketed_paste: true,
        ..NORMAL
    };

    /// Every one of the 8 modifier combinations.
    fn all_mods() -> Vec<Mods> {
        (0..8)
            .map(|n| Mods {
                ctrl: n & 1 != 0,
                shift: n & 2 != 0,
                alt: n & 4 != 0,
            })
            .collect()
    }

    fn send(bytes: &[u8]) -> KeyAction {
        KeyAction::Send(bytes.to_vec())
    }

    fn skip(bytes: &[u8]) -> KeyAction {
        KeyAction::SendSkipChar(bytes.to_vec())
    }

    fn key(vk: u16, mods: Mods) -> KeyAction {
        on_key(vk, mods, NORMAL, false)
    }

    fn themes() -> Vec<(String, ResolvedTokens)> {
        let r = ThemeRegistry::with_builtins();
        r.ids()
            .iter()
            .map(|id| (id.clone(), r.get(id).unwrap().clone()))
            .collect()
    }

    // ---- palette ----

    #[test]
    fn the_palette_takes_its_colours_from_the_theme() {
        for (id, t) in themes() {
            let p = palette(&t);
            assert_eq!((p.fg, p.bg), (t.text, t.pill), "{id}");
            assert_eq!((p.cursor, p.selection), (t.accent, t.accent), "{id}");
        }
    }

    #[test]
    fn a_dark_theme_gets_the_dark_set_and_a_light_theme_the_light_one() {
        for (id, t) in themes() {
            let want = match t.mode {
                Mode::Dark => DARK_ANSI,
                Mode::Light => LIGHT_ANSI,
            };
            assert_eq!(palette(&t).ansi, want, "{id}");
        }
        let r = ThemeRegistry::with_builtins();
        assert_eq!(r.get("paper-zine").unwrap().mode, Mode::Light);
        assert_eq!(palette(r.get("paper-zine").unwrap()).ansi, LIGHT_ANSI);
        assert_eq!(palette(r.get("clean").unwrap()).ansi, DARK_ANSI);
        assert_ne!(DARK_ANSI, LIGHT_ANSI);
    }

    #[test]
    fn a_themes_own_terminal_ansi_wins() {
        let mut r = ThemeRegistry::with_builtins();
        let list: Vec<String> = (0..16)
            .map(|i| format!("\"#{i:02X}{i:02X}{i:02X}\""))
            .collect();
        r.set_user_themes([(
            "own".to_string(),
            format!(
                "terminal_ansi = [{}]\n[meta]\nname=\"Own\"\nbase=\"paper-zine\"\n",
                list.join(", ")
            ),
        )]);
        let t = r.get("own").unwrap();
        // A light theme with its own set: the set is used, not the light default.
        assert_eq!(t.mode, Mode::Light);
        let p = palette(t);
        assert_ne!(p.ansi, LIGHT_ANSI);
        assert_eq!(p.ansi[0].to_hex(), "#000000");
        assert_eq!(p.ansi[5].to_hex(), "#050505");
        assert_eq!(p.ansi[15].to_hex(), "#0F0F0F");
        // The other colours still come from the theme.
        assert_eq!((p.fg, p.bg), (t.text, t.pill));
    }

    #[test]
    fn the_builtin_sets_are_sixteen_distinct_colours_in_the_ansi_order() {
        for (name, set) in [("dark", DARK_ANSI), ("light", LIGHT_ANSI)] {
            let mut hexes: Vec<String> = set.iter().map(|c| c.to_hex()).collect();
            hexes.sort();
            hexes.dedup();
            assert_eq!(hexes.len(), 16, "{name}");
            // The hue is the named one, normal and bright: red, green and blue lead their
            // channel, yellow is red + green, magenta red + blue, cyan green + blue.
            for base in [1usize, 9] {
                let c = |i: usize| {
                    let [r, g, b] = set[base + i].to_u8().map(i32::from);
                    (r, g, b)
                };
                let (r, g, b) = c(0);
                assert!(r > g + 40 && r > b + 40, "{name} red {base}");
                let (r, g, b) = c(1);
                assert!(g > r + 20 && g > b + 40, "{name} green {base}");
                let (r, g, b) = c(2);
                assert!(r > b + 40 && g > b + 40, "{name} yellow {base}");
                let (r, g, b) = c(3);
                assert!(b > r + 20 && b > g + 40, "{name} blue {base}");
                let (r, g, b) = c(4);
                assert!(r > g + 40 && b > g + 40, "{name} magenta {base}");
                let (r, g, b) = c(5);
                assert!(g > r + 40 && b > r + 40, "{name} cyan {base}");
            }
            // The greys are greys: black, white, bright black, bright white.
            for i in [0usize, 7, 8, 15] {
                let [r, g, b] = set[i].to_u8().map(i32::from);
                assert!((r - g).abs() < 16 && (g - b).abs() < 16, "{name} grey {i}");
            }
        }
        // Every table entry is the colour it is written as.
        assert_eq!(DARK_ANSI[1].to_hex(), "#E06C5F");
        assert_eq!(LIGHT_ANSI[15].to_hex(), "#5A554B");
    }

    #[test]
    fn every_ansi_colour_reads_on_a_dark_pill() {
        let dark: Vec<_> = themes()
            .into_iter()
            .filter(|(_, t)| t.mode == Mode::Dark)
            .collect();
        assert_eq!(dark.len(), 3);
        for (id, t) in dark {
            let p = palette(&t);
            for (i, c) in p.ansi.iter().enumerate() {
                let ratio = c.contrast(p.bg);
                match i {
                    // Black is a background colour first: it only has to show against the pill.
                    0 => assert!(ratio >= 1.4, "{id} black {ratio:.2}"),
                    _ => assert!(ratio >= MIN_TEXT_CONTRAST, "{id} ansi {i} {ratio:.2}"),
                }
            }
            // Black is darker than bright black, and white lighter than the colours.
            assert!(p.ansi[0].relative_luminance() < p.ansi[8].relative_luminance());
            assert!(p.ansi[15].contrast(p.bg) > p.ansi[7].contrast(p.bg));
            // The default text still outranks the dim grey.
            assert!(p.fg.contrast(p.bg) > p.ansi[8].contrast(p.bg));
        }
    }

    #[test]
    fn every_ansi_colour_reads_on_the_light_pill() {
        let r = ThemeRegistry::with_builtins();
        let t = r.get("paper-zine").unwrap();
        let p = palette(t);
        for (i, c) in p.ansi.iter().enumerate() {
            let ratio = c.contrast(p.bg);
            // White and bright white are greys dark enough to see; everything is at least 3:1.
            assert!(ratio >= 3.0, "ansi {i} {ratio:.2}");
            match i {
                0 | 8 => assert!(ratio >= 4.5, "grey {i} {ratio:.2}"),
                1..=6 | 9..=14 => assert!(ratio >= 3.5, "colour {i} {ratio:.2}"),
                _ => {}
            }
        }
        // Black is the darkest ink, and bright white is not lighter than white here.
        assert!(p.ansi[0].relative_luminance() < p.ansi[8].relative_luminance());
        assert!(p.ansi[15].contrast(p.bg) > p.ansi[7].contrast(p.bg));
        // Black text on a "white" (grey) background stays readable.
        assert!(p.ansi[0].contrast(p.ansi[7]) >= 3.5);
        assert!(p.fg.contrast(p.bg) >= MIN_TEXT_CONTRAST);
    }

    // ---- on_key: sequences ----

    #[test]
    fn plain_arrows_home_and_end_are_csi() {
        for (vk, fin) in [
            (vk::UP, b'A'),
            (vk::DOWN, b'B'),
            (vk::RIGHT, b'C'),
            (vk::LEFT, b'D'),
            (vk::HOME, b'H'),
            (vk::END, b'F'),
        ] {
            assert_eq!(key(vk, NONE), send(&[0x1b, b'[', fin]), "{vk:#x}");
        }
    }

    #[test]
    fn application_cursor_mode_makes_plain_arrows_home_and_end_ss3() {
        for (vk, fin) in [
            (vk::UP, b'A'),
            (vk::DOWN, b'B'),
            (vk::RIGHT, b'C'),
            (vk::LEFT, b'D'),
            (vk::HOME, b'H'),
            (vk::END, b'F'),
        ] {
            assert_eq!(
                on_key(vk, NONE, APP, false),
                send(&[0x1b, b'O', fin]),
                "{vk:#x}"
            );
            // A modifier brings the CSI form back, in either mode.
            assert_eq!(
                on_key(vk, CTRL, APP, false),
                on_key(vk, CTRL, NORMAL, false),
                "{vk:#x}"
            );
        }
        // The mode changes nothing else: Insert, Delete, Page Up and Down, F-keys.
        for vk in [
            vk::INSERT,
            vk::DELETE,
            vk::PAGE_UP,
            vk::PAGE_DOWN,
            vk::F1,
            vk::F5,
        ] {
            assert_eq!(on_key(vk, NONE, APP, false), key(vk, NONE), "{vk:#x}");
        }
    }

    #[test]
    fn modified_arrows_and_home_end_use_one_semicolon_m() {
        // m = 1 + shift + 2 alt + 4 ctrl.
        let want = |m: u8, fin: char| send(format!("\x1b[1;{m}{fin}").as_bytes());
        assert_eq!(key(vk::UP, SHIFT), want(2, 'A'));
        assert_eq!(key(vk::UP, ALT), want(3, 'A'));
        assert_eq!(
            key(
                vk::UP,
                Mods {
                    shift: true,
                    alt: true,
                    ctrl: false
                }
            ),
            want(4, 'A')
        );
        assert_eq!(key(vk::LEFT, CTRL), want(5, 'D'));
        assert_eq!(
            key(
                vk::RIGHT,
                Mods {
                    ctrl: true,
                    shift: true,
                    alt: false
                }
            ),
            want(6, 'C')
        );
        assert_eq!(key(vk::DOWN, ALTGR), want(7, 'B'));
        assert_eq!(
            key(
                vk::HOME,
                Mods {
                    ctrl: true,
                    shift: true,
                    alt: true
                }
            ),
            want(8, 'H')
        );
        assert_eq!(key(vk::END, CTRL), want(5, 'F'));
        // Every combination, for every one of the six keys, against the formula.
        for (vk, fin) in [
            (vk::UP, 'A'),
            (vk::DOWN, 'B'),
            (vk::RIGHT, 'C'),
            (vk::LEFT, 'D'),
            (vk::HOME, 'H'),
            (vk::END, 'F'),
        ] {
            for mods in all_mods() {
                let m = 1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.ctrl);
                if m == 1 {
                    continue;
                }
                assert_eq!(
                    on_key(vk, mods, NORMAL, false),
                    want(m, fin),
                    "{vk:#x} {mods:?}"
                );
            }
        }
    }

    #[test]
    fn insert_delete_and_page_keys_are_tilde_sequences() {
        for (vk, n) in [
            (vk::INSERT, 2),
            (vk::DELETE, 3),
            (vk::PAGE_UP, 5),
            (vk::PAGE_DOWN, 6),
        ] {
            // Insert with Shift alone is paste, so it is checked below.
            if vk != vk::INSERT {
                assert_eq!(
                    key(vk, NONE),
                    send(format!("\x1b[{n}~").as_bytes()),
                    "{vk:#x}"
                );
            }
            for mods in all_mods() {
                let m = 1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.ctrl);
                if m == 1 || (vk == vk::INSERT && mods == SHIFT) {
                    continue;
                }
                assert_eq!(
                    on_key(vk, mods, NORMAL, false),
                    send(format!("\x1b[{n};{m}~").as_bytes()),
                    "{vk:#x} {mods:?}"
                );
            }
        }
        assert_eq!(key(vk::INSERT, NONE), send(b"\x1b[2~"));
        assert_eq!(key(vk::DELETE, CTRL), send(b"\x1b[3;5~"));
        assert_eq!(key(vk::PAGE_UP, SHIFT), send(b"\x1b[5;2~"));
        assert_eq!(key(vk::PAGE_DOWN, ALT), send(b"\x1b[6;3~"));
    }

    #[test]
    fn function_keys() {
        // F1 to F4 are SS3 P..S.
        for (i, fin) in b"PQRS".iter().enumerate() {
            let vk = vk::F1 + i as u16;
            assert_eq!(key(vk, NONE), send(&[0x1b, b'O', *fin]), "F{}", i + 1);
            // The cursor mode doesn't touch them.
            assert_eq!(on_key(vk, NONE, APP, false), key(vk, NONE));
            // Modified: CSI 1;m P..S.
            assert_eq!(
                key(vk, SHIFT),
                send(format!("\x1b[1;2{}", *fin as char).as_bytes())
            );
            assert_eq!(
                key(vk, CTRL),
                send(format!("\x1b[1;5{}", *fin as char).as_bytes())
            );
        }
        // F5 to F12 are CSI n ~ with n = 15 17 18 19 20 21 23 24.
        let ns = [15, 17, 18, 19, 20, 21, 23, 24];
        for (i, n) in ns.iter().enumerate() {
            let vk = vk::F5 + i as u16;
            assert_eq!(
                key(vk, NONE),
                send(format!("\x1b[{n}~").as_bytes()),
                "F{}",
                i + 5
            );
            assert_eq!(key(vk, SHIFT), send(format!("\x1b[{n};2~").as_bytes()));
            assert_eq!(key(vk, ALT), send(format!("\x1b[{n};3~").as_bytes()));
            assert_eq!(key(vk, CTRL), send(format!("\x1b[{n};5~").as_bytes()));
            assert_eq!(
                key(
                    vk,
                    Mods {
                        ctrl: true,
                        shift: true,
                        alt: false
                    }
                ),
                send(format!("\x1b[{n};6~").as_bytes())
            );
        }
        assert_eq!(vk::F4 - vk::F1, 3);
        assert_eq!(vk::F5, vk::F4 + 1);
        assert_eq!(vk::F12 - vk::F5, 7);
        // F13 and the keys around the range are not ours.
        assert_eq!(key(vk::F12 + 1, NONE), KeyAction::PassToChar);
        assert_eq!(key(vk::F1 - 1, NONE), KeyAction::PassToChar);
    }

    // ---- on_key: keys that make a character ----

    #[test]
    fn shift_tab_is_back_tab_and_skips_its_char() {
        assert_eq!(key(vk::TAB, SHIFT), skip(b"\x1b[Z"));
        // Plain Tab is a character; so are the other combinations.
        assert_eq!(key(vk::TAB, NONE), KeyAction::PassToChar);
        assert_eq!(key(vk::TAB, CTRL), KeyAction::PassToChar);
        assert_eq!(key(vk::TAB, ALT), KeyAction::PassToChar);
    }

    #[test]
    fn backspace_sends_del_and_variants_and_always_skips_its_char() {
        // Windows makes 0x08 for Backspace and 0x7F for Ctrl+Backspace; we send our own.
        assert_eq!(key(vk::BACK, NONE), skip(&[0x7f]));
        assert_eq!(key(vk::BACK, SHIFT), skip(&[0x7f]));
        assert_eq!(key(vk::BACK, CTRL), skip(&[0x08]));
        assert_eq!(
            key(
                vk::BACK,
                Mods {
                    ctrl: true,
                    shift: true,
                    alt: false
                }
            ),
            skip(&[0x08])
        );
        assert_eq!(key(vk::BACK, ALT), skip(&[0x1b, 0x7f]));
        assert_eq!(
            key(
                vk::BACK,
                Mods {
                    alt: true,
                    shift: true,
                    ctrl: false
                }
            ),
            skip(&[0x1b, 0x7f])
        );
    }

    #[test]
    fn ctrl_space_is_nul_and_skips_its_char() {
        assert_eq!(key(vk::SPACE, CTRL), skip(&[0x00]));
        // A plain, shifted or Alt space is a character (Alt+Space gets its ESC in on_char).
        for mods in [NONE, SHIFT, ALT] {
            assert_eq!(key(vk::SPACE, mods), KeyAction::PassToChar, "{mods:?}");
        }
    }

    // ---- on_key: copy and paste ----

    #[test]
    fn ctrl_c_copies_only_with_a_selection_and_ctrl_shift_c_always() {
        assert_eq!(on_key(vk::C, CTRL, NORMAL, true), KeyAction::Copy);
        // No selection: the shell's interrupt, through the 0x03 char.
        assert_eq!(on_key(vk::C, CTRL, NORMAL, false), KeyAction::PassToChar);
        let cs = Mods {
            ctrl: true,
            shift: true,
            alt: false,
        };
        assert_eq!(on_key(vk::C, cs, NORMAL, false), KeyAction::Copy);
        assert_eq!(on_key(vk::C, cs, NORMAL, true), KeyAction::Copy);
        // A plain C, or C with Shift or Alt only, is typing.
        for mods in [NONE, SHIFT, ALT] {
            assert_eq!(
                on_key(vk::C, mods, NORMAL, true),
                KeyAction::PassToChar,
                "{mods:?}"
            );
        }
        // The modes don't matter.
        assert_eq!(on_key(vk::C, CTRL, APP, true), KeyAction::Copy);
        assert_eq!(on_key(vk::C, CTRL, BRACKETED, false), KeyAction::PassToChar);
    }

    #[test]
    fn ctrl_v_ctrl_shift_v_and_shift_insert_paste() {
        let cs = Mods {
            ctrl: true,
            shift: true,
            alt: false,
        };
        for has_selection in [false, true] {
            assert_eq!(on_key(vk::V, CTRL, NORMAL, has_selection), KeyAction::Paste);
            assert_eq!(on_key(vk::V, cs, NORMAL, has_selection), KeyAction::Paste);
            assert_eq!(
                on_key(vk::INSERT, SHIFT, NORMAL, has_selection),
                KeyAction::Paste
            );
        }
        for mods in [NONE, SHIFT, ALT] {
            assert_eq!(key(vk::V, mods), KeyAction::PassToChar, "{mods:?}");
        }
        // Ctrl+Insert and Alt+Shift+Insert are not paste: they are sequences.
        assert_eq!(key(vk::INSERT, CTRL), send(b"\x1b[2;5~"));
        assert_eq!(
            key(
                vk::INSERT,
                Mods {
                    shift: true,
                    alt: true,
                    ctrl: false
                }
            ),
            send(b"\x1b[2;4~")
        );
    }

    #[test]
    fn copy_and_paste_skip_the_control_char_windows_makes_except_for_shift_insert() {
        // Ctrl+C makes 0x03 and Ctrl+V 0x16: sent on, they would interrupt the shell.
        assert!(KeyAction::Copy.skips_char(vk::C));
        assert!(KeyAction::Paste.skips_char(vk::V));
        // Shift+Insert makes no char, so nothing may be skipped after it.
        assert!(!KeyAction::Paste.skips_char(vk::INSERT));
        assert!(KeyAction::SendSkipChar(vec![0x7f]).skips_char(vk::BACK));
        assert!(!KeyAction::Send(vec![0x1b]).skips_char(vk::UP));
        assert!(!KeyAction::PassToChar.skips_char(vk::C));
        // And on_key's own results agree for every key it handles.
        assert!(key(vk::BACK, NONE).skips_char(vk::BACK));
        assert!(key(vk::TAB, SHIFT).skips_char(vk::TAB));
        assert!(key(vk::SPACE, CTRL).skips_char(vk::SPACE));
        assert!(on_key(vk::C, CTRL, NORMAL, true).skips_char(vk::C));
        assert!(!on_key(vk::INSERT, SHIFT, NORMAL, false).skips_char(vk::INSERT));
        assert!(!key(vk::UP, NONE).skips_char(vk::UP));
        assert!(!key(vk::F5, NONE).skips_char(vk::F5));
    }

    #[test]
    fn everything_else_is_left_to_the_char() {
        // Letters, digits, Enter, Esc, punctuation, the numpad, and Ctrl combinations.
        for vk in [
            0x41u16, 0x5A, 0x30, 0x39, 0x0D, 0x1B, 0xBA, 0xC0, 0x60, 0x69, 0x6B,
        ] {
            for mods in all_mods() {
                assert_eq!(key(vk, mods), KeyAction::PassToChar, "{vk:#x} {mods:?}");
            }
        }
        // Ctrl+A, Ctrl+D, Ctrl+L, Ctrl+Z arrive as their control chars.
        for vk in [0x41u16, 0x44, 0x4C, 0x5A] {
            assert_eq!(key(vk, CTRL), KeyAction::PassToChar);
        }
    }

    // ---- AltGr (review focus 1) ----

    #[test]
    fn altgr_is_never_a_shortcut_in_on_key() {
        // German `@` is Ctrl+Alt+Q; Polish, French and others use more of the same.
        let altgr_shift = Mods {
            ctrl: true,
            alt: true,
            shift: true,
        };
        for mods in [ALTGR, altgr_shift] {
            for has_selection in [false, true] {
                // Letters, including the copy and paste letters.
                for vk in [
                    0x51u16,
                    vk::C,
                    vk::V,
                    0x4C,
                    0x45,
                    0x37,
                    0x38,
                    0x39,
                    0x30,
                    0xDB,
                ] {
                    assert_eq!(
                        on_key(vk, mods, NORMAL, has_selection),
                        KeyAction::PassToChar,
                        "{vk:#x} {mods:?} sel {has_selection}"
                    );
                }
                // Keys that would be shortcuts or special bytes without AltGr.
                for vk in [vk::BACK, vk::TAB, vk::SPACE, vk::INSERT] {
                    assert_eq!(
                        on_key(vk, mods, NORMAL, has_selection),
                        if vk == vk::INSERT {
                            // Not a paste; a modified Insert (it makes no character).
                            send(
                                &format!("\x1b[2;{}~", if mods.shift { 8 } else { 7 }).into_bytes(),
                            )
                        } else {
                            KeyAction::PassToChar
                        },
                        "{vk:#x} {mods:?}"
                    );
                }
            }
        }
        // Without Alt the same keys are shortcuts, so the guard is the Alt.
        assert_eq!(on_key(vk::C, CTRL, NORMAL, true), KeyAction::Copy);
        assert_eq!(on_key(vk::V, CTRL, NORMAL, false), KeyAction::Paste);
        assert_eq!(key(vk::BACK, CTRL), skip(&[0x08]));
        assert_eq!(key(vk::SPACE, CTRL), skip(&[0x00]));
    }

    // ---- on_char ----

    #[test]
    fn a_plain_character_is_its_utf8() {
        assert_eq!(on_char('a', NONE), b"a");
        assert_eq!(on_char('A', SHIFT), b"A");
        assert_eq!(on_char('é', NONE), "é".as_bytes());
        assert_eq!(on_char('€', NONE), "€".as_bytes());
        assert_eq!(on_char('日', NONE), "日".as_bytes());
        assert_eq!(on_char('😀', NONE), "😀".as_bytes());
        assert_eq!(on_char('😀', NONE).len(), 4);
        assert_eq!(on_char(' ', NONE), b" ");
        assert_eq!(on_char('\t', NONE), b"\t");
        assert_eq!(on_char('\r', NONE), b"\r");
        assert_eq!(on_char('\u{1b}', NONE), [0x1b]);
        assert_eq!(on_char('\u{7f}', NONE), [0x7f]);
    }

    #[test]
    fn a_line_feed_becomes_a_carriage_return() {
        assert_eq!(on_char('\n', NONE), b"\r");
        assert_eq!(on_char('\n', CTRL), b"\r");
        assert_eq!(on_char('\n', ALT), b"\x1b\r");
    }

    #[test]
    fn ctrl_letters_arrive_as_the_control_bytes_windows_made() {
        assert_eq!(on_char('\u{3}', CTRL), [0x03]);
        assert_eq!(on_char('\u{1}', CTRL), [0x01]);
        assert_eq!(
            on_char(
                '\u{1a}',
                Mods {
                    ctrl: true,
                    shift: true,
                    alt: false
                }
            ),
            [0x1a]
        );
        assert_eq!(on_char('\u{1d}', CTRL), [0x1d]);
    }

    #[test]
    fn alt_without_ctrl_prefixes_esc() {
        assert_eq!(on_char('b', ALT), b"\x1bb");
        assert_eq!(
            on_char(
                'B',
                Mods {
                    alt: true,
                    shift: true,
                    ctrl: false
                }
            ),
            b"\x1bB"
        );
        assert_eq!(on_char('é', ALT), [&[0x1bu8][..], "é".as_bytes()].concat());
        assert_eq!(on_char('.', ALT), b"\x1b.");
        assert_eq!(on_char(' ', ALT), b"\x1b ");
    }

    #[test]
    fn altgr_characters_arrive_as_themselves_never_esc_prefixed() {
        // German @ (Ctrl+Alt+Q), € (Ctrl+Alt+E), { [ ] } \ ~ |; French # { [ | \ ^ @ ] };
        // Polish ą ę ł ó ś ż ź ć ń; Spanish €; Czech and Nordic braces.
        for ch in [
            '@', '€', '{', '[', ']', '}', '\\', '~', '|', '#', '^', '`', 'ą', 'ę', 'ł', 'ó', 'ś',
            'ż', 'ź', 'ć', 'ń', 'µ', '²', '³', '¤',
        ] {
            let got = on_char(ch, ALTGR);
            assert_eq!(got, ch.to_string().into_bytes(), "{ch:?}");
            assert_ne!(got[0], 0x1b, "{ch:?}");
            // With Shift too (AltGr+Shift layers).
            let got = on_char(
                ch,
                Mods {
                    ctrl: true,
                    alt: true,
                    shift: true,
                },
            );
            assert_eq!(got, ch.to_string().into_bytes(), "shift {ch:?}");
        }
        // The same character with a lone Alt is the Alt key, so it does get the ESC.
        assert_eq!(on_char('@', ALT), b"\x1b@");
    }

    // ---- paste ----

    #[test]
    fn line_breaks_become_cr() {
        assert_eq!(paste_bytes("a\r\nb", NORMAL), b"a\rb");
        assert_eq!(paste_bytes("a\nb", NORMAL), b"a\rb");
        assert_eq!(paste_bytes("a\rb", NORMAL), b"a\rb");
        assert_eq!(paste_bytes("a\r\n\r\nb\n", NORMAL), b"a\r\rb\r");
        assert_eq!(paste_bytes("a\n\rb", NORMAL), b"a\r\rb");
        // A lone CR at the very end, and CRLF split across the end.
        assert_eq!(paste_bytes("a\r", NORMAL), b"a\r");
        assert_eq!(paste_bytes("\r\n", NORMAL), b"\r");
    }

    #[test]
    fn tab_and_text_survive_and_unicode_is_kept() {
        assert_eq!(paste_bytes("a\tb", NORMAL), b"a\tb");
        assert_eq!(
            paste_bytes("héllo wörld € 日本語 😀", NORMAL),
            "héllo wörld € 日本語 😀".as_bytes()
        );
        assert_eq!(paste_bytes("  indented  ", NORMAL), b"  indented  ");
        assert_eq!(paste_bytes("", NORMAL), b"");
        assert_eq!(paste_bytes("", BRACKETED), b"");
    }

    #[test]
    fn control_characters_are_dropped() {
        // Every C0 control but Tab, LF and CR (those are kept or turned into CR).
        let text: String = (0u8..0x20).map(char::from).collect();
        assert_eq!(paste_bytes(&text, NORMAL), b"\t\r\r");
        assert_eq!(
            paste_bytes("a\u{0}b\u{7}c\u{8}d\u{b}e\u{c}f\u{1b}g\u{7f}h", NORMAL),
            b"abcdefgh"
        );
        // DEL and the C1 controls (U+009B is an 8-bit CSI).
        let c1: String = ('\u{80}'..='\u{9f}').collect();
        assert_eq!(paste_bytes(&format!("x{c1}y"), NORMAL), b"xy");
        // Characters right next to the dropped ranges are kept.
        assert_eq!(
            paste_bytes("\u{20}\u{7e}\u{a0}\u{a1}", NORMAL),
            " ~\u{a0}\u{a1}".as_bytes()
        );
        // Text that is all controls leaves nothing to send, even bracketed.
        assert_eq!(paste_bytes("\u{1b}\u{0}\u{7}", BRACKETED), b"");
    }

    #[test]
    fn bracketed_paste_wraps_the_text_once() {
        assert_eq!(paste_bytes("ls", BRACKETED), b"\x1b[200~ls\x1b[201~");
        assert_eq!(
            paste_bytes("one\r\ntwo\nthree", BRACKETED),
            b"\x1b[200~one\rtwo\rthree\x1b[201~"
        );
        // Not bracketed: no markers, and a multi-line paste is still one block of bytes.
        let plain = paste_bytes("one\ntwo\nthree\n", NORMAL);
        assert_eq!(plain, b"one\rtwo\rthree\r");
        assert!(!plain.contains(&0x1b));
    }

    /// How many times `needle` occurs in `hay`.
    fn count(hay: &[u8], needle: &[u8]) -> usize {
        hay.windows(needle.len()).filter(|w| *w == needle).count()
    }

    #[test]
    fn a_pasted_end_marker_cannot_end_the_bracketed_paste_early() {
        // The hostile clipboard: end the paste, then run a command.
        let evil = "echo hi\x1b[201~\rrm -rf ~\r";
        let out = paste_bytes(evil, BRACKETED);
        // Exactly one start and one end, the end last, and nothing but text between.
        assert_eq!(count(&out, b"\x1b[200~"), 1);
        assert_eq!(count(&out, b"\x1b[201~"), 1);
        assert!(out.starts_with(b"\x1b[200~") && out.ends_with(b"\x1b[201~"));
        let inner = &out[6..out.len() - 6];
        assert!(!inner.contains(&0x1b), "{inner:?}");
        assert_eq!(inner, b"echo hi[201~\rrm -rf ~\r");
        // The same with the marker as the whole clipboard, and with a start marker inside.
        let out = paste_bytes("\x1b[201~", BRACKETED);
        assert_eq!(out, b"\x1b[200~[201~\x1b[201~");
        let out = paste_bytes("a\x1b[200~b\x1b[201~c", BRACKETED);
        assert_eq!(
            (count(&out, b"\x1b[200~"), count(&out, b"\x1b[201~")),
            (1, 1)
        );
        assert_eq!(out, b"\x1b[200~a[200~b[201~c\x1b[201~");
        // Without bracketed mode there is no ESC to inject at all.
        let out = paste_bytes(evil, NORMAL);
        assert!(!out.contains(&0x1b));
        assert_eq!(out, b"echo hi[201~\rrm -rf ~\r");
    }

    #[test]
    fn the_eight_bit_csi_and_other_escape_introducers_are_dropped_too() {
        // U+009B is CSI as a single character; U+0090 DCS, U+009D OSC, U+0098 SOS.
        for c in ['\u{9b}', '\u{90}', '\u{9d}', '\u{98}', '\u{9e}', '\u{9f}'] {
            let out = paste_bytes(&format!("a{c}201~b"), BRACKETED);
            assert_eq!(out, b"\x1b[200~a201~b\x1b[201~", "{c:?}");
            // As UTF-8 it would be C2 xx: never in the output.
            assert!(!out.contains(&0xC2), "{c:?}");
        }
    }

    #[test]
    fn a_large_paste_is_one_write() {
        let text = "line of text\r\n".repeat(20_000);
        let out = paste_bytes(&text, BRACKETED);
        assert_eq!(out.len(), 12 + 13 * 20_000);
        assert_eq!(count(&out, b"\x1b[201~"), 1);
    }

    // ---- grid and mouse ----

    #[test]
    fn grid_fit_counts_whole_cells_in_the_text_area() {
        // TERM_TEXT is 732 x 368 logical px.
        assert_eq!((TERM_TEXT.w, TERM_TEXT.h), (732.0, 368.0));
        // 100 %: 732 / 9 = 81.3, 368 / 18 = 20.4.
        assert_eq!(grid_fit(9, 18, 1.0), (81, 20));
        // 125 %: 915 x 460 device px.
        assert_eq!(grid_fit(11, 22, 1.25), (83, 20));
        // 150 %: 1098 x 552.
        assert_eq!(grid_fit(13, 27, 1.5), (84, 20));
        // 200 %: 1464 x 736.
        assert_eq!(grid_fit(18, 36, 2.0), (81, 20));
        // Exact fits keep the last cell (732 / 12 = 61, 368 / 23 = 16; 915 / 5 = 183).
        assert_eq!(grid_fit(12, 23, 1.0), (61, 16));
        assert_eq!(grid_fit(5, 20, 1.25), (183, 23));
        assert_eq!(grid_fit(1, 1, 1.0), (732, 368));
        assert_eq!(grid_fit(1, 1, 1.25), (915, 460));
        assert_eq!(grid_fit(1, 1, 1.5), (1098, 552));
        assert_eq!(grid_fit(1, 1, 2.0), (1464, 736));
    }

    #[test]
    fn the_fitted_grid_is_the_most_whole_cells_that_fit() {
        for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0] {
            for (cw, ch) in [
                (6u16, 12u16),
                (7, 15),
                (8, 16),
                (9, 19),
                (10, 20),
                (14, 30),
                (20, 40),
            ] {
                let (cols, rows) = grid_fit(cw, ch, scale);
                let (w, h) = (TERM_TEXT.w * scale, TERM_TEXT.h * scale);
                assert!(
                    f32::from(cols) * f32::from(cw) <= w + 0.01,
                    "{cw}x{ch}@{scale}"
                );
                assert!(
                    f32::from(rows) * f32::from(ch) <= h + 0.01,
                    "{cw}x{ch}@{scale}"
                );
                assert!(
                    (f32::from(cols) + 1.0) * f32::from(cw) > w,
                    "{cw}x{ch}@{scale}"
                );
                assert!(
                    (f32::from(rows) + 1.0) * f32::from(ch) > h,
                    "{cw}x{ch}@{scale}"
                );
            }
        }
    }

    #[test]
    fn the_grid_is_never_smaller_than_20_by_4() {
        assert_eq!((MIN_COLS, MIN_ROWS), (20, 4));
        assert_eq!(grid_fit(100, 100, 1.0), (20, 4));
        assert_eq!(grid_fit(u16::MAX, u16::MAX, 1.0), (20, 4));
        assert_eq!(grid_fit(40, 100, 1.0), (20, 4));
        // The minimum applies to each axis on its own.
        assert_eq!(grid_fit(9, 400, 1.0), (81, 4));
        assert_eq!(grid_fit(400, 18, 1.0), (20, 20));
        // A zero cell (no font yet) is treated as one pixel, and a bad scale as 100 %.
        assert_eq!(grid_fit(0, 0, 1.0), grid_fit(1, 1, 1.0));
        assert_eq!(grid_fit(9, 18, 0.0), grid_fit(9, 18, 1.0));
        assert_eq!(grid_fit(9, 18, -2.0), grid_fit(9, 18, 1.0));
        assert_eq!(grid_fit(9, 18, f32::NAN), grid_fit(9, 18, 1.0));
        assert_eq!(grid_fit(9, 18, f32::INFINITY), grid_fit(9, 18, 1.0));
    }

    /// A 9 x 18 cell at 100 %: 81 x 20 cells, the text area starting at (14, 40).
    fn at(x: f32, y: f32) -> Option<(usize, usize, bool)> {
        cell_at(x, y, 9, 18, 1.0, 81, 20)
    }

    #[test]
    fn cell_at_finds_the_cell_and_its_half() {
        let (ox, oy) = (TERM_TEXT.x, TERM_TEXT.y);
        assert_eq!(at(ox, oy), Some((0, 0, false)));
        assert_eq!(at(ox + 4.4, oy), Some((0, 0, false)));
        // The middle of the cell is the first point of its right half.
        assert_eq!(at(ox + 4.5, oy), Some((0, 0, true)));
        assert_eq!(at(ox + 8.9, oy), Some((0, 0, true)));
        assert_eq!(at(ox + 9.0, oy), Some((1, 0, false)));
        assert_eq!(
            at(ox + 9.0 * 10.0 + 1.0, oy + 18.0 * 3.0 + 17.0),
            Some((10, 3, false))
        );
        assert_eq!(
            at(ox + 9.0 * 10.0 + 6.0, oy + 18.0 * 3.0),
            Some((10, 3, true))
        );
        // The last cell of the grid.
        assert_eq!(
            at(ox + 9.0 * 80.0 + 1.0, oy + 18.0 * 19.0 + 1.0),
            Some((80, 19, false))
        );
        assert_eq!(
            at(ox + 9.0 * 80.0 + 8.0, oy + 18.0 * 19.0 + 17.0),
            Some((80, 19, true))
        );
    }

    #[test]
    fn cell_at_clamps_to_the_nearest_cell_outside_the_grid() {
        let (ox, oy) = (TERM_TEXT.x, TERM_TEXT.y);
        // Left of the text, and the margin of the panel: the first column, left half.
        assert_eq!(at(0.0, oy + 20.0), Some((0, 1, false)));
        assert_eq!(at(-50.0, oy + 20.0), Some((0, 1, false)));
        assert_eq!(at(ox - 0.1, oy + 20.0), Some((0, 1, false)));
        // Right of the grid (the leftover of the text area, the margin, outside the panel):
        // the last column, right half.
        assert_eq!(at(ox + 9.0 * 81.0, oy), Some((80, 0, true)));
        assert_eq!(at(ox + TERM_TEXT.w - 0.1, oy), Some((80, 0, true)));
        assert_eq!(at(layout_w(), oy), Some((80, 0, true)));
        assert_eq!(at(5000.0, oy + 40.0), Some((80, 2, true)));
        // Below the grid: the last row, keeping the column.
        assert_eq!(
            at(ox + 9.0 * 5.0 + 1.0, oy + 18.0 * 20.0),
            Some((5, 19, false))
        );
        assert_eq!(at(ox + 9.0 * 5.0 + 6.0, 5000.0), Some((5, 19, true)));
        // The gap between the header and the first row is the first row.
        assert_eq!(at(ox + 20.0, TERM_HEADER_H), Some((2, 0, false)));
        assert_eq!(at(ox + 20.0, oy - 0.5), Some((2, 0, false)));
        // A corner.
        assert_eq!(at(-10.0, 5000.0), Some((0, 19, false)));
        assert_eq!(at(5000.0, 5000.0), Some((80, 19, true)));
    }

    fn layout_w() -> f32 {
        crate::layout::TERM_W
    }

    #[test]
    fn cell_at_is_none_only_in_the_header() {
        assert_eq!(at(100.0, 0.0), None);
        assert_eq!(at(100.0, TERM_HEADER_H - 0.1), None);
        assert_eq!(at(100.0, -20.0), None);
        // Everywhere in the header, including the close button and the lamp.
        assert_eq!(at(crate::layout::TERM_CLOSE.x + 5.0, 10.0), None);
        assert_eq!(at(0.0, 0.0), None);
        // The first point below it is a cell.
        assert!(at(100.0, TERM_HEADER_H).is_some());
        // A grid with no cells, or no cell size yet, has nothing to hit.
        assert_eq!(cell_at(100.0, 100.0, 9, 18, 1.0, 0, 20), None);
        assert_eq!(cell_at(100.0, 100.0, 9, 18, 1.0, 81, 0), None);
        assert_eq!(cell_at(100.0, 100.0, 0, 18, 1.0, 81, 20), None);
        assert_eq!(cell_at(100.0, 100.0, 9, 0, 1.0, 81, 20), None);
        // A one-cell grid still works.
        assert_eq!(
            cell_at(5000.0, 5000.0, 9, 18, 1.0, 1, 1),
            Some((0, 0, true))
        );
    }

    #[test]
    fn cell_at_converts_logical_px_to_device_cells_at_each_scale() {
        // A 18 x 36 cell at 200 % is 9 x 18 logical px: the same grid as at(); the half
        // boundary is at 4.5 logical px.
        let (ox, oy) = (TERM_TEXT.x, TERM_TEXT.y);
        let f = |x: f32, y: f32| cell_at(x, y, 18, 36, 2.0, 81, 20);
        assert_eq!(f(ox + 4.4, oy), Some((0, 0, false)));
        assert_eq!(f(ox + 4.5, oy), Some((0, 0, true)));
        assert_eq!(f(ox + 9.0, oy + 18.0), Some((1, 1, false)));
        // 150 %: a 12 x 24 device cell is 8 x 16 logical px.
        let g = |x: f32, y: f32| cell_at(x, y, 12, 24, 1.5, 91, 23);
        assert_eq!(g(ox + 7.9, oy + 15.9), Some((0, 0, true)));
        assert_eq!(g(ox + 8.0, oy + 16.0), Some((1, 1, false)));
        assert_eq!(g(ox + 8.0 * 3.0 + 4.0, oy), Some((3, 0, true)));
        // A bad scale reads as 100 %.
        assert_eq!(cell_at(ox + 9.0, oy, 9, 18, 0.0, 81, 20), at(ox + 9.0, oy));
    }

    #[test]
    fn a_grid_from_grid_fit_covers_the_whole_text_area_with_cell_at() {
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let (cw, ch) = ((9.0 * scale).round() as u16, (18.0 * scale).round() as u16);
            let (cols, rows) = grid_fit(cw, ch, scale);
            // The text area's corners land in the first and last cells.
            let tl = cell_at(TERM_TEXT.x, TERM_TEXT.y, cw, ch, scale, cols, rows);
            assert_eq!(tl, Some((0, 0, false)), "@{scale}");
            let br = cell_at(
                TERM_TEXT.x + TERM_TEXT.w - 0.01,
                TERM_TEXT.y + TERM_TEXT.h - 0.01,
                cw,
                ch,
                scale,
                cols,
                rows,
            );
            let (c, r, _) = br.unwrap();
            assert_eq!(
                (c, r),
                (usize::from(cols) - 1, usize::from(rows) - 1),
                "@{scale}"
            );
        }
    }

    #[test]
    fn the_wheel_scrolls_three_lines_a_notch_with_the_sign_kept() {
        assert_eq!(wheel_lines(120), 3);
        assert_eq!(wheel_lines(-120), -3);
        assert_eq!(wheel_lines(240), 6);
        assert_eq!(wheel_lines(-360), -9);
        assert_eq!(wheel_lines(0), 0);
        // Parts of a notch are cut toward zero, the same both ways.
        assert_eq!(wheel_lines(40), 1);
        assert_eq!(wheel_lines(-40), -1);
        assert_eq!(wheel_lines(39), 0);
        assert_eq!(wheel_lines(-39), 0);
        assert_eq!(wheel_lines(60), 1);
        assert_eq!(wheel_lines(-60), -1);
        // No overflow at the extremes.
        assert_eq!(wheel_lines(i32::MAX), i32::MAX / 40);
        assert_eq!(wheel_lines(i32::MIN), i32::MIN / 40);
    }

    #[test]
    fn small_wheel_steps_add_up_with_the_accumulator() {
        // A touchpad sending 10 at a time: nothing from wheel_lines, 3 lines per 120 here.
        assert_eq!(wheel_lines(10), 0);
        let mut w = WheelAccum::default();
        let total: i32 = (0..12).map(|_| w.feed(10)).sum();
        assert_eq!(total, 3);
        // A full notch is 3 lines at once, as wheel_lines says.
        let mut w = WheelAccum::default();
        assert_eq!(w.feed(120), 3);
        assert_eq!(w.feed(-120), -3);
        // The same total whichever way the delta is split, both directions.
        for delta in [-30i32, -7, 7, 30, 45] {
            let mut w = WheelAccum::default();
            let n = 1200 / delta.abs();
            let total: i32 = (0..n).map(|_| w.feed(delta)).sum();
            assert_eq!(total, wheel_lines(delta * n), "{delta}");
        }
        // Zero changes nothing.
        let mut w = WheelAccum::default();
        assert_eq!(w.feed(20), 0);
        assert_eq!(w.feed(0), 0);
        assert_eq!(w.feed(20), 1);
        // A change of direction drops what was left over instead of cancelling it.
        let mut w = WheelAccum::default();
        assert_eq!(w.feed(30), 0);
        assert_eq!(w.feed(-30), 0);
        assert_eq!(w.feed(-30), -1);
    }
}
