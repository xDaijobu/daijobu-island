//! The terminal panel: a real shell inside the island, like VS Code's integrated terminal.
//!
//! This module is pure. It holds the data that crosses crates: `dj-term` (the emulator and
//! the ConPTY) fills a [`Screen`] using a [`Palette`]; `dj-render` draws it; the app maps
//! keys and pastes to bytes with the helpers here. Nothing here touches Win32 or Skia.

use crate::color::Rgb;

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
