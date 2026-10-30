//! The Music views: the compact pill (224 x 36, plus the timer overlay while a timer
//! runs), the expanded player (400 x 184) and its native session status. Same layout rules as
//! the Home grid: content is anchored to the pill's top-centre, so it holds still while
//! the pill springs around it.

use crate::fonts::Weight;
use crate::icons::draw_icon;
use crate::paint::{col, draw_text, fill_paint, fit, stroke_paint, text_width, Frame, Painter};
use dj_core::color::Rgb;
use dj_core::features::Icon;
use dj_core::layout::{self as lay, Rect as LRect};
use dj_core::music::{artwork_allowed, fmt_time};
use skia_safe::canvas::SrcRectConstraint;
use skia_safe::image::CachingHint;
use skia_safe::{
    surfaces, Canvas, ClipOp, Color, Data, Image, ImageInfo, Paint, PathBuilder, RRect, Rect,
};

/// Cover art is kept at most this big. The pill shows it at 88 px.
const ART_PX: i32 = 256;

#[derive(Clone, Copy)]
enum Glyph {
    Prev,
    Play,
    Pause,
    Next,
}

impl Painter {
    /// Decodes cover art, keeps a square 256 px copy and returns its average colour.
    /// Undecodable bytes give `None` and leave the previous art in place.
    pub(crate) fn set_art(&mut self, encoded: &[u8]) -> Option<Rgb> {
        let src = Image::from_encoded(Data::new_copy(encoded))?;
        let (w, h) = (src.width(), src.height());
        let side = w.min(h);
        if !artwork_allowed(encoded.len(), w, h) {
            return None;
        }
        let n = side.min(ART_PX);
        let crop = Rect::from_xywh(
            ((w - side) / 2) as f32,
            ((h - side) / 2) as f32,
            side as f32,
            side as f32,
        );
        let mut surface = surfaces::raster_n32_premul((n, n))?;
        surface.canvas().draw_image_rect(
            &src,
            Some((&crop, SrcRectConstraint::Strict)),
            Rect::from_wh(n as f32, n as f32),
            &Paint::default(),
        );
        let img = surface.image_snapshot();
        let info = ImageInfo::new_n32_premul((n, n), None);
        let row = (n * 4) as usize;
        let mut buf = vec![0u8; row * n as usize];
        if !img.read_pixels(&info, &mut buf, row, (0, 0), CachingHint::Allow) {
            return None;
        }
        // N32 is BGRA on little-endian. Sampling every 7th pixel is plenty for an average.
        let (mut r, mut g, mut b, mut count) = (0u64, 0u64, 0u64, 0u64);
        for px in buf.as_chunks::<4>().0.iter().step_by(7) {
            b += u64::from(px[0]);
            g += u64::from(px[1]);
            r += u64::from(px[2]);
            count += 1;
        }
        if count == 0 {
            return None;
        }
        let k = 255.0 * count as f32;
        self.art = Some(img);
        Some(Rgb::new(r as f32 / k, g as f32 / k, b as f32 / k))
    }

    pub(crate) fn clear_art(&mut self) {
        self.art = None;
    }

    /// The compact pill: art, title and a static playback indicator.
    pub(crate) fn music_compact(&mut self, canvas: &Canvas, f: &Frame, rect: &Rect) {
        let (s, t, m) = (f.scale, f.tokens, &f.music);
        let ox = rect.center_x() - lay::MUSIC_COMPACT_W * s / 2.0;
        let oy = rect.top;
        let at = |r: LRect| Rect::from_xywh(ox + r.x * s, oy + r.y * s, r.w * s, r.h * s);
        let accent = col(m.accent, 1.0);

        self.draw_art(
            canvas,
            at(lay::MC_ART),
            6.0 * s,
            s,
            col(t.surface2, 1.0),
            accent,
        );

        // A running (or paused) timer takes the end of the title's room for its digits.
        let timer_on = f.timer.is_active();
        if let Some(now) = m.now {
            let font = self.fonts.font(&t.font_label, Weight::Medium, 11.0 * s);
            let tr = at(lay::MC_TITLE);
            let room = if timer_on {
                (lay::MC_TIMER.x - lay::MC_TITLE.x - 6.0) * s
            } else {
                tr.width()
            };
            let line = fit(&font, &now.title, 0.0, room);
            draw_text(
                canvas,
                &font,
                &line,
                tr.left,
                tr.center_y() + 0.35 * 11.0 * s,
                0.0,
                &fill_paint(col(t.text, 1.0)),
            );
        }
        if let Some(now) = m.now {
            draw_glyph(
                canvas,
                at(lay::MC_PLAYBACK),
                if now.playing {
                    Glyph::Play
                } else {
                    Glyph::Pause
                },
                fill_paint(accent),
            );
        }
        if timer_on {
            self.music_timer_overlay(canvas, f, rect);
        }
    }

    /// The native expanded player and no-session/error states.
    pub(crate) fn music_expanded(&mut self, canvas: &Canvas, f: &Frame, rect: &Rect) {
        let (s, t, m) = (f.scale, f.tokens, &f.music);
        let ox = rect.center_x() - lay::MUSIC_EXP_W * s / 2.0;
        let oy = rect.top;
        let at = |r: LRect| Rect::from_xywh(ox + r.x * s, oy + r.y * s, r.w * s, r.h * s);
        let accent = col(m.accent, 1.0);
        let text = col(t.text, 1.0);
        let dim = col(t.text_dim, 1.0);
        let label = self.fonts.font(&t.font_label, Weight::Medium, 8.5 * s);

        // Header: lamp + name, and the play state on the right.
        let h = at(lay::ME_HEADER);
        let hy = h.center_y();
        canvas.draw_circle((h.left + 3.0 * s, hy), 2.6 * s, &fill_paint(accent));
        let tracking = 1.6 * s;
        draw_text(
            canvas,
            &label,
            &fit(
                &label,
                m.now.map_or("SYSTEM MEDIA", |n| n.source.as_str()),
                tracking,
                h.width() - 80.0 * s,
            ),
            h.left + 12.0 * s,
            hy + 0.35 * 8.5 * s,
            tracking,
            &fill_paint(dim),
        );
        if let Some(now) = m.now {
            let state = if now.playing { "PLAYING" } else { "PAUSED" };
            let tw = text_width(&label, state, tracking);
            draw_text(
                canvas,
                &label,
                state,
                h.right() - tw,
                hy + 0.35 * 8.5 * s,
                tracking,
                &fill_paint(dim),
            );
        }

        let font_title = self.fonts.font(&t.font_title, Weight::Medium, 15.0 * s);
        let font_artist = self.fonts.font(&t.font_title, Weight::Regular, 12.0 * s);
        let font_time = self.fonts.font(&t.font_label, Weight::Regular, 9.0 * s);
        self.draw_art(
            canvas,
            at(lay::ME_ART),
            14.0 * s,
            s,
            col(t.surface2, 1.0),
            accent,
        );

        match m.now {
            Some(now) => {
                let tr = at(lay::ME_TITLE);
                let line = fit(&font_title, &now.title, 0.0, tr.width());
                draw_text(
                    canvas,
                    &font_title,
                    &line,
                    tr.left,
                    tr.center_y() + 0.35 * 15.0 * s,
                    0.0,
                    &fill_paint(text),
                );
                let ar = at(lay::ME_ARTIST);
                let sub = if now.artist.is_empty() {
                    now.album.as_str()
                } else {
                    now.artist.as_str()
                };
                let line = fit(&font_artist, sub, 0.0, ar.width());
                draw_text(
                    canvas,
                    &font_artist,
                    &line,
                    ar.left,
                    ar.center_y() + 0.35 * 12.0 * s,
                    0.0,
                    &fill_paint(dim),
                );

                draw_glyph(
                    canvas,
                    at(lay::ME_PREV),
                    Glyph::Prev,
                    fill_paint(if now.controls.previous { text } else { dim }),
                );
                let play = at(lay::ME_PLAY);
                canvas.draw_circle(
                    (play.center_x(), play.center_y()),
                    play.width() / 2.0,
                    &fill_paint(
                        if if now.playing {
                            now.controls.pause
                        } else {
                            now.controls.play
                        } {
                            accent
                        } else {
                            col(t.surface2, 1.0)
                        },
                    ),
                );
                let glyph = if now.playing {
                    Glyph::Pause
                } else {
                    Glyph::Play
                };
                draw_glyph(canvas, play, glyph, fill_paint(col(t.pill, 1.0)));
                draw_glyph(
                    canvas,
                    at(lay::ME_NEXT),
                    Glyph::Next,
                    fill_paint(if now.controls.next { text } else { dim }),
                );

                // Scrubber: track, progress, and the times under it.
                let sc = at(lay::ME_SCRUB);
                let bar = Rect::from_xywh(sc.left, sc.center_y() - 2.0 * s, sc.width(), 4.0 * s);
                let rr = |r: Rect| RRect::new_rect_xy(r, 2.0 * s, 2.0 * s);
                canvas.draw_rrect(rr(bar), &fill_paint(col(t.surface2, 1.0)));
                let frac = if now.dur > 0.0 {
                    (m.pos / now.dur).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                if frac > 0.0 {
                    let done = Rect::from_xywh(bar.left, bar.top, bar.width() * frac, bar.height());
                    canvas.draw_rrect(rr(done), &fill_paint(accent));
                }
                let base = oy + 131.0 * s;
                draw_text(
                    canvas,
                    &font_time,
                    &fmt_time(m.pos),
                    sc.left,
                    base,
                    0.0,
                    &fill_paint(dim),
                );
                let total = if now.dur > 0.0 {
                    fmt_time(now.dur)
                } else {
                    "--:--".into()
                };
                let tw = text_width(&font_time, &total, 0.0);
                draw_text(
                    canvas,
                    &font_time,
                    &total,
                    sc.right() - tw,
                    base,
                    0.0,
                    &fill_paint(dim),
                );
            }
            None => {
                let tr = at(lay::ME_TITLE);
                draw_text(
                    canvas,
                    &font_title,
                    if m.ui.available {
                        "NO MEDIA SESSION"
                    } else {
                        "MEDIA UNAVAILABLE"
                    },
                    tr.left,
                    tr.center_y() + 0.35 * 15.0 * s,
                    0.0,
                    &fill_paint(dim),
                );
                if m.ui.integration_required {
                    let ar = at(lay::ME_ARTIST);
                    draw_text(
                        canvas,
                        &font_artist,
                        "Windows media access denied",
                        ar.left,
                        ar.center_y() + 0.35 * 12.0 * s,
                        0.0,
                        &fill_paint(dim),
                    );
                }
            }
        }

        let btn = self.fonts.font(&t.font_label, Weight::Medium, 9.0 * s);
        let status = if m.now.is_none() && m.ui.available {
            "Start playback in an external player"
        } else {
            m.status
        };
        let status = fit(&font_time, status, 0.0, 368.0 * s);
        draw_text(
            canvas,
            &font_time,
            &status,
            ox + 16.0 * s,
            oy + 149.0 * s,
            0.0,
            &fill_paint(dim),
        );
        if !m.ui.available || !m.ui.has_track {
            button(
                canvas,
                &btn,
                at(lay::ME_RETRY),
                "RETRY",
                text,
                col(t.surface1, 1.0),
                col(t.surface2, 1.0),
                0.0,
                s,
            );
        }
        if let Some(label) = m.setup_label {
            button(
                canvas,
                &btn,
                at(lay::ME_SETUP),
                label,
                text,
                col(t.surface1, 1.0),
                col(t.surface2, 1.0),
                0.0,
                s,
            );
        }
        button(
            canvas,
            &btn,
            at(lay::ME_BTN_OPEN),
            "YOUTUBE MUSIC",
            text,
            col(t.surface1, 1.0),
            col(t.surface2, 1.0),
            0.0,
            s,
        );
        button(
            canvas,
            &btn,
            at(lay::ME_BTN_SPOTIFY),
            "SPOTIFY",
            text,
            col(t.surface1, 1.0),
            col(t.surface2, 1.0),
            0.0,
            s,
        );
        self.home_button(canvas, f, &at);
    }
    /// Shared bottom-right "HOME" button of the expanded view.
    fn home_button(&mut self, canvas: &Canvas, f: &Frame, at: &dyn Fn(LRect) -> Rect) {
        let (s, t) = (f.scale, f.tokens);
        let btn = self.fonts.font(&t.font_label, Weight::Medium, 9.0 * s);
        button(
            canvas,
            &btn,
            at(lay::ME_BTN_HOME),
            "HOME",
            col(t.text, 1.0),
            col(t.surface1, 1.0),
            col(t.surface2, 1.0),
            1.2 * s,
            s,
        );
    }

    /// Cover art clipped to a rounded square, or a placeholder until art arrives.
    fn draw_art(
        &self,
        canvas: &Canvas,
        rect: Rect,
        radius: f32,
        s: f32,
        fallback: Color,
        accent: Color,
    ) {
        canvas.save();
        canvas.clip_rrect(
            RRect::new_rect_xy(rect, radius, radius),
            ClipOp::Intersect,
            true,
        );
        match &self.art {
            Some(img) => {
                canvas.draw_image_rect(img, None, rect, &Paint::default());
            }
            None => {
                canvas.draw_rect(rect, &fill_paint(fallback));
                draw_icon(
                    canvas,
                    Icon::Media,
                    rect.center_x(),
                    rect.center_y(),
                    rect.width() * 0.42,
                    &stroke_paint(accent, 1.6 * s),
                );
            }
        }
        canvas.restore();
    }
}

/// A labelled button: filled body, hairline edge, centred label.
#[allow(clippy::too_many_arguments)]
pub(crate) fn button(
    canvas: &Canvas,
    font: &skia_safe::Font,
    r: Rect,
    label: &str,
    ink: Color,
    body: Color,
    edge: Color,
    tracking: f32,
    s: f32,
) {
    let rr = RRect::new_rect_xy(r, 6.0 * s, 6.0 * s);
    canvas.draw_rrect(rr, &fill_paint(body));
    canvas.draw_rrect(
        RRect::new_rect_xy(r.with_inset((0.5, 0.5)), 5.5 * s, 5.5 * s),
        &stroke_paint(edge, 1.0),
    );
    let tw = text_width(font, label, tracking);
    draw_text(
        canvas,
        font,
        label,
        r.center_x() - tw / 2.0,
        r.center_y() + 0.35 * font.size(),
        tracking,
        &fill_paint(ink),
    );
}

/// A transport glyph centred in `r`, drawn as filled paths.
fn draw_glyph(canvas: &Canvas, r: Rect, glyph: Glyph, paint: Paint) {
    let s = r.width() / 34.0;
    let (cx, cy) = (r.center_x(), r.center_y());
    let tri = |pts: [(f32, f32); 3]| {
        let mut pb = PathBuilder::new();
        pb.move_to(pts[0]).line_to(pts[1]).line_to(pts[2]).close();
        pb.detach()
    };
    match glyph {
        Glyph::Play => {
            canvas.draw_path(
                &tri([
                    (cx - 4.0 * s, cy - 6.5 * s),
                    (cx - 4.0 * s, cy + 6.5 * s),
                    (cx + 6.5 * s, cy),
                ]),
                &paint,
            );
        }
        Glyph::Pause => {
            canvas.draw_rect(
                Rect::from_xywh(cx - 5.0 * s, cy - 6.5 * s, 3.5 * s, 13.0 * s),
                &paint,
            );
            canvas.draw_rect(
                Rect::from_xywh(cx + 1.5 * s, cy - 6.5 * s, 3.5 * s, 13.0 * s),
                &paint,
            );
        }
        Glyph::Prev => {
            canvas.draw_path(
                &tri([
                    (cx + 5.0 * s, cy - 6.0 * s),
                    (cx + 5.0 * s, cy + 6.0 * s),
                    (cx - 3.0 * s, cy),
                ]),
                &paint,
            );
            canvas.draw_rect(
                Rect::from_xywh(cx - 6.0 * s, cy - 6.0 * s, 2.0 * s, 12.0 * s),
                &paint,
            );
        }
        Glyph::Next => {
            canvas.draw_path(
                &tri([
                    (cx - 5.0 * s, cy - 6.0 * s),
                    (cx - 5.0 * s, cy + 6.0 * s),
                    (cx + 3.0 * s, cy),
                ]),
                &paint,
            );
            canvas.draw_rect(
                Rect::from_xywh(cx + 4.0 * s, cy - 6.0 * s, 2.0 * s, 12.0 * s),
                &paint,
            );
        }
    }
}
