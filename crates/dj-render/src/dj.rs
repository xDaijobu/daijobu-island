//! The dj pills: what `dj progress`, `dj done` and `dj notify` show on the island.
//!
//! Both are compact rest pills (36 px tall, fully rounded) drawn like the Music and Timer
//! compact ones: a lamp 3.2 px in radius with its 20 % halo, 14 px of padding, mono 11 for
//! the progress row and the title face at 12 for the notice. Everything is laid out in
//! logical px and multiplied by `scale`, and, like the other compact layers, anchored at the
//! pill's centre and top at its final size, so the content holds still while the pill grows
//! around it. The content is clipped to the pill's current rect, so a morph in progress never
//! draws outside it.
//!
//! * **Progress**: lamp (accent), the label (`working` when empty), a 4 px bar (track in the
//!   text colour at 15 %, accent fill, width proportional to the percent) and the percent
//!   right-aligned in a fixed slot (so the bar never jumps between 9 %, 10 % and 100 %).
//!   Open-ended: the track only, and an ellipsis where the percent would be. Done: a check
//!   mark drawn as a path in the accent where the lamp was, and the message in its place.
//! * **Notice**: a lamp in `highlight` and the message, the pair centred in the pill.
//!
//! Every glyph that is a fixed part of the look is either ASCII, a glyph the embedded fonts
//! have (the ellipsis, checked at run time) or a drawn shape (the check mark, the dots that
//! stand in for a missing ellipsis). Text is cut with `..` to its slot and clipped to it.

use crate::fonts::{Fonts, Weight};
use crate::paint::{col, draw_text, fill_paint, fit, stroke_paint, text_width};
use dj_core::color::Rgb;
use dj_core::layout;
use dj_core::theme::ResolvedTokens;
use skia_safe::{Canvas, ClipOp, Font, Paint, PathBuilder, RRect, Rect};

/// The progress pill's content. `label` may be empty (drawn as `working`); `percent` is
/// `None` for an open-ended one; `done` is the message of a finished one.
#[derive(Clone, Copy, Debug)]
pub struct DjProgressPill<'a> {
    pub label: &'a str,
    pub percent: Option<u8>,
    pub done: Option<&'a str>,
}

/// The notice pill's content.
#[derive(Clone, Copy, Debug)]
pub struct DjNoticePill<'a> {
    pub message: &'a str,
}

// Logical px. Both pills share a height.
const PROGRESS_W: f32 = layout::DJ_PROGRESS_W;
const NOTICE_W: f32 = layout::DJ_NOTICE_W;
const PILL_H: f32 = layout::DJ_PROGRESS_H;
const _: () = assert!(layout::DJ_PROGRESS_H == layout::DJ_NOTICE_H);
/// Left and right padding, as in the plan's `dev` mock-up.
const PAD: f32 = 14.0;
/// The lamp and its halo (radius, alpha), the same as the idle pill's lamp.
const LAMP_R: f32 = 3.2;
const HALO_R: f32 = 6.5;
const HALO_ALPHA: f32 = 0.20;
/// Between the lamp, the label, the bar and the percent.
const GAP: f32 = 10.0;
const BAR_H: f32 = 4.0;
/// The bar keeps at least this much, however long the label is.
const BAR_MIN_W: f32 = 48.0;
const TRACK_ALPHA: f32 = 0.15;
/// The progress row's mono size and the notice's title size.
const ROW_SIZE: f32 = 11.0;
const NOTICE_SIZE: f32 = 12.0;
/// What an empty label and an empty done message read.
const DEFAULT_LABEL: &str = "working";
const DEFAULT_DONE: &str = "done";
/// The widest percents the slot is sized for.
const WIDEST_PERCENTS: [&str; 2] = ["100%", "88%"];

/// A text with control characters turned into spaces and the ends trimmed, so a stray
/// newline can never draw as a box.
fn plain(s: &str) -> String {
    let spaced: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    spaced.trim().to_string()
}

/// `s` cut with `..` to fit `room` (the island's `fit`), without the space a cut right
/// after a word would leave in front of the dots.
fn cut(font: &Font, s: &str, room: f32) -> String {
    let fitted = fit(font, s, 0.0, room);
    match fitted.strip_suffix("..") {
        Some(head) if fitted != s => format!("{}..", head.trim_end()),
        _ => fitted,
    }
}

/// A band as tall as the pill between two x positions: the room a text may use.
fn band(x0: f32, x1: f32, top: f32, s: f32) -> Rect {
    Rect::new(x0, top, x1.max(x0), top + PILL_H * s)
}

/// `text` with its left edge at `x`, centred vertically in `slot` and clipped to it.
fn text_in(canvas: &Canvas, font: &Font, text: &str, slot: &Rect, x: f32, paint: &Paint) {
    if text.is_empty() || slot.width() <= 0.0 {
        return;
    }
    canvas.save();
    canvas.clip_rect(*slot, ClipOp::Intersect, true);
    draw_text(
        canvas,
        font,
        text,
        x,
        slot.center_y() + 0.35 * font.size(),
        0.0,
        paint,
    );
    canvas.restore();
}

/// The lamp: a halo at 20 % and the solid dot.
fn draw_lamp(canvas: &Canvas, c: (f32, f32), s: f32, ink: Rgb) {
    canvas.draw_circle(c, HALO_R * s, &fill_paint(col(ink, HALO_ALPHA)));
    canvas.draw_circle(c, LAMP_R * s, &fill_paint(col(ink, 1.0)));
}

/// The done mark: a check drawn as a stroked path (no glyph), centred on `c`.
fn draw_check(canvas: &Canvas, c: (f32, f32), s: f32, ink: Rgb) {
    let mut pb = PathBuilder::new();
    pb.move_to((c.0 - 3.8 * s, c.1 + 0.2 * s))
        .line_to((c.0 - 1.3 * s, c.1 + 2.9 * s))
        .line_to((c.0 + 3.9 * s, c.1 - 3.0 * s));
    canvas.draw_path(&pb.detach(), &stroke_paint(col(ink, 1.0), 1.8 * s));
}

/// Whether `font` has a glyph for `c` (Skia does not fall back: a missing one is a box).
fn has_glyph(font: &Font, c: char) -> bool {
    font.unichar_to_glyph(c as i32) != 0
}

/// Three round dots ending at `right`, sitting on `baseline`: the ellipsis when the font has
/// no `…`.
fn draw_dots(canvas: &Canvas, right: f32, baseline: f32, size: f32, paint: &Paint) {
    let r = 0.085 * size;
    let pitch = 0.33 * size;
    for i in 0..3 {
        canvas.draw_circle((right - r - pitch * i as f32, baseline - r), r, paint);
    }
}

/// An ellipsis right-aligned in `slot`: the font's `…`, or three dots.
fn draw_ellipsis(canvas: &Canvas, font: &Font, slot: &Rect, paint: &Paint) {
    if has_glyph(font, '…') {
        let x = slot.right - text_width(font, "…", 0.0);
        text_in(canvas, font, "…", slot, x, paint);
    } else {
        canvas.save();
        canvas.clip_rect(*slot, ClipOp::Intersect, true);
        let baseline = slot.center_y() + 0.35 * font.size();
        draw_dots(canvas, slot.right, baseline, font.size(), paint);
        canvas.restore();
    }
}

/// Where the parts of the progress pill sit, in device px.
struct ProgressGeom {
    /// The pill's top edge; the content is centred on `top + 18 s` (the final height).
    top: f32,
    lamp: (f32, f32),
    /// Where the label (or the done message) starts, and the right edge of the content.
    text_x: f32,
    right: f32,
    /// The label as it is drawn (cut to fit) and the room it may use.
    label: String,
    label_slot: Rect,
    /// The bar's track, on whole device pixels.
    bar: Rect,
    /// The percent's slot (also where an ellipsis goes).
    pct: Rect,
}

impl ProgressGeom {
    fn new(fonts: &mut Fonts, t: &ResolvedTokens, rect: Rect, s: f32, label: &str) -> Self {
        let font = fonts.font(&t.font_label, Weight::Medium, ROW_SIZE * s);
        let left = rect.center_x() - PROGRESS_W * s / 2.0;
        let top = rect.top;
        let cy = top + PILL_H * s / 2.0;
        let lamp = (left + (PAD + LAMP_R) * s, cy);
        let text_x = left + (PAD + 2.0 * LAMP_R + GAP) * s;
        let right = left + (PROGRESS_W - PAD) * s;

        let pct_w = WIDEST_PERCENTS
            .iter()
            .map(|w| text_width(&font, w, 0.0))
            .fold(0.0, f32::max)
            .ceil();
        let pct = band(right - pct_w, right, top, s);
        let bar_right = (pct.left - GAP * s).round();

        let cap = (bar_right - (BAR_MIN_W + GAP) * s - text_x).max(0.0);
        let shown = cut(&font, label, cap);
        let label_w = text_width(&font, &shown, 0.0);
        let bar_left = (text_x + label_w + GAP * s).round();
        let bar_h = (BAR_H * s).round().max(1.0);
        let bar_top = (cy - bar_h / 2.0).round();
        ProgressGeom {
            top,
            lamp,
            text_x,
            right,
            label: shown,
            label_slot: band(text_x, text_x + cap, top, s),
            bar: Rect::new(bar_left, bar_top, bar_right.max(bar_left), bar_top + bar_h),
            pct,
        }
    }
}

/// Content of the progress pill inside `rect` (device px, the pill's current rect): a lamp,
/// the label (mono 11, `working` when empty, cut to fit), a 4 px bar (track in the text
/// colour at 15 %, accent fill) and the percent right-aligned; open-ended: no fill and an
/// ellipsis instead of the percent; done: a drawn check mark in the accent and the message.
///
/// The content is anchored at `rect`'s horizontal centre and top at the pill's final size
/// (300 x 36 logical px) and clipped to `rect`.
pub(crate) fn paint_progress(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    rect: Rect,
    scale: f32,
    p: &DjProgressPill,
) {
    let s = scale;
    let font = fonts.font(&t.font_label, Weight::Medium, ROW_SIZE * s);
    let ink = fill_paint(col(t.text, 1.0));
    canvas.save();
    canvas.clip_rect(rect, ClipOp::Intersect, true);

    if let Some(done) = p.done {
        let g = ProgressGeom::new(fonts, t, rect, s, "");
        draw_check(canvas, g.lamp, s, t.accent);
        let msg = plain(done);
        let msg = if msg.is_empty() { DEFAULT_DONE } else { &msg };
        let slot = band(g.text_x, g.right, g.top, s);
        let line = cut(&font, msg, slot.width());
        text_in(canvas, &font, &line, &slot, g.text_x, &ink);
        canvas.restore();
        return;
    }

    let label = plain(p.label);
    let label = if label.is_empty() {
        DEFAULT_LABEL
    } else {
        &label
    };
    let g = ProgressGeom::new(fonts, t, rect, s, label);
    draw_lamp(canvas, g.lamp, s, t.accent);
    text_in(canvas, &font, &g.label, &g.label_slot, g.text_x, &ink);

    // The track, then the fill on top of it: round at both ends, like the mock-up's.
    let radius = g.bar.height() / 2.0;
    canvas.draw_rrect(
        RRect::new_rect_xy(g.bar, radius, radius),
        &fill_paint(col(t.text, TRACK_ALPHA)),
    );
    match p.percent {
        Some(pct) => {
            let pct = pct.min(100);
            let w = (g.bar.width() * f32::from(pct) / 100.0).round();
            if w > 0.0 {
                let fill = Rect::new(g.bar.left, g.bar.top, g.bar.left + w, g.bar.bottom);
                let r = radius.min(w / 2.0);
                canvas.draw_rrect(
                    RRect::new_rect_xy(fill, r, r),
                    &fill_paint(col(t.accent, 1.0)),
                );
            }
            let text = format!("{pct}%");
            let x = g.pct.right - text_width(&font, &text, 0.0);
            text_in(canvas, &font, &text, &g.pct, x, &ink);
        }
        None => draw_ellipsis(canvas, &font, &g.pct, &ink),
    }
    canvas.restore();
}

/// Where the parts of the notice pill sit, in device px.
struct NoticeGeom {
    lamp: (f32, f32),
    text_x: f32,
    /// The message as it is drawn (cut to fit) and the room it may use.
    message: String,
    slot: Rect,
}

impl NoticeGeom {
    fn new(fonts: &mut Fonts, t: &ResolvedTokens, rect: Rect, s: f32, message: &str) -> Self {
        let font = fonts.font(&t.font_title, Weight::Medium, NOTICE_SIZE * s);
        let top = rect.top;
        let cy = top + PILL_H * s / 2.0;
        let lead = (2.0 * LAMP_R + GAP) * s;
        let room = (NOTICE_W - 2.0 * PAD) * s - lead;
        let shown = cut(&font, message, room);
        let group = lead + text_width(&font, &shown, 0.0);
        // The lamp and the message are centred as a pair, like the idle clock.
        let x0 = rect.center_x() - group / 2.0;
        let text_x = x0 + lead;
        NoticeGeom {
            lamp: (x0 + LAMP_R * s, cy),
            text_x,
            message: shown,
            slot: band(text_x, text_x + room, top, s),
        }
    }
}

/// The notice pill inside `rect` (device px, the pill's current rect): a lamp in
/// `highlight` and the message (title face, 12, cut to fit), the pair centred. Anchored and
/// clipped like [`paint_progress`] (360 x 36 logical px at its final size).
pub(crate) fn paint_notice(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    rect: Rect,
    scale: f32,
    p: &DjNoticePill,
) {
    let s = scale;
    let font = fonts.font(&t.font_title, Weight::Medium, NOTICE_SIZE * s);
    let g = NoticeGeom::new(fonts, t, rect, s, &plain(p.message));
    canvas.save();
    canvas.clip_rect(rect, ClipOp::Intersect, true);
    draw_lamp(canvas, g.lamp, s, t.highlight);
    text_in(
        canvas,
        &font,
        &g.message,
        &g.slot,
        g.text_x,
        &fill_paint(col(t.text, 1.0)),
    );
    canvas.restore();
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::theme::{ThemeRegistry, DEFAULT_THEME};
    use skia_safe::image::CachingHint;
    use skia_safe::{surfaces, AlphaType, Color, ColorType, ImageInfo};

    fn theme(id: &str) -> ResolvedTokens {
        ThemeRegistry::with_builtins().get(id).unwrap().clone()
    }

    fn tokens() -> ResolvedTokens {
        theme(DEFAULT_THEME)
    }

    /// `[b, g, r, a]` of an opaque theme colour, as the CPU surface stores it.
    fn bgra(c: Rgb) -> [u8; 4] {
        let [r, g, b] = c.to_u8();
        [b, g, r, 255]
    }

    fn near(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
        a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= tol)
    }

    /// A theme colour at `alpha` on nothing: premultiplied, as the surface stores it.
    fn premul(c: Rgb, alpha: f32) -> [u8; 4] {
        let a = (alpha * 255.0 + 0.5).floor() / 255.0;
        let [r, g, b] = c.to_u8();
        let q = |v: u8| (v as f32 * a).round() as u8;
        [q(b), q(g), q(r), (a * 255.0).round() as u8]
    }

    /// An opaque pixel as a colour.
    fn rgb_of(px: [u8; 4]) -> Rgb {
        Rgb::new(
            px[2] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[0] as f32 / 255.0,
        )
    }

    /// A painted CPU surface (BGRA, premultiplied).
    struct Shot {
        w: i32,
        h: i32,
        px: Vec<u8>,
    }

    /// x, y, w, h in device px.
    type IRect = (i32, i32, i32, i32);

    impl Shot {
        fn at(&self, x: i32, y: i32) -> [u8; 4] {
            assert!(x >= 0 && y >= 0 && x < self.w && y < self.h, "({x},{y})");
            let i = ((y * self.w + x) * 4) as usize;
            [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
        }

        /// The device pixels of a Skia rect that has any part inside it.
        fn irect(r: &Rect) -> IRect {
            let (x0, y0) = (r.left.floor() as i32, r.top.floor() as i32);
            let (x1, y1) = (r.right.ceil() as i32, r.bottom.ceil() as i32);
            (x0, y0, x1 - x0, y1 - y0)
        }

        fn lit(&self, x: i32, y: i32) -> bool {
            self.at(x, y)[3] > 0
        }

        /// Pixels with any coverage inside the rect.
        fn lit_in(&self, r: IRect) -> usize {
            (r.1..r.1 + r.3)
                .flat_map(|y| (r.0..r.0 + r.2).map(move |x| (x, y)))
                .filter(|&(x, y)| self.lit(x, y))
                .count()
        }

        /// Pixels within `tol` of `c` inside the rect.
        fn count_in(&self, r: IRect, c: [u8; 4], tol: u8) -> usize {
            (r.1..r.1 + r.3)
                .flat_map(|y| (r.0..r.0 + r.2).map(move |x| (x, y)))
                .filter(|&(x, y)| near(self.at(x, y), c, tol))
                .count()
        }

        /// The first lit pixel outside the rect, if any.
        fn lit_outside(&self, r: &Rect) -> Option<(i32, i32)> {
            let r = Self::irect(r);
            (0..self.h)
                .flat_map(|y| (0..self.w).map(move |x| (x, y)))
                .find(|&(x, y)| {
                    let within = x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3;
                    !within && self.lit(x, y)
                })
        }

        /// The bounding box (x0, y0, x1, y1 inclusive) of the lit pixels inside `r`.
        fn bbox(&self, r: IRect) -> Option<(i32, i32, i32, i32)> {
            let mut b: Option<(i32, i32, i32, i32)> = None;
            for y in r.1..r.1 + r.3 {
                for x in r.0..r.0 + r.2 {
                    if self.lit(x, y) {
                        b = Some(match b {
                            None => (x, y, x, y),
                            Some(o) => (o.0.min(x), o.1.min(y), o.2.max(x), o.3.max(y)),
                        });
                    }
                }
            }
            b
        }
    }

    /// Draws on a CPU surface of `size` (transparent, or the pill colour when `pill`).
    fn shot(
        t: &ResolvedTokens,
        pill: bool,
        size: (i32, i32),
        draw: impl FnOnce(&Canvas, &mut Fonts),
    ) -> Shot {
        let mut fonts = Fonts::new().unwrap();
        let mut surface = surfaces::raster_n32_premul(size).unwrap();
        let [r, g, b] = t.pill.to_u8();
        surface.canvas().clear(if pill {
            Color::from_rgb(r, g, b)
        } else {
            Color::TRANSPARENT
        });
        draw(surface.canvas(), &mut fonts);
        let info = ImageInfo::new(size, ColorType::BGRA8888, AlphaType::Premul, None);
        let mut px = vec![0u8; (size.0 * size.1 * 4) as usize];
        assert!(surface.image_snapshot().read_pixels(
            &info,
            &mut px,
            (size.0 * 4) as usize,
            (0, 0),
            CachingHint::Disallow
        ));
        Shot {
            w: size.0,
            h: size.1,
            px,
        }
    }

    /// The pill's rect at `scale`: 20 px from the left, 10 from the top.
    fn pill_rect(w: f32, scale: f32) -> Rect {
        Rect::from_xywh(20.0, 10.0, w * scale, PILL_H * scale)
    }

    fn size_for(rect: &Rect) -> (i32, i32) {
        (
            rect.right.ceil() as i32 + 20,
            rect.bottom.ceil() as i32 + 10,
        )
    }

    fn progress(t: &ResolvedTokens, scale: f32, p: &DjProgressPill, pill: bool) -> Shot {
        let rect = pill_rect(PROGRESS_W, scale);
        shot(t, pill, size_for(&rect), |c, f| {
            paint_progress(c, f, t, rect, scale, p)
        })
    }

    fn notice(t: &ResolvedTokens, scale: f32, message: &str, pill: bool) -> Shot {
        let rect = pill_rect(NOTICE_W, scale);
        shot(t, pill, size_for(&rect), |c, f| {
            paint_notice(c, f, t, rect, scale, &DjNoticePill { message })
        })
    }

    fn working<'a>(label: &'a str, percent: Option<u8>) -> DjProgressPill<'a> {
        DjProgressPill {
            label,
            percent,
            done: None,
        }
    }

    fn finished(message: &str) -> DjProgressPill<'_> {
        DjProgressPill {
            label: "ignored",
            percent: Some(40),
            done: Some(message),
        }
    }

    fn geom(t: &ResolvedTokens, scale: f32, label: &str) -> ProgressGeom {
        let mut fonts = Fonts::new().unwrap();
        ProgressGeom::new(
            &mut fonts,
            t,
            pill_rect(PROGRESS_W, scale),
            scale,
            &plain(label),
        )
    }

    const SCALES: [f32; 3] = [1.0, 1.5, 2.0];

    // ----- the progress pill -----

    #[test]
    fn the_lamp_is_the_accent_with_a_faint_halo() {
        let t = tokens();
        for s in SCALES {
            let sh = progress(&t, s, &working("deploy", Some(40)), false);
            let g = geom(&t, s, "deploy");
            let (cx, cy) = (g.lamp.0.floor() as i32, g.lamp.1.floor() as i32);
            assert!(near(sh.at(cx, cy), bgra(t.accent), 2), "centre at {s}");
            // Between the dot (3.2) and the halo's edge (6.5): the accent at 20 %.
            let off = (5.0 * s).round() as i32;
            assert!(
                near(sh.at(cx - off, cy), premul(t.accent, HALO_ALPHA), 4),
                "halo at {s}: {:?}",
                sh.at(cx - off, cy)
            );
            // Past the halo: nothing.
            let far = (7.5 * s).round() as i32;
            assert!(!sh.lit(cx - far, cy), "outside the halo at {s}");
        }
    }

    #[test]
    fn the_bar_fill_is_proportional_to_the_percent() {
        let t = tokens();
        for s in SCALES {
            let g = geom(&t, s, "deploy");
            let (bx, by, bw, bh) = Shot::irect(&g.bar);
            assert_eq!(bh, (4.0 * s).round() as i32, "bar height at {s}");
            let mid = by + bh / 2;
            for pct in [0u8, 1, 25, 50, 75, 99, 100] {
                let sh = progress(&t, s, &working("deploy", Some(pct)), false);
                let row = (bx - 2, mid, bw + 4, 1);
                let fill = sh.count_in(row, bgra(t.accent), 4) as f32;
                let track = sh.count_in(row, premul(t.text, TRACK_ALPHA), 4) as f32;
                let want = (bw as f32 * pct as f32 / 100.0).round();
                assert!(
                    (fill - want).abs() <= 2.0,
                    "{pct} % at {s}: fill {fill}, want {want}"
                );
                assert!(
                    (fill + track - bw as f32).abs() <= 3.0,
                    "{pct} % at {s}: fill {fill} + track {track} should be the bar's {bw}"
                );
                // The bar is exactly its height, and nothing but it sits above and below.
                let col_x = bx + bw - 3;
                let lit = (by - 3..by + bh + 3).filter(|&y| sh.lit(col_x, y)).count();
                assert_eq!(lit as i32, bh, "bar thickness at {s}");
            }
        }
    }

    #[test]
    fn the_fill_grows_to_the_right_and_a_larger_percent_never_shrinks_it() {
        let t = tokens();
        let g = geom(&t, 1.0, "deploy");
        let (bx, by, bw, bh) = Shot::irect(&g.bar);
        let mut last = 0;
        for pct in (0u8..=100).step_by(5) {
            let sh = progress(&t, 1.0, &working("deploy", Some(pct)), false);
            let n = sh.count_in((bx - 1, by + bh / 2, bw + 2, 1), bgra(t.accent), 4);
            assert!(n >= last, "{pct} %: {n} < {last}");
            last = n;
        }
        // Over 100 draws a full bar, not a longer one.
        let over = progress(&t, 1.0, &working("deploy", Some(250)), false);
        assert_eq!(over.lit_outside(&pill_rect(PROGRESS_W, 1.0)), None);
        let full = over.count_in((bx - 1, by + bh / 2, bw + 2, 1), bgra(t.accent), 4);
        assert!((full as i32 - bw).abs() <= 2);
    }

    #[test]
    fn the_percent_is_right_aligned_in_its_slot_and_the_bar_does_not_move() {
        let t = tokens();
        let g = geom(&t, 1.0, "deploy");
        let slot = Shot::irect(&g.pct);
        let right = g.pct.right.round() as i32;
        let bar = Shot::irect(&g.bar);
        let mid = bar.1 + bar.3 / 2;
        for pct in [5u8, 40, 100] {
            let sh = progress(&t, 1.0, &working("deploy", Some(pct)), false);
            let ink = sh.bbox(slot).expect("the percent is drawn");
            assert!(ink.2 <= right, "{pct} %: ink ends at {}", ink.2);
            assert!(ink.2 >= right - 3, "{pct} %: flush right ({})", ink.2);
            assert!(ink.0 >= slot.0, "{pct} %: inside the slot");
            // The mono 11 digits are about 8 px tall.
            assert!((6..=11).contains(&(ink.3 - ink.1 + 1)), "digit height");
            // Wider percents do not push the bar: it starts and ends in the same places,
            // with the gap before and after it empty.
            assert!(
                sh.lit(bar.0, mid) && sh.lit(bar.0 + bar.2 - 1, mid),
                "{pct} %: ends"
            );
            assert!(
                !sh.lit(bar.0 - 2, mid) && !sh.lit(bar.0 + bar.2 + 1, mid),
                "{pct} %: gaps"
            );
        }
    }

    #[test]
    fn an_open_ended_progress_is_a_track_and_an_ellipsis_and_has_no_fill() {
        let t = tokens();
        for s in SCALES {
            let sh = progress(&t, s, &working("build", None), false);
            let g = geom(&t, s, "build");
            let bar = Shot::irect(&g.bar);
            assert_eq!(sh.count_in(bar, bgra(t.accent), 40), 0, "no fill at {s}");
            let row = (bar.0 - 1, bar.1 + bar.3 / 2, bar.2 + 2, 1);
            let track = sh.count_in(row, premul(t.text, TRACK_ALPHA), 4);
            assert!((track as i32 - bar.2).abs() <= 3, "track at {s}: {track}");
            // The slot holds something small and low: an ellipsis, not digits.
            let slot = Shot::irect(&g.pct);
            let ink = sh.bbox(slot).expect("an ellipsis");
            let some = progress(&t, s, &working("build", Some(50)), false);
            assert!(sh.lit_in(slot) < some.lit_in(slot), "less ink than 50%");
            assert!(
                (ink.3 - ink.1 + 1) as f32 <= 3.0 * s + 1.0,
                "dots are at most {} px tall at {s}",
                3.0 * s + 1.0
            );
            assert!(ink.1 > g.lamp.1 as i32, "the dots sit on the baseline");
            assert!(ink.2 <= g.pct.right.round() as i32, "flush right");
            assert_eq!(sh.lit_outside(&pill_rect(PROGRESS_W, s)), None);
        }
    }

    #[test]
    fn the_ellipsis_is_a_glyph_the_font_has_or_three_drawn_dots() {
        let mut fonts = Fonts::new().unwrap();
        // Every built-in theme's progress font has the glyph, so the real thing is drawn.
        let reg = ThemeRegistry::with_builtins();
        for id in reg.ids() {
            let t = reg.get(id).unwrap();
            let font = fonts.font(&t.font_label, Weight::Medium, 11.0);
            assert!(has_glyph(&font, '…'), "{id}: {} lacks …", t.font_label);
        }
        // A font without it takes the dots path.
        let font = fonts.font("Geist Mono", Weight::Medium, 11.0);
        assert!(
            !has_glyph(&font, '\u{E000}'),
            "a private-use char is missing"
        );
        let sh = shot(&tokens(), false, (40, 20), |c, _| {
            let paint = fill_paint(Color::WHITE);
            draw_dots(c, 30.0, 14.0, 11.0, &paint);
        });
        // Three separate runs of lit columns, all above the baseline.
        let cols: Vec<bool> = (0..40).map(|x| (0..20).any(|y| sh.lit(x, y))).collect();
        let runs = cols.windows(2).filter(|w| !w[0] && w[1]).count();
        assert_eq!(runs, 3, "{cols:?}");
        assert!(sh.bbox((0, 0, 40, 20)).unwrap().3 <= 14);
        assert!(sh.bbox((0, 0, 40, 20)).unwrap().2 <= 30);
        // The real ellipsis is flush right in its slot and stays inside it.
        let slot = Rect::from_xywh(2.0, 0.0, 30.0, 20.0);
        let glyph = shot(&tokens(), false, (40, 20), |c, f| {
            let font = f.font("Geist Mono", Weight::Medium, 11.0);
            draw_ellipsis(c, &font, &slot, &fill_paint(Color::WHITE));
        });
        let ink = glyph.bbox((0, 0, 40, 20)).expect("ellipsis");
        assert!(ink.0 >= 2 && ink.2 <= 32 && ink.2 >= 29, "{ink:?}");
    }

    #[test]
    fn an_empty_label_reads_working() {
        let t = tokens();
        let named = progress(&t, 1.0, &working("working", Some(30)), false);
        for blank in ["", "   ", "\n\t"] {
            let sh = progress(&t, 1.0, &working(blank, Some(30)), false);
            assert_eq!(sh.px, named.px, "{blank:?}");
        }
        // And it is text: the label area has ink.
        let g = geom(&t, 1.0, "working");
        assert!(named.lit_in(Shot::irect(&g.label_slot)) > 30);
    }

    #[test]
    fn a_long_label_is_cut_before_the_bar_and_the_bar_keeps_its_room() {
        let t = tokens();
        let long = "W".repeat(60);
        let long2 = "deploy production eu-west-1 canary rollout stage three of five";
        for s in SCALES {
            for label in [long.as_str(), long2] {
                let sh = progress(&t, s, &working(label, Some(60)), false);
                let g = geom(&t, s, label);
                assert_eq!(sh.lit_outside(&pill_rect(PROGRESS_W, s)), None, "{label}");
                assert!(
                    g.label.ends_with(".."),
                    "{label}: cut with two dots, got {:?}",
                    g.label
                );
                assert!(
                    g.bar.width() >= BAR_MIN_W * s - 1.0,
                    "bar {}",
                    g.bar.width()
                );
                // The label is cut inside its slot: the gap before the bar is empty.
                let gap = (
                    g.bar.left as i32 - (7.0 * s) as i32,
                    0,
                    (7.0 * s) as i32,
                    sh.h,
                );
                assert_eq!(sh.lit_in(gap), 0, "{label}: gap before the bar at {s}");
                // The label ends within its slot.
                let slot = Shot::irect(&g.label_slot);
                let ink = sh.bbox(slot).expect("label ink");
                assert!(ink.2 as f32 <= g.label_slot.right + 1.0, "{label}");
                // The long label has ink right up to near the end of its room.
                let tail = (
                    g.label_slot.right as i32 - (14.0 * s) as i32,
                    slot.1,
                    (14.0 * s) as i32,
                    slot.3,
                );
                assert!(sh.lit_in(tail) > 0, "{label}: fills its room at {s}");
                // The percent is untouched by the label.
                let some = progress(&t, s, &working("x", Some(60)), false);
                let pct = Shot::irect(&g.pct);
                assert_eq!(sh.lit_in(pct), some.lit_in(pct), "{label}");
            }
        }
    }

    #[test]
    fn a_cut_never_leaves_a_space_before_the_dots() {
        let mut fonts = Fonts::new().unwrap();
        let font = fonts.font("Geist Mono", Weight::Medium, 11.0);
        let s = "cargo build --release for the whole workspace";
        // Every room from nothing to the whole text: always fits, never "word ..".
        let full = text_width(&font, s, 0.0);
        let mut cuts = 0;
        for room in (0..=full as i32 + 4).step_by(3) {
            let shown = cut(&font, s, room as f32);
            assert!(text_width(&font, &shown, 0.0) <= room as f32 || shown.is_empty());
            assert!(!shown.ends_with(" .."), "{room}: {shown:?}");
            cuts += shown.ends_with("..") as i32;
        }
        assert!(cuts > 10, "most rooms cut the text");
        // The whole text and a text that ends in dots are left alone.
        assert_eq!(cut(&font, s, full + 1.0), s);
        assert_eq!(cut(&font, "wait..", 500.0), "wait..");
    }

    #[test]
    fn a_short_label_leaves_the_bar_nearly_the_whole_row() {
        let t = tokens();
        let short = geom(&t, 1.0, "ci");
        let long = geom(&t, 1.0, "a much longer label than that");
        assert!(short.bar.width() > long.bar.width());
        assert!(short.bar.left < long.bar.left);
        // Same right end whatever the label.
        assert_eq!(short.bar.right, long.bar.right);
        assert!(short.bar.width() <= PROGRESS_W - 2.0 * PAD - 40.0);
    }

    // ----- done -----

    #[test]
    fn done_is_a_drawn_check_in_the_accent_and_the_message_with_no_bar() {
        let t = tokens();
        for s in SCALES {
            let sh = progress(&t, s, &finished("ok"), false);
            let g = geom(&t, s, "ok");
            // The mark: accent pixels in a box about the lamp's slot, about 8 x 6 px.
            let slot = (
                (g.lamp.0 - 6.0 * s).floor() as i32,
                (g.lamp.1 - 6.0 * s).floor() as i32,
                (12.0 * s).ceil() as i32,
                (12.0 * s).ceil() as i32,
            );
            let solid = (slot.1..slot.1 + slot.3)
                .flat_map(|y| (slot.0..slot.0 + slot.2).map(move |x| (x, y)))
                .filter(|&(x, y)| near(sh.at(x, y), bgra(t.accent), 6))
                .count();
            assert!(solid as f32 >= 8.0 * s, "check pixels at {s}: {solid}");
            let ink = sh.bbox(slot).expect("the check mark");
            let (w, h) = ((ink.2 - ink.0 + 1) as f32, (ink.3 - ink.1 + 1) as f32);
            assert!(w >= 7.0 * s && w <= 11.0 * s, "check width {w} at {s}");
            assert!(h >= 5.0 * s && h <= 9.0 * s, "check height {h} at {s}");
            assert!(w > h, "a check is wider than tall");
            // It is not a lamp: the disc's halo ring is absent and it is not round.
            let off = (5.0 * s).round() as i32;
            assert!(
                !near(
                    sh.at(g.lamp.0 as i32 - off, g.lamp.1 as i32),
                    premul(t.accent, HALO_ALPHA),
                    4
                ),
                "no halo behind the check"
            );
            // The message is in the text colour, after the mark, and no bar or percent.
            let msg = (g.text_x as i32, 0, (24.0 * s) as i32, sh.h);
            assert!(sh.count_in(msg, bgra(t.text), 20) > 6, "message ink at {s}");
            let after = g.text_x as i32 + (20.0 * s) as i32;
            assert_eq!(
                sh.lit_in((after, 0, g.pct.right as i32 - after, sh.h)),
                0,
                "no bar track and no percent after the message at {s}"
            );
            assert_eq!(sh.lit_outside(&pill_rect(PROGRESS_W, s)), None);
        }
    }

    #[test]
    fn done_with_an_empty_message_reads_done_and_a_long_one_is_cut() {
        let t = tokens();
        let named = progress(&t, 1.0, &finished("done"), false);
        for blank in ["", "  "] {
            assert_eq!(progress(&t, 1.0, &finished(blank), false).px, named.px);
        }
        let g = geom(&t, 1.0, "");
        let long = "x".repeat(120);
        let sh = progress(&t, 1.0, &finished(&long), false);
        assert_eq!(sh.lit_outside(&pill_rect(PROGRESS_W, 1.0)), None);
        let ink = sh
            .bbox((g.text_x as i32, 0, sh.w - g.text_x as i32, sh.h))
            .expect("message");
        assert!(
            ink.2 as f32 <= g.right + 1.0,
            "ends at {} of {}",
            ink.2,
            g.right
        );
        assert!(ink.2 as f32 >= g.right - 14.0, "fills the row: {}", ink.2);
        // The done mark does not depend on the stale progress it replaces.
        let other = progress(
            &t,
            1.0,
            &DjProgressPill {
                label: "different",
                percent: None,
                done: Some("done"),
            },
            false,
        );
        assert_eq!(other.px, named.px);
    }

    // ----- the notice -----

    #[test]
    fn the_notice_is_a_highlight_lamp_and_its_message_centred_inside_the_pill() {
        let t = tokens();
        for s in SCALES {
            let sh = notice(&t, s, "tests failed", false);
            let rect = pill_rect(NOTICE_W, s);
            let mut fonts = Fonts::new().unwrap();
            let g = NoticeGeom::new(&mut fonts, &t, rect, s, "tests failed");
            let (cx, cy) = (g.lamp.0.floor() as i32, g.lamp.1.floor() as i32);
            assert!(near(sh.at(cx, cy), bgra(t.highlight), 2), "lamp at {s}");
            let off = (5.0 * s).round() as i32;
            assert!(
                near(sh.at(cx - off, cy), premul(t.highlight, HALO_ALPHA), 4),
                "halo at {s}"
            );
            // The message is in the text colour, right of the lamp.
            let after = (
                g.text_x as i32,
                rect.top as i32,
                (rect.right as i32 - g.text_x as i32).min(sh.w - g.text_x as i32),
                rect.height() as i32,
            );
            assert!(
                sh.count_in(after, bgra(t.text), 20) > 10,
                "message ink at {s}"
            );
            // The pair is centred: the lamp is left of the centre, the text reaches past it.
            let font = fonts.font(&t.font_title, Weight::Medium, 12.0 * s);
            assert!(g.lamp.0 < rect.center_x() - 10.0 * s);
            assert!(g.text_x + text_width(&font, &g.message, 0.0) > rect.center_x());
            assert_eq!(sh.lit_outside(&rect), None);
        }
    }

    #[test]
    fn a_long_notice_is_cut_inside_the_padding() {
        let t = tokens();
        let long = "the quick brown fox jumps over the lazy dog and keeps running ".repeat(2);
        for s in SCALES {
            let sh = notice(&t, s, &long, false);
            let rect = pill_rect(NOTICE_W, s);
            assert_eq!(sh.lit_outside(&rect), None);
            let ink = sh.bbox(Shot::irect(&rect)).expect("ink");
            // The halo is on the left; the text ends at the right padding at most.
            assert!(
                ink.2 as f32 <= rect.right - PAD * s + 1.0,
                "ends at {} of {} at {s}",
                ink.2,
                rect.right - PAD * s
            );
            assert!(
                ink.0 as f32 >= rect.left + PAD * s - 4.0 * s,
                "starts at {}",
                ink.0
            );
            // A full row: the cut text reaches near the right padding.
            assert!(ink.2 as f32 >= rect.right - PAD * s - 12.0 * s, "{}", ink.2);
            let mut fonts = Fonts::new().unwrap();
            let g = NoticeGeom::new(&mut fonts, &t, rect, s, &long);
            assert!(g.message.ends_with(".."), "{:?}", g.message);
        }
    }

    #[test]
    fn control_characters_never_draw_as_boxes() {
        let t = tokens();
        // Control characters become spaces and the ends are trimmed: the same pixels.
        let spaced = notice(&t, 1.0, "line one line two", false);
        let messy = notice(&t, 1.0, "\nline one\nline two\u{7f}", false);
        assert_eq!(spaced.px, messy.px);
        let a = progress(&t, 1.0, &working("a b", Some(5)), false);
        let b = progress(&t, 1.0, &working("a\tb\r\n", Some(5)), false);
        assert_eq!(a.px, b.px);
        assert_eq!(plain("\n a\tb \r"), "a b");
    }

    // ----- anywhere on the pill -----

    #[test]
    fn nothing_is_drawn_outside_the_pills_current_rect_while_it_grows() {
        let t = tokens();
        // A pill part way through its morph: smaller than its final size, off to one side.
        for (w, h) in [(120.0, 30.0), (200.0, 36.0), (80.0, 20.0), (300.0, 36.0)] {
            let rect = Rect::from_xywh(70.0, 12.0, w, h);
            let size = (400, 70);
            let inside = |sh: &Shot, what: &str, shows: bool| {
                assert_eq!(sh.lit_outside(&rect), None, "{what} in {w} x {h}");
                assert!(
                    !shows || sh.lit_in(Shot::irect(&rect)) > 0,
                    "{what} in {w} x {h}: something shows"
                );
            };
            let p = shot(&t, false, size, |c, f| {
                paint_progress(c, f, &t, rect, 1.0, &working("deploy", Some(60)))
            });
            inside(&p, "progress", true);
            let o = shot(&t, false, size, |c, f| {
                paint_progress(c, f, &t, rect, 1.0, &working("deploy", None))
            });
            inside(&o, "open-ended", true);
            let d = shot(&t, false, size, |c, f| {
                paint_progress(c, f, &t, rect, 1.0, &finished("all good"))
            });
            // The message hangs off the left of a narrow rect: nothing of it shows there.
            inside(&d, "done", w >= 200.0);
            let n = shot(&t, false, size, |c, f| {
                paint_notice(
                    c,
                    f,
                    &t,
                    rect,
                    1.0,
                    &DjNoticePill {
                        message: "hello there",
                    },
                )
            });
            inside(&n, "notice", true);
        }
    }

    #[test]
    fn the_content_holds_still_while_the_pill_grows_around_it() {
        let t = tokens();
        let full = Rect::from_xywh(70.0, 12.0, 300.0, 36.0);
        let small = Rect::from_xywh(70.0 + 60.0, 12.0, 180.0, 36.0);
        let size = (400, 70);
        let draw = |rect: Rect| {
            shot(&t, false, size, |c, f| {
                paint_progress(c, f, &t, rect, 1.0, &working("deploy", Some(60)))
            })
        };
        let (a, b) = (draw(full), draw(small));
        // The part of the small rect that both show is pixel for pixel the same.
        for y in 12..48 {
            for x in 130..310 {
                assert_eq!(a.at(x, y), b.at(x, y), "({x},{y})");
            }
        }
    }

    #[test]
    fn a_zero_size_rect_draws_nothing_and_does_not_panic() {
        let t = tokens();
        let rect = Rect::from_xywh(100.0, 20.0, 0.0, 0.0);
        let sh = shot(&t, false, (300, 60), |c, f| {
            paint_progress(c, f, &t, rect, 1.0, &working("deploy", Some(60)));
            paint_progress(c, f, &t, rect, 1.0, &finished("ok"));
            paint_notice(c, f, &t, rect, 1.0, &DjNoticePill { message: "hi" });
        });
        assert_eq!(sh.lit_in((0, 0, 300, 60)), 0);
    }

    #[test]
    fn at_150_percent_everything_scales_by_one_and_a_half() {
        let t = tokens();
        let one = progress(&t, 1.0, &working("deploy", Some(50)), false);
        let big = progress(&t, 1.5, &working("deploy", Some(50)), false);
        let (g1, g2) = (geom(&t, 1.0, "deploy"), geom(&t, 1.5, "deploy"));
        // The bar's thickness and length, and the lamp, scale by 1.5.
        assert_eq!(g1.bar.height(), 4.0);
        assert_eq!(g2.bar.height(), 6.0);
        assert!((g2.bar.width() / g1.bar.width() - 1.5).abs() < 0.02);
        assert!(
            (g2.lamp.1 - g2.top - 27.0).abs() < 0.01,
            "cy is 27 px below the top"
        );
        // The label text is 1.5 times as tall (and wide).
        let ink = |sh: &Shot, g: &ProgressGeom| sh.bbox(Shot::irect(&g.label_slot)).unwrap();
        let (a, b) = (ink(&one, &g1), ink(&big, &g2));
        let (ah, bh) = ((a.3 - a.1 + 1) as f32, (b.3 - b.1 + 1) as f32);
        let (aw, bw) = ((a.2 - a.0 + 1) as f32, (b.2 - b.0 + 1) as f32);
        assert!((bh / ah - 1.5).abs() < 0.25, "text height {ah} -> {bh}");
        assert!((bw / aw - 1.5).abs() < 0.1, "text width {aw} -> {bw}");
        assert_eq!(big.lit_outside(&pill_rect(PROGRESS_W, 1.5)), None);
        // The fill is half of the bar at either scale.
        let half = |sh: &Shot, g: &ProgressGeom| {
            let r = Shot::irect(&g.bar);
            sh.count_in((r.0, r.1 + r.3 / 2, r.2, 1), bgra(t.accent), 4) as f32 / r.2 as f32
        };
        assert!((half(&one, &g1) - 0.5).abs() < 0.02);
        assert!((half(&big, &g2) - 0.5).abs() < 0.02);
    }

    // ----- fonts and themes -----

    #[test]
    fn the_fixed_glyphs_are_in_the_embedded_fonts() {
        let mut fonts = Fonts::new().unwrap();
        let reg = ThemeRegistry::with_builtins();
        let fixed = format!("{DEFAULT_LABEL}{DEFAULT_DONE}0123456789%.");
        for id in reg.ids() {
            let t = reg.get(id).unwrap();
            let label = fonts.typeface(&t.font_label, Weight::Medium);
            for c in fixed.chars() {
                assert_ne!(
                    label.unichar_to_glyph(c as i32),
                    0,
                    "{id}: {} lacks {c}",
                    t.font_label
                );
            }
            // The progress row's ellipsis is a real glyph in every built-in theme.
            assert_ne!(
                label.unichar_to_glyph('…' as i32),
                0,
                "{id}: {} lacks …",
                t.font_label
            );
            // `fit` cuts with two ASCII dots, in both faces.
            let title = fonts.typeface(&t.font_title, Weight::Medium);
            assert_ne!(title.unichar_to_glyph('.' as i32), 0, "{id}: title face");
        }
        // The two marks that are not in any font are drawn: neither is ever sent as text.
        let mono = fonts.typeface("Geist Mono", Weight::Medium);
        for c in ['✓', '‥'] {
            assert_eq!(
                mono.unichar_to_glyph(c as i32),
                0,
                "{c} is not in Geist Mono"
            );
        }
    }

    #[test]
    fn the_text_reads_on_the_pill_in_every_builtin_theme() {
        let reg = ThemeRegistry::with_builtins();
        for id in reg.ids() {
            let t = reg.get(id).unwrap();
            assert!(t.text.contrast(t.pill) >= 4.5, "{id}: text on the pill");
            let s = 2.0;
            // The best contrast any pixel of the text reaches against the pill: a fully
            // covered stem is the text colour itself, so it must clear 4.5:1.
            let best = |sh: &Shot, r: IRect| {
                (r.1..r.1 + r.3)
                    .flat_map(|y| (r.0..r.0 + r.2).map(move |x| (x, y)))
                    .map(|(x, y)| rgb_of(sh.at(x, y)).contrast(t.pill))
                    .fold(1.0_f32, f32::max)
            };
            let g = geom(t, s, "deploy");
            let label = Shot::irect(&g.label_slot);
            let pct = Shot::irect(&g.pct);
            let sh = progress(t, s, &working("deploy", Some(40)), true);
            assert!(
                best(&sh, label) >= 4.5,
                "{id}: label {:.2}",
                best(&sh, label)
            );
            assert!(best(&sh, pct) >= 4.5, "{id}: percent {:.2}", best(&sh, pct));
            let open = progress(t, s, &working("deploy", None), true);
            assert!(
                best(&open, pct) >= 4.5,
                "{id}: ellipsis {:.2}",
                best(&open, pct)
            );
            let done = progress(t, s, &finished("build ok"), true);
            let line = (g.text_x as i32, g.top as i32, 100, 72);
            assert!(
                best(&done, line) >= 4.5,
                "{id}: done message {:.2}",
                best(&done, line)
            );
            let rect = pill_rect(NOTICE_W, s);
            let n = notice(t, s, "tests failed", true);
            let mut fonts = Fonts::new().unwrap();
            let ng = NoticeGeom::new(&mut fonts, t, rect, s, "tests failed");
            let text = (ng.text_x as i32, rect.top as i32, 120, 72);
            assert!(best(&n, text) >= 4.5, "{id}: notice {:.2}", best(&n, text));

            // The bar reads too: the fill differs from the track and the track from the pill.
            let bar = Shot::irect(&g.bar);
            let (fill_px, track_px) = (
                sh.at(bar.0 + 2, bar.1 + bar.3 / 2),
                sh.at(bar.0 + bar.2 - 3, bar.1 + bar.3 / 2),
            );
            assert!(near(fill_px, bgra(t.accent), 2), "{id}: fill is the accent");
            assert!(
                rgb_of(fill_px).contrast(rgb_of(track_px)) >= 2.0,
                "{id}: fill {:?} vs track {:?}",
                fill_px,
                track_px
            );
            assert!(!near(track_px, bgra(t.pill), 4), "{id}: the track shows");
            // The lamp is the accent on the pill, and the notice's the highlight.
            let lamp = (g.lamp.0 as i32, g.lamp.1 as i32);
            assert!(near(sh.at(lamp.0, lamp.1), bgra(t.accent), 2), "{id}: lamp");
            let nl = (ng.lamp.0 as i32, ng.lamp.1 as i32);
            assert!(
                near(n.at(nl.0, nl.1), bgra(t.highlight), 2),
                "{id}: notice lamp"
            );
        }
    }
}
