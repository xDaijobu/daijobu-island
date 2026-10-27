//! The emulator's grid as a [`Screen`]: colours resolved, wide characters marked, the cursor
//! and the selection mapped to the visible rows. Everything here is a pure function over a
//! `Term`, so the tests feed it bytes with `vte::ansi::Processor` and need no PTY.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::{Cell as TermCell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape as VteShape, NamedColor};
use dj_core::color::Rgb;
use dj_core::terminal::{Cell, Cursor, CursorShape, Modes, Palette, Screen, SelSpan, Style};

/// A dim cell keeps 66 % of its text colour and takes 34 % of the background behind it.
/// (Alacritty uses the same 0.66 factor; mixing 66 % *toward* the background would leave
/// dim text such as PSReadLine's inline predictions nearly invisible.)
pub(crate) const DIM_TOWARD_BG: f32 = 0.34;

/// The xterm 6x6x6 colour cube levels.
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

// Indices into `Term::colors()` / the escape-sequence colour numbers.
const IDX_FG: usize = 256;
const IDX_BG: usize = 257;
const IDX_CURSOR: usize = 258;

fn rgb8(r: u8, g: u8, b: u8) -> Rgb {
    Rgb::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

/// xterm palette entry `idx`: 0-15 from the theme's `ansi`, 16-231 the cube, 232-255 greys.
pub(crate) fn indexed_rgb(idx: u8, palette: &Palette) -> Rgb {
    match idx {
        0..=15 => palette.ansi[idx as usize],
        16..=231 => {
            let n = (idx - 16) as usize;
            rgb8(CUBE[n / 36], CUBE[(n / 6) % 6], CUBE[n % 6])
        }
        232..=255 => {
            let v = 8 + 10 * (idx - 232);
            rgb8(v, v, v)
        }
    }
}

/// The palette's colour for an escape-sequence colour number (0-255 indexed, 256 text, 257
/// background, 258 cursor), for answering OSC 4 / 10 / 11 / 12 queries.
pub(crate) fn palette_color(index: usize, palette: &Palette) -> Option<Rgb> {
    match index {
        0..=255 => Some(indexed_rgb(index as u8, palette)),
        IDX_FG => Some(palette.fg),
        IDX_BG => Some(palette.bg),
        IDX_CURSOR => Some(palette.cursor),
        _ => None,
    }
}

/// A colour the program set with OSC 4 / 10 / 11 / 12, which wins over the theme.
fn overridden(colors: &Colors, index: usize) -> Option<Rgb> {
    colors[index].map(|c| rgb8(c.r, c.g, c.b))
}

/// `None` is the default background (nothing to paint).
fn resolve(color: Color, colors: &Colors, palette: &Palette, bright: bool) -> Option<Rgb> {
    let ansi = |i: usize| overridden(colors, i).unwrap_or(palette.ansi[i]);
    match color {
        Color::Spec(c) => Some(rgb8(c.r, c.g, c.b)),
        Color::Indexed(i) => {
            let i = if bright && i < 8 { i + 8 } else { i };
            Some(overridden(colors, i as usize).unwrap_or_else(|| indexed_rgb(i, palette)))
        }
        Color::Named(n) => match n {
            NamedColor::Black => Some(ansi(if bright { 8 } else { 0 })),
            NamedColor::Red => Some(ansi(if bright { 9 } else { 1 })),
            NamedColor::Green => Some(ansi(if bright { 10 } else { 2 })),
            NamedColor::Yellow => Some(ansi(if bright { 11 } else { 3 })),
            NamedColor::Blue => Some(ansi(if bright { 12 } else { 4 })),
            NamedColor::Magenta => Some(ansi(if bright { 13 } else { 5 })),
            NamedColor::Cyan => Some(ansi(if bright { 14 } else { 6 })),
            NamedColor::White => Some(ansi(if bright { 15 } else { 7 })),
            NamedColor::BrightBlack => Some(ansi(8)),
            NamedColor::BrightRed => Some(ansi(9)),
            NamedColor::BrightGreen => Some(ansi(10)),
            NamedColor::BrightYellow => Some(ansi(11)),
            NamedColor::BrightBlue => Some(ansi(12)),
            NamedColor::BrightMagenta => Some(ansi(13)),
            NamedColor::BrightCyan => Some(ansi(14)),
            NamedColor::BrightWhite => Some(ansi(15)),
            NamedColor::DimBlack => Some(ansi(0)),
            NamedColor::DimRed => Some(ansi(1)),
            NamedColor::DimGreen => Some(ansi(2)),
            NamedColor::DimYellow => Some(ansi(3)),
            NamedColor::DimBlue => Some(ansi(4)),
            NamedColor::DimMagenta => Some(ansi(5)),
            NamedColor::DimCyan => Some(ansi(6)),
            NamedColor::DimWhite => Some(ansi(7)),
            NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground => {
                Some(overridden(colors, IDX_FG).unwrap_or(palette.fg))
            }
            // The default background is only "something" when a program replaced it.
            NamedColor::Background => overridden(colors, IDX_BG),
            NamedColor::Cursor => Some(overridden(colors, IDX_CURSOR).unwrap_or(palette.cursor)),
        },
    }
}

/// One emulator cell as the panel shows it.
///
/// Order: bold-as-bright on ANSI 0-7 (not when dim), inverse swaps the two colours (a default
/// background becomes `palette.bg`), dim mixes the text toward the background, hidden makes the
/// text the background colour.
pub(crate) fn resolve_cell(cell: &TermCell, colors: &Colors, palette: &Palette) -> Cell {
    let flags = cell.flags;
    let bold = flags.contains(Flags::BOLD);
    let dim = flags.contains(Flags::DIM);
    let default_bg = overridden(colors, IDX_BG).unwrap_or(palette.bg);

    let mut fg = resolve(cell.fg, colors, palette, bold && !dim).unwrap_or(default_bg);
    let mut bg = resolve(cell.bg, colors, palette, false);

    if flags.contains(Flags::INVERSE) {
        let swapped_fg = bg.unwrap_or(default_bg);
        bg = Some(fg);
        fg = swapped_fg;
    }
    if dim {
        fg = fg.lerp(bg.unwrap_or(default_bg), DIM_TOWARD_BG);
    }
    if flags.contains(Flags::HIDDEN) {
        fg = bg.unwrap_or(default_bg);
    }

    let spacer = flags.contains(Flags::WIDE_CHAR_SPACER);
    // The leading spacer is the empty last cell of a row that a wide character wrapped past.
    let blank = spacer || flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) || cell.c.is_control();
    Cell {
        ch: if blank { ' ' } else { cell.c },
        fg,
        bg,
        style: Style {
            bold,
            italic: flags.contains(Flags::ITALIC),
            underline: flags.intersects(Flags::ALL_UNDERLINES),
            strike: flags.contains(Flags::STRIKEOUT),
            wide: flags.contains(Flags::WIDE_CHAR),
            spacer,
        },
    }
}

/// The visible grid, scrolled back by the display offset.
pub(crate) fn build_screen<T: EventListener>(
    term: &Term<T>,
    palette: &Palette,
    focused: bool,
) -> Screen {
    let content = term.renderable_content();
    let cols = term.columns();
    let rows = term.screen_lines();
    let offset = content.display_offset;
    let grid = term.grid();

    let mut cells = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        let line = &grid[Line(row as i32 - offset as i32)];
        for col in 0..cols {
            cells.push(resolve_cell(&line[Column(col)], content.colors, palette));
        }
    }

    // `renderable_content` already moves a cursor on a wide character's spacer onto the
    // character itself.
    let cursor = {
        let c = content.cursor;
        let row = c.point.line.0 + offset as i32;
        let shape = match c.shape {
            VteShape::Hidden => None,
            VteShape::Block if focused => Some(CursorShape::Block),
            VteShape::Block | VteShape::HollowBlock => Some(CursorShape::HollowBlock),
            VteShape::Underline => Some(CursorShape::Underline),
            VteShape::Beam => Some(CursorShape::Beam),
        };
        shape.and_then(|shape| {
            (row >= 0 && (row as usize) < rows && c.point.column.0 < cols).then_some(Cursor {
                col: c.point.column.0,
                row: row as usize,
                shape,
            })
        })
    };

    let selection = match content.selection {
        Some(range) => selection_spans(term, range.start, range.end, range.is_block, offset),
        None => Vec::new(),
    };

    Screen {
        cols,
        rows,
        cells,
        cursor,
        selection,
        offset,
        history: term.history_size(),
    }
}

/// Per visible row, the selected columns `start..end`. A span is widened so it never splits a
/// double-width character from its spacer.
fn selection_spans<T>(
    term: &Term<T>,
    start: Point,
    end: Point,
    is_block: bool,
    offset: usize,
) -> Vec<SelSpan> {
    let (cols, rows) = (term.columns(), term.screen_lines());
    let grid = term.grid();
    let mut spans = Vec::new();
    for row in 0..rows {
        let line = Line(row as i32 - offset as i32);
        if line < start.line || line > end.line {
            continue;
        }
        let (mut s, mut e) = if is_block {
            (start.column.0, end.column.0 + 1)
        } else {
            (
                if line == start.line {
                    start.column.0
                } else {
                    0
                },
                if line == end.line {
                    end.column.0 + 1
                } else {
                    cols
                },
            )
        };
        e = e.min(cols);
        if s >= e {
            continue;
        }
        let cells = &grid[line];
        if s > 0 && cells[Column(s)].flags.contains(Flags::WIDE_CHAR_SPACER) {
            s -= 1;
        }
        if e < cols && cells[Column(e - 1)].flags.contains(Flags::WIDE_CHAR) {
            e += 1;
        }
        spans.push(SelSpan {
            row,
            start: s,
            end: e,
        });
    }
    spans
}

/// The mode flags that change what a key or a paste sends.
pub(crate) fn modes_of<T>(term: &Term<T>) -> Modes {
    let m = *term.mode();
    Modes {
        app_cursor: m.contains(TermMode::APP_CURSOR),
        app_keypad: m.contains(TermMode::APP_KEYPAD),
        bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
    }
}

/// A visible (col, row) as a grid point, clamped into the viewport.
fn point_at<T>(term: &Term<T>, col: usize, row: usize) -> Point {
    let col = col.min(term.columns().saturating_sub(1));
    let row = row.min(term.screen_lines().saturating_sub(1));
    let offset = term.grid().display_offset() as i32;
    Point::new(Line(row as i32 - offset), Column(col))
}

fn side(right_half: bool) -> Side {
    if right_half {
        Side::Right
    } else {
        Side::Left
    }
}

pub(crate) fn select_start<T>(
    term: &mut Term<T>,
    col: usize,
    row: usize,
    right_half: bool,
    word: bool,
) {
    let ty = if word {
        SelectionType::Semantic
    } else {
        SelectionType::Simple
    };
    let point = point_at(term, col, row);
    term.selection = Some(Selection::new(ty, point, side(right_half)));
}

pub(crate) fn select_update<T>(term: &mut Term<T>, col: usize, row: usize, right_half: bool) {
    let point = point_at(term, col, row);
    if let Some(sel) = term.selection.as_mut() {
        sel.update(point, side(right_half));
    }
}

/// `true` for a non-empty selection: a click without a drag selects nothing.
pub(crate) fn has_selection<T>(term: &Term<T>) -> bool {
    term.selection
        .as_ref()
        .and_then(|s| s.to_range(term))
        .is_some()
}
