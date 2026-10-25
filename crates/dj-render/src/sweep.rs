//! The Sweep view inside Vitals: category checklist, preview pages, quick actions and
//! the Clean button. Every element sits on the rect that `dj_core::layout` gives the
//! hit test, so clicks and pixels agree. Text choices are pure helpers (tested below).

use crate::fonts::Weight;
use crate::paint::{col, draw_text, fill_paint, fit, stroke_paint, text_width, Frame, Painter};
use dj_core::color::Rgb;
use dj_core::layout::{self, Rect as LRect};
use dj_core::sweep::{format_bytes, CatView, Category, Confirm, Phase, SweepView};
use dj_core::theme::ResolvedTokens;
use skia_safe::{Canvas, Color, Font, PathBuilder, RRect, Rect};

/// Which theme ink a line of text uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Text,
    Dim,
    Accent,
    Warn,
}

impl Tone {
    fn rgb(self, t: &ResolvedTokens) -> Rgb {
        match self {
            Tone::Text => t.text,
            Tone::Dim => t.text_dim,
            Tone::Accent => t.accent,
            Tone::Warn => t.warn,
        }
    }
}

/// Header line on the right: the reclaimable total, the open list's total, or the phase.
pub(crate) fn header_text(v: &SweepView) -> (String, Tone) {
    if let Some((cat, _)) = v.preview {
        let c = v.cat(cat);
        return (
            format!("{} · {}", cat.label(), format_bytes(c.bytes)),
            Tone::Accent,
        );
    }
    match v.phase {
        Phase::Scanning => ("scanning…".to_string(), Tone::Dim),
        Phase::Cleaning { .. } => ("cleaning…".to_string(), Tone::Dim),
        Phase::Done { .. } => ("done".to_string(), Tone::Accent),
        Phase::Ready => (
            format!("{} reclaimable", format_bytes(v.reclaimable_bytes())),
            Tone::Accent,
        ),
    }
}

/// Label on the Clean button.
pub(crate) fn clean_label(v: &SweepView) -> String {
    match v.phase {
        Phase::Scanning => "previewing…".to_string(),
        Phase::Ready => match v.selected_bytes() {
            0 => "Clean".to_string(),
            b => format!("Clean {}", format_bytes(b)),
        },
        Phase::Cleaning { done, total } => format!("{done} / {total}"),
        Phase::Done { .. } => "back to vitals".to_string(),
    }
}

/// Fraction of the clean that is done, while cleaning.
pub(crate) fn progress(phase: Phase) -> Option<f32> {
    match phase {
        Phase::Cleaning { done, total } if total > 0 => {
            Some((done as f32 / total as f32).clamp(0.0, 1.0))
        }
        Phase::Cleaning { .. } => Some(0.0),
        _ => None,
    }
}

/// The status line left of the Clean button. A confirm or quick-action message wins.
pub(crate) fn status_text(v: &SweepView) -> (String, Tone) {
    if let Some(s) = &v.status {
        let tone = if v.confirm.is_some() {
            Tone::Warn
        } else {
            Tone::Text
        };
        return (s.clone(), tone);
    }
    match v.phase {
        Phase::Scanning => {
            let done = v.cats.iter().filter(|c| c.scanned).count();
            (format!("scanning {done}/{}", v.cats.len()), Tone::Dim)
        }
        Phase::Ready => match v.preview {
            Some((Category::BigFiles, _)) => ("one by one, to the bin".to_string(), Tone::Dim),
            Some((cat, _)) if cat.removal() == dj_core::sweep::Removal::Recycle => {
                ("→ recycle bin · logged".to_string(), Tone::Dim)
            }
            Some(_) => ("removed for good · logged".to_string(), Tone::Dim),
            None => ("cache removed · purge → bin".to_string(), Tone::Dim),
        },
        Phase::Cleaning { .. } => ("cleaning…".to_string(), Tone::Dim),
        Phase::Done { freed, failed } => {
            let mut s = format!("Freed {}", format_bytes(freed));
            if failed > 0 {
                s.push_str(&format!(" · {failed} failed"));
            }
            (s, Tone::Accent)
        }
    }
}

/// Right half of a checklist row: what opens on click, or why the row is short.
pub(crate) fn row_right(c: &CatView) -> (String, Tone) {
    if !c.scanned {
        return ("…".to_string(), Tone::Dim);
    }
    if let Some(note) = &c.note {
        return (note.clone(), Tone::Warn);
    }
    match c.items.len() {
        0 => ("empty".to_string(), Tone::Dim),
        n => {
            let noun = match (c.cat, n) {
                (Category::BigFiles, 1) => "file",
                (Category::BigFiles, _) => "files",
                (_, 1) => "item",
                _ => "items",
            };
            (format!("{n} {noun}"), Tone::Dim)
        }
    }
}

/// "2 / 5" for the open preview, None when it fits on one page.
pub(crate) fn page_text(v: &SweepView) -> Option<String> {
    let (cat, page) = v.preview?;
    let pages = v.pages(cat);
    (pages > 1).then(|| format!("{} / {pages}", page.min(pages - 1) + 1))
}

/// A rounded chip with a centred label.
#[allow(clippy::too_many_arguments)]
fn chip(
    canvas: &Canvas,
    font: &Font,
    r: Rect,
    label: &str,
    ink: Rgb,
    body: Rgb,
    edge: Rgb,
    alpha: f32,
    s: f32,
) {
    let rad = (r.height() / 2.0).min(8.0 * s);
    canvas.draw_rrect(
        RRect::new_rect_xy(r, rad, rad),
        &fill_paint(col(body, alpha)),
    );
    canvas.draw_rrect(
        RRect::new_rect_xy(r.with_inset((0.5, 0.5)), rad - 0.5, rad - 0.5),
        &stroke_paint(col(edge, 0.9 * alpha), 1.0),
    );
    let txt = fit(font, label, 0.0, r.width() - 8.0 * s);
    let tw = text_width(font, &txt, 0.0);
    draw_text(
        canvas,
        font,
        &txt,
        r.center_x() - tw / 2.0,
        r.center_y() + 0.35 * font.size(),
        0.0,
        &fill_paint(col(ink, alpha)),
    );
}

/// A small open chevron pointing left (`dir = -1`) or right (`dir = 1`).
fn chevron(canvas: &Canvas, cx: f32, cy: f32, size: f32, dir: f32, ink: Color, s: f32) {
    let h = size / 2.0;
    let mut b = PathBuilder::new();
    b.move_to((cx - dir * h * 0.5, cy - h));
    b.line_to((cx + dir * h * 0.5, cy));
    b.line_to((cx - dir * h * 0.5, cy + h));
    canvas.draw_path(&b.detach(), &stroke_paint(ink, 1.4 * s));
}

/// A tick box: outlined when off, accent-filled with a pill-coloured tick when on.
fn checkbox(canvas: &Canvas, x: f32, cy: f32, on: bool, t: &ResolvedTokens, alpha: f32, s: f32) {
    let size = 12.0 * s;
    let r = Rect::from_xywh(x, cy - size / 2.0, size, size);
    let rad = 3.5 * s;
    if on {
        canvas.draw_rrect(
            RRect::new_rect_xy(r, rad, rad),
            &fill_paint(col(t.accent, alpha)),
        );
        let mut b = PathBuilder::new();
        b.move_to((r.left + 3.0 * s, r.center_y()));
        b.line_to((r.left + 5.2 * s, r.center_y() + 2.4 * s));
        b.line_to((r.right - 2.8 * s, r.top + 3.4 * s));
        canvas.draw_path(&b.detach(), &stroke_paint(col(t.pill, alpha), 1.6 * s));
    } else {
        canvas.draw_rrect(
            RRect::new_rect_xy(r.with_inset((0.75 * s, 0.75 * s)), rad, rad),
            &stroke_paint(col(t.text_dim, alpha), 1.3 * s),
        );
    }
}

impl Painter {
    /// Sweep view, 400 x 236 (`layout::SWEEP_*`): back control and header, then either
    /// the checklist with quick actions or one category's preview page, then the status
    /// line and the Clean button. Anchored to the pill's centre-top, like the hit test.
    pub(crate) fn sweep_view(&mut self, canvas: &Canvas, f: &Frame, rect: &Rect) {
        let (s, t, v) = (f.scale, f.tokens, f.sweep);
        let ox = rect.center_x() - layout::SWEEP_W * s / 2.0;
        let oy = rect.top;
        let at = |r: LRect| Rect::from_xywh(ox + r.x * s, oy + r.y * s, r.w * s, r.h * s);
        let busy = matches!(v.phase, Phase::Cleaning { .. });

        let label = self.fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
        let small = self.fonts.font(&t.font_label, Weight::Regular, 9.0 * s);
        let head = self.fonts.font(&t.font_label, Weight::Medium, 13.0 * s);
        // Back control and header.
        let back = at(layout::SW_BACK);
        let bcy = back.center_y();
        chevron(
            canvas,
            back.left + 10.0 * s,
            bcy,
            9.0 * s,
            -1.0,
            col(t.text_dim, 1.0),
            s,
        );
        let back_txt = if v.preview.is_some() {
            "list"
        } else {
            "vitals"
        };
        draw_text(
            canvas,
            &label,
            back_txt,
            back.left + 20.0 * s,
            bcy + 0.35 * 10.0 * s,
            0.0,
            &fill_paint(col(t.text_dim, 1.0)),
        );
        let title_x = back.left + 20.0 * s + text_width(&label, back_txt, 0.0) + 12.0 * s;
        draw_text(
            canvas,
            &small,
            "SWEEP",
            title_x,
            bcy + 0.35 * 9.0 * s,
            1.2 * s,
            &fill_paint(col(t.text_dim, 0.7)),
        );
        let (htxt, htone) = header_text(v);
        let right = ox + (layout::SWEEP_W - 16.0) * s;
        let room = right - (title_x + 52.0 * s);
        let htxt = fit(&head, &htxt, 0.0, room);
        let hw = text_width(&head, &htxt, 0.0);
        draw_text(
            canvas,
            &head,
            &htxt,
            right - hw,
            bcy + 0.35 * 13.0 * s,
            0.0,
            &fill_paint(col(htone.rgb(t), 1.0)),
        );

        match v.preview {
            Some((cat, _)) => self.sweep_preview(canvas, f, cat, &at, busy),
            None => self.sweep_checklist(canvas, f, &at, busy),
        }

        // Status line, left of the Clean button.
        let clean = at(layout::SW_CLEAN);
        let (stxt, stone) = status_text(v);
        let done = matches!(v.phase, Phase::Done { .. }) && v.status.is_none();
        let sfont = if done { &head } else { &label };
        let sx = ox + 16.0 * s;
        let stxt = fit(sfont, &stxt, 0.0, clean.left - sx - 8.0 * s);
        draw_text(
            canvas,
            sfont,
            &stxt,
            sx,
            clean.center_y() + 0.35 * sfont.size(),
            0.0,
            &fill_paint(col(stone.rgb(t), 1.0)),
        );

        // Clean button: pill-shaped, like plan.html. Disabled until a finished preview.
        let rad = clean.height() / 2.0;
        let rr = RRect::new_rect_xy(clean, rad, rad);
        let clabel = clean_label(v);
        let (body, ink, edge) = if matches!(v.phase, Phase::Done { .. }) {
            (t.accent, t.pill, t.accent)
        } else if v.can_clean() {
            (t.text, t.pill, t.text)
        } else {
            (t.surface1, t.text_dim, t.surface2)
        };
        canvas.draw_rrect(rr, &fill_paint(col(body, 1.0)));
        if let Some(p) = progress(v.phase) {
            canvas.save();
            canvas.clip_rrect(rr, skia_safe::ClipOp::Intersect, true);
            let fill = Rect::from_xywh(clean.left, clean.top, clean.width() * p, clean.height());
            canvas.draw_rect(fill, &fill_paint(col(t.accent, 0.55)));
            canvas.restore();
        }
        canvas.draw_rrect(
            RRect::new_rect_xy(clean.with_inset((0.5, 0.5)), rad - 0.5, rad - 0.5),
            &stroke_paint(col(edge, 0.9), 1.0),
        );
        let bfont = self.fonts.font(&t.font_label, Weight::Medium, 11.0 * s);
        let clabel = fit(&bfont, &clabel, 0.0, clean.width() - 16.0 * s);
        let cw = text_width(&bfont, &clabel, 0.0);
        let cink = if progress(v.phase).is_some() {
            t.text
        } else {
            ink
        };
        draw_text(
            canvas,
            &bfont,
            &clabel,
            clean.center_x() - cw / 2.0,
            clean.center_y() + 0.35 * 11.0 * s,
            0.0,
            &fill_paint(col(cink, 1.0)),
        );
    }

    /// Five category rows (tick, name, size | count or note, chevron) and the quick actions.
    fn sweep_checklist(
        &mut self,
        canvas: &Canvas,
        f: &Frame,
        at: &dyn Fn(LRect) -> Rect,
        busy: bool,
    ) {
        let (s, t, v) = (f.scale, f.tokens, f.sweep);
        let row_font = self.fonts.font(&t.font_label, Weight::Regular, 11.0 * s);
        let small = self.fonts.font(&t.font_label, Weight::Regular, 9.0 * s);
        let size_font = self.fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
        let a = if busy { 0.5 } else { 1.0 };

        for (i, c) in v.cats.iter().enumerate() {
            let r = at(layout::sw_row(i));
            let cy = r.center_y();
            let split = at(LRect::new(layout::SW_ROW_SPLIT, 0.0, 0.0, 0.0)).left;
            if i > 0 {
                canvas.draw_line(
                    (r.left + 6.0 * s, r.top),
                    (r.right - 6.0 * s, r.top),
                    &stroke_paint(col(t.surface2, 0.5), 1.0),
                );
            }
            if c.cat.in_clean() {
                checkbox(canvas, r.left + 6.0 * s, cy, c.checked, t, a, s);
            } else {
                // Big files never join "Clean": a hollow dot instead of a tick box.
                canvas.draw_circle(
                    (r.left + 12.0 * s, cy),
                    3.0 * s,
                    &stroke_paint(col(t.text_dim, a), 1.2 * s),
                );
            }
            let on = c.checked || !c.cat.in_clean();
            let name_ink = if on { t.text } else { t.text_dim };
            let size_txt = if c.scanned {
                format_bytes(c.bytes)
            } else {
                "…".to_string()
            };
            let sw = text_width(&size_font, &size_txt, 0.0);
            let size_right = split - 10.0 * s;
            let name_x = r.left + 26.0 * s;
            let name = fit(
                &row_font,
                c.cat.label(),
                0.0,
                size_right - sw - 8.0 * s - name_x,
            );
            draw_text(
                canvas,
                &row_font,
                &name,
                name_x,
                cy + 0.35 * 11.0 * s,
                0.0,
                &fill_paint(col(name_ink, a)),
            );
            draw_text(
                canvas,
                &size_font,
                &size_txt,
                size_right - sw,
                cy + 0.35 * 10.0 * s,
                0.0,
                &fill_paint(col(if on { t.text } else { t.text_dim }, a)),
            );

            canvas.draw_line(
                (split, r.top + 6.0 * s),
                (split, r.bottom - 6.0 * s),
                &stroke_paint(col(t.surface2, 0.9), 1.0),
            );
            let opens = c.scanned && !busy;
            let chev_x = r.right - 8.0 * s;
            if opens {
                chevron(canvas, chev_x, cy, 8.0 * s, 1.0, col(t.text_dim, 1.0), s);
            }
            let (rtxt, rtone) = row_right(c);
            let rx = split + 10.0 * s;
            let rtxt = fit(&small, &rtxt, 0.0, chev_x - 8.0 * s - rx);
            draw_text(
                canvas,
                &small,
                &rtxt,
                rx,
                cy + 0.35 * 9.0 * s,
                0.0,
                &fill_paint(col(rtone.rgb(t), a)),
            );
        }

        // Quick actions. Empty bin needs a second click; the armed chip turns warn.
        let qa = if busy { 0.45 } else { 1.0 };
        chip(
            canvas,
            &small,
            at(layout::SW_DNS),
            "flush dns",
            t.text,
            t.surface1,
            t.surface2,
            qa,
            s,
        );
        let armed = matches!(v.confirm, Some(Confirm::EmptyBin(_)));
        let (bl, bink, bbody, bedge) = if armed {
            ("sure? empty", t.pill, t.warn, t.warn)
        } else {
            ("empty bin", t.text, t.surface1, t.surface2)
        };
        chip(
            canvas,
            &small,
            at(layout::SW_BIN),
            bl,
            bink,
            bbody,
            bedge,
            qa,
            s,
        );
    }

    /// One page of a category's items, largest first. Big files get Reveal and Recycle.
    fn sweep_preview(
        &mut self,
        canvas: &Canvas,
        f: &Frame,
        cat: Category,
        at: &dyn Fn(LRect) -> Rect,
        busy: bool,
    ) {
        let (s, t, v) = (f.scale, f.tokens, f.sweep);
        let row_font = self.fonts.font(&t.font_label, Weight::Regular, 10.5 * s);
        let size_font = self.fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
        let small = self.fonts.font(&t.font_label, Weight::Regular, 9.0 * s);
        let big = cat == Category::BigFiles;
        let items = v.page_items();

        if items.is_empty() {
            let mid = at(layout::sw_line(2));
            let txt = v
                .cat(cat)
                .note
                .clone()
                .unwrap_or_else(|| "nothing to clean here".to_string());
            let txt = fit(&row_font, &txt, 0.0, mid.width() - 16.0 * s);
            let tw = text_width(&row_font, &txt, 0.0);
            draw_text(
                canvas,
                &row_font,
                &txt,
                mid.center_x() - tw / 2.0,
                mid.center_y() + 0.35 * 10.5 * s,
                0.0,
                &fill_paint(col(t.text_dim, 1.0)),
            );
        }

        for (slot, (_, it)) in items.iter().enumerate() {
            let r = at(layout::sw_line(slot));
            let cy = r.center_y();
            if slot % 2 == 1 {
                let band = RRect::new_rect_xy(r, 6.0 * s, 6.0 * s);
                canvas.draw_rrect(band, &fill_paint(col(t.surface1, 0.6)));
            }
            let size_right = if big {
                at(layout::sw_reveal(slot)).left - 8.0 * s
            } else {
                r.right - 8.0 * s
            };
            let size_txt = format_bytes(it.bytes);
            let sw = text_width(&size_font, &size_txt, 0.0);
            draw_text(
                canvas,
                &size_font,
                &size_txt,
                size_right - sw,
                cy + 0.35 * 10.0 * s,
                0.0,
                &fill_paint(col(t.text_dim, 1.0)),
            );
            let lx = r.left + 8.0 * s;
            let name = fit(&row_font, &it.label, 0.0, size_right - sw - 10.0 * s - lx);
            draw_text(
                canvas,
                &row_font,
                &name,
                lx,
                cy + 0.35 * 10.5 * s,
                0.0,
                &fill_paint(col(t.text, 1.0)),
            );
            if big {
                let a = if busy { 0.45 } else { 1.0 };
                chip(
                    canvas,
                    &small,
                    at(layout::sw_reveal(slot)),
                    "reveal",
                    t.text,
                    t.surface2,
                    t.surface2,
                    a,
                    s,
                );
                let armed = matches!(&v.confirm, Some(Confirm::Recycle(p)) if *p == it.path);
                let (l, ink, body) = if armed {
                    ("sure?", t.pill, t.warn)
                } else {
                    ("recycle", t.text, t.surface2)
                };
                chip(
                    canvas,
                    &small,
                    at(layout::sw_recycle(slot)),
                    l,
                    ink,
                    body,
                    body,
                    a,
                    s,
                );
            }
        }

        // Pager, only when the list spans more than one page.
        if let (Some(txt), Some((_, page))) = (page_text(v), v.preview) {
            let last = v.pages(cat) - 1;
            let prev = at(layout::SW_PREV);
            let next = at(layout::SW_NEXT);
            let pa = if page > 0 { 1.0 } else { 0.4 };
            let na = if page < last { 1.0 } else { 0.4 };
            chip(
                canvas, &small, prev, "", t.text, t.surface1, t.surface2, pa, s,
            );
            chevron(
                canvas,
                prev.center_x(),
                prev.center_y(),
                8.0 * s,
                -1.0,
                col(t.text, pa),
                s,
            );
            chip(
                canvas, &small, next, "", t.text, t.surface1, t.surface2, na, s,
            );
            chevron(
                canvas,
                next.center_x(),
                next.center_y(),
                8.0 * s,
                1.0,
                col(t.text, na),
                s,
            );
            let tw = text_width(&small, &txt, 0.0);
            let mid_x = (prev.right + next.left) / 2.0;
            draw_text(
                canvas,
                &small,
                &txt,
                mid_x - tw / 2.0,
                prev.center_y() + 0.35 * 9.0 * s,
                0.0,
                &fill_paint(col(t.text_dim, 1.0)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::sweep::ItemView;
    use std::path::PathBuf;

    fn item(p: &str, bytes: u64) -> ItemView {
        ItemView {
            path: PathBuf::from(p),
            bytes,
            label: p.to_string(),
        }
    }

    fn ready() -> SweepView {
        let mut v = SweepView::new();
        v.set_category(Category::Temp, vec![item("t", 3 * 1024 * 1024)], None);
        v.set_category(Category::Browser, vec![], Some("chrome is running".into()));
        v.set_category(Category::AppCache, vec![], None);
        v.set_category(Category::DevPurge, vec![item("p", 1024)], None);
        v.set_category(
            Category::BigFiles,
            (0..8).map(|i| item(&format!("b{i}"), 10)).collect(),
            None,
        );
        v.finish_scan();
        v
    }

    #[test]
    fn clean_label_follows_the_phase() {
        let mut v = SweepView::new();
        assert_eq!(clean_label(&v), "previewing…");
        v = ready();
        assert_eq!(clean_label(&v), "Clean 3.0 MB");
        v.cats[0].checked = false;
        assert_eq!(clean_label(&v), "Clean");
        v.begin_clean(4);
        v.clean_progress(1, 4);
        assert_eq!(clean_label(&v), "1 / 4");
        assert_eq!(progress(v.phase), Some(0.25));
        v.finish_clean(5, 0, &[]);
        assert_eq!(clean_label(&v), "back to vitals");
        assert_eq!(progress(v.phase), None);
        assert_eq!(progress(Phase::Cleaning { done: 3, total: 0 }), Some(0.0));
    }

    #[test]
    fn header_and_status() {
        let mut v = SweepView::new();
        assert_eq!(header_text(&v), ("scanning…".to_string(), Tone::Dim));
        assert_eq!(status_text(&v).0, "scanning 0/5");
        v = ready();
        // Temp 3 MB + dev purge 1 KB; big files never count.
        assert_eq!(header_text(&v).0, "3.0 MB reclaimable");
        v.preview = Some((Category::Temp, 0));
        assert_eq!(header_text(&v).0, "temp & reports · 3.0 MB");
        assert_eq!(status_text(&v).0, "removed for good · logged");
        v.preview = Some((Category::DevPurge, 0));
        assert_eq!(status_text(&v).0, "→ recycle bin · logged");
        v.preview = None;
        v.status = Some("click again to empty the bin".into());
        v.confirm = Some(Confirm::EmptyBin(std::time::Instant::now()));
        assert_eq!(status_text(&v).1, Tone::Warn);
        v.status = None;
        v.confirm = None;
        v.finish_clean((4.1 * 1024.0 * 1024.0 * 1024.0) as u64, 2, &[]);
        assert_eq!(
            status_text(&v),
            ("Freed 4.1 GB · 2 failed".into(), Tone::Accent)
        );
    }

    #[test]
    fn row_right_text() {
        let v = ready();
        assert_eq!(row_right(v.cat(Category::Temp)).0, "1 item");
        assert_eq!(
            row_right(v.cat(Category::Browser)),
            ("chrome is running".into(), Tone::Warn)
        );
        assert_eq!(row_right(v.cat(Category::AppCache)).0, "empty");
        assert_eq!(row_right(v.cat(Category::BigFiles)).0, "8 files");
        assert_eq!(row_right(SweepView::new().cat(Category::Temp)).0, "…");
    }

    #[test]
    fn page_text_only_with_several_pages() {
        let mut v = ready();
        assert_eq!(page_text(&v), None);
        v.preview = Some((Category::Temp, 0));
        assert_eq!(page_text(&v), None);
        v.preview = Some((Category::BigFiles, 1));
        assert_eq!(page_text(&v).as_deref(), Some("2 / 2"));
    }
}
