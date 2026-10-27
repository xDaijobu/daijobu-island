//! The terminal panel's drawing: a header (the theme lamp, the shell's title, a close ✕), the
//! grid of cells from `dj-term`, a cursor, the selection, a scroll indicator and a one-line
//! notice. The island painter has already drawn the pill body; this only adds the content,
//! like `tile::paint_tile_panel`.
//!
//! Everything is laid out in panel-local logical px by `dj-core::layout` (`TERM_*`) and
//! multiplied by `scale` here. The grid is whole device pixels: one cell is `cell_size` px,
//! and every glyph is placed at its own cell's x, so a row never drifts however long it is
//! (runs of glyphs are batched, but each glyph carries its own position). Nothing is drawn
//! outside the panel's rect, and the cells, cursor, selection, scroll bar and notice are
//! clipped to `TERM_TEXT`.
//!
//! Glyphs the terminal's family lacks (CJK, emoji, symbols, Nerd-font icons) come from a
//! system font that has them (`Fonts::fallback_face`) and are fitted to their own cell span:
//! a fallback glyph wider or taller than its one or two cells is scaled down, never allowed
//! to run into the next cell. Bold and italic use the family's own faces when it has them
//! and are faked (an embolden, a skew) when not; embedded Geist Mono has no italic.

use crate::fonts::{Fonts, Weight};
use crate::paint::{col, draw_text, fill_paint, fit, stroke_paint};
use dj_core::layout::{self, Rect as LRect};
use dj_core::terminal::{Cell, Cursor, CursorShape, Palette, Screen, SELECTION_ALPHA};
use dj_core::theme::ResolvedTokens;
use skia_safe::{Canvas, ClipOp, Color, Font, GlyphId, Paint, Point, RRect, Rect};

/// What the terminal panel shows. The app picks the texts; `screen` and `palette` come
/// from the session and the theme.
#[derive(Clone, Copy, Debug)]
pub struct TerminalPanel<'a> {
    /// The header text, already chosen by the app (the shell's title or its name).
    pub title: &'a str,
    pub screen: &'a Screen,
    pub palette: &'a Palette,
    /// The cells' font family; embedded Geist Mono when no installed font has the name.
    pub font: &'a str,
    /// The cells' font size in logical px.
    pub font_size: f32,
    /// One line over the bottom of the text area (`process exited · press Enter to restart`).
    pub notice: Option<&'a str>,
    /// The pointer is over the ✕.
    pub close_hover: bool,
}

/// Header and notice text: `font_label`, medium, this size in logical px, with the Tile
/// panel's letter spacing (as a fraction of the size).
const TEXT_SIZE: f32 = 11.0;
const TEXT_TRACKING: f32 = 0.06;
/// The font size assumed when the config gives nonsense, and the sizes a request is clamped to.
const DEFAULT_SIZE: f32 = 12.0;
const MIN_SIZE: f32 = 4.0;
const MAX_SIZE: f32 = 96.0;

/// The header lamp, as the idle pill draws it: radius, halo radius and halo alpha. Its halo's
/// left edge lines up with the text area's.
const LAMP_R: f32 = 3.2;
const HALO_R: f32 = 6.5;
const HALO_ALPHA: f32 = 0.20;
/// The title starts this far after the lamp's halo and stops this far before the ✕.
const TITLE_GAP: f32 = 8.0;
const TITLE_END_GAP: f32 = 10.0;
/// The ✕: its arm (from the centre), stroke and hover plate, in logical px.
const CLOSE_ARM: f32 = 4.5;
const CLOSE_STROKE: f32 = 1.5;
const CLOSE_PLATE_R: f32 = 8.0;
const CLOSE_PLATE_ALPHA: f32 = 0.10;
/// The hairline under the header.
const DIVIDER_ALPHA: f32 = 0.12;

/// Cursor bars (underline and beam), the hollow outline, in logical px / device px.
const CURSOR_BAR: f32 = 2.0;
const OUTLINE: f32 = 1.0;

/// The scroll indicator: bar width, gap from the right edge, shortest bar, alpha.
const BAR_W: f32 = 3.0;
const BAR_GAP: f32 = 2.0;
const BAR_MIN: f32 = 18.0;
const BAR_ALPHA: f32 = 0.5;

/// The notice band: its smallest height, left padding, the warn dot and its alpha for the
/// band's top hairline.
const NOTICE_MIN_H: f32 = 20.0;
const NOTICE_PAD: f32 = 10.0;
const NOTICE_DOT_R: f32 = 3.2;
const NOTICE_LINE_ALPHA: f32 = 0.15;

/// A usable device scale: finite and positive, else 1.
fn sane(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// The cell font's size in device px.
fn font_px(size: f32, s: f32) -> f32 {
    let size = if size.is_finite() && size > 0.0 {
        size
    } else {
        DEFAULT_SIZE
    };
    (size.clamp(MIN_SIZE, MAX_SIZE) * s).max(1.0)
}

/// One cell's geometry in whole device px, from the regular face of the cell font.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Metrics {
    cw: f32,
    ch: f32,
    /// The baseline, from the top of the cell.
    base: f32,
    /// The 1 px underline's and strike's top, from the top of the cell.
    underline: f32,
    strike: f32,
}

fn metrics(fonts: &mut Fonts, family: &str, size: f32, s: f32) -> Metrics {
    let px = font_px(size, s);
    let font = fonts.cell_face(family, false, false).font(px);
    let adv = font.measure_str("M", None).0;
    let cw = if adv.is_finite() && adv > 0.0 {
        adv.round().clamp(1.0, 4096.0)
    } else {
        (px * 0.6).round().clamp(1.0, 4096.0)
    };
    let (_, fm) = font.metrics();
    let ch = (-fm.ascent + fm.descent + fm.leading)
        .round()
        .clamp(1.0, 4096.0);
    let last = (ch - 1.0).max(0.0);
    let base = (-fm.ascent + fm.leading / 2.0).round().clamp(0.0, last);
    let below = fm.underline_position().map_or(1.0, |p| p.round().max(1.0));
    let up = if fm.x_height > 0.0 {
        fm.x_height / 2.0
    } else {
        0.3 * ch
    };
    Metrics {
        cw,
        ch,
        base,
        underline: (base + below).min(last),
        strike: (base - up.round()).clamp(0.0, last),
    }
}

/// The cell size in whole device px for `font` at `size` logical px × `scale`: the advance of
/// `M` rounded, and the line height (ascent + descent + leading) rounded. Never below 1 × 1.
pub(crate) fn cell_size(fonts: &mut Fonts, font: &str, size: f32, scale: f32) -> (u16, u16) {
    let m = metrics(fonts, font, size, sane(scale));
    (m.cw as u16, m.ch as u16)
}

/// A rect in device px from a logical one at `s`, each edge rounded to a device pixel.
fn device_rect(r: LRect, s: f32) -> Rect {
    Rect::new(
        (r.x * s).round(),
        (r.y * s).round(),
        ((r.x + r.w) * s).round(),
        ((r.y + r.h) * s).round(),
    )
}

/// A crisp (not anti-aliased) fill for the grid's rectangles.
fn flat(c: Color) -> Paint {
    let mut p = Paint::default();
    p.set_anti_alias(false);
    p.set_color(c);
    p
}

/// A character that draws nothing: spaces, controls and zero-width marks.
fn is_invisible(ch: char) -> bool {
    ch.is_whitespace()
        || ch.is_control()
        || matches!(ch, '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{feff}' | '\u{fe00}'..='\u{fe0f}')
}

/// Draws the terminal panel's content inside `at`: the panel's rect at its final size, in
/// window logical px (as `paint_tile_panel` takes it), at `scale` device px per logical px.
/// Clipped to `at`; the grid and what sits on it are clipped to `TERM_TEXT` too.
// The island painter calls this once the coordinator hooks the layer in.
#[allow(dead_code)]
pub(crate) fn paint_terminal(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    at: LRect,
    scale: f32,
    p: &TerminalPanel,
) {
    let s = sane(scale);
    let area = device_rect(at, s);
    let base = canvas.save();
    canvas.clip_rect(area, ClipOp::Intersect, false);

    header(canvas, fonts, t, area, s, p);

    let text = Rect::new(
        (area.left + layout::TERM_TEXT.x * s).round(),
        (area.top + layout::TERM_TEXT.y * s).round(),
        (area.left + (layout::TERM_TEXT.x + layout::TERM_TEXT.w) * s).round(),
        (area.top + (layout::TERM_TEXT.y + layout::TERM_TEXT.h) * s).round(),
    );
    canvas.clip_rect(text, ClipOp::Intersect, false);

    let m = metrics(fonts, p.font, p.font_size, s);
    let mut grid = Grid {
        canvas,
        fonts,
        family: p.font,
        px: font_px(p.font_size, s),
        m,
        left: text.left,
        top: text.top,
        cols: (((text.right - text.left) / m.cw).ceil() as usize).min(p.screen.cols),
        rows: (((text.bottom - text.top) / m.ch).ceil() as usize).min(p.screen.rows),
        primary: [None, None, None, None],
    };
    grid.backgrounds(p.screen);
    grid.glyphs(p.screen);
    grid.decorations(p.screen);
    grid.selection(p.screen, p.palette);
    grid.cursor(p.screen, p.palette, s);
    scroll_bar(canvas, p, text, s);
    if let Some(n) = p.notice.filter(|n| !n.is_empty()) {
        notice(canvas, grid.fonts, t, p.palette, text, s, m, n);
    }
    canvas.restore_to_count(base);
}

/// The lamp, the title, the ✕ and the hairline under them.
fn header(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    area: Rect,
    s: f32,
    p: &TerminalPanel,
) {
    let cy = area.top + layout::TERM_HEADER_H * s / 2.0;
    let lamp_x = area.left + (layout::TERM_TEXT.x + HALO_R) * s;
    canvas.draw_circle(
        (lamp_x, cy),
        HALO_R * s,
        &fill_paint(col(t.accent, HALO_ALPHA)),
    );
    canvas.draw_circle((lamp_x, cy), LAMP_R * s, &fill_paint(col(t.accent, 1.0)));

    // The title: left of the ✕, cut with `..` and clipped to the header.
    let font = fonts.font(&t.font_label, Weight::Medium, TEXT_SIZE * s);
    let tracking = TEXT_TRACKING * TEXT_SIZE * s;
    let x = lamp_x + (HALO_R + TITLE_GAP) * s;
    let right = area.left + (layout::TERM_CLOSE.x - TITLE_END_GAP) * s;
    let title = fit(&font, p.title, tracking, right - x);
    if !title.is_empty() {
        canvas.save();
        canvas.clip_rect(
            Rect::new(x, area.top, right, area.top + layout::TERM_HEADER_H * s),
            ClipOp::Intersect,
            true,
        );
        draw_text(
            canvas,
            &font,
            &title,
            x,
            cy + 0.35 * TEXT_SIZE * s,
            tracking,
            &fill_paint(col(t.text, 1.0)),
        );
        canvas.restore();
    }

    // The ✕: two strokes, brighter (and on a faint plate) while hovered.
    let c = layout::TERM_CLOSE;
    let (cx, ccy) = (
        area.left + (c.x + c.w / 2.0) * s,
        area.top + (c.y + c.h / 2.0) * s,
    );
    if p.close_hover {
        let plate = Rect::from_xywh(area.left + c.x * s, area.top + c.y * s, c.w * s, c.h * s);
        let r = CLOSE_PLATE_R * s;
        canvas.draw_rrect(
            RRect::new_rect_xy(plate, r, r),
            &fill_paint(col(t.text, CLOSE_PLATE_ALPHA)),
        );
    }
    let ink = if p.close_hover { t.text } else { t.text_dim };
    let stroke = stroke_paint(col(ink, 1.0), CLOSE_STROKE * s);
    let arm = CLOSE_ARM * s;
    canvas.draw_line((cx - arm, ccy - arm), (cx + arm, ccy + arm), &stroke);
    canvas.draw_line((cx - arm, ccy + arm), (cx + arm, ccy - arm), &stroke);

    // The hairline: the header's last device row, as wide as the text area.
    let line_y = area.top + (layout::TERM_HEADER_H * s).round() - 1.0;
    canvas.draw_rect(
        Rect::new(
            (area.left + layout::TERM_TEXT.x * s).round(),
            line_y,
            (area.left + (layout::TERM_TEXT.x + layout::TERM_TEXT.w) * s).round(),
            line_y + 1.0,
        ),
        &flat(col(t.text, DIVIDER_ALPHA)),
    );
}

/// The grid's painter: one text area, the cell geometry, the fonts, and which cells to draw.
struct Grid<'a> {
    canvas: &'a Canvas,
    fonts: &'a mut Fonts,
    family: &'a str,
    /// The cell font's size in device px.
    px: f32,
    m: Metrics,
    /// The top-left of cell (0, 0) in device px.
    left: f32,
    top: f32,
    /// How many columns and rows are drawn (what the screen has and the text area holds).
    cols: usize,
    rows: usize,
    /// The family's regular, bold, italic and bold-italic fonts, made when first used.
    primary: [Option<Font>; 4],
}

impl Grid<'_> {
    fn x(&self, col: usize) -> f32 {
        self.left + col as f32 * self.m.cw
    }

    fn y(&self, row: usize) -> f32 {
        self.top + row as f32 * self.m.ch
    }

    fn primary(&mut self, bold: bool, italic: bool) -> &Font {
        let (fonts, family, px) = (&mut *self.fonts, self.family, self.px);
        self.primary[bold as usize | (italic as usize) << 1]
            .get_or_insert_with(|| fonts.cell_face(family, bold, italic).font(px))
    }

    /// Non-default backgrounds, merged per run of equal colour in a row. The default
    /// background is the panel itself and is left alone.
    fn backgrounds(&self, screen: &Screen) {
        for row in 0..self.rows {
            let (top, bottom) = (self.y(row), self.y(row) + self.m.ch);
            let mut run: Option<(usize, usize, Color)> = None;
            for c in 0..=self.cols {
                let bg = screen
                    .cell(c, row)
                    .filter(|_| c < self.cols)
                    .and_then(|cell| cell.bg)
                    .map(|bg| col(bg, 1.0));
                run = match (run, bg) {
                    (Some((from, _, color)), Some(next)) if next == color => {
                        Some((from, c + 1, color))
                    }
                    (prev, next) => {
                        if let Some((from, to, color)) = prev {
                            self.canvas.draw_rect(
                                Rect::new(self.x(from), top, self.x(to), bottom),
                                &flat(color),
                            );
                        }
                        next.map(|color| (c, c + 1, color))
                    }
                };
            }
        }
    }

    /// Glyphs. A glyph the family has is batched with the row's others of the same colour and
    /// style, every glyph at its own cell's x; one it lacks is drawn alone from a fallback font.
    fn glyphs(&mut self, screen: &Screen) {
        for row in 0..self.rows {
            let baseline = self.y(row) + self.m.base;
            // (colour, style index, glyphs, positions)
            let mut groups: Vec<(Color, usize, Vec<GlyphId>, Vec<Point>)> = Vec::new();
            for c in 0..self.cols {
                let Some(cell) = screen.cell(c, row) else {
                    continue;
                };
                if cell.style.spacer || is_invisible(cell.ch) {
                    continue;
                }
                let (bold, italic) = (cell.style.bold, cell.style.italic);
                let color = col(cell.fg, 1.0);
                let gid = self.primary(bold, italic).unichar_to_glyph(cell.ch as i32);
                if gid == 0 {
                    self.lacking(cell, self.x(c), baseline, color);
                    continue;
                }
                let style = bold as usize | (italic as usize) << 1;
                let at = Point::new(self.x(c), 0.0);
                match groups.iter_mut().find(|g| g.0 == color && g.1 == style) {
                    Some(g) => {
                        g.2.push(gid);
                        g.3.push(at);
                    }
                    None => groups.push((color, style, vec![gid], vec![at])),
                }
            }
            // A glyph that overshoots its line box (box-drawing lines do) is cut at the row,
            // so it neither paints over the rows around it nor leaves a gap in a vertical line.
            self.canvas.save();
            self.canvas.clip_rect(
                Rect::new(
                    self.left,
                    self.y(row),
                    self.x(self.cols),
                    self.y(row) + self.m.ch,
                ),
                ClipOp::Intersect,
                false,
            );
            for (color, style, glyphs, at) in groups {
                if let Some(font) = &self.primary[style] {
                    self.canvas.draw_glyphs_at(
                        &glyphs,
                        &at[..],
                        (0.0, baseline),
                        font,
                        &fill_paint(color),
                    );
                }
            }
            self.canvas.restore();
        }
    }

    /// A character the family has no glyph for, at `x` on `baseline`: from a system font that
    /// has one, scaled down until it fits its cell span and clipped to it, so it never
    /// touches the next cell or row. When no font has it, the family's own "missing" glyph.
    fn lacking(&mut self, cell: &Cell, x: f32, baseline: f32, color: Color) {
        let (bold, italic) = (cell.style.bold, cell.style.italic);
        let span = if cell.style.wide { 2.0 } else { 1.0 } * self.m.cw;
        let (above, below) = (self.m.base, self.m.ch - self.m.base);
        let found = self.fonts.fallback_face(self.family, cell.ch, bold, italic);
        let (font, gid) = match found {
            Some(face) => {
                let font = face.font(self.px);
                let gid = font.unichar_to_glyph(cell.ch as i32);
                (font, gid)
            }
            None => (self.primary(bold, italic).clone(), 0),
        };
        let mut bounds = [Rect::default()];
        font.get_bounds(&[gid], &mut bounds, None);
        let b = bounds[0];
        // Scale down (about the baseline) until the ink fits the span's width and the cell's
        // height; ink that starts left of the origin is moved right of the cell's edge.
        let mut k = 1.0f32;
        let needs = b.right - b.left.min(0.0);
        if needs > span {
            k = k.min(span / needs);
        }
        if b.top < -above {
            k = k.min(above / -b.top);
        }
        if b.bottom > below {
            k = k.min(below / b.bottom);
        }
        let dx = -b.left.min(0.0) * k;
        // The clip catches what anti-aliasing and pixel snapping leave over the fit.
        let canvas = self.canvas;
        canvas.save();
        canvas.clip_rect(
            Rect::new(x, baseline - above, x + span, baseline + below),
            ClipOp::Intersect,
            false,
        );
        canvas.translate((x + dx, baseline));
        canvas.scale((k, k));
        canvas.draw_glyphs_at(
            &[gid],
            &[Point::new(0.0, 0.0)][..],
            (0.0, 0.0),
            &font,
            &fill_paint(color),
        );
        canvas.restore();
    }

    /// Underline and strike: 1 device px lines across the cell, a wide character's two cells
    /// as one, merged per run of the same colour.
    fn decorations(&self, screen: &Screen) {
        for row in 0..self.rows {
            for (line, offset) in [(0, self.m.underline), (1, self.m.strike)] {
                let y = self.y(row) + offset;
                let mut run: Option<(usize, usize, Color)> = None;
                let flush = |run: Option<(usize, usize, Color)>| {
                    if let Some((from, to, color)) = run {
                        self.canvas.draw_rect(
                            Rect::new(self.x(from), y, self.x(to), y + 1.0),
                            &flat(color),
                        );
                    }
                };
                for c in 0..self.cols {
                    let Some(cell) = screen.cell(c, row) else {
                        continue;
                    };
                    let on = !cell.style.spacer
                        && if line == 0 {
                            cell.style.underline
                        } else {
                            cell.style.strike
                        };
                    if !on {
                        // A spacer carries on the wide cell's run; anything else ends it.
                        if !cell.style.spacer {
                            flush(run.take());
                        }
                        continue;
                    }
                    let color = col(cell.fg, 1.0);
                    let end = c + if cell.style.wide { 2 } else { 1 };
                    run = match run {
                        Some((from, to, prev)) if prev == color && to == c => {
                            Some((from, end, color))
                        }
                        other => {
                            flush(other);
                            Some((c, end, color))
                        }
                    };
                }
                flush(run);
            }
        }
    }

    /// The selection: `palette.selection` at `SELECTION_ALPHA` over each span's cells.
    fn selection(&self, screen: &Screen, palette: &Palette) {
        let paint = flat(col(palette.selection, SELECTION_ALPHA));
        for span in &screen.selection {
            let end = span.end.min(self.cols);
            if span.row >= self.rows || span.start >= end {
                continue;
            }
            self.canvas.draw_rect(
                Rect::new(
                    self.x(span.start),
                    self.y(span.row),
                    self.x(end),
                    self.y(span.row) + self.m.ch,
                ),
                &paint,
            );
        }
    }

    /// The cursor, over the cells and the selection. On a wide character it covers both cells;
    /// a cursor on the spacer is drawn from the wide cell.
    fn cursor(&mut self, screen: &Screen, palette: &Palette, s: f32) {
        let Some(Cursor { col: c, row, shape }) = screen.cursor else {
            return;
        };
        if row >= self.rows || c >= screen.cols {
            return;
        }
        let (mut c, mut span) = (c, 1usize);
        if let Some(cell) = screen.cell(c, row) {
            if cell.style.spacer && c > 0 {
                c -= 1;
                span = 2;
            } else if cell.style.wide {
                span = 2;
            }
        }
        let (x0, y0) = (self.x(c), self.y(row));
        let (x1, y1) = (x0 + span as f32 * self.m.cw, y0 + self.m.ch);
        let bar = (CURSOR_BAR * s).round().max(1.0);
        let paint = flat(col(palette.cursor, 1.0));
        let canvas = self.canvas;
        match shape {
            CursorShape::Block => {
                canvas.draw_rect(Rect::new(x0, y0, x1, y1), &paint);
                if let Some(cell) = screen.cell(c, row) {
                    if !cell.style.spacer && !is_invisible(cell.ch) {
                        let under = Cell {
                            fg: palette.bg,
                            ..*cell
                        };
                        self.redraw(
                            &under,
                            x0,
                            self.y(row) + self.m.base,
                            Rect::new(x0, y0, x1, y1),
                        );
                    }
                }
            }
            CursorShape::HollowBlock => {
                let w = OUTLINE.max(1.0).min(self.m.cw).min(self.m.ch);
                for r in [
                    Rect::new(x0, y0, x1, y0 + w),
                    Rect::new(x0, y1 - w, x1, y1),
                    Rect::new(x0, y0, x0 + w, y1),
                    Rect::new(x1 - w, y0, x1, y1),
                ] {
                    canvas.draw_rect(r, &paint);
                }
            }
            CursorShape::Underline => {
                canvas.draw_rect(Rect::new(x0, (y1 - bar).max(y0), x1, y1), &paint);
            }
            CursorShape::Beam => {
                canvas.draw_rect(Rect::new(x0, y0, (x0 + bar).min(x1), y1), &paint);
            }
        }
    }

    /// One cell's glyph alone, in the cell's own colour (the block cursor's inverted glyph).
    fn redraw(&mut self, cell: &Cell, x: f32, baseline: f32, clip: Rect) {
        let (bold, italic) = (cell.style.bold, cell.style.italic);
        let color = col(cell.fg, 1.0);
        let gid = self.primary(bold, italic).unichar_to_glyph(cell.ch as i32);
        if gid == 0 {
            self.lacking(cell, x, baseline, color);
            return;
        }
        let font = self.primary(bold, italic).clone();
        self.canvas.save();
        self.canvas.clip_rect(clip, ClipOp::Intersect, false);
        self.canvas.draw_glyphs_at(
            &[gid],
            &[Point::new(x, 0.0)][..],
            (0.0, baseline),
            &font,
            &fill_paint(color),
        );
        self.canvas.restore();
    }
}

/// While scrolled back: a thin bar at the right edge of the text area. Its length is the share
/// of the whole scrollback the screen shows, and it sits at the bottom when following the
/// output and at the top at the oldest line.
fn scroll_bar(canvas: &Canvas, p: &TerminalPanel, text: Rect, s: f32) {
    let sc = p.screen;
    if sc.offset == 0 {
        return;
    }
    let history = sc.history.max(sc.offset) as f32;
    let track = text.height();
    let len = (track * sc.rows.max(1) as f32 / (sc.rows.max(1) as f32 + history))
        .max(BAR_MIN * s)
        .min(track);
    let top = text.top + (track - len) * (1.0 - sc.offset as f32 / history);
    let w = (BAR_W * s).round().max(2.0);
    let right = text.right - (BAR_GAP * s).round();
    canvas.draw_rrect(
        RRect::new_rect_xy(
            Rect::new(right - w, top, right, top + len),
            w / 2.0,
            w / 2.0,
        ),
        &fill_paint(col(p.palette.fg, BAR_ALPHA)),
    );
}

/// One line on a `palette.bg` band over the bottom of the text area: a warn dot and the text
/// in the theme's text colour, cut with `..` to fit.
#[allow(clippy::too_many_arguments)]
fn notice(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    palette: &Palette,
    text: Rect,
    s: f32,
    m: Metrics,
    line: &str,
) {
    let h = m.ch.max((NOTICE_MIN_H * s).round()).min(text.height());
    let band = Rect::new(text.left, text.bottom - h, text.right, text.bottom);
    canvas.draw_rect(band, &flat(col(palette.bg, 1.0)));
    canvas.draw_rect(
        Rect::new(band.left, band.top, band.right, band.top + 1.0),
        &flat(col(palette.fg, NOTICE_LINE_ALPHA)),
    );
    let cy = band.center_y();
    let dot_x = band.left + NOTICE_PAD * s;
    canvas.draw_circle((dot_x, cy), NOTICE_DOT_R * s, &fill_paint(col(t.warn, 1.0)));
    let font = fonts.font(&t.font_label, Weight::Medium, TEXT_SIZE * s);
    let tracking = TEXT_TRACKING * TEXT_SIZE * s;
    let x = dot_x + (NOTICE_DOT_R + TITLE_GAP) * s;
    let line = fit(&font, line, tracking, band.right - x - NOTICE_PAD * s);
    draw_text(
        canvas,
        &font,
        &line,
        x,
        cy + 0.35 * TEXT_SIZE * s,
        tracking,
        &fill_paint(col(t.text, 1.0)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::color::Rgb;
    use dj_core::terminal::{SelSpan, Style};
    use dj_core::theme::{ThemeRegistry, DEFAULT_THEME};
    use skia_safe::image::CachingHint;
    use skia_safe::{surfaces, AlphaType, ColorType, ImageInfo};

    const SCALES: [f32; 3] = [1.0, 1.5, 2.0];
    const FAMILY: &str = "Geist Mono";
    const SIZE: f32 = 12.0;
    /// The panel's origin on the test surface, logical px (whole device px at every scale).
    const ORIGIN: (f32, f32) = (30.0, 24.0);
    /// Test colours, apart from the theme's, so a pixel says which part drew it.
    const RED: Rgb = Rgb::new(1.0, 0.0, 0.0);
    const GREEN: Rgb = Rgb::new(0.0, 1.0, 0.0);
    const BLUE: Rgb = Rgb::new(0.0, 0.0, 1.0);
    const CURSOR: Rgb = Rgb::new(0.1, 0.9, 0.3);
    const SELECT: Rgb = Rgb::new(0.9, 0.2, 0.9);

    fn tokens() -> ResolvedTokens {
        ThemeRegistry::with_builtins()
            .get(DEFAULT_THEME)
            .unwrap()
            .clone()
    }

    fn palette(t: &ResolvedTokens) -> Palette {
        Palette {
            fg: t.text,
            bg: t.pill,
            cursor: CURSOR,
            selection: SELECT,
            ansi: [t.text_dim; 16],
        }
    }

    /// `[b, g, r, a]` of an opaque colour, as the CPU surface stores it.
    fn bgra(c: Rgb) -> [u8; 4] {
        let [r, g, b] = c.to_u8();
        [b, g, r, 255]
    }

    fn near(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
        a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= tol)
    }

    fn distance(a: [u8; 4], b: [u8; 4]) -> u32 {
        a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y) as u32).sum()
    }

    /// `c` at `alpha` drawn over the premultiplied pixel `dst`.
    fn blend(dst: [u8; 4], c: Rgb, alpha: f32) -> [u8; 4] {
        let a = (alpha * 255.0 + 0.5).floor() / 255.0;
        let [r, g, b] = c.to_u8();
        let mix = |src: u8, d: u8| (src as f32 * a + d as f32 * (1.0 - a)).round() as u8;
        [
            mix(b, dst[0]),
            mix(g, dst[1]),
            mix(r, dst[2]),
            mix(255, dst[3]),
        ]
    }

    /// x, y, w, h in device px.
    type IRect = (i32, i32, i32, i32);

    /// A painted CPU surface (BGRA, premultiplied).
    struct Shot {
        w: i32,
        h: i32,
        px: Vec<u8>,
    }

    impl Shot {
        fn at(&self, x: i32, y: i32) -> [u8; 4] {
            assert!(x >= 0 && y >= 0 && x < self.w && y < self.h, "({x},{y})");
            let i = ((y * self.w + x) * 4) as usize;
            [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
        }

        fn whole(&self) -> IRect {
            (0, 0, self.w, self.h)
        }

        fn each(&self, r: IRect, mut f: impl FnMut(i32, i32, [u8; 4])) {
            for y in r.1..r.1 + r.3 {
                for x in r.0..r.0 + r.2 {
                    f(x, y, self.at(x, y));
                }
            }
        }

        /// Pixels with any coverage inside the rect.
        fn lit_in(&self, r: IRect) -> usize {
            let mut n = 0;
            self.each(r, |_, _, p| n += (p[3] > 0) as usize);
            n
        }

        /// Pixels within `tol` of `c` inside the rect.
        fn count_in(&self, r: IRect, c: [u8; 4], tol: u8) -> usize {
            let mut n = 0;
            self.each(r, |_, _, p| n += near(p, c, tol) as usize);
            n
        }

        /// How close any pixel of the rect comes to `c` (the sum of the channels' distances).
        fn closest(&self, r: IRect, c: [u8; 4]) -> u32 {
            let mut best = u32::MAX;
            self.each(r, |_, _, p| best = best.min(distance(p, c)));
            best
        }

        /// The sum of the alpha of the pixels inside the rect.
        fn alpha_sum(&self, r: IRect) -> u32 {
            let mut n = 0;
            self.each(r, |_, _, p| n += p[3] as u32);
            n
        }

        /// The first lit pixel of `area` outside every rect of `inside`, if any.
        fn lit_outside(&self, area: IRect, inside: &[IRect]) -> Option<(i32, i32)> {
            let mut found = None;
            self.each(area, |x, y, p| {
                let within = inside
                    .iter()
                    .any(|r| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3);
                if found.is_none() && !within && p[3] > 0 {
                    found = Some((x, y));
                }
            });
            found
        }

        /// The extent (min x, min y, max x, max y) of the lit pixels inside the rect.
        fn lit_bounds(&self, r: IRect) -> Option<(i32, i32, i32, i32)> {
            let mut b: Option<(i32, i32, i32, i32)> = None;
            self.each(r, |x, y, p| {
                if p[3] > 0 {
                    b = Some(match b {
                        None => (x, y, x, y),
                        Some((a, c, d, e)) => (a.min(x), c.min(y), d.max(x), e.max(y)),
                    });
                }
            });
            b
        }

        /// The alpha of each pixel of the rect, row by row.
        fn pattern(&self, r: IRect) -> Vec<u8> {
            let mut v = Vec::new();
            self.each(r, |_, _, p| v.push(p[3]));
            v
        }

        /// Alpha-weighted mean x of the lit pixels of the rect.
        fn centroid_x(&self, r: IRect) -> f32 {
            let (mut sum, mut weight) = (0.0f32, 0.0f32);
            self.each(r, |x, _, p| {
                sum += x as f32 * p[3] as f32;
                weight += p[3] as f32;
            });
            sum / weight.max(1.0)
        }

        /// Whether the rect has a glyph's ink in `c`: its most opaque pixel, un-premultiplied.
        fn has_ink(&self, r: IRect, c: Rgb) -> bool {
            let mut peak = [0u8; 4];
            self.each(r, |_, _, p| {
                if p[3] > peak[3] {
                    peak = p;
                }
            });
            let a = peak[3] as f32;
            let flat = [0, 1, 2].map(|i| (peak[i] as f32 * 255.0 / a.max(1.0)).round() as u8);
            peak[3] >= 120 && near([flat[0], flat[1], flat[2], 255], bgra(c), 12)
        }
    }

    fn snapshot(mut surface: skia_safe::Surface, w: i32, h: i32) -> Shot {
        let info = ImageInfo::new((w, h), ColorType::BGRA8888, AlphaType::Premul, None);
        let mut px = vec![0u8; (w * h * 4) as usize];
        assert!(surface.image_snapshot().read_pixels(
            &info,
            &mut px,
            (w * 4) as usize,
            (0, 0),
            CachingHint::Disallow
        ));
        Shot { w, h, px }
    }

    /// Where things are on the surface at one scale, in device px.
    struct Geo {
        s: f32,
        cw: i32,
        ch: i32,
        /// The whole panel.
        panel: IRect,
        /// `TERM_TEXT`.
        text: IRect,
    }

    impl Geo {
        fn new(s: f32) -> Geo {
            let mut fonts = Fonts::new().unwrap();
            let (cw, ch) = cell_size(&mut fonts, FAMILY, SIZE, s);
            let area = (
                (ORIGIN.0 * s).round(),
                (ORIGIN.1 * s).round(),
                ((ORIGIN.0 + layout::TERM_W) * s).round(),
                ((ORIGIN.1 + layout::TERM_H) * s).round(),
            );
            let t = layout::TERM_TEXT;
            let edge = |from: f32, d: f32| (from + d * s).round() as i32;
            let (x0, y0) = (edge(area.0, t.x), edge(area.1, t.y));
            let (x1, y1) = (edge(area.0, t.x + t.w), edge(area.1, t.y + t.h));
            Geo {
                s,
                cw: cw as i32,
                ch: ch as i32,
                panel: (
                    area.0 as i32,
                    area.1 as i32,
                    (area.2 - area.0) as i32,
                    (area.3 - area.1) as i32,
                ),
                text: (x0, y0, x1 - x0, y1 - y0),
            }
        }

        /// `span` cells from (col, row).
        fn cell(&self, col: usize, row: usize, span: usize) -> IRect {
            (
                self.text.0 + col as i32 * self.cw,
                self.text.1 + row as i32 * self.ch,
                span as i32 * self.cw,
                self.ch,
            )
        }

        /// The header strip: from the panel's top to the text area's.
        fn header(&self) -> IRect {
            (
                self.panel.0,
                self.panel.1,
                self.panel.2,
                self.text.1 - self.panel.1,
            )
        }

        /// A point of the panel at logical (x, y) from its top-left, in device px.
        fn pt(&self, x: f32, y: f32) -> (i32, i32) {
            (
                self.panel.0 + (x * self.s).floor() as i32,
                self.panel.1 + (y * self.s).floor() as i32,
            )
        }

        /// The header's last row, where the hairline is.
        fn line_y(&self) -> i32 {
            self.panel.1 + (layout::TERM_HEADER_H * self.s).round() as i32 - 1
        }
    }

    fn blank(cols: usize, rows: usize, pal: &Palette) -> Screen {
        let mut s = Screen::blank(cols, rows, pal);
        s.cursor = None;
        s
    }

    fn put(
        sc: &mut Screen,
        col: usize,
        row: usize,
        ch: char,
        fg: Rgb,
        bg: Option<Rgb>,
        style: Style,
    ) {
        sc.cells[row * sc.cols + col] = Cell { ch, fg, bg, style };
    }

    fn plain() -> Style {
        Style::default()
    }

    fn panel<'a>(sc: &'a Screen, pal: &'a Palette) -> TerminalPanel<'a> {
        TerminalPanel {
            title: "pwsh",
            screen: sc,
            palette: pal,
            font: FAMILY,
            font_size: SIZE,
            notice: None,
            close_hover: false,
        }
    }

    fn render(t: &ResolvedTokens, p: &TerminalPanel, s: f32) -> (Shot, Geo) {
        let mut fonts = Fonts::new().unwrap();
        let w = ((ORIGIN.0 + layout::TERM_W + 30.0) * s).round() as i32;
        let h = ((ORIGIN.1 + layout::TERM_H + 30.0) * s).round() as i32;
        let mut surface = surfaces::raster_n32_premul((w, h)).unwrap();
        surface.canvas().clear(Color::TRANSPARENT);
        let at = LRect::new(ORIGIN.0, ORIGIN.1, layout::TERM_W, layout::TERM_H);
        paint_terminal(surface.canvas(), &mut fonts, t, at, s, p);
        (snapshot(surface, w, h), Geo::new(s))
    }

    /// The candidates that an installed font has and Geist Mono lacks. Empty (and a note on
    /// stderr) on a machine with no such font: the tests that need one then pass vacuously.
    fn fallback_chars(candidates: &[char]) -> Vec<char> {
        let mut fonts = Fonts::new().unwrap();
        let primary = fonts.cell_face(FAMILY, false, false);
        let found: Vec<char> = candidates
            .iter()
            .copied()
            .filter(|&c| primary.typeface.unichar_to_glyph(c as i32) == 0)
            .filter(|&c| fonts.fallback_face(FAMILY, c, false, false).is_some())
            .collect();
        if found.is_empty() {
            eprintln!("skipped: no installed font has any of {candidates:?}");
        }
        found
    }

    const WIDE: Style = Style {
        bold: false,
        italic: false,
        underline: false,
        strike: false,
        wide: true,
        spacer: false,
    };
    const SPACER: Style = Style {
        bold: false,
        italic: false,
        underline: false,
        strike: false,
        wide: false,
        spacer: true,
    };

    // ----- cells -----

    #[test]
    fn cells_land_at_their_own_column_and_row() {
        let t = tokens();
        let pal = palette(&t);
        for s in SCALES {
            let spots = [(0, 0), (5, 3), (40, 9), (79, 19)];
            let mut sc = blank(80, 20, &pal);
            for (c, r) in spots {
                put(&mut sc, c, r, 'M', RED, None, plain());
            }
            let (shot, g) = render(&t, &panel(&sc, &pal), s);
            let cells: Vec<IRect> = spots.iter().map(|&(c, r)| g.cell(c, r, 1)).collect();
            for (cell, spot) in cells.iter().zip(spots) {
                assert!(shot.lit_in(*cell) > 8, "scale {s}: {spot:?} has ink");
                assert!(
                    shot.has_ink(*cell, RED),
                    "scale {s}: {spot:?} is in its colour"
                );
            }
            // The ink stays in its own cell, and nothing else is drawn on the grid.
            assert_eq!(shot.lit_outside(g.text, &cells), None, "scale {s}");
        }
    }

    #[test]
    fn a_long_row_does_not_drift() {
        let t = tokens();
        let pal = palette(&t);
        for s in SCALES {
            let mut sc = blank(90, 4, &pal);
            for c in 0..90 {
                put(&mut sc, c, 0, 'M', RED, None, plain());
                put(&mut sc, c, 3, 'g', RED, None, plain());
            }
            let (shot, g) = render(&t, &panel(&sc, &pal), s);
            let first = shot.pattern(g.cell(0, 0, 1));
            let first_g = shot.pattern(g.cell(0, 3, 1));
            assert!(first.iter().any(|&a| a > 0));
            for c in [1, 17, 44, 89] {
                assert_eq!(shot.pattern(g.cell(c, 0, 1)), first, "scale {s}: col {c}");
                assert_eq!(shot.pattern(g.cell(c, 3, 1)), first_g, "scale {s}: col {c}");
            }
            assert_eq!(
                shot.lit_in(g.cell(0, 1, 90)),
                0,
                "the rows between are empty"
            );
        }
    }

    #[test]
    fn the_default_background_is_unpainted_and_others_are_painted() {
        let t = tokens();
        let pal = palette(&t);
        for s in SCALES {
            let mut sc = blank(80, 20, &pal);
            put(&mut sc, 2, 1, ' ', RED, Some(BLUE), plain());
            for c in 4..8 {
                put(&mut sc, c, 1, ' ', RED, Some(GREEN), plain());
            }
            put(&mut sc, 8, 1, ' ', RED, Some(RED), plain());
            put(&mut sc, 20, 5, 'M', RED, Some(BLUE), plain());
            let (shot, g) = render(&t, &panel(&sc, &pal), s);
            let area = |r: IRect| (r.2 * r.3) as usize;
            let blue = g.cell(2, 1, 1);
            assert_eq!(shot.count_in(blue, bgra(BLUE), 0), area(blue), "scale {s}");
            // A run of equal colours is one rect: no seam between its cells.
            let green = g.cell(4, 1, 4);
            assert_eq!(
                shot.count_in(green, bgra(GREEN), 0),
                area(green),
                "scale {s}"
            );
            let red = g.cell(8, 1, 1);
            assert_eq!(shot.count_in(red, bgra(RED), 0), area(red), "scale {s}");
            // Unpainted: the gap, the default-coloured cells and the rest of the grid.
            assert_eq!(shot.lit_in(g.cell(3, 1, 1)), 0, "scale {s}");
            assert_eq!(shot.lit_in(g.cell(9, 1, 20)), 0, "scale {s}");
            // A glyph on a painted cell keeps the cell's colour around it.
            let under = g.cell(20, 5, 1);
            assert!(shot.count_in(under, bgra(BLUE), 0) > area(under) / 2);
            assert_eq!(
                shot.lit_outside(g.text, &[blue, green, red, under]),
                None,
                "scale {s}"
            );
        }
    }

    #[test]
    fn bold_italic_underline_and_strike_are_drawn() {
        let t = tokens();
        let pal = palette(&t);
        for s in SCALES {
            let bold = Style {
                bold: true,
                ..plain()
            };
            let italic = Style {
                italic: true,
                ..plain()
            };
            let under = Style {
                underline: true,
                ..plain()
            };
            let strike = Style {
                strike: true,
                ..plain()
            };
            let both = Style {
                underline: true,
                strike: true,
                ..plain()
            };
            let mut sc = blank(80, 20, &pal);
            put(&mut sc, 1, 1, 'H', RED, None, plain());
            put(&mut sc, 3, 1, 'H', RED, None, bold);
            put(&mut sc, 5, 1, '|', RED, None, plain());
            put(&mut sc, 7, 1, '|', RED, None, italic);
            put(&mut sc, 5, 2, ' ', RED, None, under);
            put(&mut sc, 6, 2, ' ', RED, None, under);
            put(&mut sc, 8, 2, ' ', RED, None, strike);
            put(&mut sc, 11, 2, ' ', RED, None, both);
            // A wide cell's line runs over its spacer.
            put(
                &mut sc,
                14,
                2,
                ' ',
                RED,
                None,
                Style {
                    wide: true,
                    ..under
                },
            );
            put(
                &mut sc,
                15,
                2,
                ' ',
                RED,
                None,
                Style {
                    spacer: true,
                    ..under
                },
            );
            let (shot, g) = render(&t, &panel(&sc, &pal), s);

            let regular = shot.alpha_sum(g.cell(1, 1, 1));
            let heavier = shot.alpha_sum(g.cell(3, 1, 1));
            assert!(
                heavier as f32 > regular as f32 * 1.1,
                "scale {s}: bold {heavier} vs {regular}"
            );

            // Italic leans: the top of the bar sits further right than its foot.
            let lean = |col: usize| {
                let (x, y, w, h) = g.cell(col, 1, 1);
                let third = h / 3;
                shot.centroid_x((x, y, w, third)) - shot.centroid_x((x, y + h - third, w, third))
            };
            assert!(
                lean(7) > lean(5) + 0.5,
                "scale {s}: {} vs {}",
                lean(7),
                lean(5)
            );

            // 1 device px lines, in the cell's colour, across the cell.
            let line_rows = |r: IRect| -> Vec<i32> {
                let mut rows = Vec::new();
                for y in r.1..r.1 + r.3 {
                    let n = shot.count_in((r.0, y, r.2, 1), bgra(RED), 0);
                    if n > 0 {
                        assert_eq!(n as i32, r.2, "scale {s}: the line spans the cell");
                        rows.push(y);
                    }
                }
                rows
            };
            let u = line_rows(g.cell(5, 2, 2));
            let k = line_rows(g.cell(8, 2, 1));
            let b = line_rows(g.cell(11, 2, 1));
            let w = line_rows(g.cell(14, 2, 2));
            assert_eq!(u.len(), 1, "scale {s}: underline is one device px");
            assert_eq!(k.len(), 1, "scale {s}: strike is one device px");
            assert_eq!(w, u, "scale {s}: a wide cell's underline");
            assert!(k[0] < u[0], "scale {s}: strike above underline");
            assert_eq!(b, vec![k[0], u[0]], "scale {s}: both");
            assert_eq!(shot.lit_in(g.cell(14, 2, 2)) as i32, 2 * g.cw, "scale {s}");
            assert_eq!(shot.lit_in(g.cell(5, 2, 2)) as i32, 2 * g.cw, "scale {s}");
        }
    }

    #[test]
    fn a_wide_character_spans_two_cells_and_its_spacer_draws_only_a_background() {
        let t = tokens();
        let pal = palette(&t);
        for s in SCALES {
            // The primary face's glyph, marked wide: ink in its first cell, the spacer's
            // background behind the second, and a spacer's own character never drawn.
            let mut sc = blank(80, 20, &pal);
            put(&mut sc, 3, 1, 'M', RED, None, WIDE);
            put(&mut sc, 4, 1, 'X', RED, Some(GREEN), SPACER);
            let (shot, g) = render(&t, &panel(&sc, &pal), s);
            let second = g.cell(4, 1, 1);
            assert_eq!(
                shot.count_in(second, bgra(GREEN), 0),
                (second.2 * second.3) as usize,
                "scale {s}: the spacer is its background only"
            );
            assert_eq!(shot.lit_outside(g.text, &[g.cell(3, 1, 2)]), None);
            assert!(shot.has_ink(g.cell(3, 1, 1), RED), "scale {s}");
        }
        // A real double-width glyph (from a system font) fills the two cells and goes no further.
        let Some(&ch) = fallback_chars(&['漢', '日', '한', 'あ']).first() else {
            return;
        };
        for s in SCALES {
            let mut sc = blank(80, 20, &pal);
            put(&mut sc, 3, 1, ch, RED, None, WIDE);
            put(&mut sc, 4, 1, ' ', RED, Some(BLUE), SPACER);
            put(&mut sc, 5, 1, 'M', GREEN, None, plain());
            let (shot, g) = render(&t, &panel(&sc, &pal), s);
            let span = g.cell(3, 1, 2);
            let (first, second) = (g.cell(3, 1, 1), g.cell(4, 1, 1));
            assert!(shot.lit_in(first) > 10, "scale {s}: the glyph is drawn");
            // It reaches over into the second cell, which keeps its background elsewhere.
            let all = (second.2 * second.3) as usize;
            let blue = shot.count_in(second, bgra(BLUE), 0);
            assert!(blue < all, "scale {s}: ink over the spacer");
            assert!(blue > 0, "scale {s}: its background is still there");
            // Review focus 4: it never touches the cell after it.
            let next = g.cell(5, 1, 1);
            assert_eq!(
                shot.lit_outside(g.text, &[span, next]),
                None,
                "scale {s}: ink outside its two cells"
            );
            assert!(shot.has_ink(next, GREEN), "scale {s}: the next glyph");
            assert_eq!(shot.count_in(next, bgra(RED), 30), 0, "scale {s}: no spill");
        }
    }

    #[test]
    fn a_glyph_from_a_fallback_font_never_runs_into_the_next_cell() {
        let t = tokens();
        let pal = palette(&t);
        let chars = fallback_chars(&['漢', 'あ', '😀', '\u{e0b0}', '\u{f07b}', '★', '✓']);
        for ch in chars {
            for s in SCALES {
                for wide in [false, true] {
                    let style = if wide { WIDE } else { plain() };
                    let span = if wide { 2 } else { 1 };
                    let mut sc = blank(80, 20, &pal);
                    put(&mut sc, 3, 1, ch, RED, None, style);
                    if wide {
                        put(&mut sc, 4, 1, ' ', RED, None, SPACER);
                    }
                    let (shot, g) = render(&t, &panel(&sc, &pal), s);
                    let cells = g.cell(3, 1, span);
                    assert!(
                        shot.lit_in(cells) > 4,
                        "{ch:?} scale {s} wide {wide}: it is drawn"
                    );
                    assert_eq!(
                        shot.lit_outside(g.text, &[cells]),
                        None,
                        "{ch:?} scale {s} wide {wide}: ink outside its {span} cell(s)"
                    );
                }
            }
        }
    }

    #[test]
    fn a_character_no_font_has_stays_in_its_cell() {
        let t = tokens();
        let pal = palette(&t);
        for s in SCALES {
            let mut sc = blank(80, 20, &pal);
            put(&mut sc, 3, 1, '\u{10fffe}', RED, None, plain());
            put(&mut sc, 5, 1, 'M', RED, None, plain());
            let (shot, g) = render(&t, &panel(&sc, &pal), s);
            assert_eq!(
                shot.lit_outside(g.text, &[g.cell(3, 1, 1), g.cell(5, 1, 1)]),
                None,
                "scale {s}"
            );
            assert!(shot.lit_in(g.cell(5, 1, 1)) > 8);
        }
    }

    #[test]
    fn a_missing_glyph_falls_back_to_a_system_font() {
        let t = tokens();
        let pal = palette(&t);
        let Some(&ch) = fallback_chars(&['漢', 'あ', '日', '한', '★']).first() else {
            return;
        };
        let mut fonts = Fonts::new().unwrap();
        let primary = fonts.cell_face(FAMILY, false, false);
        assert_eq!(primary.typeface.unichar_to_glyph(ch as i32), 0);
        let face = fonts.fallback_face(FAMILY, ch, false, false).unwrap();
        assert_ne!(face.typeface.unichar_to_glyph(ch as i32), 0);
        // The answer is kept: the same face again.
        let again = fonts.fallback_face(FAMILY, ch, false, false).unwrap();
        assert_eq!(face.typeface.unique_id(), again.typeface.unique_id());
        // It is drawn, in the cell's colour.
        for s in SCALES {
            let mut sc = blank(80, 20, &pal);
            put(&mut sc, 3, 1, ch, RED, None, plain());
            let (shot, g) = render(&t, &panel(&sc, &pal), s);
            assert!(shot.lit_in(g.cell(3, 1, 1)) > 6, "scale {s}");
            assert!(shot.has_ink(g.cell(3, 1, 1), RED), "scale {s}");
        }
    }

    // ----- cursor -----

    fn cursor_at(sc: &mut Screen, col: usize, row: usize, shape: CursorShape) {
        sc.cursor = Some(Cursor { col, row, shape });
    }

    #[test]
    fn a_block_cursor_is_a_cursor_coloured_box_with_the_glyph_in_the_panel_colour() {
        let t = tokens();
        let pal = palette(&t);
        for s in SCALES {
            let mut sc = blank(80, 20, &pal);
            put(&mut sc, 7, 3, 'M', RED, None, plain());
            put(&mut sc, 10, 3, 'M', RED, None, WIDE);
            put(&mut sc, 11, 3, ' ', RED, None, SPACER);
            cursor_at(&mut sc, 7, 3, CursorShape::Block);
            let (shot, g) = render(&t, &panel(&sc, &pal), s);
            let cell = g.cell(7, 3, 1);
            let area = (cell.2 * cell.3) as usize;
            assert!(
                shot.closest(cell, bgra(pal.bg)) < 80,
                "scale {s}: the glyph is redrawn in the panel colour"
            );
            assert!(shot.count_in(cell, bgra(CURSOR), 0) > area / 2, "scale {s}");
            assert!(!shot.has_ink(cell, RED), "scale {s}: not in its own colour");
            let wide_m = g.cell(10, 3, 2);
            assert_eq!(shot.lit_outside(g.text, &[cell, wide_m]), None, "scale {s}");
            assert_eq!(
                shot.at(cell.0, cell.1),
                bgra(CURSOR),
                "scale {s}: the corner"
            );

            // On a wide character: both cells. On its spacer: the same box.
            for at in [10, 11] {
                cursor_at(&mut sc, at, 3, CursorShape::Block);
                let (shot, g) = render(&t, &panel(&sc, &pal), s);
                let two = g.cell(10, 3, 2);
                assert!(
                    shot.count_in(two, bgra(CURSOR), 0) > (two.2 * two.3) as usize * 3 / 4,
                    "scale {s} at {at}"
                );
                let far = (two.0 + two.2 - 1, two.1 + two.3 - 1);
                assert_eq!(
                    shot.at(far.0, far.1),
                    bgra(CURSOR),
                    "scale {s} at {at}: far corner"
                );
                assert_eq!(
                    shot.lit_outside(g.text, &[two, g.cell(7, 3, 1)]),
                    None,
                    "scale {s} at {at}"
                );
            }
        }
    }

    #[test]
    fn hollow_underline_and_beam_cursors() {
        let t = tokens();
        let pal = palette(&t);
        let c = bgra(CURSOR);
        for s in SCALES {
            let bar = (2.0 * s).round() as i32;
            for (col, span) in [(7usize, 1usize), (10, 2)] {
                let mut sc = blank(80, 20, &pal);
                if span == 2 {
                    put(&mut sc, 10, 3, ' ', RED, None, WIDE);
                    put(&mut sc, 11, 3, ' ', RED, None, SPACER);
                }
                let (x, y, w, h) = Geo::new(s).cell(col, 3, span);

                // Hollow: a 1 px outline round the cell(s), nothing inside.
                cursor_at(&mut sc, col, 3, CursorShape::HollowBlock);
                let (shot, g) = render(&t, &panel(&sc, &pal), s);
                for px in 0..w {
                    assert_eq!(shot.at(x + px, y), c, "scale {s}: top");
                    assert_eq!(shot.at(x + px, y + h - 1), c, "scale {s}: bottom");
                }
                for py in 0..h {
                    assert_eq!(shot.at(x, y + py), c, "scale {s}: left");
                    assert_eq!(shot.at(x + w - 1, y + py), c, "scale {s}: right");
                }
                assert_eq!(
                    shot.lit_in((x + 1, y + 1, w - 2, h - 2)),
                    0,
                    "scale {s}: hollow"
                );
                assert_eq!(shot.lit_outside(g.text, &[(x, y, w, h)]), None);

                // Underline: the cell's bottom `bar` rows, across every cell of the character.
                cursor_at(&mut sc, col, 3, CursorShape::Underline);
                let (shot, g) = render(&t, &panel(&sc, &pal), s);
                assert_eq!(
                    shot.count_in((x, y + h - bar, w, bar), c, 0),
                    (w * bar) as usize
                );
                assert_eq!(
                    shot.lit_in((x, y, w, h - bar)),
                    0,
                    "scale {s}: only the bar"
                );
                assert_eq!(shot.lit_outside(g.text, &[(x, y, w, h)]), None);

                // Beam: the cell's left `bar` columns, full height, even on a wide character.
                cursor_at(&mut sc, col, 3, CursorShape::Beam);
                let (shot, g) = render(&t, &panel(&sc, &pal), s);
                assert_eq!(shot.count_in((x, y, bar, h), c, 0), (bar * h) as usize);
                assert_eq!(
                    shot.lit_in((x + bar, y, w - bar, h)),
                    0,
                    "scale {s}: only the beam"
                );
                assert_eq!(shot.lit_outside(g.text, &[(x, y, w, h)]), None);
            }
        }
    }

    #[test]
    fn no_cursor_is_drawn_when_it_is_hidden() {
        let t = tokens();
        let pal = palette(&t);
        let sc = blank(80, 20, &pal);
        let (shot, g) = render(&t, &panel(&sc, &pal), 1.0);
        assert_eq!(shot.lit_in(g.text), 0);
    }

    // ----- selection -----

    #[test]
    fn the_selection_is_the_selection_colour_at_its_alpha_over_the_cells() {
        let t = tokens();
        let pal = palette(&t);
        for s in SCALES {
            let mut sc = blank(80, 20, &pal);
            put(&mut sc, 4, 2, ' ', RED, Some(BLUE), plain());
            sc.selection = vec![
                SelSpan {
                    row: 2,
                    start: 3,
                    end: 9,
                },
                SelSpan {
                    row: 3,
                    start: 0,
                    end: 500,
                },
            ];
            let (shot, g) = render(&t, &panel(&sc, &pal), s);
            let over_blank = blend([0, 0, 0, 0], SELECT, SELECTION_ALPHA);
            let over_blue = blend(bgra(BLUE), SELECT, SELECTION_ALPHA);
            let one = (g.cw * g.ch) as usize;
            // Over an unpainted cell the fill is the colour at its alpha, all over.
            assert_eq!(
                shot.count_in(g.cell(3, 2, 1), over_blank, 3),
                one,
                "scale {s}"
            );
            assert_eq!(
                shot.count_in(g.cell(5, 2, 6), over_blank, 3),
                one * 4,
                "scale {s}"
            );
            // Over a painted one it blends with it.
            assert_eq!(
                shot.count_in(g.cell(4, 2, 1), over_blue, 3),
                one,
                "scale {s}"
            );
            // A span past the last column stops at the grid's edge.
            let row = g.cell(0, 3, 80);
            assert_eq!(shot.count_in(row, over_blank, 3), one * 80);
            assert_eq!(
                shot.lit_outside(g.text, &[g.cell(3, 2, 6), row]),
                None,
                "scale {s}"
            );
        }
    }

    #[test]
    fn a_selection_over_text_leaves_the_glyphs_visible() {
        let t = tokens();
        let pal = palette(&t);
        let mut sc = blank(80, 20, &pal);
        put(&mut sc, 3, 2, 'M', RED, None, plain());
        sc.selection = vec![SelSpan {
            row: 2,
            start: 3,
            end: 4,
        }];
        let (shot, g) = render(&t, &panel(&sc, &pal), 1.0);
        let cell = g.cell(3, 2, 1);
        let fill = blend([0, 0, 0, 0], SELECT, SELECTION_ALPHA);
        assert!(shot.count_in(cell, fill, 3) > 10);
        let ink = blend(bgra(RED), SELECT, SELECTION_ALPHA);
        assert!(shot.closest(cell, ink) < 60, "the glyph shows through");
    }

    // ----- header -----

    #[test]
    fn the_header_has_the_lamp_the_title_the_close_and_a_hairline() {
        let t = tokens();
        let pal = palette(&t);
        let sc = blank(80, 20, &pal);
        for s in SCALES {
            let (shot, g) = render(&t, &panel(&sc, &pal), s);
            // The lamp: the accent at its centre, the halo as the accent at 20 % round it.
            let (lx, ly) = g.pt(20.5, 18.0);
            assert!(
                near(shot.at(lx, ly), bgra(t.accent), 4),
                "scale {s}: lamp {:?}",
                shot.at(lx, ly)
            );
            let halo = blend([0, 0, 0, 0], t.accent, 0.20);
            let probe = shot.at(lx + (4.85 * s) as i32, ly);
            assert!(near(probe, halo, 6), "scale {s}: halo {probe:?}");
            // The title is in the text colour, between the lamp and the close.
            let (tx0, ty0) = g.pt(35.0, 6.0);
            let (tx1, _) = g.pt(layout::TERM_CLOSE.x - 10.0, 0.0);
            let title = (tx0, ty0, tx1 - tx0, (24.0 * s) as i32);
            assert!(shot.lit_in(title) > 20, "scale {s}: title");
            assert!(shot.has_ink(title, t.text), "scale {s}: title colour");
            // The hairline: the header's last row, between the text area's edges.
            let (dx, dy) = (g.text.0 + g.text.2 / 2, g.line_y());
            let line = blend([0, 0, 0, 0], t.text, 0.12);
            assert!(near(shot.at(dx, dy), line, 3), "scale {s}");
            assert_eq!(shot.at(dx, dy - 1)[3], 0, "scale {s}: one device px");
            assert_eq!(shot.at(dx, dy + 1)[3], 0, "scale {s}: one device px");
            assert_eq!(
                shot.at(g.text.0 - 1, dy)[3],
                0,
                "scale {s}: from the text area's edge"
            );
            // Nothing is below the header: the grid is empty.
            assert_eq!(
                shot.lit_outside(shot.whole(), &[g.header()]),
                None,
                "scale {s}"
            );
        }
    }

    #[test]
    fn the_close_cross_is_dim_and_brighter_with_a_plate_on_hover() {
        let t = tokens();
        let pal = palette(&t);
        let sc = blank(80, 20, &pal);
        for s in SCALES {
            let c = layout::TERM_CLOSE;
            let g = Geo::new(s);
            let (cx, cy) = g.pt(c.x + c.w / 2.0, c.y + c.h / 2.0);
            let (px, py) = g.pt(c.x, c.y);
            let rect = (px, py, (c.w * s) as i32, (c.h * s) as i32);
            let rest = render(&t, &panel(&sc, &pal), s).0;
            let hover_panel = TerminalPanel {
                close_hover: true,
                ..panel(&sc, &pal)
            };
            let hover = render(&t, &hover_panel, s).0;
            // Two strokes crossing at the middle, and not a filled box.
            assert!(
                near(rest.at(cx, cy), bgra(t.text_dim), 50),
                "scale {s}: {:?}",
                rest.at(cx, cy)
            );
            let lit = rest.lit_in(rect);
            assert!(
                lit > 10 && lit < (rect.2 * rect.3) as usize / 2,
                "scale {s}: {lit}"
            );
            let side = (px + 1, py + rect.3 / 2);
            assert_eq!(rest.at(side.0, side.1)[3], 0, "no plate at rest");
            // Hover: the text colour on a faint plate.
            assert!(
                near(hover.at(cx, cy), bgra(t.text), 50),
                "scale {s}: {:?}",
                hover.at(cx, cy)
            );
            let plate = blend([0, 0, 0, 0], t.text, 0.10);
            assert!(near(hover.at(side.0, side.1), plate, 4), "scale {s}: plate");
            // Both stay inside the close rect.
            let right = (
                px,
                g.panel.1,
                g.panel.0 + g.panel.2 - px,
                g.line_y() - g.panel.1,
            );
            assert_eq!(hover.lit_outside(right, &[rect]), None, "scale {s}");
            assert_eq!(rest.lit_outside(right, &[rect]), None, "scale {s}");
        }
    }

    #[test]
    fn a_long_title_is_cut_before_the_close_and_an_empty_one_draws_nothing() {
        let t = tokens();
        let pal = palette(&t);
        let sc = blank(80, 20, &pal);
        for s in SCALES {
            let long = "x".repeat(300);
            let p = TerminalPanel {
                title: &long,
                ..panel(&sc, &pal)
            };
            let (shot, g) = render(&t, &p, s);
            let (from, top) = g.pt(layout::TERM_CLOSE.x - 10.0, 0.0);
            let gap = (from, top, (10.0 * s) as i32, (30.0 * s) as i32);
            assert_eq!(
                shot.lit_in(gap),
                0,
                "scale {s}: nothing between title and close"
            );
            let (tx, ty) = g.pt(35.0, 6.0);
            let area = (tx, ty, from - tx, (24.0 * s) as i32);
            assert!(shot.lit_in(area) > 100, "scale {s}: cut, not gone");
            // An empty title draws no text.
            let p = TerminalPanel {
                title: "",
                ..panel(&sc, &pal)
            };
            let (shot, _) = render(&t, &p, s);
            assert_eq!(shot.lit_in(area), 0, "scale {s}");
        }
    }

    // ----- scroll indicator -----

    #[test]
    fn the_scroll_bar_shows_only_while_scrolled_and_follows_the_offset() {
        let t = tokens();
        let pal = palette(&t);
        for s in SCALES {
            let strip = |g: &Geo| {
                let w = (12.0 * s).round() as i32;
                (g.text.0 + g.text.2 - w, g.text.1, w, g.text.3)
            };
            let mut sc = blank(80, 20, &pal);
            sc.history = 100;
            let (shot, g) = render(&t, &panel(&sc, &pal), s);
            assert_eq!(
                shot.lit_in(strip(&g)),
                0,
                "scale {s}: nothing while following"
            );

            let track = g.text.3 as f32;
            let len = (track * 20.0 / 120.0).max(18.0 * s);
            for offset in [1usize, 40, 100] {
                sc.offset = offset;
                let (shot, g) = render(&t, &panel(&sc, &pal), s);
                let (x, y, mx, my) = shot.lit_bounds(strip(&g)).expect("the bar");
                let w = (3.0 * s).round() as i32;
                let right = g.text.0 + g.text.2 - (2.0 * s).round() as i32;
                assert_eq!(
                    (x, mx),
                    (right - w, right - 1),
                    "scale {s}: right edge, {w} px"
                );
                let top = g.text.1 as f32 + (track - len) * (1.0 - offset as f32 / 100.0);
                assert!(
                    (y as f32 - top).abs() <= 2.0,
                    "scale {s} offset {offset}: top {y} vs {top}"
                );
                let got = (my - y + 1) as f32;
                assert!((got - len).abs() <= 2.0, "scale {s}: length {got} vs {len}");
                assert_eq!(
                    shot.lit_outside(shot.whole(), &[g.header(), strip(&g)]),
                    None
                );
            }
            // At the oldest line the bar touches the top.
            sc.offset = 100;
            let (shot, g) = render(&t, &panel(&sc, &pal), s);
            assert!(shot.lit_bounds(strip(&g)).unwrap().1 <= g.text.1 + 1);
            // Scrolled one line into a one-line history: the whole is near full length.
            sc.offset = 1;
            sc.history = 1;
            let (shot, g) = render(&t, &panel(&sc, &pal), s);
            let (_, y, _, my) = shot.lit_bounds(strip(&g)).unwrap();
            assert!(
                y <= g.text.1 + 1 && (my - y + 1) as f32 > track * 0.9,
                "scale {s}"
            );
        }
    }

    // ----- notice -----

    #[test]
    fn the_notice_is_a_line_on_a_panel_coloured_band_over_the_bottom_of_the_text_area() {
        let t = tokens();
        let pal = palette(&t);
        for s in SCALES {
            let mut sc = blank(80, 40, &pal);
            for r in 0..40 {
                for c in 0..80 {
                    put(&mut sc, c, r, ' ', RED, Some(BLUE), plain());
                }
            }
            let p = TerminalPanel {
                notice: Some("process exited · press Enter to restart"),
                ..panel(&sc, &pal)
            };
            let (shot, g) = render(&t, &p, s);
            let band_h = g.ch.max((20.0 * s).round() as i32).min(g.text.3);
            let (left, bottom) = (g.text.0, g.text.1 + g.text.3);
            // The cells above the band are untouched, the band is the panel colour.
            assert_eq!(
                shot.at(left + 2, bottom - band_h - 3),
                bgra(BLUE),
                "scale {s}"
            );
            assert_eq!(shot.at(left + 2, bottom - 3), bgra(pal.bg), "scale {s}");
            let band = (left, bottom - band_h, g.text.2, band_h);
            // The warn dot, then the text in the theme's text colour.
            let (dx, dy) = (left + (10.0 * s) as i32, bottom - band_h / 2);
            assert!(
                near(shot.at(dx, dy), bgra(t.warn), 6),
                "scale {s}: dot {:?}",
                shot.at(dx, dy)
            );
            let text = (dx + (12.0 * s) as i32, band.1 + 1, band.2 / 2, band.3 - 1);
            assert!(
                shot.closest(text, bgra(t.text)) < 60,
                "scale {s}: text colour"
            );
            assert!(shot.count_in(text, bgra(pal.bg), 0) < (text.2 * text.3) as usize);
            assert_eq!(shot.lit_outside(shot.whole(), &[g.header(), g.text]), None);

            // A notice too long for the band is cut and keeps the right edge clear.
            let long = "restart ".repeat(60);
            let p = TerminalPanel {
                notice: Some(&long),
                ..panel(&sc, &pal)
            };
            let (shot, g) = render(&t, &p, s);
            let pad = (8.0 * s) as i32;
            let edge = (
                g.text.0 + g.text.2 - pad,
                bottom - band_h + 2,
                pad,
                band_h - 2,
            );
            assert_eq!(
                shot.count_in(edge, bgra(pal.bg), 0),
                (edge.2 * edge.3) as usize,
                "scale {s}"
            );
            assert_eq!(shot.lit_outside(shot.whole(), &[g.header(), g.text]), None);
        }
    }

    #[test]
    fn without_a_notice_the_last_rows_are_whole() {
        let t = tokens();
        let pal = palette(&t);
        let mut sc = blank(80, 40, &pal);
        for c in 0..80 {
            put(&mut sc, c, 22, ' ', RED, Some(BLUE), plain());
        }
        let (shot, g) = render(&t, &panel(&sc, &pal), 1.0);
        let row = g.cell(0, 22, 80);
        assert_eq!(shot.count_in(row, bgra(BLUE), 0), (row.2 * row.3) as usize);
    }

    // ----- clipping -----

    /// A screen of everything at once, bigger than the text area holds.
    fn dense(pal: &Palette) -> Screen {
        let mut sc = blank(130, 45, pal);
        for r in 0..45 {
            for c in 0..130 {
                let ch = ['M', 'W', '|', '漢', '😀', '\u{e0b0}', '→'][(r * 7 + c) % 7];
                let style = Style {
                    bold: c % 3 == 0,
                    italic: c % 4 == 0,
                    underline: c % 5 == 0,
                    strike: c % 6 == 0,
                    wide: matches!(ch, '漢' | '😀'),
                    spacer: false,
                };
                let bg = if c % 2 == 0 { BLUE } else { GREEN };
                put(&mut sc, c, r, ch, RED, Some(bg), style);
            }
            sc.selection.push(SelSpan {
                row: r,
                start: 2,
                end: 500,
            });
        }
        sc.offset = 30;
        sc.history = 100;
        sc
    }

    #[test]
    fn nothing_is_drawn_outside_the_panel_or_outside_the_text_area_but_the_header() {
        let t = tokens();
        let pal = palette(&t);
        let long = "title ".repeat(80);
        let note = "notice ".repeat(80);
        let shapes = [
            CursorShape::Block,
            CursorShape::HollowBlock,
            CursorShape::Underline,
            CursorShape::Beam,
        ];
        for s in SCALES {
            for shape in shapes {
                let mut sc = dense(&pal);
                cursor_at(&mut sc, 100, 5, shape);
                let p = TerminalPanel {
                    title: &long,
                    notice: Some(&note),
                    close_hover: true,
                    ..panel(&sc, &pal)
                };
                let (shot, g) = render(&t, &p, s);
                assert!(shot.lit_in(g.text) > 1000, "scale {s}: the grid is drawn");
                let outside = shot.lit_outside(shot.whole(), &[g.panel]);
                assert_eq!(outside, None, "scale {s} {shape:?}: outside the panel");
                let outside = shot.lit_outside(shot.whole(), &[g.header(), g.text]);
                assert_eq!(outside, None, "scale {s} {shape:?}: outside the text area");
            }
        }
    }

    #[test]
    fn a_cursor_off_the_grid_is_not_an_error() {
        let t = tokens();
        let pal = palette(&t);
        let mut sc = blank(80, 20, &pal);
        for at in [(500, 3), (3, 500), (80, 20)] {
            cursor_at(&mut sc, at.0, at.1, CursorShape::Block);
            let (shot, g) = render(&t, &panel(&sc, &pal), 1.0);
            assert_eq!(shot.lit_in(g.text), 0, "{at:?}");
        }
    }

    #[test]
    fn the_panel_rect_clips_what_it_is_given() {
        let t = tokens();
        let pal = palette(&t);
        let sc = dense(&pal);
        let p = TerminalPanel {
            notice: Some("x"),
            ..panel(&sc, &pal)
        };
        let mut fonts = Fonts::new().unwrap();
        // A rect smaller than the final size: nothing spills out of it.
        let (w, h) = (900, 500);
        let mut surface = surfaces::raster_n32_premul((w, h)).unwrap();
        surface.canvas().clear(Color::TRANSPARENT);
        let at = LRect::new(40.0, 30.0, 300.0, 200.0);
        paint_terminal(surface.canvas(), &mut fonts, &t, at, 1.0, &p);
        let shot = snapshot(surface, w, h);
        assert!(shot.lit_in((40, 30, 300, 200)) > 1000);
        assert_eq!(shot.lit_outside(shot.whole(), &[(40, 30, 300, 200)]), None);
    }

    #[test]
    fn a_blank_screen_draws_nothing_on_the_grid() {
        let t = tokens();
        let pal = palette(&t);
        for s in SCALES {
            let sc = blank(104, 23, &pal);
            let (shot, g) = render(&t, &panel(&sc, &pal), s);
            assert_eq!(shot.lit_in(g.text), 0, "scale {s}");
            assert!(
                shot.lit_in(g.header()) > 50,
                "scale {s}: but the header is there"
            );
        }
    }

    // ----- cell size -----

    #[test]
    fn the_cell_is_whole_pixels_from_the_advance_of_m_and_the_line_height() {
        let mut fonts = Fonts::new().unwrap();
        for s in SCALES {
            let (cw, ch) = cell_size(&mut fonts, FAMILY, SIZE, s);
            let font = fonts.font(FAMILY, Weight::Regular, SIZE * s);
            let adv = font.measure_str("M", None).0;
            assert_eq!(cw as f32, adv.round(), "scale {s}");
            let (_, m) = font.metrics();
            assert_eq!(
                ch as f32,
                (-m.ascent + m.descent + m.leading).round(),
                "scale {s}"
            );
            // The same every time, from a fresh set of fonts too.
            assert_eq!(cell_size(&mut fonts, FAMILY, SIZE, s), (cw, ch));
            assert_eq!(
                cell_size(&mut Fonts::new().unwrap(), FAMILY, SIZE, s),
                (cw, ch)
            );
        }
        let (w1, h1) = cell_size(&mut fonts, FAMILY, SIZE, 1.0);
        let (w2, h2) = cell_size(&mut fonts, FAMILY, SIZE, 2.0);
        assert!(w1 >= 5 && h1 > w1, "a plausible cell: {w1} x {h1}");
        assert!(
            w2.abs_diff(2 * w1) <= 1 && h2.abs_diff(2 * h1) <= 1,
            "{w1}x{h1} -> {w2}x{h2}"
        );
        assert!(
            cell_size(&mut fonts, FAMILY, 16.0, 1.0).0 > w1,
            "bigger font, bigger cell"
        );
    }

    #[test]
    fn an_unknown_font_gives_geist_mono_cells_and_bad_input_a_usable_cell() {
        let mut fonts = Fonts::new().unwrap();
        let geist = cell_size(&mut fonts, FAMILY, SIZE, 1.5);
        assert_eq!(
            cell_size(&mut fonts, "No Such Font Family 123", SIZE, 1.5),
            geist
        );
        assert_eq!(cell_size(&mut fonts, "", SIZE, 1.5), geist);
        let bad = [
            (0.0, 1.0),
            (-3.0, 1.0),
            (f32::NAN, 1.0),
            (12.0, 0.0),
            (12.0, f32::NAN),
            (1e9, 1.0),
            (12.0, 1e6),
        ];
        for (size, scale) in bad {
            let (cw, ch) = cell_size(&mut fonts, FAMILY, size, scale);
            assert!(cw >= 1 && ch >= 1, "({size}, {scale}) -> {cw}x{ch}");
        }
        let nan = cell_size(&mut fonts, FAMILY, f32::NAN, 1.0);
        assert_eq!(nan, cell_size(&mut fonts, FAMILY, DEFAULT_SIZE, 1.0));
    }

    #[test]
    fn the_cell_font_resolves_bold_and_italic() {
        let mut fonts = Fonts::new().unwrap();
        let regular = fonts.cell_face(FAMILY, false, false);
        assert!(!regular.fake_bold && !regular.fake_italic);
        let bold = fonts.cell_face(FAMILY, true, false);
        assert!(
            bold.fake_bold && !bold.fake_italic,
            "the Medium face emboldened"
        );
        let italic = fonts.cell_face(FAMILY, false, true);
        assert!(
            italic.fake_italic && !italic.fake_bold,
            "no italic face: a skew"
        );
        // A font nobody has is Geist Mono.
        let other = fonts.cell_face("No Such Font Family 123", false, false);
        assert_eq!(other.typeface.unique_id(), regular.typeface.unique_id());
    }
}
