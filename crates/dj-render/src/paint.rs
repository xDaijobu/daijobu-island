//! Draws the island: pill body, lofi texture, idle content and the home grid.
//! Everything is laid out in logical px and multiplied by `scale` explicitly, so
//! grain, dither and scanlines stay exactly one device pixel.

use crate::fonts::{Fonts, Weight};
use crate::fx::Effects;
use crate::icons::{draw_icon, draw_lock, draw_volume_icon};
use dj_core::color::Rgb;
use dj_core::features::FEATURES;
use dj_core::island::IslandView;
use dj_core::layout::{self, Rect as LRect};
use dj_core::theme::ResolvedTokens;
use skia_safe::canvas::SaveLayerRec;
use skia_safe::paint::{Cap, Join, Style};
use skia_safe::{image_filters, Canvas, ClipOp, Color, Font, Paint, RRect, Rect, TileMode};

pub struct Frame<'a> {
    pub tokens: &'a ResolvedTokens,
    pub view: &'a IslandView,
    /// Device pixels per logical pixel.
    pub scale: f32,
    pub clock: (u32, u32),
    /// Seconds since start; only used to seed the grain.
    pub time: f32,
    /// Shown in the home footer when no hint is active (e.g. a rejected theme).
    pub notice: Option<&'a str>,
}

pub struct Painter {
    fonts: Fonts,
    fx: Effects,
}

fn col(c: Rgb, alpha: f32) -> Color {
    let [r, g, b] = c.to_u8();
    Color::from_argb((alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8, r, g, b)
}

fn fill_paint(c: Color) -> Paint {
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_color(c);
    p
}

fn stroke_paint(c: Color, width: f32) -> Paint {
    let mut p = fill_paint(c);
    p.set_style(Style::Stroke);
    p.set_stroke_width(width);
    p.set_stroke_cap(Cap::Round);
    p.set_stroke_join(Join::Round);
    p
}

fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn text_width(font: &Font, s: &str, tracking: f32) -> f32 {
    let n = s.chars().count() as f32;
    font.measure_str(s, None).0 + tracking * (n - 1.0).max(0.0)
}

fn draw_text(canvas: &Canvas, font: &Font, s: &str, x: f32, y: f32, tracking: f32, paint: &Paint) {
    if tracking == 0.0 {
        canvas.draw_str(s, (x, y), font, paint);
        return;
    }
    let mut cx = x;
    for ch in s.chars() {
        let st = ch.to_string();
        canvas.draw_str(&st, (cx, y), font, paint);
        cx += font.measure_str(&st, None).0 + tracking;
    }
}

/// Shortens `s` with ".." until it fits `max_w`.
fn fit(font: &Font, s: &str, tracking: f32, max_w: f32) -> String {
    if text_width(font, s, tracking) <= max_w {
        return s.to_string();
    }
    let mut chars: Vec<char> = s.chars().collect();
    while chars.len() > 1 {
        chars.pop();
        let cand: String = chars.iter().collect::<String>() + "..";
        if text_width(font, &cand, tracking) <= max_w {
            return cand;
        }
    }
    String::new()
}

impl Painter {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            fonts: Fonts::new()?,
            fx: Effects::new()?,
        })
    }

    pub fn paint(&mut self, canvas: &Canvas, f: &Frame) {
        let (s, t, v) = (f.scale, f.tokens, f.view);
        let r = v.rect;
        let rect = Rect::new(
            (r.x * s).round(),
            (r.y * s).round(),
            ((r.x + r.w) * s).round(),
            ((r.y + r.h) * s).round(),
        );
        let radius = v.radius * s;
        let rrect = RRect::new_rect_xy(rect, radius, radius);

        canvas.draw_rrect(rrect, &fill_paint(col(t.pill, 1.0)));

        canvas.save();
        canvas.clip_rrect(rrect, ClipOp::Intersect, true);

        if t.inner_glow > 0.001 {
            let tint = [t.highlight.r, t.highlight.g, t.highlight.b];
            let shader = self.fx.glow(
                [rect.left, rect.top],
                [rect.width(), rect.height()],
                t.dither_levels.round().max(2.0),
                (t.dither_cell * s).round().max(1.0),
                t.inner_glow * 1.6,
                tint,
            );
            if let Some(shader) = shader {
                let mut p = Paint::default();
                p.set_shader(shader);
                canvas.draw_rect(rect, &p);
            }
        }

        // The OSD takes over the pill, so idle and home fade out while it is up.
        let osd_a = v.osd.alpha;
        let idle_op = (1.0 - smoothstep(0.0, 0.45, v.fade)) * (1.0 - osd_a);
        let home_op = smoothstep(0.35, 1.0, v.fade) * (1.0 - osd_a);
        let center = (rect.center_x(), rect.center_y());
        if idle_op > 0.01 {
            begin_layer(canvas, &rect, idle_op, center, s);
            self.idle(canvas, f, &rect);
            canvas.restore();
        }
        if home_op > 0.01 {
            begin_layer(canvas, &rect, home_op, center, s);
            self.home(canvas, f, &rect);
            canvas.restore();
        }
        if osd_a > 0.01 {
            begin_layer(canvas, &rect, osd_a, center, s);
            self.osd(canvas, f, &rect);
            canvas.restore();
        }

        if t.grain > 0.001 || t.scanlines > 0.001 {
            let seed = (f.time * t.grain_fps).floor().rem_euclid(256.0);
            if let Some(shader) = self.fx.texture(seed, t.grain, t.scanlines) {
                let mut p = Paint::default();
                p.set_shader(shader);
                canvas.draw_rect(rect, &p);
            }
        }
        canvas.restore();

        if t.hairline > 0.001 {
            let inset = rect.with_inset((0.5, 0.5));
            let hr = RRect::new_rect_xy(inset, (radius - 0.5).max(0.0), (radius - 0.5).max(0.0));
            canvas.draw_rrect(hr, &stroke_paint(col(t.text, t.hairline), 1.0));
        }
    }

    fn idle(&mut self, canvas: &Canvas, f: &Frame, rect: &Rect) {
        let (s, t) = (f.scale, f.tokens);
        let text = format!("{:02}:{:02}", f.clock.0, f.clock.1);
        let font = self.fonts.font(&t.font_label, Weight::Medium, 13.0 * s);
        let tw = text_width(&font, &text, 0.0);
        let (lamp_r, gap) = (3.2 * s, 9.0 * s);
        let group = lamp_r * 2.0 + gap + tw;
        let x0 = rect.center_x() - group / 2.0;
        let cy = rect.center_y();
        canvas.draw_circle((x0 + lamp_r, cy), 6.5 * s, &fill_paint(col(t.accent, 0.20)));
        canvas.draw_circle((x0 + lamp_r, cy), lamp_r, &fill_paint(col(t.accent, 1.0)));
        draw_text(
            canvas,
            &font,
            &text,
            x0 + lamp_r * 2.0 + gap,
            cy + 0.35 * 13.0 * s,
            0.0,
            &fill_paint(col(t.text, 1.0)),
        );
    }

    fn home(&mut self, canvas: &Canvas, f: &Frame, rect: &Rect) {
        let (s, t, v) = (f.scale, f.tokens, f.view);
        // Anchored to the pill's centre-top so the content holds still while the
        // pill grows around it.
        let ox = rect.center_x() - layout::HOME_W * s / 2.0;
        let oy = rect.top;
        let at = |r: LRect| Rect::from_xywh(ox + r.x * s, oy + r.y * s, r.w * s, r.h * s);

        // Header.
        let label = self.fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
        let h = layout::HEADER;
        let hy = oy + (h.y + h.h / 2.0) * s;
        canvas.draw_circle(
            (ox + (h.x + 3.0) * s, hy),
            2.6 * s,
            &fill_paint(col(t.accent, 1.0)),
        );
        draw_text(
            canvas,
            &label,
            "DAIJOBU ISLAND",
            ox + (h.x + 14.0) * s,
            hy + 0.35 * 10.0 * s,
            1.8 * s,
            &fill_paint(col(t.text_dim, 1.0)),
        );
        let digits = self.fonts.font(&t.font_digits, Weight::Regular, 22.0 * s);
        let clock = format!("{:02}:{:02}", f.clock.0, f.clock.1);
        let cw = text_width(&digits, &clock, 0.0);
        draw_text(
            canvas,
            &digits,
            &clock,
            ox + (h.x + h.w) * s - cw,
            hy + 0.35 * 22.0 * s,
            0.0,
            &fill_paint(col(t.accent, 1.0)),
        );

        // Tiles.
        let name_font = self.fonts.font(&t.font_label, Weight::Medium, 8.5 * s);
        let tiles = layout::home_tiles();
        let tile_radius = 14.0 * t.radius_scale.min(1.2) * s;
        for (i, (tile, feat)) in tiles.iter().zip(FEATURES.iter()).enumerate() {
            let dx = match v.shake {
                Some((j, dx)) if j == i => dx * s,
                _ => 0.0,
            };
            let tr = at(*tile).with_offset((dx, 0.0));
            let hovered = v.hover == Some(i);
            let body = if hovered { t.surface2 } else { t.surface1 };
            let rr = RRect::new_rect_xy(tr, tile_radius, tile_radius);
            canvas.draw_rrect(rr, &fill_paint(col(body, 1.0)));
            canvas.draw_rrect(
                RRect::new_rect_xy(
                    tr.with_inset((0.5, 0.5)),
                    tile_radius - 0.5,
                    tile_radius - 0.5,
                ),
                &stroke_paint(col(t.surface2, 0.9), 1.0),
            );
            let ink = if hovered { t.text } else { t.text_dim };
            let (cx, top) = (tr.center_x(), tr.top);
            draw_icon(
                canvas,
                feat.icon,
                cx,
                top + 27.0 * s,
                24.0 * s,
                &stroke_paint(col(ink, 0.95), 1.6 * s),
            );
            draw_lock(
                canvas,
                tr.right - 11.0 * s,
                top + 11.0 * s,
                10.0 * s,
                &fill_paint(col(t.text_dim, 0.75)),
            );
            let name = feat.name.to_uppercase();
            let tracking = 0.7 * s;
            let nm = fit(&name_font, &name, tracking, tr.width() - 8.0 * s);
            let nw = text_width(&name_font, &nm, tracking);
            draw_text(
                canvas,
                &name_font,
                &nm,
                cx - nw / 2.0,
                top + 62.0 * s,
                tracking,
                &fill_paint(col(ink, 1.0)),
            );
        }

        // Footer: hint > notice > default line, plus the theme name on the right.
        let foot = at(layout::FOOTER);
        let small = self.fonts.font(&t.font_label, Weight::Regular, 9.0 * s);
        let tracking = 1.2 * s;
        let base_y = foot.center_y() + 0.35 * 9.0 * s;
        let theme_name = t.name.to_uppercase();
        let tw = text_width(&small, &theme_name, tracking);
        draw_text(
            canvas,
            &small,
            &theme_name,
            foot.right - tw,
            base_y,
            tracking,
            &fill_paint(col(t.text_dim, 0.7)),
        );
        let room = foot.width() - tw - 16.0 * s;

        let hint = v.hint.map(|(i, a)| (FEATURES[i].name.to_uppercase(), a));
        let hint_a = hint.as_ref().map_or(0.0, |h| h.1);
        let idle_line = match f.notice {
            Some(n) => (fit(&small, &n.to_uppercase(), tracking, room), t.warn),
            None => ("13 FEATURES - ALL COMING SOON".to_string(), t.text_dim),
        };
        draw_text(
            canvas,
            &small,
            &idle_line.0,
            foot.left,
            base_y,
            tracking,
            &fill_paint(col(idle_line.1, 1.0 - hint_a)),
        );
        if let Some((name, a)) = hint {
            let line = fit(&small, &format!("{name} - COMING SOON"), tracking, room);
            draw_text(
                canvas,
                &small,
                &line,
                foot.left,
                base_y,
                tracking,
                &fill_paint(col(t.accent, a)),
            );
        }
    }

    /// Volume OSD: icon, level bar and percent, centred on the pill.
    fn osd(&mut self, canvas: &Canvas, f: &Frame, rect: &Rect) {
        let (s, t, o) = (f.scale, f.tokens, f.view.osd);
        let ox = rect.center_x() - layout::OSD_W * s / 2.0;
        let oy = rect.center_y() - layout::OSD_H * s / 2.0;
        let mid_y = oy + layout::OSD_H * s / 2.0;
        let at = |r: LRect| Rect::from_xywh(ox + r.x * s, oy + r.y * s, r.w * s, r.h * s);
        let ink = if o.muted { t.text_dim } else { t.text };

        draw_volume_icon(
            canvas,
            o.icon,
            ox + layout::OSD_ICON_CX * s,
            mid_y,
            layout::OSD_ICON_SIZE * s,
            &stroke_paint(col(ink, 1.0), 1.6 * s),
        );

        let bar = at(layout::OSD_BAR);
        let bar_r = bar.height() / 2.0;
        canvas.draw_rrect(
            RRect::new_rect_xy(bar, bar_r, bar_r),
            &fill_paint(col(t.surface2, 1.0)),
        );
        let fill_w = o.level.clamp(0.0, 1.0) * layout::OSD_BAR.w * s;
        if fill_w >= 0.5 * s {
            let fill = Rect::from_xywh(bar.left, bar.top, fill_w, bar.height());
            canvas.draw_rrect(
                RRect::new_rect_xy(fill, 2.0 * s, 2.0 * s),
                &fill_paint(col(ink, 1.0)),
            );
        }

        let font = self.fonts.font(&t.font_digits, Weight::Medium, 12.0 * s);
        let txt = o.percent.to_string();
        let tw = text_width(&font, &txt, 0.0);
        draw_text(
            canvas,
            &font,
            &txt,
            ox + layout::OSD_VALUE_RIGHT * s - tw,
            mid_y + 0.35 * 12.0 * s,
            0.0,
            &fill_paint(col(ink, 1.0)),
        );
    }
}

/// Starts a cross-fade layer: opacity, blur and a 0.94 -> 1 scale about `pivot`.
/// The caller draws, then calls `canvas.restore()`.
fn begin_layer(canvas: &Canvas, bounds: &Rect, opacity: f32, pivot: (f32, f32), scale: f32) {
    let mut p = Paint::default();
    p.set_alpha_f(opacity);
    let sigma = (1.0 - opacity) * 7.0 * scale;
    if sigma > 0.3 {
        p.set_image_filter(image_filters::blur(
            (sigma, sigma),
            TileMode::Decal,
            None,
            None,
        ));
    }
    canvas.save_layer(&SaveLayerRec::default().bounds(bounds).paint(&p));
    let k = 0.94 + 0.06 * opacity;
    canvas.translate(pivot);
    canvas.scale((k, k));
    canvas.translate((-pivot.0, -pivot.1));
}
