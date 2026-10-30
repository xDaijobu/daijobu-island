//! The Timer views: the panel (400 x 184), Timer compact (172 x 36), the session-end
//! alert (360 x 72) and the progress arc on Music compact. Content is anchored to the
//! pill's top-centre like the other views. Digits use the theme's digit font (Doto);
//! focus draws in the accent, breaks in the text colour and a finished session in warn.

use crate::fonts::Weight;
use crate::icons::draw_icon;
use crate::music::button;
use crate::paint::{col, draw_text, fill_paint, fit, stroke_paint, text_width, Frame, Painter};
use crate::vitals::ring_sweep;
use dj_core::color::Rgb;
use dj_core::features::Icon;
use dj_core::layout::{self as lay, Rect as LRect};
use dj_core::theme::ResolvedTokens;
use dj_core::timer::{Phase, Status, TimerView, PRESETS};
use skia_safe::{Canvas, ContourMeasureIter, PathBuilder, RRect, Rect};

/// Disabled controls are drawn at this opacity.
const DISABLED: f32 = 0.4;

/// Ink for the clock and the ring: warn when done, accent for focus, text for breaks.
fn phase_ink(t: &ResolvedTokens, v: &TimerView) -> Rgb {
    match (v.status, v.phase) {
        (Status::Done, _) => t.warn,
        (_, Phase::Focus) => t.accent,
        _ => t.text,
    }
}

/// Ring arc in degrees for `progress` (0..=1).
pub fn progress_sweep(progress: f32) -> f32 {
    ring_sweep(progress * 100.0)
}

/// A progress ring centred in `r`: a surface2 track and an arc from the top.
fn ring(canvas: &Canvas, r: Rect, stroke: f32, progress: f32, track: Rgb, ink: Rgb) {
    let radius = r.width().min(r.height()) / 2.0 - stroke / 2.0;
    let (cx, cy) = (r.center_x(), r.center_y());
    canvas.draw_circle((cx, cy), radius, &stroke_paint(col(track, 1.0), stroke));
    let sweep = progress_sweep(progress);
    if sweep > 0.0 {
        let oval = Rect::from_xywh(cx - radius, cy - radius, radius * 2.0, radius * 2.0);
        canvas.draw_arc(
            oval,
            -90.0,
            sweep,
            false,
            &stroke_paint(col(ink, 1.0), stroke),
        );
    }
}

impl Painter {
    /// The Timer panel: header with the phase label, ring, big digits, preset chips
    /// and the start / skip / reset row. A finished session shows inline (warn ink,
    /// continue and stop).
    pub(crate) fn timer_panel(&mut self, canvas: &Canvas, f: &Frame, rect: &Rect) {
        let (s, t, v) = (f.scale, f.tokens, f.timer);
        let ox = rect.center_x() - lay::TIMER_W * s / 2.0;
        let oy = rect.top;
        let at = |r: LRect| Rect::from_xywh(ox + r.x * s, oy + r.y * s, r.w * s, r.h * s);
        let ink = phase_ink(t, v);
        let done = v.status == Status::Done;

        // Header: lamp + POMODORO on the left, the phase label on the right.
        let label = self.fonts.font(&t.font_label, Weight::Medium, 8.5 * s);
        let h = at(lay::TP_HEADER);
        let hy = h.center_y();
        let tracking = 1.6 * s;
        canvas.draw_circle((h.left + 3.0 * s, hy), 2.6 * s, &fill_paint(col(ink, 1.0)));
        draw_text(
            canvas,
            &label,
            "POMODORO",
            h.left + 12.0 * s,
            hy + 0.35 * 8.5 * s,
            tracking,
            &fill_paint(col(t.text_dim, 1.0)),
        );
        let phase_font = self.fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
        let pw = text_width(&phase_font, &v.label, 0.0);
        let phase_ink = if done { t.warn } else { t.text };
        draw_text(
            canvas,
            &phase_font,
            &v.label,
            h.right() - pw,
            hy + 0.35 * 10.0 * s,
            0.0,
            &fill_paint(col(phase_ink, 1.0)),
        );

        // Ring with the timer glyph in the middle.
        let rr = at(lay::TP_RING);
        ring(canvas, rr, 5.0 * s, v.progress, t.surface2, ink);
        draw_icon(
            canvas,
            Icon::Timer,
            rr.center_x(),
            rr.center_y(),
            22.0 * s,
            &stroke_paint(col(ink, 0.9), 1.6 * s),
        );

        // Big digits.
        let d = at(lay::TP_DIGITS);
        let size = 46.0 * s;
        let digits = self.fonts.font(&t.font_digits, Weight::Regular, size);
        let line = fit(&digits, &v.digits, 0.0, d.width());
        draw_text(
            canvas,
            &digits,
            &line,
            d.left,
            d.center_y() + 0.35 * size,
            0.0,
            &fill_paint(col(ink, 1.0)),
        );

        // Preset chips. They only change while Stopped, so they dim otherwise.
        let chip_font = self.fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
        let chips_on = v.status == Status::Stopped;
        let a = if chips_on { 1.0 } else { DISABLED };
        for (i, (focus, brk)) in PRESETS.iter().enumerate() {
            let r = at(lay::tp_chip(i));
            let picked = v.preset == Some(i);
            let rad = r.height() / 2.0;
            let (body, edge, txt) = if picked {
                (t.surface2, t.accent, t.accent)
            } else {
                (t.surface1, t.surface2, t.text_dim)
            };
            canvas.draw_rrect(RRect::new_rect_xy(r, rad, rad), &fill_paint(col(body, a)));
            canvas.draw_rrect(
                RRect::new_rect_xy(r.with_inset((0.5, 0.5)), rad - 0.5, rad - 0.5),
                &stroke_paint(col(edge, a), 1.0),
            );
            let text = format!("{focus}/{brk}");
            let tw = text_width(&chip_font, &text, 0.0);
            draw_text(
                canvas,
                &chip_font,
                &text,
                r.center_x() - tw / 2.0,
                r.center_y() + 0.35 * 10.0 * s,
                0.0,
                &fill_paint(col(txt, a)),
            );
        }

        // Buttons: start/pause/resume (continue when done), skip, reset (stop when done).
        let btn = self.fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
        let toggle = match v.status {
            Status::Stopped => "START",
            Status::Running => "PAUSE",
            Status::Paused => "RESUME",
            Status::Done => "CONTINUE",
        };
        button(
            canvas,
            &btn,
            at(lay::TP_TOGGLE),
            toggle,
            col(t.pill, 1.0),
            col(t.accent, 1.0),
            col(t.accent, 1.0),
            1.2 * s,
            s,
        );
        let skip_a = if v.is_active() { 1.0 } else { DISABLED };
        button(
            canvas,
            &btn,
            at(lay::TP_SKIP),
            "SKIP",
            col(t.text, skip_a),
            col(t.surface1, skip_a),
            col(t.surface2, skip_a),
            1.2 * s,
            s,
        );
        let (reset, reset_ink) = if done {
            ("STOP", t.warn)
        } else {
            ("RESET", t.text)
        };
        button(
            canvas,
            &btn,
            at(lay::TP_RESET),
            reset,
            col(reset_ink, 1.0),
            col(t.surface1, 1.0),
            col(t.surface2, 1.0),
            1.2 * s,
            s,
        );
    }

    /// Timer compact: ring 18, the phase (or "paused") and Doto 18 digits.
    pub(crate) fn timer_compact(&mut self, canvas: &Canvas, f: &Frame, rect: &Rect) {
        let (s, t, v) = (f.scale, f.tokens, f.timer);
        let ox = rect.center_x() - lay::TIMER_COMPACT_W * s / 2.0;
        let oy = rect.top;
        let at = |r: LRect| Rect::from_xywh(ox + r.x * s, oy + r.y * s, r.w * s, r.h * s);
        let ink = phase_ink(t, v);
        let cy = oy + lay::TIMER_COMPACT_H * s / 2.0;

        ring(
            canvas,
            at(lay::TC_RING),
            2.2 * s,
            v.progress,
            t.surface2,
            ink,
        );

        let size = 18.0 * s;
        let digits = self.fonts.font(&t.font_digits, Weight::Regular, size);
        let dw = text_width(&digits, &v.digits, 0.0);
        let right = ox + lay::TC_DIGITS_RIGHT * s;
        draw_text(
            canvas,
            &digits,
            &v.digits,
            right - dw,
            cy + 0.35 * size,
            0.0,
            &fill_paint(col(ink, 1.0)),
        );

        let small = self.fonts.font(&t.font_label, Weight::Regular, 9.0 * s);
        let text = if v.status == Status::Paused {
            "paused"
        } else {
            v.phase.name()
        };
        let left = ox + lay::TC_LABEL_X * s;
        let line = fit(&small, text, 0.0, right - dw - 6.0 * s - left);
        draw_text(
            canvas,
            &small,
            &line,
            left,
            cy + 0.35 * 9.0 * s,
            0.0,
            &fill_paint(col(t.text_dim, 1.0)),
        );
    }

    /// The session-end alert: `focus done · break 5:00`, then continue and stop.
    pub(crate) fn timer_alert(&mut self, canvas: &Canvas, f: &Frame, rect: &Rect) {
        let (s, t, v) = (f.scale, f.tokens, f.timer);
        let ox = rect.center_x() - lay::ALERT_W * s / 2.0;
        let oy = rect.top;
        let at = |r: LRect| Rect::from_xywh(ox + r.x * s, oy + r.y * s, r.w * s, r.h * s);

        let tr = at(lay::AL_TEXT);
        let small = self.fonts.font(&t.font_label, Weight::Medium, 8.5 * s);
        let tracking = 1.6 * s;
        let top_y = tr.top + 8.0 * s;
        canvas.draw_circle(
            (tr.left + 3.0 * s, top_y),
            2.6 * s,
            &fill_paint(col(t.warn, 1.0)),
        );
        draw_text(
            canvas,
            &small,
            "TIMER",
            tr.left + 12.0 * s,
            top_y + 0.35 * 8.5 * s,
            tracking,
            &fill_paint(col(t.text_dim, 1.0)),
        );
        let main = self.fonts.font(&t.font_label, Weight::Medium, 12.0 * s);
        let line = fit(&main, &v.alert, 0.0, tr.width());
        draw_text(
            canvas,
            &main,
            &line,
            tr.left,
            tr.bottom - 6.0 * s,
            0.0,
            &fill_paint(col(t.text, 1.0)),
        );

        // ▸ continue: accent body, a play triangle and the label.
        let c = at(lay::AL_CONTINUE);
        let rad = 8.0 * s;
        canvas.draw_rrect(
            RRect::new_rect_xy(c, rad, rad),
            &fill_paint(col(t.accent, 1.0)),
        );
        let btn = self.fonts.font(&t.font_label, Weight::Medium, 9.5 * s);
        let label = "CONTINUE";
        let lw = text_width(&btn, label, 0.6 * s);
        let tri_w = 6.0 * s;
        let gap = 5.0 * s;
        let x0 = c.center_x() - (tri_w + gap + lw) / 2.0;
        let mut pb = PathBuilder::new();
        pb.move_to((x0, c.center_y() - 4.0 * s))
            .line_to((x0, c.center_y() + 4.0 * s))
            .line_to((x0 + tri_w, c.center_y()))
            .close();
        let pill_ink = fill_paint(col(t.pill, 1.0));
        canvas.draw_path(&pb.detach(), &pill_ink);
        draw_text(
            canvas,
            &btn,
            label,
            x0 + tri_w + gap,
            c.center_y() + 0.35 * 9.5 * s,
            0.6 * s,
            &pill_ink,
        );

        // × stop.
        let x = at(lay::AL_STOP);
        canvas.draw_rrect(
            RRect::new_rect_xy(x, rad, rad),
            &fill_paint(col(t.surface1, 1.0)),
        );
        canvas.draw_rrect(
            RRect::new_rect_xy(x.with_inset((0.5, 0.5)), rad - 0.5, rad - 0.5),
            &stroke_paint(col(t.surface2, 1.0), 1.0),
        );
        let k = 4.5 * s;
        let cross = stroke_paint(col(t.text, 1.0), 1.6 * s);
        let (cx, cy) = (x.center_x(), x.center_y());
        canvas.draw_line((cx - k, cy - k), (cx + k, cy + k), &cross);
        canvas.draw_line((cx - k, cy + k), (cx + k, cy - k), &cross);
    }

    /// On Music compact while a timer runs: a thin progress line along the pill's edge
    /// (clockwise from the top centre) and a small mono mm:ss before the playback indicator.
    pub(crate) fn music_timer_overlay(&mut self, canvas: &Canvas, f: &Frame, rect: &Rect) {
        let (s, t, v) = (f.scale, f.tokens, f.timer);
        let ink = phase_ink(t, v);
        let ox = rect.center_x() - lay::MUSIC_COMPACT_W * s / 2.0;
        let oy = rect.top;

        let font = self.fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
        let m = lay::MC_TIMER;
        let tw = text_width(&font, &v.digits, 0.0);
        draw_text(
            canvas,
            &font,
            &v.digits,
            ox + (m.x + m.w) * s - tw,
            oy + (m.y + m.h / 2.0) * s + 0.35 * 10.0 * s,
            0.0,
            &fill_paint(col(ink, 1.0)),
        );

        let stroke = 1.5 * s;
        let edge = rect.with_inset((stroke, stroke));
        let r = (f.view.radius * s - stroke).clamp(0.0, edge.height() / 2.0);
        let outline = edge_path(edge, r);
        canvas.draw_path(&outline, &stroke_paint(col(t.surface2, 0.8), stroke));
        let p = v.progress.clamp(0.0, 1.0);
        if p <= 0.0 {
            return;
        }
        if let Some(cm) = ContourMeasureIter::new(&outline, false, None).next() {
            let mut seg = PathBuilder::new();
            if cm.get_segment(0.0, cm.length() * p, &mut seg, true) {
                canvas.draw_path(&seg.detach(), &stroke_paint(col(ink, 1.0), stroke));
            }
        }
    }
}

/// A rounded-rect outline that starts at the top centre and runs clockwise, so a
/// prefix of its length is a progress line.
fn edge_path(r: Rect, radius: f32) -> skia_safe::Path {
    let d = radius * 2.0;
    let mut pb = PathBuilder::new();
    pb.move_to((r.center_x(), r.top))
        .line_to((r.right - radius, r.top))
        .arc_to(
            Rect::from_xywh(r.right - d, r.top, d, d),
            -90.0,
            90.0,
            false,
        )
        .line_to((r.right, r.bottom - radius))
        .arc_to(
            Rect::from_xywh(r.right - d, r.bottom - d, d, d),
            0.0,
            90.0,
            false,
        )
        .line_to((r.left + radius, r.bottom))
        .arc_to(
            Rect::from_xywh(r.left, r.bottom - d, d, d),
            90.0,
            90.0,
            false,
        )
        .line_to((r.left, r.top + radius))
        .arc_to(Rect::from_xywh(r.left, r.top, d, d), 180.0, 90.0, false)
        .close();
    pb.detach()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweep_follows_progress_and_clamps() {
        assert_eq!(progress_sweep(0.0), 0.0);
        assert_eq!(progress_sweep(0.5), 180.0);
        assert_eq!(progress_sweep(1.0), 360.0);
        assert_eq!(progress_sweep(2.0), 360.0);
        assert_eq!(progress_sweep(-1.0), 0.0);
    }

    #[test]
    fn edge_path_is_one_closed_loop_the_length_of_the_outline() {
        // A 224 x 36 stadium: two straights of 188 and a circle of radius 18.
        let path = edge_path(Rect::from_xywh(0.0, 0.0, 224.0, 36.0), 18.0);
        let mut it = ContourMeasureIter::new(&path, false, None);
        let cm = it.next().expect("one contour");
        assert!(it.next().is_none());
        assert!(cm.is_closed());
        let want = 2.0 * 188.0 + std::f32::consts::TAU * 18.0;
        assert!((cm.length() - want).abs() < 1.0, "{}", cm.length());
        // It starts at the top centre.
        let (p, _) = cm.pos_tan(0.0).unwrap();
        assert!((p.x - 112.0).abs() < 0.01 && p.y.abs() < 0.01);
    }
}
