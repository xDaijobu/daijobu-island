//! Headless tests. The emulator tests feed bytes straight into a `Term` with
//! `vte::ansi::Processor` (no PTY); two tests start a real `cmd.exe` in a ConPTY (hidden: a
//! ConPTY never shows a window).

use std::os::windows::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener, VoidListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::{Osc52, Term};
use alacritty_terminal::vte::ansi::Processor;
use dj_core::color::Rgb;
use dj_core::terminal::{Cell, CursorShape, Palette, Screen};

use crate::listener::{clean_title, Listener, Shared, TermEvent};
use crate::screen::{self, build_screen, DIM_TOWARD_BG};
use crate::session::{clamp_dims, term_config, window_size, Size};
use crate::shell::{locate_in, pick_cwd, quote_program, resolve_with};
use crate::{resolve_shell, Session, SpawnOptions};

// ---------------------------------------------------------------- fixtures

fn rgb8(r: u8, g: u8, b: u8) -> Rgb {
    Rgb::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

/// Every ANSI slot is distinct, so a wrong mapping cannot pass by accident.
fn pal() -> Palette {
    let mut ansi = [Rgb::new(0.0, 0.0, 0.0); 16];
    for (i, c) in ansi.iter_mut().enumerate() {
        *c = rgb8(16 * i as u8, 255 - 5 * i as u8, 7 * i as u8 + 1);
    }
    Palette {
        fg: rgb8(200, 201, 202),
        bg: rgb8(10, 11, 12),
        cursor: rgb8(250, 0, 250),
        selection: rgb8(250, 0, 250),
        ansi,
    }
}

fn u8s(c: Rgb) -> [u8; 3] {
    c.to_u8()
}

/// A `Term` and the parser that feeds it, like the PTY thread does.
struct Fed<T: EventListener> {
    term: Term<T>,
    parser: Processor,
}

impl<T: EventListener> Fed<T> {
    fn with(term: Term<T>) -> Self {
        Fed {
            term,
            parser: Processor::default(),
        }
    }

    fn feed(&mut self, s: &str) {
        self.parser.advance(&mut self.term, s.as_bytes());
    }

    fn screen(&self) -> Screen {
        build_screen(&self.term, &pal(), true)
    }
}

fn fed(cols: usize, rows: usize) -> Fed<VoidListener> {
    Fed::with(Term::new(
        term_config(100),
        &Size { cols, rows },
        VoidListener,
    ))
}

fn cell(s: &Screen, col: usize, row: usize) -> Cell {
    *s.cell(col, row).expect("cell in range")
}

fn row_text(s: &Screen, row: usize) -> String {
    (0..s.cols)
        .map(|c| cell(s, c, row).ch)
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn text(s: &Screen) -> String {
    (0..s.rows)
        .map(|r| row_text(s, r))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------- colours

#[test]
fn named_colours_normal_bright_and_bold() {
    let mut t = fed(20, 3);
    t.feed("\x1b[31mA\x1b[91mB\x1b[1;31mC\x1b[0mD\x1b[1mE");
    let s = t.screen();
    let p = pal();
    assert_eq!(u8s(cell(&s, 0, 0).fg), u8s(p.ansi[1]), "31 is ansi 1");
    assert_eq!(u8s(cell(&s, 1, 0).fg), u8s(p.ansi[9]), "91 is ansi 9");
    let c = cell(&s, 2, 0);
    assert_eq!(u8s(c.fg), u8s(p.ansi[9]), "bold 31 is bright red");
    assert!(c.style.bold, "bold keeps the heavier face too");
    assert_eq!(u8s(cell(&s, 3, 0).fg), u8s(p.fg), "reset is the default fg");
    let e = cell(&s, 4, 0);
    assert_eq!(u8s(e.fg), u8s(p.fg), "bold on the default fg stays default");
    assert!(e.style.bold);
    assert!(
        (0..5).all(|c| cell(&s, c, 0).bg.is_none()),
        "no background was set"
    );
}

#[test]
fn bold_brightens_only_ansi_0_to_7_and_not_when_dim() {
    let mut t = fed(20, 3);
    // bold + indexed 1 -> 9; bold + indexed 9 stays 9; bold + indexed 20 (cube) unchanged;
    // bold + dim does not brighten.
    t.feed("\x1b[1;38;5;1mA\x1b[1;38;5;9mB\x1b[1;38;5;20mC\x1b[0;1;2;31mD");
    let s = t.screen();
    let p = pal();
    assert_eq!(u8s(cell(&s, 0, 0).fg), u8s(p.ansi[9]));
    assert_eq!(u8s(cell(&s, 1, 0).fg), u8s(p.ansi[9]));
    assert_eq!(
        u8s(cell(&s, 2, 0).fg),
        [0, 0, 215],
        "indexed 20 is cube (0,0,4)"
    );
    let d = cell(&s, 3, 0);
    assert_eq!(
        u8s(d.fg),
        u8s(p.ansi[1].lerp(p.bg, DIM_TOWARD_BG)),
        "bold+dim is dim red, not bright red"
    );
}

#[test]
fn indexed_colours_cube_and_grey_ramp() {
    let mut t = fed(30, 3);
    t.feed("\x1b[38;5;16mA\x1b[38;5;196mB\x1b[38;5;67mC\x1b[38;5;231mD");
    t.feed("\x1b[38;5;232mE\x1b[38;5;255mF\x1b[38;5;5mG\x1b[38;5;12mH\x1b[48;5;21mI");
    let s = t.screen();
    let p = pal();
    assert_eq!(u8s(cell(&s, 0, 0).fg), [0, 0, 0], "16 is the cube origin");
    assert_eq!(u8s(cell(&s, 1, 0).fg), [255, 0, 0], "196");
    // 67 = 16 + 51: r = 51/36 = 1 (95), g = (51/6)%6 = 2 (135), b = 51%6 = 3 (175).
    assert_eq!(u8s(cell(&s, 2, 0).fg), [95, 135, 175], "67");
    assert_eq!(
        u8s(cell(&s, 3, 0).fg),
        [255, 255, 255],
        "231 is the cube end"
    );
    assert_eq!(u8s(cell(&s, 4, 0).fg), [8, 8, 8], "232 is the grey start");
    assert_eq!(
        u8s(cell(&s, 5, 0).fg),
        [238, 238, 238],
        "255 is the grey end"
    );
    assert_eq!(
        u8s(cell(&s, 6, 0).fg),
        u8s(p.ansi[5]),
        "indexed 5 is the theme's"
    );
    assert_eq!(
        u8s(cell(&s, 7, 0).fg),
        u8s(p.ansi[12]),
        "indexed 12 is the theme's"
    );
    assert_eq!(
        cell(&s, 8, 0).bg.map(u8s),
        Some([0, 0, 255]),
        "48;5;21 is the cube's blue"
    );
}

#[test]
fn truecolor_is_passed_through() {
    let mut t = fed(20, 3);
    t.feed("\x1b[38;2;12;34;56;48;2;200;100;50mA");
    let c = cell(&t.screen(), 0, 0);
    assert_eq!(u8s(c.fg), [12, 34, 56]);
    assert_eq!(c.bg.map(u8s), Some([200, 100, 50]));
}

#[test]
fn programs_can_override_colours_with_osc() {
    let mut t = fed(20, 3);
    // OSC 4 changes ansi 1 and indexed 200; OSC 11 changes the default background.
    t.feed("\x1b]4;1;rgb:ff/00/00\x1b\\\x1b]4;200;rgb:01/02/03\x07");
    t.feed("\x1b[31mA\x1b[38;5;1mB\x1b[38;5;200mC");
    let s = t.screen();
    assert_eq!(
        u8s(cell(&s, 0, 0).fg),
        [255, 0, 0],
        "OSC 4 wins over the theme"
    );
    assert_eq!(u8s(cell(&s, 1, 0).fg), [255, 0, 0]);
    assert_eq!(u8s(cell(&s, 2, 0).fg), [1, 2, 3]);
    assert_eq!(
        cell(&s, 3, 0).bg,
        None,
        "no default-background override yet"
    );

    t.feed("\x1b[0m\x1b]11;rgb:11/22/33\x07D");
    let d = cell(&t.screen(), 3, 0);
    assert_eq!(
        d.bg.map(u8s),
        Some([0x11, 0x22, 0x33]),
        "OSC 11 replaces the bg"
    );
    t.feed("\x1b]104;1\x07\x1b]111\x07\x1b[31mE");
    let e = cell(&t.screen(), 4, 0);
    assert_eq!(
        u8s(e.fg),
        u8s(pal().ansi[1]),
        "OSC 104 resets back to the theme"
    );
    assert_eq!(e.bg, None, "OSC 111 resets the background");
}

#[test]
fn inverse_swaps_and_a_default_background_becomes_the_panel_colour() {
    let mut t = fed(20, 3);
    t.feed("\x1b[7mA\x1b[0;31;44;7mB\x1b[0;42;7mC");
    let s = t.screen();
    let p = pal();
    let a = cell(&s, 0, 0);
    assert_eq!(
        u8s(a.fg),
        u8s(p.bg),
        "inverse default: text takes palette.bg"
    );
    assert_eq!(a.bg.map(u8s), Some(u8s(p.fg)));
    let b = cell(&s, 1, 0);
    assert_eq!(u8s(b.fg), u8s(p.ansi[4]));
    assert_eq!(b.bg.map(u8s), Some(u8s(p.ansi[1])));
    let c = cell(&s, 2, 0);
    assert_eq!(u8s(c.fg), u8s(p.ansi[2]));
    assert_eq!(c.bg.map(u8s), Some(u8s(p.fg)));
}

#[test]
fn dim_mixes_the_text_toward_the_background() {
    let mut t = fed(20, 3);
    t.feed("\x1b[2mA\x1b[0;2;44mB");
    let s = t.screen();
    let p = pal();
    assert_eq!(u8s(cell(&s, 0, 0).fg), u8s(p.fg.lerp(p.bg, DIM_TOWARD_BG)));
    assert_eq!(
        u8s(cell(&s, 1, 0).fg),
        u8s(p.fg.lerp(p.ansi[4], DIM_TOWARD_BG)),
        "dim mixes toward the cell's own background"
    );
    // Dim text must stay readable: it keeps most of its own colour.
    const { assert!(DIM_TOWARD_BG > 0.0 && DIM_TOWARD_BG < 0.5) };
}

#[test]
fn hidden_text_takes_the_background_colour() {
    let mut t = fed(20, 3);
    t.feed("\x1b[8mA\x1b[0;44;8mB");
    let s = t.screen();
    let p = pal();
    assert_eq!(u8s(cell(&s, 0, 0).fg), u8s(p.bg));
    assert_eq!(u8s(cell(&s, 1, 0).fg), u8s(p.ansi[4]));
}

#[test]
fn text_styles() {
    let mut t = fed(20, 3);
    t.feed("\x1b[1mB\x1b[0;3mI\x1b[0;4mU\x1b[0;9mS\x1b[0;4:3mC\x1b[0mN");
    let s = t.screen();
    assert!(cell(&s, 0, 0).style.bold);
    assert!(cell(&s, 1, 0).style.italic);
    assert!(cell(&s, 2, 0).style.underline);
    assert!(cell(&s, 3, 0).style.strike);
    assert!(cell(&s, 4, 0).style.underline, "undercurl still underlines");
    assert_eq!(cell(&s, 5, 0).style, Default::default());
}

#[test]
fn control_characters_in_cells_draw_as_blanks() {
    let mut t = fed(20, 3);
    t.feed("a\tb");
    let s = t.screen();
    assert!(
        (0..s.cols).all(|c| !cell(&s, c, 0).ch.is_control()),
        "a tab is stored as a control character in the grid; the panel must not see it"
    );
    assert_eq!(cell(&s, 8, 0).ch, 'b');
}

// ---------------------------------------------------------------- wide characters

#[test]
fn wide_char_marks_the_cell_and_its_spacer() {
    let mut t = fed(20, 3);
    t.feed("a\u{4f60}b");
    let s = t.screen();
    assert_eq!(cell(&s, 0, 0).ch, 'a');
    let wide = cell(&s, 1, 0);
    assert_eq!(wide.ch, '\u{4f60}');
    assert!(wide.style.wide && !wide.style.spacer);
    let spacer = cell(&s, 2, 0);
    assert_eq!(spacer.ch, ' ');
    assert!(spacer.style.spacer && !spacer.style.wide);
    assert_eq!(cell(&s, 3, 0).ch, 'b');
    let cur = s.cursor.expect("cursor");
    assert_eq!(
        (cur.col, cur.row),
        (4, 0),
        "the cursor is after the 3 cells used"
    );
}

#[test]
fn cursor_on_a_wide_chars_second_half_is_reported_on_the_character() {
    let mut t = fed(20, 3);
    // After the wide char the cursor is at column 2; CSI 1 D moves it onto the spacer.
    t.feed("\u{4f60}\x1b[1D");
    let s = t.screen();
    let cur = s.cursor.expect("cursor");
    assert_eq!(
        cur.col, 0,
        "a cursor on the spacer is drawn on the wide cell"
    );
    assert!(cell(&s, cur.col, cur.row).style.wide);
}

#[test]
fn wide_char_that_does_not_fit_wraps_and_leaves_a_blank_not_a_spacer() {
    let mut t = fed(5, 3);
    t.feed("abcd\u{4f60}");
    let s = t.screen();
    let gap = cell(&s, 4, 0);
    assert_eq!(gap.ch, ' ');
    assert!(
        !gap.style.spacer && !gap.style.wide,
        "the end-of-row gap is just blank"
    );
    assert!(cell(&s, 0, 1).style.wide);
    assert!(cell(&s, 1, 1).style.spacer);
    assert_eq!(
        s.cursor.map(|c| (c.col, c.row)),
        Some((2, 1)),
        "the cursor follows the character onto the next row"
    );
}

#[test]
fn zero_width_combining_marks_are_dropped() {
    let mut t = fed(20, 3);
    t.feed("e\u{301}x");
    let s = t.screen();
    assert_eq!(cell(&s, 0, 0).ch, 'e');
    assert_eq!(cell(&s, 1, 0).ch, 'x', "the accent takes no cell");
}

// ---------------------------------------------------------------- cursor and modes

#[test]
fn cursor_position_visibility_and_shape() {
    let mut t = fed(20, 5);
    t.feed("abc\r\ndef");
    let cur = t.screen().cursor.expect("cursor");
    assert_eq!((cur.col, cur.row, cur.shape), (3, 1, CursorShape::Block));

    t.feed("\x1b[?25l");
    assert_eq!(t.screen().cursor, None, "ESC[?25l hides it");
    t.feed("\x1b[?25h");
    assert!(t.screen().cursor.is_some());

    let hollow = build_screen(&t.term, &pal(), false).cursor.expect("cursor");
    assert_eq!(
        hollow.shape,
        CursorShape::HollowBlock,
        "unfocused block is hollow"
    );

    t.feed("\x1b[5 q");
    assert_eq!(t.screen().cursor.unwrap().shape, CursorShape::Beam);
    assert_eq!(
        build_screen(&t.term, &pal(), false).cursor.unwrap().shape,
        CursorShape::Beam,
        "only a block becomes hollow"
    );
    t.feed("\x1b[4 q");
    assert_eq!(t.screen().cursor.unwrap().shape, CursorShape::Underline);
    t.feed("\x1b[2 q");
    assert_eq!(t.screen().cursor.unwrap().shape, CursorShape::Block);
}

#[test]
fn modes_follow_the_escape_sequences() {
    let mut t = fed(20, 3);
    let m = screen::modes_of(&t.term);
    assert!(!m.app_cursor && !m.app_keypad && !m.bracketed_paste);
    t.feed("\x1b[?1h\x1b[?2004h\x1b=");
    let m = screen::modes_of(&t.term);
    assert!(m.app_cursor && m.app_keypad && m.bracketed_paste);
    t.feed("\x1b[?1l\x1b[?2004l\x1b>");
    let m = screen::modes_of(&t.term);
    assert!(!m.app_cursor && !m.app_keypad && !m.bracketed_paste);
}

// ---------------------------------------------------------------- scrollback

fn numbered(rows: usize, count: usize) -> Fed<VoidListener> {
    let mut t = fed(20, rows);
    let lines: Vec<String> = (1..=count).map(|i| format!("L{i}")).collect();
    t.feed(&lines.join("\r\n"));
    t
}

#[test]
fn scrollback_offset_history_and_cursor() {
    let mut t = numbered(5, 12);
    let s = t.screen();
    assert_eq!((s.offset, s.history), (0, 7));
    assert_eq!(row_text(&s, 0), "L8");
    assert_eq!(row_text(&s, 4), "L12");
    assert_eq!(s.cursor.map(|c| c.row), Some(4));

    t.term.scroll_display(Scroll::Delta(3));
    let s = t.screen();
    assert_eq!((s.offset, s.history), (3, 7));
    assert_eq!(row_text(&s, 0), "L5");
    assert_eq!(row_text(&s, 4), "L9");
    assert_eq!(s.cursor, None, "the cursor row scrolled out of view");

    t.term.scroll_display(Scroll::Delta(100));
    assert_eq!(t.screen().offset, 7, "clamped to the history");
    assert_eq!(row_text(&t.screen(), 0), "L1");
    t.term.scroll_display(Scroll::Delta(-2));
    assert_eq!(t.screen().offset, 5);
    t.term.scroll_display(Scroll::Bottom);
    assert_eq!(t.screen().offset, 0);
    assert!(t.screen().cursor.is_some());
}

#[test]
fn scrollback_zero_keeps_no_history() {
    let mut t = Fed::with(Term::new(
        term_config(0),
        &Size { cols: 20, rows: 3 },
        VoidListener,
    ));
    t.feed("1\r\n2\r\n3\r\n4\r\n5");
    assert_eq!(t.screen().history, 0);
}

// ---------------------------------------------------------------- selection

#[test]
fn selection_spans_and_text_on_one_row() {
    let mut t = fed(20, 5);
    t.feed("hello world\r\nsecond line");
    screen::select_start(&mut t.term, 0, 0, false, false);
    screen::select_update(&mut t.term, 4, 0, true);
    assert!(screen::has_selection(&t.term));
    let s = t.screen();
    assert_eq!(s.selection.len(), 1);
    assert_eq!(
        (s.selection[0].row, s.selection[0].start, s.selection[0].end),
        (0, 0, 5)
    );
    assert_eq!(t.term.selection_to_string().as_deref(), Some("hello"));
}

#[test]
fn selection_across_rows_runs_to_the_row_end_and_from_the_row_start() {
    let mut t = fed(20, 5);
    t.feed("hello world\r\nsecond line");
    screen::select_start(&mut t.term, 6, 0, false, false);
    screen::select_update(&mut t.term, 5, 1, true);
    let s = t.screen();
    let spans: Vec<_> = s
        .selection
        .iter()
        .map(|x| (x.row, x.start, x.end))
        .collect();
    assert_eq!(spans, vec![(0, 6, 20), (1, 0, 6)]);
    assert_eq!(
        t.term.selection_to_string().as_deref(),
        Some("world\nsecond")
    );
}

#[test]
fn dragging_backwards_selects_the_same_cells() {
    let mut t = fed(20, 5);
    t.feed("hello world");
    screen::select_start(&mut t.term, 4, 0, true, false);
    screen::select_update(&mut t.term, 0, 0, false);
    let s = t.screen();
    assert_eq!((s.selection[0].start, s.selection[0].end), (0, 5));
    assert_eq!(t.term.selection_to_string().as_deref(), Some("hello"));
}

#[test]
fn double_click_selects_a_word() {
    let mut t = fed(20, 5);
    t.feed("hello world");
    screen::select_start(&mut t.term, 7, 0, false, true);
    let s = t.screen();
    assert_eq!((s.selection[0].start, s.selection[0].end), (6, 11));
    assert_eq!(t.term.selection_to_string().as_deref(), Some("world"));
}

#[test]
fn a_click_without_a_drag_selects_nothing() {
    let mut t = fed(20, 5);
    t.feed("hello");
    screen::select_start(&mut t.term, 2, 0, false, false);
    assert!(
        !screen::has_selection(&t.term),
        "Ctrl+C must still reach the shell"
    );
    assert!(t.screen().selection.is_empty());
    assert_eq!(t.term.selection_to_string(), None);
    // A drag within the same cell, left half to right half, is one cell.
    screen::select_update(&mut t.term, 2, 0, true);
    assert!(screen::has_selection(&t.term));
    t.term.selection = None;
    assert!(!screen::has_selection(&t.term));
    assert!(t.screen().selection.is_empty());
}

#[test]
fn selection_never_splits_a_wide_character() {
    let mut t = fed(20, 3);
    t.feed("a\u{4f60}b");
    // Only the spacer half is touched: the span grows to include the character.
    screen::select_start(&mut t.term, 2, 0, false, false);
    screen::select_update(&mut t.term, 2, 0, true);
    let s = t.screen();
    assert_eq!((s.selection[0].start, s.selection[0].end), (1, 3));
    assert_eq!(t.term.selection_to_string().as_deref(), Some("\u{4f60}"));
    // Only the character half is touched: the span grows to include the spacer.
    screen::select_start(&mut t.term, 1, 0, false, false);
    screen::select_update(&mut t.term, 1, 0, true);
    let s = t.screen();
    assert_eq!((s.selection[0].start, s.selection[0].end), (1, 3));
}

#[test]
fn selection_in_a_scrolled_view_uses_the_visible_rows() {
    let mut t = numbered(5, 12);
    t.term.scroll_display(Scroll::Delta(3));
    // The top visible row is now "L5".
    screen::select_start(&mut t.term, 0, 0, false, false);
    screen::select_update(&mut t.term, 1, 0, true);
    let s = t.screen();
    assert_eq!(
        (s.selection[0].row, s.selection[0].start, s.selection[0].end),
        (0, 0, 2)
    );
    assert_eq!(t.term.selection_to_string().as_deref(), Some("L5"));
    // Back at the bottom the same selection is off screen (history), so nothing is painted.
    t.term.scroll_display(Scroll::Bottom);
    assert!(t.screen().selection.is_empty());
}

#[test]
fn selection_points_are_clamped_into_the_view() {
    let mut t = fed(10, 3);
    t.feed("hello");
    screen::select_start(&mut t.term, 0, 0, false, false);
    // A drag that left the text area reports cells past the edge.
    screen::select_update(&mut t.term, 999, 999, true);
    let s = t.screen();
    assert!(!s.selection.is_empty());
    assert!(s.selection.iter().all(|x| x.row < 3 && x.end <= 10));
}

// ---------------------------------------------------------------- events and replies

struct Wired {
    t: Fed<Listener>,
    shared: Arc<Shared>,
    wakes: Arc<AtomicUsize>,
    written: Arc<Mutex<Vec<u8>>>,
}

fn wired(cols: usize, rows: usize) -> Wired {
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let size = window_size(cols as u16, rows as u16, 8, 16);
    let shared = Arc::new(Shared::new(
        Box::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }),
        size,
    ));
    let written = Arc::new(Mutex::new(Vec::new()));
    let sink = written.clone();
    let _ = shared
        .reply
        .set(Box::new(move |b| sink.lock().unwrap().extend(b)));
    let term = Term::new(
        term_config(100),
        &Size { cols, rows },
        Listener(shared.clone()),
    );
    Wired {
        t: Fed::with(term),
        shared,
        wakes,
        written,
    }
}

impl Wired {
    fn wakes(&self) -> usize {
        self.wakes.load(Ordering::SeqCst)
    }
    fn written(&self) -> Vec<u8> {
        std::mem::take(&mut *self.written.lock().unwrap())
    }
}

#[test]
fn title_bell_and_reset_title_events() {
    let mut w = wired(20, 5);
    w.t.feed("\x1b]0;my title\x07");
    assert_eq!(
        w.shared.take_events(),
        vec![TermEvent::Title("my title".into())]
    );
    w.t.feed("\x1b]2;second\x07\x07");
    assert_eq!(
        w.shared.take_events(),
        vec![TermEvent::Title("second".into()), TermEvent::Bell]
    );
    assert!(w.shared.take_events().is_empty());
    Listener(w.shared.clone()).send_event(Event::ResetTitle);
    assert_eq!(w.shared.take_events(), vec![TermEvent::ResetTitle]);
}

#[test]
fn titles_lose_their_control_characters() {
    assert_eq!(clean_title("a\u{1b}b\nc\u{7}"), "abc");
    assert_eq!(clean_title("plain \u{4f60}"), "plain \u{4f60}");
}

#[test]
fn wake_is_called_once_until_take_events_rearms_it() {
    let mut w = wired(20, 5);
    assert_eq!(w.wakes(), 0);
    w.t.feed("\x1b]0;a\x07\x07\x07\x1b]0;b\x07");
    assert_eq!(w.wakes(), 1, "four events, one wake");
    w.t.feed("\x07");
    assert_eq!(w.wakes(), 1, "still armed: the UI has not drained yet");
    Listener(w.shared.clone()).send_event(Event::Wakeup);
    assert_eq!(w.wakes(), 1, "output wakes are coalesced with the rest");

    assert_eq!(w.shared.take_events().len(), 5);
    w.t.feed("\x07");
    assert_eq!(
        w.wakes(),
        2,
        "take_events cleared the flag, so the next event wakes"
    );
    Listener(w.shared.clone()).send_event(Event::Wakeup);
    assert_eq!(w.wakes(), 2);
    let _ = w.shared.take_events();
    Listener(w.shared.clone()).send_event(Event::Wakeup);
    assert_eq!(w.wakes(), 3, "output alone wakes too");
}

#[test]
fn no_wake_after_close() {
    let w = wired(20, 5);
    w.shared.close();
    Listener(w.shared.clone()).send_event(Event::Wakeup);
    assert_eq!(w.wakes(), 0);
}

#[test]
fn exited_is_reported_once_with_the_code() {
    let w = wired(20, 5);
    let l = Listener(w.shared.clone());
    l.send_event(Event::ChildExit(ExitStatus::from_raw(3)));
    l.send_event(Event::Exit);
    l.send_event(Event::ChildExit(ExitStatus::from_raw(9)));
    assert_eq!(w.shared.take_events(), vec![TermEvent::Exited(Some(3))]);

    let w = wired(20, 5);
    Listener(w.shared.clone()).send_event(Event::Exit);
    assert_eq!(w.shared.take_events(), vec![TermEvent::Exited(None)]);
}

#[test]
fn the_emulator_answers_status_queries_through_the_reply_channel() {
    let mut w = wired(20, 5);
    w.t.feed("ab\x1b[6n");
    assert_eq!(w.written(), b"\x1b[1;3R", "cursor position report");
    w.t.feed("\x1b[c");
    assert_eq!(w.written(), b"\x1b[?6c", "primary device attributes");
    w.t.feed("\x1b[18t");
    assert_eq!(w.written(), b"\x1b[8;5;20t", "size in cells");
    w.t.feed("\x1b[14t");
    assert_eq!(
        w.written(),
        b"\x1b[4;80;160t",
        "size in pixels: 5x16 by 20x8"
    );
    // Replies are not events for the UI.
    assert!(w.shared.take_events().is_empty());
}

#[test]
fn colour_queries_are_answered_from_the_last_palette() {
    let mut w = wired(20, 5);
    w.t.feed("\x1b]11;?\x07");
    assert!(w.written().is_empty(), "no palette yet: no guess");

    w.shared.set_palette(&pal());
    w.t.feed("\x1b]11;?\x07");
    assert_eq!(w.written(), b"\x1b]11;rgb:0a0a/0b0b/0c0c\x07", "background");
    w.t.feed("\x1b]10;?\x07");
    assert_eq!(w.written(), b"\x1b]10;rgb:c8c8/c9c9/caca\x07", "foreground");
    w.t.feed("\x1b]4;1;?\x07");
    let [r, g, b] = pal().ansi[1].to_u8();
    assert_eq!(
        w.written(),
        format!("\x1b]4;1;rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x07").into_bytes()
    );
}

#[test]
fn osc_52_clipboard_access_is_disabled() {
    assert_eq!(term_config(10).osc52, Osc52::Disabled);
    let mut w = wired(20, 5);
    w.t.feed("\x1b]52;c;aGVsbG8=\x07");
    assert!(
        w.shared.take_events().is_empty(),
        "no clipboard store event"
    );
    w.t.feed("\x1b]52;c;?\x07");
    assert!(w.written().is_empty(), "no clipboard read either");
    Listener(w.shared.clone()).send_event(Event::ClipboardStore(
        alacritty_terminal::term::ClipboardType::Clipboard,
        "x".into(),
    ));
    assert!(
        w.shared.take_events().is_empty(),
        "and the listener would ignore one anyway"
    );
}

// ---------------------------------------------------------------- sizes

#[test]
fn sizes_are_clamped() {
    assert_eq!(clamp_dims(0, 0), (2, 1));
    assert_eq!(clamp_dims(80, 24), (80, 24));
    assert_eq!(clamp_dims(u16::MAX, u16::MAX), (500, 500));
    let w = window_size(80, 24, 0, u16::MAX);
    assert_eq!((w.cell_width, w.cell_height), (1, 100));
    // 500 rows of 100 px still fit the emulator's u16 arithmetic.
    assert!(500u32 * 100 <= u16::MAX as u32);
}

#[test]
fn term_resize_follows_the_clamped_size() {
    let mut t = fed(20, 5);
    t.feed("hello");
    let (c, r) = clamp_dims(1, 0);
    t.term.resize(Size {
        cols: c as usize,
        rows: r as usize,
    });
    let s = t.screen();
    assert_eq!((s.cols, s.rows), (2, 1));
    assert_eq!(s.cells.len(), 2);
    assert_eq!(t.term.columns(), 2);
}

// ---------------------------------------------------------------- shell resolution

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn resolve_shell_words_and_paths() {
    let ps = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
    // Auto: pwsh when it is on PATH, else Windows PowerShell; both quiet.
    assert_eq!(
        resolve_with("", &[], true, ps),
        ("pwsh.exe".to_string(), strs(&["-NoLogo"]))
    );
    assert_eq!(
        resolve_with("", &[], false, ps),
        (ps.to_string(), strs(&["-NoLogo"]))
    );
    // Args given: used as given, no -NoLogo added.
    assert_eq!(
        resolve_with("", &strs(&["-NoProfile"]), true, ps),
        ("pwsh.exe".to_string(), strs(&["-NoProfile"]))
    );
    // Words, in any case, with whitespace around them.
    assert_eq!(resolve_with("pwsh", &[], false, ps).0, "pwsh.exe");
    assert_eq!(resolve_with(" PowerShell ", &[], true, ps).0, ps);
    assert_eq!(
        resolve_with("cmd", &[], true, ps),
        ("cmd.exe".to_string(), vec![])
    );
    assert_eq!(
        resolve_with("CMD", &strs(&["/k", "echo hi"]), true, ps),
        ("cmd.exe".to_string(), strs(&["/k", "echo hi"]))
    );
    assert_eq!(
        resolve_with("wsl", &strs(&["-d", "Ubuntu"]), true, ps),
        ("wsl.exe".to_string(), strs(&["-d", "Ubuntu"]))
    );
    // Anything else is used as given, args untouched (no -NoLogo even for a PowerShell path).
    assert_eq!(
        resolve_with(
            r"C:\Program Files\Git\bin\bash.exe",
            &strs(&["-l"]),
            true,
            ps
        ),
        (
            r"C:\Program Files\Git\bin\bash.exe".to_string(),
            strs(&["-l"])
        )
    );
    assert_eq!(
        resolve_with("nu", &[], true, ps),
        ("nu".to_string(), Vec::<String>::new())
    );
}

#[test]
fn resolve_shell_on_this_machine_picks_a_powershell() {
    let (program, args) = resolve_shell("", &[]);
    let lower = program.to_ascii_lowercase();
    assert!(
        lower.ends_with("pwsh.exe") || lower.ends_with("powershell.exe"),
        "got {program}"
    );
    assert_eq!(args, strs(&["-NoLogo"]));
    assert_eq!(resolve_shell("cmd", &[]).0, "cmd.exe");
}

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dj-term-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn locate_program_finds_files_like_createprocess() {
    let dir = scratch_dir("locate");
    std::fs::write(dir.join("fake-shell.exe"), b"").unwrap();
    std::fs::write(dir.join("tool.cmd"), b"").unwrap();
    std::fs::create_dir_all(dir.join("folder.exe")).unwrap();
    let dirs = [dir.clone()];

    assert_eq!(
        locate_in("fake-shell", &dirs),
        Some(dir.join("fake-shell.exe"))
    );
    assert_eq!(
        locate_in("fake-shell.exe", &dirs),
        Some(dir.join("fake-shell.exe"))
    );
    assert_eq!(locate_in("tool.cmd", &dirs), Some(dir.join("tool.cmd")));
    assert_eq!(locate_in("tool", &dirs), None, "no extension means .exe");
    assert_eq!(
        locate_in("folder", &dirs),
        None,
        "a directory is not a program"
    );
    assert_eq!(locate_in("missing", &dirs), None);
    assert_eq!(locate_in("", &dirs), None);

    let full = dir.join("fake-shell.exe");
    assert_eq!(locate_in(&full.to_string_lossy(), &[]), Some(full.clone()));
    let no_ext = dir.join("fake-shell");
    assert_eq!(
        locate_in(&no_ext.to_string_lossy(), &[]),
        Some(full.clone())
    );
    let quoted = format!("\"{}\"", full.to_string_lossy());
    assert_eq!(locate_in(&quoted, &[]), Some(full));
    assert_eq!(locate_in(r"C:\definitely\missing\shell.exe", &dirs), None);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn locate_program_finds_cmd_in_system32() {
    let found = crate::shell::locate_program("cmd.exe").expect("cmd.exe");
    assert!(found.is_file());
    assert!(crate::shell::locate_program("dj-term-no-such-program-xyz").is_none());
}

#[test]
fn programs_with_spaces_are_quoted_for_the_command_line() {
    assert_eq!(
        quote_program(r"C:\Program Files\PowerShell\7\pwsh.exe"),
        "\"C:\\Program Files\\PowerShell\\7\\pwsh.exe\""
    );
    assert_eq!(
        quote_program(r"C:\Windows\System32\cmd.exe"),
        r"C:\Windows\System32\cmd.exe"
    );
    assert_eq!(quote_program("\"C:\\a b\\x.exe\""), "\"C:\\a b\\x.exe\"");
}

#[test]
fn working_directory_falls_back_to_home() {
    let dir = scratch_dir("cwd");
    assert_eq!(pick_cwd(Some(&dir)), Some(dir.clone()));
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    assert_eq!(pick_cwd(Some(&dir.join("missing"))), home);
    assert_eq!(pick_cwd(None), home);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- real ConPTY

fn opts(program: &str, args: &[&str]) -> SpawnOptions {
    SpawnOptions {
        program: program.to_string(),
        args: strs(args),
        cwd: None,
        cols: 80,
        rows: 24,
        cell_w: 8,
        cell_h: 16,
        scrollback: 1000,
    }
}

/// Takes events and the screen after every wake until `done` or the timeout. Returns the
/// last screen and every event seen.
fn wait_for(
    session: &mut Session,
    rx: &Receiver<()>,
    timeout: Duration,
    mut done: impl FnMut(&Screen, &[TermEvent]) -> bool,
) -> (Screen, Vec<TermEvent>) {
    let deadline = Instant::now() + timeout;
    let mut events = Vec::new();
    loop {
        events.extend(session.take_events());
        let screen = session.screen(&pal(), true);
        if done(&screen, &events) || Instant::now() >= deadline {
            return (screen, events);
        }
        let _ = rx.recv_timeout(Duration::from_millis(100));
    }
}

fn waker() -> (Box<dyn Fn() + Send + Sync>, Receiver<()>) {
    let (tx, rx) = channel();
    (
        Box::new(move || {
            let _ = tx.send(());
        }),
        rx,
    )
}

#[test]
fn conpty_round_trip_echo_then_exited() {
    let (wake, rx) = waker();
    let started = Instant::now();
    let mut session = Session::spawn(&opts("cmd.exe", &["/c", "echo", "dj-term-ok"]), wake)
        .unwrap_or_else(|e| panic!("spawn failed: {e}"));
    assert!(session.pid().is_some());

    let (screen, events) = wait_for(&mut session, &rx, Duration::from_secs(15), |s, ev| {
        text(s).contains("dj-term-ok") && ev.iter().any(|e| matches!(e, TermEvent::Exited(_)))
    });
    let shown = text(&screen);
    assert!(
        shown.contains("dj-term-ok"),
        "the echo output is on the screen after {:?}:\n{shown}",
        started.elapsed()
    );
    let exits: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, TermEvent::Exited(_)))
        .collect();
    assert_eq!(
        exits,
        vec![&TermEvent::Exited(Some(0))],
        "one Exited event, with cmd's exit code"
    );
    assert!(session.exited());
    assert!(session.take_events().is_empty(), "Exited is not repeated");
    // Writing to a finished session must not panic or hang.
    session.write(b"ignored\r".to_vec());
}

/// Starts `cmd.exe /k`, waits for its prompt, runs `then`, and drops it; returns how long
/// the drop took.
fn drop_a_live_cmd(then: impl FnOnce(&mut Session, &Receiver<()>)) -> Duration {
    let (wake, rx) = waker();
    let mut session = Session::spawn(&opts("cmd.exe", &["/k"]), wake)
        .unwrap_or_else(|e| panic!("spawn failed: {e}"));
    let pid = session.pid().expect("pid");
    let (screen, _) = wait_for(&mut session, &rx, Duration::from_secs(15), |s, _| {
        text(s).contains('>')
    });
    assert!(
        text(&screen).contains('>'),
        "the shell is up:\n{}",
        text(&screen)
    );
    assert!(!session.exited());

    then(&mut session, &rx);

    let started = Instant::now();
    drop(session);
    let took = started.elapsed();
    eprintln!("session drop took {took:?}");
    assert!(
        crate::child::process_ended(pid, 3000),
        "cmd.exe (pid {pid}) must not outlive its session"
    );
    took
}

#[test]
fn dropping_a_live_session_returns_quickly_and_ends_the_shell() {
    let took = drop_a_live_cmd(|session, _| {
        // A resize and a write while it lives.
        session.resize(100, 30, 8, 16);
        assert_eq!(session.screen(&pal(), true).cols, 100);
        session.write(b"echo still-here\r".to_vec());
    });
    assert!(took < Duration::from_secs(3), "drop took {took:?}");
}

#[test]
fn dropping_a_session_that_is_flooding_output_does_not_hang() {
    let took = drop_a_live_cmd(|session, rx| {
        session.write(b"for /L %i in (1,1,500000) do @echo flood-%i\r".to_vec());
        let (screen, _) = wait_for(session, rx, Duration::from_secs(15), |s, _| {
            text(s).contains("flood-")
        });
        assert!(text(&screen).contains("flood-"), "the flood started");
        session.resize(60, 20, 8, 16);
    });
    assert!(took < Duration::from_secs(3), "drop took {took:?}");
}

#[test]
fn a_dead_terminal_loop_is_reported_as_exited() {
    let (wake, rx) = waker();
    let mut session = Session::spawn(&opts("cmd.exe", &["/k"]), wake)
        .unwrap_or_else(|e| panic!("spawn failed: {e}"));
    let (screen, _) = wait_for(&mut session, &rx, Duration::from_secs(15), |s, _| {
        text(s).contains('>')
    });
    assert!(text(&screen).contains('>'));
    assert!(!session.exited());

    // The loop ends without the emulator having seen the shell exit (as after an I/O error).
    session.end_loop_for_test();
    let (_, events) = wait_for(&mut session, &rx, Duration::from_secs(15), |_, ev| {
        ev.iter().any(|e| matches!(e, TermEvent::Exited(_)))
    });
    assert_eq!(
        events,
        vec![TermEvent::Exited(None)],
        "the watcher reports it once, and wakes the UI to say so"
    );
    assert!(session.exited());
    // The shell is still alive; dropping the session ends it.
    let pid = session.pid().expect("pid");
    drop(session);
    assert!(crate::child::process_ended(pid, 3000));
}
#[test]
fn a_session_can_move_between_threads() {
    fn is_send<T: Send>() {}
    is_send::<Session>();
}

#[test]
fn spawning_a_missing_program_is_an_error_not_a_panic() {
    for program in [
        r"C:\definitely\missing\shell.exe",
        "dj-term-no-such-shell",
        "",
    ] {
        let (wake, _rx) = waker();
        let err = Session::spawn(&opts(program, &[]), wake)
            .err()
            .unwrap_or_else(|| panic!("`{program}` must not start"));
        assert!(err.contains("not found"), "readable message, got: {err}");
    }
}
