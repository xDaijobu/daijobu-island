//! The Settings window painter (Stage V2): title bar, sidebar with the search field and
//! pages, the scrolling rows and the footer buttons. Draws exactly what the
//! `dj_core::settings_ui::ViewModel` describes; all rects are window-local logical px.

use crate::fonts::{Fonts, Weight};
use crate::paint::{col, draw_text, fill_paint, fit, stroke_paint, text_width};
use crate::popup::{dev, ink_on, paint_field, paint_panel, DIMMED};
use dj_core::layout::Rect as LRect;
use dj_core::settings_ui::{
    Button, ButtonView, Control, NavItem, RowView, ViewModel, PANEL, PANEL_R, SIDEBAR_W, TITLE_H,
};
use dj_core::theme::ResolvedTokens;
use skia_safe::paint::Cap;
use skia_safe::{Canvas, ClipOp, Color, Font, RRect, Rect};

/// Inner padding of a row.
const PAD: f32 = 16.0;

/// Where the slider thumb's centre is, in the same space as `track`.
pub(crate) fn slider_thumb_x(track: &Rect, value: f32, min: f32, max: f32) -> f32 {
    let span = max - min;
    let t = if span.abs() < f32::EPSILON {
        0.0
    } else {
        ((value - min) / span).clamp(0.0, 1.0)
    };
    track.left + t * track.width()
}

fn pill(canvas: &Canvas, r: Rect, fill: Color) {
    let rad = r.height() / 2.0;
    canvas.draw_rrect(RRect::new_rect_xy(r, rad, rad), &fill_paint(fill));
}

fn pill_outline(canvas: &Canvas, r: Rect, color: Color, s: f32) {
    let rad = r.height() / 2.0;
    canvas.draw_rrect(
        RRect::new_rect_xy(
            r.with_inset((0.5 * s, 0.5 * s)),
            rad - 0.5 * s,
            rad - 0.5 * s,
        ),
        &stroke_paint(color, s.max(1.0)),
    );
}

/// Baseline that centres text of `size` on `cy`.
fn base(cy: f32, size: f32) -> f32 {
    cy + 0.35 * size
}

pub fn paint_settings(
    canvas: &Canvas,
    fonts: &mut Fonts,
    tokens: &ResolvedTokens,
    scale: f32,
    view: &ViewModel,
) {
    let (t, s) = (tokens, scale);
    canvas.clear(Color::TRANSPARENT);
    paint_panel(canvas, t, s, PANEL, PANEL_R);

    // Everything else stays inside the rounded body.
    canvas.save();
    let body = RRect::new_rect_xy(dev(PANEL, s), PANEL_R * s, PANEL_R * s);
    canvas.clip_rrect(body, ClipOp::Intersect, true);

    title_bar(canvas, fonts, t, s, view);
    sidebar(canvas, fonts, t, s, view);

    // Content: dividers, page title, rows, scrollbar, footer.
    let hair = col(t.text, 0.10);
    let sx = (PANEL.x + SIDEBAR_W) * s;
    let top = (PANEL.y + TITLE_H) * s;
    let bottom = (PANEL.y + PANEL.h) * s;
    canvas.draw_line((sx, top), (sx, bottom), &stroke_paint(hair, s.max(1.0)));
    canvas.draw_line(
        (PANEL.x * s, top),
        ((PANEL.x + PANEL.w) * s, top),
        &stroke_paint(hair, s.max(1.0)),
    );

    let big = fonts.font(&t.font_label, Weight::SemiBold, 14.0 * s);
    draw_text(
        canvas,
        &big,
        &view.title,
        sx + 24.0 * s,
        base((PANEL.y + TITLE_H / 2.0) * s, 14.0 * s),
        0.0,
        &fill_paint(col(t.text, 1.0)),
    );

    let content = dev(view.content, s);
    canvas.save();
    canvas.clip_rect(content, ClipOp::Intersect, true);
    for row in &view.rows {
        let r = dev(row.rect, s);
        if r.bottom < content.top || r.top > content.bottom {
            continue;
        }
        paint_row(canvas, fonts, t, s, row);
    }
    canvas.restore();

    if let Some((pos, frac)) = view.scrollbar {
        scrollbar(canvas, t, s, view.content, pos, frac);
    }

    footer(canvas, fonts, t, s, view);
    canvas.restore();

    // The close button sits on top of the clip so its hover plate is never cut.
    if let Some(b) = view.buttons.iter().find(|b| b.button == Button::Close) {
        close_button(canvas, t, s, b);
    }
}

fn title_bar(canvas: &Canvas, fonts: &mut Fonts, t: &ResolvedTokens, s: f32, view: &ViewModel) {
    let bar = dev(view.title_bar, s);
    let font = fonts.font(&t.font_label, Weight::Medium, 11.0 * s);
    let cy = bar.center_y();
    canvas.draw_circle(
        (bar.left + 22.0 * s, cy),
        3.0 * s,
        &fill_paint(col(t.accent, 1.0)),
    );
    draw_text(
        canvas,
        &font,
        "SETTINGS",
        bar.left + 34.0 * s,
        base(cy, 11.0 * s),
        1.6 * s,
        &fill_paint(col(t.text_dim, 1.0)),
    );
}

fn close_button(canvas: &Canvas, t: &ResolvedTokens, s: f32, b: &ButtonView) {
    let r = dev(b.rect, s);
    if b.hovered && b.enabled {
        let rad = (r.height().min(r.width()) / 2.0).min(10.0 * s);
        canvas.draw_rrect(
            RRect::new_rect_xy(r, rad, rad),
            &fill_paint(col(t.surface2, 1.0)),
        );
    }
    let a = if b.enabled { 1.0 } else { DIMMED };
    let ink = if b.hovered { t.text } else { t.text_dim };
    let mut p = stroke_paint(col(ink, a), 1.5 * s);
    p.set_stroke_cap(Cap::Round);
    let d = 4.5 * s;
    let (cx, cy) = (r.center_x(), r.center_y());
    canvas.draw_line((cx - d, cy - d), (cx + d, cy + d), &p);
    canvas.draw_line((cx - d, cy + d), (cx + d, cy - d), &p);
}

fn sidebar(canvas: &Canvas, fonts: &mut Fonts, t: &ResolvedTokens, s: f32, view: &ViewModel) {
    paint_field(canvas, fonts, t, s, &view.search, true, 13.0);
    for item in &view.nav {
        nav_item(canvas, fonts, t, s, item);
    }
}

fn nav_item(canvas: &Canvas, fonts: &mut Fonts, t: &ResolvedTokens, s: f32, n: &NavItem) {
    let r = dev(n.rect, s);
    let alpha = if n.matches == Some(0) { DIMMED } else { 1.0 };
    let rad = 9.0 * s;
    if n.active {
        canvas.draw_rrect(
            RRect::new_rect_xy(r, rad, rad),
            &fill_paint(col(t.surface2, 1.0)),
        );
    } else if n.hovered {
        canvas.draw_rrect(
            RRect::new_rect_xy(r, rad, rad),
            &fill_paint(col(t.surface2, 0.45)),
        );
    }
    if n.active {
        // A small accent tick on the left of the active page.
        canvas.draw_rrect(
            RRect::new_rect_xy(
                Rect::from_xywh(r.left + 5.0 * s, r.center_y() - 7.0 * s, 2.5 * s, 14.0 * s),
                1.2 * s,
                1.2 * s,
            ),
            &fill_paint(col(t.accent, 1.0)),
        );
    }
    let size = 12.5 * s;
    let font = fonts.font(
        &t.font_label,
        if n.active {
            Weight::SemiBold
        } else {
            Weight::Medium
        },
        size,
    );
    let ink = if n.active || n.hovered {
        t.text
    } else {
        t.text_dim
    };
    draw_text(
        canvas,
        &font,
        n.page.label(),
        r.left + 16.0 * s,
        base(r.center_y(), size),
        0.0,
        &fill_paint(col(ink, alpha)),
    );
    if let Some(count) = n.matches.filter(|c| *c > 0) {
        let mono = fonts.font("Geist Mono", Weight::Medium, 10.0 * s);
        let txt = count.to_string();
        let tw = text_width(&mono, &txt, 0.0);
        let w = (tw + 10.0 * s).max(18.0 * s);
        let b = Rect::from_xywh(r.right - w - 10.0 * s, r.center_y() - 9.0 * s, w, 18.0 * s);
        pill(canvas, b, col(t.accent, 1.0));
        draw_text(
            canvas,
            &mono,
            &txt,
            b.center_x() - tw / 2.0,
            base(b.center_y(), 10.0 * s),
            0.0,
            &fill_paint(col(ink_on(t, t.accent), 1.0)),
        );
    }
}

fn paint_row(canvas: &Canvas, fonts: &mut Fonts, t: &ResolvedTokens, s: f32, row: &RowView) {
    let r = dev(row.rect, s);
    if !row.enabled {
        let mut p = skia_safe::Paint::default();
        p.set_alpha_f(DIMMED);
        canvas.save_layer(
            &skia_safe::canvas::SaveLayerRec::default()
                .bounds(&r)
                .paint(&p),
        );
    }
    if row.hovered && row.enabled {
        canvas.draw_rrect(
            RRect::new_rect_xy(r.with_inset((6.0 * s, 2.0 * s)), 10.0 * s, 10.0 * s),
            &fill_paint(col(t.surface2, 0.4)),
        );
    }

    let lx = r.left + PAD * s + 6.0 * s;
    let lsize = 13.0 * s;
    let hsize = 11.0 * s;
    let label_font = fonts.font(&t.font_label, Weight::Medium, lsize);
    let hint_font = fonts.font(&t.font_label, Weight::Regular, hsize);
    // The control's left edge limits the label's width.
    let cr = dev(row.control_rect, s);
    let max_w = (cr.left - lx - 12.0 * s).max(40.0 * s);
    let top_h = dj_core::settings_ui::ROW_H * s;
    let cy = r.top + top_h / 2.0;
    let (label_y, hint_y) = if row.hint.is_empty() {
        (base(cy, lsize), 0.0)
    } else {
        (cy - 3.0 * s, cy + 13.0 * s)
    };
    draw_label(
        canvas,
        &label_font,
        t,
        row.label,
        row.matched,
        lx,
        label_y,
        max_w,
    );
    if !row.hint.is_empty() {
        let txt = fit(&hint_font, row.hint, 0.0, max_w);
        draw_text(
            canvas,
            &hint_font,
            &txt,
            lx,
            hint_y,
            0.0,
            &fill_paint(col(t.text_dim, 1.0)),
        );
    }

    match &row.control {
        Control::Toggle { on } => toggle(canvas, t, s, cr, *on),
        Control::Segmented { options, selected } => {
            segmented(canvas, fonts, t, s, cr, row, options, *selected)
        }
        Control::Chip { text, warn } => chip(canvas, fonts, t, s, cr, text, *warn),
        Control::Slider {
            value,
            min,
            max,
            text,
            dragging,
        } => slider(
            canvas,
            fonts,
            t,
            s,
            cr,
            row,
            (*value, *min, *max),
            text,
            *dragging,
        ),
        Control::Fixed { text } => {
            let mono = fonts.font("Geist Mono", Weight::Regular, 11.5 * s);
            let tw = text_width(&mono, text, 0.0);
            draw_text(
                canvas,
                &mono,
                text,
                cr.right - tw,
                base(r.top + top_h / 2.0, 11.5 * s),
                0.0,
                &fill_paint(col(t.text_dim, 1.0)),
            );
        }
        Control::Lines { lines } => {
            let mono = fonts.font("Geist Mono", Weight::Regular, 11.0 * s);
            for (i, line) in lines.iter().enumerate() {
                let y = r.top + top_h + (i as f32 * 18.0 + 4.0) * s;
                let txt = fit(&mono, line, 0.0, r.right - lx - 16.0 * s);
                draw_text(
                    canvas,
                    &mono,
                    &txt,
                    lx + 12.0 * s,
                    y,
                    0.0,
                    &fill_paint(col(t.text_dim, 1.0)),
                );
            }
        }
    }
    if !row.enabled {
        canvas.restore();
    }
}

/// The label with the matched char range (`[a, b)`) in the accent colour.
#[allow(clippy::too_many_arguments)]
fn draw_label(
    canvas: &Canvas,
    font: &Font,
    t: &ResolvedTokens,
    label: &str,
    matched: Option<(usize, usize)>,
    x: f32,
    y: f32,
    max_w: f32,
) {
    let text = fit(font, label, 0.0, max_w);
    let chars: Vec<char> = text.chars().collect();
    let (a, b) = matched.map_or((0, 0), |(a, b)| (a.min(chars.len()), b.min(chars.len())));
    if a >= b {
        draw_text(
            canvas,
            font,
            &text,
            x,
            y,
            0.0,
            &fill_paint(col(t.text, 1.0)),
        );
        return;
    }
    let seg = |from: usize, to: usize| chars[from..to].iter().collect::<String>();
    let (pre, mid, post) = (seg(0, a), seg(a, b), seg(b, chars.len()));
    let mut cx = x;
    for (part, color) in [(pre, t.text), (mid, t.accent), (post, t.text)] {
        if part.is_empty() {
            continue;
        }
        draw_text(
            canvas,
            font,
            &part,
            cx,
            y,
            0.0,
            &fill_paint(col(color, 1.0)),
        );
        cx += text_width(font, &part, 0.0);
    }
}

fn toggle(canvas: &Canvas, t: &ResolvedTokens, s: f32, r: Rect, on: bool) {
    let (track, edge) = if on {
        (t.accent, t.accent)
    } else {
        (t.surface1, t.surface2)
    };
    pill(canvas, r, col(track, 1.0));
    pill_outline(canvas, r, col(edge, 1.0), s);
    let rad = r.height() / 2.0;
    let knob_x = if on { r.right - rad } else { r.left + rad };
    let knob = if on { ink_on(t, t.accent) } else { t.text_dim };
    canvas.draw_circle(
        (knob_x, r.center_y()),
        rad - 4.0 * s,
        &fill_paint(col(knob, 1.0)),
    );
}

#[allow(clippy::too_many_arguments)]
fn segmented(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    s: f32,
    cr: Rect,
    row: &RowView,
    options: &[&'static str],
    selected: usize,
) {
    pill(canvas, cr, col(t.surface1, 1.0));
    pill_outline(canvas, cr, col(t.surface2, 1.0), s);
    let size = 11.5 * s;
    let font = fonts.font(&t.font_label, Weight::Medium, size);
    for (i, label) in options.iter().enumerate() {
        let Some(part) = row.parts.get(i) else { break };
        let p = dev(*part, s);
        let on = i == selected;
        if on {
            pill(canvas, p.with_inset((3.0 * s, 3.0 * s)), col(t.accent, 1.0));
        }
        let ink = if on { ink_on(t, t.accent) } else { t.text_dim };
        let tw = text_width(&font, label, 0.0);
        draw_text(
            canvas,
            &font,
            label,
            p.center_x() - tw / 2.0,
            base(p.center_y(), size),
            0.0,
            &fill_paint(col(ink, 1.0)),
        );
    }
}

fn chip(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    s: f32,
    cr: Rect,
    text: &str,
    warn: bool,
) {
    let (edge, ink) = if warn {
        (t.warn, t.warn)
    } else {
        (t.surface2, t.text)
    };
    pill(canvas, cr, col(t.surface1, 1.0));
    pill_outline(canvas, cr, col(edge, 1.0), s);
    let size = 11.5 * s;
    let font = fonts.font("Geist Mono", Weight::Medium, size);
    let txt = fit(&font, text, 0.0, cr.width() - 16.0 * s);
    let tw = text_width(&font, &txt, 0.0);
    draw_text(
        canvas,
        &font,
        &txt,
        cr.center_x() - tw / 2.0,
        base(cr.center_y(), size),
        0.0,
        &fill_paint(col(ink, 1.0)),
    );
}

#[allow(clippy::too_many_arguments)]
fn slider(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    s: f32,
    cr: Rect,
    row: &RowView,
    (value, min, max): (f32, f32, f32),
    text: &str,
    dragging: bool,
) {
    let track = row.parts.first().map_or(cr, |p| dev(*p, s));
    let cy = track.center_y();
    let th = 4.0 * s;
    let bar = Rect::from_xywh(track.left, cy - th / 2.0, track.width(), th);
    pill(canvas, bar, col(t.surface2, 1.0));
    let x = slider_thumb_x(&track, value, min, max);
    pill(
        canvas,
        Rect::from_xywh(bar.left, bar.top, (x - bar.left).max(0.0), th),
        col(t.accent, 1.0),
    );
    let r = if dragging { 9.0 * s } else { 7.0 * s };
    if dragging {
        canvas.draw_circle((x, cy), r + 4.0 * s, &fill_paint(col(t.accent, 0.25)));
    }
    canvas.draw_circle((x, cy), r, &fill_paint(col(t.text, 1.0)));
    canvas.draw_circle((x, cy), r, &stroke_paint(col(t.accent, 1.0), 1.5 * s));
    let size = 11.5 * s;
    let mono = fonts.font("Geist Mono", Weight::Medium, size);
    let tw = text_width(&mono, text, 0.0);
    draw_text(
        canvas,
        &mono,
        text,
        cr.right - tw,
        base(cy, size),
        0.0,
        &fill_paint(col(t.text, 1.0)),
    );
}

fn scrollbar(canvas: &Canvas, t: &ResolvedTokens, s: f32, content: LRect, pos: f32, frac: f32) {
    let c = dev(content, s);
    let track = Rect::from_xywh(
        c.right - 7.0 * s,
        c.top + 4.0 * s,
        3.0 * s,
        c.height() - 8.0 * s,
    );
    let len = (track.height() * frac.clamp(0.05, 1.0)).max(16.0 * s);
    let y = track.top + (track.height() - len).max(0.0) * pos.clamp(0.0, 1.0);
    let r = 1.5 * s;
    canvas.draw_rrect(
        RRect::new_rect_xy(Rect::from_xywh(track.left, y, track.width(), len), r, r),
        &fill_paint(col(t.text_dim, 0.55)),
    );
}

fn footer(canvas: &Canvas, fonts: &mut Fonts, t: &ResolvedTokens, s: f32, view: &ViewModel) {
    let top = (PANEL.y + PANEL.h - dj_core::settings_ui::FOOTER_H) * s;
    let x0 = (PANEL.x + SIDEBAR_W) * s;
    canvas.draw_line(
        (x0, top),
        ((PANEL.x + PANEL.w) * s, top),
        &stroke_paint(col(t.text, 0.10), s.max(1.0)),
    );
    let mid = top + dj_core::settings_ui::FOOTER_H * s / 2.0;
    if let Some(status) = &view.status {
        let size = 11.5 * s;
        let font = fonts.font(&t.font_label, Weight::Medium, size);
        canvas.draw_circle((x0 + 28.0 * s, mid), 3.0 * s, &fill_paint(col(t.warn, 1.0)));
        draw_text(
            canvas,
            &font,
            status,
            x0 + 40.0 * s,
            base(mid, size),
            0.0,
            &fill_paint(col(t.text_dim, 1.0)),
        );
    }
    for b in view.buttons.iter().filter(|b| b.button != Button::Close) {
        footer_button(canvas, fonts, t, s, b);
    }
}

fn footer_button(canvas: &Canvas, fonts: &mut Fonts, t: &ResolvedTokens, s: f32, b: &ButtonView) {
    let r = dev(b.rect, s);
    let a = if b.enabled { 1.0 } else { DIMMED };
    let rad = 9.0 * s;
    let rr = RRect::new_rect_xy(r, rad, rad);
    let ink = if b.primary {
        let fill = if b.hovered && b.enabled {
            t.accent.lerp(t.text, 0.15)
        } else {
            t.accent
        };
        canvas.draw_rrect(rr, &fill_paint(col(fill, a)));
        ink_on(t, t.accent)
    } else {
        if b.hovered && b.enabled {
            canvas.draw_rrect(rr, &fill_paint(col(t.surface2, 1.0)));
        }
        canvas.draw_rrect(
            RRect::new_rect_xy(
                r.with_inset((0.5 * s, 0.5 * s)),
                rad - 0.5 * s,
                rad - 0.5 * s,
            ),
            &stroke_paint(col(t.surface2.lerp(t.text_dim, 0.4), a), s.max(1.0)),
        );
        t.text
    };
    let size = 12.0 * s;
    let font = fonts.font(&t.font_label, Weight::SemiBold, size);
    let tw = text_width(&font, b.label, 0.0);
    draw_text(
        canvas,
        &font,
        b.label,
        r.center_x() - tw / 2.0,
        base(r.center_y(), size),
        0.0,
        &fill_paint(col(ink, a)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::settings_ui::{
        FieldView, NavItem, Page, RowKey, FOOTER_H, MARGIN, ROW_H, WIN_H, WIN_W,
    };
    use dj_core::theme::ThemeRegistry;
    use skia_safe::{surfaces, AlphaType, ColorType, ImageInfo};

    fn tokens() -> ResolvedTokens {
        ThemeRegistry::with_builtins()
            .get(dj_core::theme::DEFAULT_THEME)
            .unwrap()
            .clone()
    }

    pub(crate) struct Shot {
        pub w: i32,
        pub px: Vec<u8>,
    }

    impl Shot {
        pub fn alpha(&self, x: i32, y: i32) -> u8 {
            self.px[((y * self.w + x) * 4 + 3) as usize]
        }
        pub fn rgb(&self, x: i32, y: i32) -> [u8; 3] {
            let i = ((y * self.w + x) * 4) as usize;
            [self.px[i + 2], self.px[i + 1], self.px[i]]
        }
    }

    pub(crate) fn shoot(
        scale: f32,
        paint: impl FnOnce(&Canvas, &mut Fonts, &ResolvedTokens),
    ) -> Shot {
        let (w, h) = ((WIN_W * scale) as i32, (WIN_H * scale) as i32);
        let mut surface = surfaces::raster_n32_premul((w, h)).unwrap();
        let mut fonts = Fonts::new().unwrap();
        paint(surface.canvas(), &mut fonts, &tokens());
        let info = ImageInfo::new((w, h), ColorType::BGRA8888, AlphaType::Premul, None);
        let mut px = vec![0u8; (w * h * 4) as usize];
        assert!(surface.image_snapshot().read_pixels(
            &info,
            &mut px,
            (w * 4) as usize,
            (0, 0),
            skia_safe::image::CachingHint::Allow
        ));
        Shot { w, px }
    }

    fn lr(x: f32, y: f32, w: f32, h: f32) -> LRect {
        LRect::new(x, y, w, h)
    }

    fn row(i: usize, key: RowKey, label: &'static str, control: Control) -> RowView {
        let cx = PANEL.x + SIDEBAR_W;
        let y = PANEL.y + TITLE_H + 8.0 + i as f32 * ROW_H;
        let rect = lr(cx + 8.0, y, PANEL.w - SIDEBAR_W - 16.0, ROW_H);
        let right = rect.x + rect.w - 16.0;
        let (control_rect, parts) = match &control {
            Control::Toggle { .. } => (lr(right - 40.0, y + 16.0, 40.0, 24.0), vec![]),
            Control::Segmented { options, .. } => {
                let n = options.len() as f32;
                let cr = lr(right - 80.0 * n, y + 12.0, 80.0 * n, 32.0);
                let parts = (0..options.len())
                    .map(|k| lr(cr.x + 80.0 * k as f32, cr.y, 80.0, cr.h))
                    .collect();
                (cr, parts)
            }
            Control::Chip { .. } => (lr(right - 120.0, y + 14.0, 120.0, 28.0), vec![]),
            Control::Slider { .. } => (
                lr(right - 300.0, y + 12.0, 300.0, 32.0),
                vec![lr(right - 300.0, y + 12.0, 220.0, 32.0)],
            ),
            Control::Fixed { .. } => (lr(right - 120.0, y + 14.0, 120.0, 28.0), vec![]),
            Control::Lines { .. } => (lr(right - 10.0, y + 14.0, 10.0, 28.0), vec![]),
        };
        RowView {
            key,
            label,
            hint: "A short hint under the label",
            control,
            rect,
            control_rect,
            parts,
            enabled: true,
            hovered: false,
            matched: None,
        }
    }

    fn view(rows: Vec<RowView>) -> ViewModel {
        let cx = PANEL.x + SIDEBAR_W;
        ViewModel {
            title: "Notch".into(),
            nav: Page::ALL
                .iter()
                .enumerate()
                .map(|(i, p)| NavItem {
                    page: *p,
                    rect: lr(
                        PANEL.x + 12.0,
                        PANEL.y + TITLE_H + 64.0 + i as f32 * 36.0,
                        176.0,
                        32.0,
                    ),
                    active: i == 1,
                    hovered: i == 2,
                    matches: None,
                })
                .collect(),
            search: FieldView {
                rect: lr(PANEL.x + 12.0, PANEL.y + TITLE_H + 12.0, 176.0, 36.0),
                text: String::new(),
                caret: 0,
                focused: false,
                placeholder: "Search settings",
            },
            content: lr(
                cx,
                PANEL.y + TITLE_H,
                PANEL.w - SIDEBAR_W,
                PANEL.h - TITLE_H - FOOTER_H,
            ),
            rows,
            scrollbar: None,
            buttons: vec![
                ButtonView {
                    button: Button::Close,
                    label: "Close",
                    rect: lr(PANEL.x + PANEL.w - 40.0, PANEL.y + 6.0, 32.0, 32.0),
                    enabled: true,
                    hovered: true,
                    primary: false,
                },
                ButtonView {
                    button: Button::Reset,
                    label: "Reset",
                    rect: lr(PANEL.x + 230.0, PANEL.y + PANEL.h - 46.0, 80.0, 36.0),
                    enabled: true,
                    hovered: false,
                    primary: false,
                },
                ButtonView {
                    button: Button::Apply,
                    label: "Apply",
                    rect: lr(
                        PANEL.x + PANEL.w - 190.0,
                        PANEL.y + PANEL.h - 46.0,
                        80.0,
                        36.0,
                    ),
                    enabled: false,
                    hovered: false,
                    primary: false,
                },
                ButtonView {
                    button: Button::Save,
                    label: "Save",
                    rect: lr(
                        PANEL.x + PANEL.w - 100.0,
                        PANEL.y + PANEL.h - 46.0,
                        80.0,
                        36.0,
                    ),
                    enabled: true,
                    hovered: true,
                    primary: true,
                },
            ],
            status: Some("Unsaved changes".into()),
            title_bar: lr(PANEL.x, PANEL.y, PANEL.w, TITLE_H),
        }
    }

    fn slider_ctl(value: f32, dragging: bool) -> Control {
        Control::Slider {
            value,
            min: 0.0,
            max: 100.0,
            text: format!("{value:.0} px"),
            dragging,
        }
    }

    fn all_controls() -> Vec<RowView> {
        let mut rows = vec![
            row(
                0,
                RowKey::Autostart,
                "Start with Windows",
                Control::Toggle { on: true },
            ),
            row(
                1,
                RowKey::Reserve,
                "Reserve top space",
                Control::Toggle { on: false },
            ),
            row(
                2,
                RowKey::Style,
                "Island style",
                Control::Segmented {
                    options: vec!["notch", "pill"],
                    selected: 0,
                },
            ),
            row(
                3,
                RowKey::SnapHotkey,
                "Snap",
                Control::Chip {
                    text: "Ctrl+Shift+S".into(),
                    warn: true,
                },
            ),
            row(4, RowKey::NotchWidth, "Notch width", slider_ctl(40.0, true)),
            row(
                5,
                RowKey::TerminalHotkey,
                "Terminal",
                Control::Fixed {
                    text: "Ctrl+Alt+`".into(),
                },
            ),
            row(
                6,
                RowKey::SpotlightFolders,
                "Folders searched",
                Control::Lines {
                    lines: vec!["Desktop".into(), "Documents".into()],
                },
            ),
        ];
        rows[1].enabled = false;
        rows[2].hovered = true;
        rows[0].matched = Some((0, 5));
        rows
    }

    #[test]
    fn every_control_renders_inside_the_panel_and_margin_stays_clear() {
        for scale in [1.0, 1.5] {
            let mut v = view(all_controls());
            v.scrollbar = Some((0.3, 0.5));
            let shot = shoot(scale, |c, f, t| paint_settings(c, f, t, scale, &v));
            let m = (MARGIN * scale) as i32;
            // Panel centre is painted, the far corner of the margin is clear.
            assert!(shot.alpha(m + (PANEL.w * scale) as i32 / 2, m + 20) > 200);
            assert!(shot.alpha(2, 2) <= 2, "corner alpha {}", shot.alpha(2, 2));
            assert!(shot.alpha(shot.w - 3, (WIN_H * scale) as i32 - 3) <= 2);
            // The drop shadow reaches into the margin below the panel.
            let below = m + (PANEL.h * scale) as i32 + 3;
            assert!(shot.alpha(shot.w / 2, below) > 0);
        }
    }

    #[test]
    fn search_state_dirty_status_and_empty_page_render() {
        let mut v = view(all_controls());
        v.title = "Search".into();
        v.search.text = "notch".into();
        v.search.caret = 3;
        v.search.focused = true;
        for (i, n) in v.nav.iter_mut().enumerate() {
            n.matches = Some(if i == 1 { 3 } else { 0 });
        }
        v.status = Some("Press \u{2715} again to discard".into());
        let shot = shoot(1.0, |c, f, t| paint_settings(c, f, t, 1.0, &v));
        assert!(shot.alpha((MARGIN + 400.0) as i32, (MARGIN + 100.0) as i32) > 200);
        // No rows, no status, no buttons: still fine.
        v.rows.clear();
        v.status = None;
        v.buttons.clear();
        let _ = shoot(1.0, |c, f, t| paint_settings(c, f, t, 1.0, &v));
    }

    #[test]
    fn slider_thumb_follows_the_value() {
        let track = Rect::from_xywh(100.0, 10.0, 200.0, 4.0);
        assert_eq!(slider_thumb_x(&track, 0.0, 0.0, 100.0), 100.0);
        assert_eq!(slider_thumb_x(&track, 50.0, 0.0, 100.0), 200.0);
        assert_eq!(slider_thumb_x(&track, 500.0, 0.0, 100.0), 300.0);
        assert_eq!(slider_thumb_x(&track, 5.0, 5.0, 5.0), 100.0);

        // And the drawn thumb: its centre pixel is the text colour, the same spot is not
        // when the value moves away.
        let t = tokens();
        let at = |value: f32| {
            let mut r = row(
                0,
                RowKey::NotchWidth,
                "Notch width",
                slider_ctl(value, false),
            );
            r.hint = "";
            let v = view(vec![r.clone()]);
            let track = dev(r.parts[0], 1.0);
            let x = slider_thumb_x(&track, value, 0.0, 100.0);
            let shot = shoot(1.0, |c, f, tk| paint_settings(c, f, tk, 1.0, &v));
            (shot.rgb(x as i32, track.center_y() as i32), x)
        };
        let [tr, tg, tb] = t.text.to_u8();
        let (c20, x20) = at(20.0);
        let (c80, x80) = at(80.0);
        assert!(x80 > x20 + 100.0);
        for c in [c20, c80] {
            assert!(
                (c[0] as i32 - tr as i32).abs() < 12,
                "{c:?} vs {:?}",
                t.text
            );
            assert!((c[1] as i32 - tg as i32).abs() < 12);
            assert!((c[2] as i32 - tb as i32).abs() < 12);
        }
    }

    #[test]
    fn rows_outside_the_content_are_clipped() {
        let mut r = row(0, RowKey::Autostart, "Start", Control::Toggle { on: true });
        // Scrolled far above the content area.
        r.rect.y = PANEL.y + 2.0;
        r.control_rect.y = PANEL.y + 10.0;
        let v = view(vec![r]);
        let _ = shoot(1.0, |c, f, t| paint_settings(c, f, t, 1.0, &v));
    }
}
