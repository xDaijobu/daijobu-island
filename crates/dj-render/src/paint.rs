//! Draws the island: pill body, lofi texture, idle content and the home grid.
//! Everything is laid out in logical px and multiplied by `scale` explicitly, so
//! grain, dither and scanlines stay exactly one device pixel.

use crate::fonts::{Fonts, Weight};
use crate::fx::Effects;
use crate::icons::{draw_icon, draw_lock, draw_volume_icon};
use crate::vitals::{format_rate, ring_sweep, spark_points};
use dj_core::color::Rgb;
use dj_core::features::FEATURES;
use dj_core::island::{IslandView, MusicUi};
use dj_core::layout::{self, Rect as LRect};
use dj_core::music::NowPlaying;
use dj_core::theme::ResolvedTokens;
use dj_core::vitals::Vitals;
use skia_safe::canvas::SaveLayerRec;
use skia_safe::paint::{Cap, Join, Style};
use skia_safe::{
    image_filters, Canvas, ClipOp, Color, Font, Image, Paint, PathBuilder, RRect, Rect, TileMode,
};

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
    pub music: MusicFrame<'a>,
    /// Latest reading and health for the Vitals panel and the idle lamp.
    pub vitals: &'a Vitals,
    /// Config `[vitals] health_lamp`: the idle lamp follows the health band.
    pub health_lamp: bool,
}

/// What the Music views draw. Built by the app from the Music session.
pub struct MusicFrame<'a> {
    pub ui: MusicUi,
    pub now: Option<&'a NowPlaying>,
    /// VU levels 0..1, or `None` to hide the VU.
    pub bands: Option<&'a [f32]>,
    /// Accent for the Music views: the theme accent, mixed with the cover colour.
    pub accent: Rgb,
    /// Playback position in seconds, interpolated between state messages.
    pub pos: f32,
}

pub struct Painter {
    pub(crate) fonts: Fonts,
    pub(crate) fx: Effects,
    /// Cover art, decoded and downscaled by `set_art`.
    pub(crate) art: Option<Image>,
}

pub(crate) fn col(c: Rgb, alpha: f32) -> Color {
    let [r, g, b] = c.to_u8();
    Color::from_argb((alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8, r, g, b)
}

pub(crate) fn fill_paint(c: Color) -> Paint {
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_color(c);
    p
}

pub(crate) fn stroke_paint(c: Color, width: f32) -> Paint {
    let mut p = fill_paint(c);
    p.set_style(Style::Stroke);
    p.set_stroke_width(width);
    p.set_stroke_cap(Cap::Round);
    p.set_stroke_join(Join::Round);
    p
}

pub(crate) fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(crate) fn text_width(font: &Font, s: &str, tracking: f32) -> f32 {
    let n = s.chars().count() as f32;
    font.measure_str(s, None).0 + tracking * (n - 1.0).max(0.0)
}

pub(crate) fn draw_text(
    canvas: &Canvas,
    font: &Font,
    s: &str,
    x: f32,
    y: f32,
    tracking: f32,
    paint: &Paint,
) {
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
pub(crate) fn fit(font: &Font, s: &str, tracking: f32, max_w: f32) -> String {
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
            art: None,
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

        // The outgoing content (idle) fades early and the incoming one late, so two
        // layers are rarely half-visible at once. The OSD takes over the pill, so every
        // content layer fades out while it is up.
        let osd_a = v.osd.alpha;
        let lay = v.layers;
        let not_osd = 1.0 - osd_a;
        let idle_op = (1.0 - smoothstep(0.0, 0.45, 1.0 - lay.idle)) * not_osd;
        let home_op = smoothstep(0.35, 1.0, lay.home) * not_osd;
        let compact_op = smoothstep(0.35, 1.0, lay.compact) * not_osd;
        let expanded_op = smoothstep(0.35, 1.0, lay.expanded) * not_osd;
        let vitals_op = smoothstep(0.35, 1.0, lay.vitals) * not_osd;
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
        if compact_op > 0.01 {
            begin_layer(canvas, &rect, compact_op, center, s);
            self.music_compact(canvas, f, &rect);
            canvas.restore();
        }
        if expanded_op > 0.01 {
            begin_layer(canvas, &rect, expanded_op, center, s);
            self.music_expanded(canvas, f, &rect);
            canvas.restore();
        }
        if vitals_op > 0.01 {
            begin_layer(canvas, &rect, vitals_op, center, s);
            self.vitals_panel(canvas, f, &rect);
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
        // The lamp shows health when the config asks for it and the band is a warn band.
        let lamp = match f.vitals.health() {
            Some(h) if f.health_lamp && h.band.is_warn() => t.warn,
            _ => t.accent,
        };
        canvas.draw_circle((x0 + lamp_r, cy), 6.5 * s, &fill_paint(col(lamp, 0.20)));
        canvas.draw_circle((x0 + lamp_r, cy), lamp_r, &fill_paint(col(lamp, 1.0)));
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

    /// Vitals panel, 400 x 150: header with score and band, three rings, the net
    /// text with a sparkline, then battery (when present) and the top reason tag.
    /// Layout follows plan.html. Before the first reading the values show "…".
    fn vitals_panel(&mut self, canvas: &Canvas, f: &Frame, rect: &Rect) {
        let (s, t) = (f.scale, f.tokens);
        let ox = rect.center_x() - layout::VITALS_W * s / 2.0;
        let oy = rect.top;
        let reading = f.vitals.latest();
        let health = f.vitals.health();
        let ink = fill_paint(col(t.text, 1.0));

        let label = self.fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
        let digits = self.fonts.font(&t.font_digits, Weight::Medium, 13.0 * s);

        // Header: "daijobu? · health" on the left, "87 · daijobu" on the right.
        let cy = oy + 22.0 * s;
        draw_text(
            canvas,
            &label,
            "daijobu? · health",
            ox + 20.0 * s,
            cy + 0.35 * 10.0 * s,
            0.0,
            &ink,
        );
        let (num, num_font, band, band_ink) = match health {
            Some(h) => {
                let c = if h.band.is_warn() { t.warn } else { t.accent };
                (
                    h.score.to_string(),
                    digits.clone(),
                    format!(" · {}", h.band.label()),
                    c,
                )
            }
            None => ("…".to_string(), label.clone(), String::new(), t.text_dim),
        };
        let right = ox + (layout::VITALS_W - 20.0) * s;
        let bw = text_width(&label, &band, 0.0);
        let nw = text_width(&num_font, &num, 0.0);
        draw_text(
            canvas,
            &label,
            &band,
            right - bw,
            cy + 0.35 * 10.0 * s,
            0.0,
            &fill_paint(col(band_ink, 1.0)),
        );
        draw_text(
            canvas,
            &num_font,
            &num,
            right - bw - nw,
            cy + 0.35 * 13.0 * s,
            0.0,
            &fill_paint(col(band_ink, 1.0)),
        );

        // Three rings, 56 px across with a 4 px stroke. The arc starts at the top and
        // turns warn past the metric's second penalty breakpoint (CPU 85, RAM 88, disk 93).
        let ring_font = self.fonts.font(&t.font_digits, Weight::Medium, 12.0 * s);
        let name_font = self.fonts.font(&t.font_label, Weight::Regular, 9.0 * s);
        let rings = [
            ("CPU", reading.map(|r| r.cpu_pct), 85.0),
            ("RAM", reading.map(|r| r.ram_pct), 88.0),
            ("DISK", reading.map(|r| r.disk_pct), 93.0),
        ];
        for (i, (name, value, hot_at)) in rings.iter().enumerate() {
            let cx = ox + (20.0 + 28.0 + i as f32 * 70.0) * s;
            let ring_cy = oy + (42.0 + 28.0) * s;
            let pct = value.unwrap_or(0.0);
            let arc = if pct > *hot_at { t.warn } else { t.accent };
            let r_mid = 26.0 * s;
            canvas.draw_circle(
                (cx, ring_cy),
                r_mid,
                &stroke_paint(col(t.surface2, 1.0), 4.0 * s),
            );
            let sweep = ring_sweep(pct);
            if sweep > 0.0 {
                let oval = Rect::from_xywh(cx - r_mid, ring_cy - r_mid, r_mid * 2.0, r_mid * 2.0);
                canvas.draw_arc(
                    oval,
                    -90.0,
                    sweep,
                    false,
                    &stroke_paint(col(arc, 1.0), 4.0 * s),
                );
            }

            let (txt, font) = match value {
                Some(v) => (format!("{:.0}", v.clamp(0.0, 100.0)), ring_font.clone()),
                None => ("…".to_string(), label.clone()),
            };
            let tw = text_width(&font, &txt, 0.0);
            draw_text(
                canvas,
                &font,
                &txt,
                cx - tw / 2.0,
                ring_cy + 0.35 * 12.0 * s,
                0.0,
                &ink,
            );

            let tracking = 0.7 * s;
            let nw = text_width(&name_font, name, tracking);
            draw_text(
                canvas,
                &name_font,
                name,
                cx - nw / 2.0,
                oy + 115.0 * s,
                tracking,
                &fill_paint(col(t.text_dim, 1.0)),
            );
        }

        // Net: download and upload text, then the download sparkline in the rest of the width.
        let net_x = ox + 236.0 * s;
        let net_w = 144.0 * s;
        let (down, up) = match reading {
            Some(r) => (format_rate(r.net_down_bps), format_rate(r.net_up_bps)),
            None => ("…".to_string(), "…".to_string()),
        };
        draw_text(
            canvas,
            &label,
            &format!("↓ {down}"),
            net_x,
            oy + 56.0 * s,
            0.0,
            &ink,
        );
        draw_text(
            canvas,
            &label,
            &format!("↑ {up}"),
            net_x,
            oy + 69.0 * s,
            0.0,
            &fill_paint(col(t.text_dim, 1.0)),
        );
        let history: Vec<f64> = f.vitals.net_history().collect();
        let points = spark_points(&history, net_w, 24.0 * s);
        if points.len() > 1 {
            let mut b = PathBuilder::new();
            for (i, (x, y)) in points.iter().enumerate() {
                let p = (net_x + x, oy + 76.0 * s + y);
                if i == 0 {
                    b.move_to(p);
                } else {
                    b.line_to(p);
                }
            }
            canvas.draw_path(&b.detach(), &stroke_paint(col(t.accent, 1.0), 1.5 * s));
        }

        // Bottom row: the top reason tag on the left, battery on the right.
        let row_cy = oy + 132.0 * s;
        if let Some(reason) = health.and_then(|h| h.reasons.first()) {
            let text = reason.tag();
            let tw = text_width(&name_font, text, 0.0);
            let tag = Rect::from_xywh(ox + 20.0 * s, row_cy - 8.0 * s, tw + 14.0 * s, 16.0 * s);
            let tag_r = 8.0 * s;
            canvas.draw_rrect(
                RRect::new_rect_xy(tag, tag_r, tag_r),
                &fill_paint(col(t.warn, 0.16)),
            );
            draw_text(
                canvas,
                &name_font,
                text,
                tag.left + 7.0 * s,
                row_cy + 0.35 * 9.0 * s,
                0.0,
                &fill_paint(col(t.warn, 1.0)),
            );
        }
        if let Some(b) = reading.and_then(|r| r.battery) {
            let txt = format!("BAT {}%{}", b.percent, if b.charging { " +" } else { "" });
            let bw = text_width(&label, &txt, 0.0);
            draw_text(
                canvas,
                &label,
                &txt,
                right - bw,
                row_cy + 0.35 * 10.0 * s,
                0.0,
                &fill_paint(col(t.text_dim, 1.0)),
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
pub(crate) fn begin_layer(
    canvas: &Canvas,
    bounds: &Rect,
    opacity: f32,
    pivot: (f32, f32),
    scale: f32,
) {
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
