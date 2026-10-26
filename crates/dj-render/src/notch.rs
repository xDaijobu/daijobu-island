//! The island body's shape in device px: the pill's rounded rect, or the notch.
//!
//! The notch hangs from the screen's top edge like a hardware notch: a flat top at
//! y = 0, a concave shoulder of radius `NOTCH_SHOULDER` on each side where it meets
//! the edge, straight sides and the pill's rounded bottom corners. Only the body
//! grows: the pill rect, the content layers and hit testing stay where they are.

use dj_core::config::IslandStyle;
use dj_core::layout::NOTCH_SHOULDER;
use skia_safe::{Path, PathBuilder, RRect, Rect};
use std::f32::consts::FRAC_1_SQRT_2;

/// The hairline runs this far inside the body, so its 1 px stroke stays on the fill.
const HAIRLINE_INSET: f32 = 0.5;

/// The conic weight that makes a conic an exact quarter circle.
const QUARTER: f32 = FRAC_1_SQRT_2;

/// The body to fill and clip to, for the pill rect `rect` with corner `radius` (both
/// device px). Pill: the rounded rect, as before the notch existed. Notch: covers
/// `layout::notch_bounds` × `scale` (less when a morph frame is too short for the
/// shoulders) and contains the pill's rounded rect.
pub(crate) fn body_path(rect: Rect, radius: f32, style: IslandStyle, scale: f32) -> Path {
    match style {
        IslandStyle::Pill => Path::rrect(RRect::new_rect_xy(rect, radius, radius), None),
        IslandStyle::Notch => Notch::new(rect, radius, scale).map_or_else(Path::new, Notch::fill),
    }
}

/// The hairline, 0.5 px inside the body. Pill: the closed inset rounded rect. Notch:
/// open at the top (shoulders, sides and bottom), so no line runs along the screen edge.
pub(crate) fn body_outline(rect: Rect, radius: f32, style: IslandStyle, scale: f32) -> Path {
    match style {
        IslandStyle::Pill => {
            let inset = rect.with_inset((HAIRLINE_INSET, HAIRLINE_INSET));
            let r = (radius - HAIRLINE_INSET).max(0.0);
            Path::rrect(RRect::new_rect_xy(inset, r, r), None)
        }
        IslandStyle::Notch => {
            Notch::new(rect, radius, scale).map_or_else(Path::new, Notch::outline)
        }
    }
}

/// Notch geometry in device px, clamped so every frame of the spring morph is valid.
#[derive(Clone, Copy, Debug)]
struct Notch {
    left: f32,
    right: f32,
    bottom: f32,
    /// Shoulder radius.
    sh: f32,
    /// Bottom corner radius.
    rad: f32,
}

impl Notch {
    /// `None` for an empty or inverted pill rect: nothing to draw.
    fn new(rect: Rect, radius: f32, scale: f32) -> Option<Self> {
        let (w, h, bottom) = (rect.width(), rect.height(), rect.bottom);
        if !(w > 0.0 && h > 0.0 && bottom > 0.0 && bottom.is_finite()) {
            return None;
        }
        // A pill shorter than the shoulders (mid-morph only) gets smaller shoulders,
        // so they never reach below its bottom.
        let sh = (NOTCH_SHOULDER * scale).min(bottom).max(0.0);
        // The pill's own corners (Skia shrinks a radius past half the size the same
        // way, and treats a non-finite one as square), kept under the shoulders.
        let radius = if radius.is_finite() { radius } else { 0.0 };
        let rad = radius.min(w / 2.0).min(h / 2.0).min(bottom - sh).max(0.0);
        Some(Self {
            left: rect.left,
            right: rect.right,
            bottom,
            sh,
            rad,
        })
    }

    /// Clockwise from the left shoulder's tip along the top edge (y = 0 exactly).
    fn fill(self) -> Path {
        let Self {
            left: l,
            right: r,
            bottom: b,
            sh,
            rad,
        } = self;
        let mut p = PathBuilder::new();
        p.move_to((l - sh, 0.0))
            .line_to((r + sh, 0.0))
            // Right shoulder: the quarter circle about (r + sh, sh), bowing up-left.
            .conic_to((r, 0.0), (r, sh), QUARTER)
            .line_to((r, b - rad))
            .conic_to((r, b), (r - rad, b), QUARTER)
            .line_to((l + rad, b))
            .conic_to((l, b), (l, b - rad), QUARTER)
            .line_to((l, sh))
            // Left shoulder: the quarter circle about (l - sh, sh), bowing up-right.
            .conic_to((l, 0.0), (l - sh, 0.0), QUARTER)
            .close();
        p.detach()
    }

    /// The fill's edge minus the top, moved `HAIRLINE_INSET` into the material: the
    /// shoulder arcs grow by it (the material is outside their circles), the sides and
    /// bottom move in, and the bottom corners stay concentric with the fill's. Starts
    /// and ends just above the window, where the stroke is cut off.
    fn outline(self) -> Path {
        let Self {
            left: l,
            right: r,
            bottom: b,
            sh,
            rad,
        } = self;
        let i = HAIRLINE_INSET;
        let ri = (rad - i).max(0.0);
        let (l, r, b) = (l + i, r - i, b - i);
        let mut p = PathBuilder::new();
        p.move_to((l - i - sh, -i))
            .conic_to((l, -i), (l, sh), QUARTER)
            .line_to((l, b - ri))
            .conic_to((l, b), (l + ri, b), QUARTER)
            .line_to((r - ri, b))
            .conic_to((r, b), (r, b - ri), QUARTER)
            .line_to((r, sh))
            .conic_to((r, -i), (r + i + sh, -i), QUARTER);
        p.detach()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::{fill_paint, stroke_paint};
    use dj_core::layout::{notch_bounds, Rect as LRect, TOP_MARGIN};
    use skia_safe::image::CachingHint;
    use skia_safe::{surfaces, AlphaType, Canvas, ClipOp, Color, ColorType, ImageInfo, Paint};
    use skia_safe::{Surface, Vector};

    const SCALES: [f32; 2] = [1.0, 2.0];
    /// An idle-sized pill in window-logical px, as the view lays it out.
    const PILL: LRect = LRect::new(60.0, TOP_MARGIN, 180.0, 36.0);
    const RADIUS: f32 = 18.0;

    /// `r` in device px, rounded the way `Painter::paint` rounds the view's rect.
    fn device(r: LRect, s: f32) -> Rect {
        Rect::new(
            (r.x * s).round(),
            (r.y * s).round(),
            ((r.x + r.w) * s).round(),
            ((r.y + r.h) * s).round(),
        )
    }

    fn scaled(r: LRect, s: f32) -> Rect {
        Rect::from_xywh(r.x * s, r.y * s, r.w * s, r.h * s)
    }

    fn surface(s: f32) -> Surface {
        surfaces::raster_n32_premul(((300.0 * s) as i32, (60.0 * s) as i32)).unwrap()
    }

    /// Premultiplied RGBA, row-major.
    fn pixels(surface: &mut Surface) -> Vec<u8> {
        let (w, h) = (surface.width(), surface.height());
        let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Premul, None);
        let mut px = vec![0u8; (w * h * 4) as usize];
        let img = surface.image_snapshot();
        assert!(img.read_pixels(
            &info,
            &mut px,
            (w * 4) as usize,
            (0, 0),
            CachingHint::Disallow
        ));
        px
    }

    fn alpha(px: &[u8], width: i32, x: f32, y: f32) -> u8 {
        px[((y as i32 * width + x as i32) * 4 + 3) as usize]
    }

    /// The paints `Painter::paint` uses for the body and the hairline.
    fn fill() -> Paint {
        fill_paint(Color::from_argb(255, 20, 20, 24))
    }

    fn hairline() -> Paint {
        stroke_paint(Color::from_argb(60, 240, 240, 240), 1.0)
    }

    #[test]
    fn the_notch_has_a_flat_top_on_the_edge_and_concave_shoulders() {
        for s in SCALES {
            let r = device(PILL, s);
            let p = body_path(r, RADIUS * s, IslandStyle::Notch, s);
            let sh = NOTCH_SHOULDER * s;
            assert_eq!(*p.bounds(), scaled(notch_bounds(PILL), s), "s {s}");
            assert_eq!(p.bounds().top, 0.0);
            // The top row: across the middle, just inside each shoulder and half way out.
            for x in [
                r.center_x(),
                r.left - 1.0,
                r.right + 1.0,
                r.left - sh / 2.0,
                r.right + sh / 2.0,
            ] {
                assert!(p.contains((x, 0.5)), "s {s}: ({x}, 0.5) should be filled");
            }
            // Inside the quarter discs, outside the bounds, and above the edge.
            for (x, y) in [
                (r.left - sh + 1.0, sh - 1.0),
                (r.right + sh - 1.0, sh - 1.0),
                (r.left - sh / 2.0, sh - 1.0),
                (r.left - sh - 1.0, 0.5),
                (r.right + sh + 1.0, 0.5),
                (r.center_x(), -0.5),
                (r.center_x(), r.bottom + 1.0),
            ] {
                assert!(!p.contains((x, y)), "s {s}: ({x}, {y}) should be empty");
            }
            // Straight sides below the shoulders, and the pill's top inside the body.
            assert!(p.contains((r.left + 0.5, sh + 1.0)));
            assert!(!p.contains((r.left - 0.5, sh + 1.0)));
            assert!(p.contains((r.right - 0.5, sh + 1.0)));
            assert!(!p.contains((r.right + 0.5, sh + 1.0)));
            assert!(p.contains((r.left + 0.5, r.top + 0.5)));
        }
    }

    #[test]
    fn the_notch_keeps_the_pills_rounded_bottom_corners() {
        for s in SCALES {
            let r = device(PILL, s);
            let rad = RADIUS * s;
            let p = body_path(r, rad, IslandStyle::Notch, s);
            assert!(!p.contains((r.left + 1.0, r.bottom - 1.0)));
            assert!(!p.contains((r.right - 1.0, r.bottom - 1.0)));
            assert!(p.contains((r.center_x(), r.bottom - 0.5)));
            assert!(p.contains((r.left + 0.5, r.bottom - rad)));
            // Either side of the arc, on the corner's diagonal.
            let d = std::f32::consts::FRAC_1_SQRT_2;
            let (cx, cy) = (r.left + rad, r.bottom - rad);
            assert!(p.contains((cx - (rad - 1.0) * d, cy + (rad - 1.0) * d)));
            assert!(!p.contains((cx - (rad + 1.0) * d, cy + (rad + 1.0) * d)));
        }
    }

    #[test]
    fn the_notch_outline_is_open_at_the_top_and_inset_by_half_a_pixel() {
        for s in SCALES {
            let r = device(PILL, s);
            let o = body_outline(r, RADIUS * s, IslandStyle::Notch, s);
            assert!(!o.is_last_contour_closed());
            let b = *o.bounds();
            let sh = NOTCH_SHOULDER * s;
            assert_eq!(
                (b.left, b.top, b.right, b.bottom),
                (r.left - sh, -0.5, r.right + sh, r.bottom - 0.5)
            );
            // No point on the top edge between the shoulders.
            assert!(o
                .points()
                .iter()
                .all(|p| p.y > 0.0 || p.x <= r.left + 0.5 || p.x >= r.right - 0.5));
        }
    }

    #[test]
    fn the_pill_style_is_todays_rounded_rect() {
        for s in SCALES {
            let r = device(PILL, s);
            let rad = RADIUS * s;
            let p = body_path(r, rad, IslandStyle::Pill, s);
            assert_eq!(p.is_rrect(), Some(RRect::new_rect_xy(r, rad, rad)));
            assert_eq!(*p.bounds(), r);
            assert!(!p.contains((r.center_x(), 0.5)));
            let o = body_outline(r, rad, IslandStyle::Pill, s);
            let inset = r.with_inset((0.5, 0.5));
            assert_eq!(
                o.is_rrect(),
                Some(RRect::new_rect_xy(inset, rad - 0.5, rad - 0.5))
            );
        }
    }

    /// The body as `Painter::paint` draws it now: fill, a tint over the bounds clipped
    /// to the body (glow and grain), then the hairline.
    fn draw_body(canvas: &Canvas, rect: Rect, rad: f32, style: IslandStyle, s: f32) {
        let body = body_path(rect, rad, style, s);
        canvas.draw_path(&body, &fill());
        canvas.save();
        canvas.clip_path(&body, ClipOp::Intersect, true);
        canvas.draw_rect(body.bounds(), &tint());
        canvas.restore();
        let outline = body_outline(rect, rad, style, s);
        canvas.draw_path(&outline, &hairline());
    }

    fn tint() -> Paint {
        let mut p = Paint::default();
        p.set_color(Color::from_argb(90, 200, 120, 60));
        p
    }

    #[test]
    fn the_pill_style_draws_the_same_pixels_as_before() {
        for s in SCALES {
            let r = device(PILL, s);
            for rad in [RADIUS * s, 4.0 * s, 0.0, 100.0 * s] {
                let mut new = surface(s);
                draw_body(new.canvas(), r, rad, IslandStyle::Pill, s);
                // The body code before the notch style.
                let mut old = surface(s);
                let c = old.canvas();
                let rrect = RRect::new_rect_xy(r, rad, rad);
                c.draw_rrect(rrect, &fill());
                c.save();
                c.clip_rrect(rrect, ClipOp::Intersect, true);
                c.draw_rect(r, &tint());
                c.restore();
                let inset = r.with_inset((0.5, 0.5));
                let hr = (rad - 0.5).max(0.0);
                c.draw_rrect(RRect::new_rect_xy(inset, hr, hr), &hairline());
                assert!(pixels(&mut new) == pixels(&mut old), "s {s}, radius {rad}");
            }
        }
    }

    /// The content is clipped to the body, so the notch must hold the whole pill. Checked
    /// on the geometry: the convex rrect and the concave notch go through different AA
    /// walkers, so their edge pixels can differ by a few levels.
    #[test]
    fn the_notch_contains_the_pills_rounded_rect() {
        for s in SCALES {
            for radius in [RADIUS, 6.0, 0.0, 100.0] {
                let r = device(PILL, s);
                let notch = body_path(r, radius * s, IslandStyle::Notch, s);
                let pill = body_path(r, radius * s, IslandStyle::Pill, s);
                let (cols, rows) = ((r.width() * 2.0) as i32, (r.height() * 2.0) as i32);
                for (i, j) in (0..rows).flat_map(|j| (0..cols).map(move |i| (i, j))) {
                    let pt = (
                        r.left + (i as f32 + 0.5) / 2.0,
                        r.top + (j as f32 + 0.5) / 2.0,
                    );
                    assert!(
                        !pill.contains(pt) || notch.contains(pt),
                        "s {s}, radius {radius}: {pt:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_notch_fill_reaches_the_top_row() {
        for s in SCALES {
            let r = device(PILL, s);
            let rad = RADIUS * s;
            let sh = NOTCH_SHOULDER * s;
            let mut notch = surface(s);
            notch
                .canvas()
                .draw_path(&body_path(r, rad, IslandStyle::Notch, s), &fill());
            let w = notch.width();
            let px = pixels(&mut notch);
            let a = |x: f32, y: f32| alpha(&px, w, x, y);
            // Whole pixels, by their top-left corner.
            for (x, y) in [
                (r.center_x(), 0.0),
                (r.left - 1.0, 0.0),
                (r.right, 0.0),
                (r.center_x(), r.bottom - 1.0),
            ] {
                assert_eq!(a(x, y), 255, "s {s}: pixel ({x}, {y}) should be solid");
            }
            for (x, y) in [
                (r.left - sh, sh - 1.0),
                (r.right + sh - 1.0, sh - 1.0),
                (r.left - sh - 1.0, 0.0),
                (r.right + sh, 0.0),
                (r.left, r.bottom - 1.0),
                (r.right - 1.0, r.bottom - 1.0),
                (r.center_x(), r.bottom),
            ] {
                assert_eq!(a(x, y), 0, "s {s}: pixel ({x}, {y}) should be clear");
            }

            // The pill style leaves the top row clear.
            let mut pill = surface(s);
            pill.canvas()
                .draw_path(&body_path(r, rad, IslandStyle::Pill, s), &fill());
            let pill_px = pixels(&mut pill);
            assert_eq!(alpha(&pill_px, w, r.center_x(), 0.0), 0);
            assert_eq!(alpha(&pill_px, w, r.center_x(), r.top), 255);
        }
    }

    #[test]
    fn degenerate_morph_frames_stay_inside_the_notch_bounds() {
        // Pill rects (logical px) and radii the spring can pass through, and worse.
        let frames = [
            (LRect::new(100.0, TOP_MARGIN, 0.5, 0.5), 40.0),
            (LRect::new(100.0, TOP_MARGIN, 3.0, 1.0), 40.0),
            (LRect::new(100.0, TOP_MARGIN, 1.0, 30.0), 30.0),
            (LRect::new(100.0, TOP_MARGIN, 200.0, 0.25), 12.0),
            (LRect::new(100.0, 1.0, 10.0, 2.0), 9.0),
            (LRect::new(100.0, 0.0, 10.0, 0.5), 1.0),
            (LRect::new(100.0, TOP_MARGIN, 50.0, 30.0), f32::NAN),
            (LRect::new(100.0, TOP_MARGIN, 50.0, 30.0), f32::INFINITY),
            (LRect::new(100.0, TOP_MARGIN, 50.0, 30.0), -5.0),
            (LRect::new(100.0, TOP_MARGIN, 0.0, 0.0), 10.0),
            (LRect::new(100.0, TOP_MARGIN, -4.0, 10.0), 5.0),
        ];
        for s in SCALES {
            for (pill, radius) in frames {
                let rect = scaled(pill, s);
                let rad = radius * s;
                let p = body_path(rect, rad, IslandStyle::Notch, s);
                let o = body_outline(rect, rad, IslandStyle::Notch, s);
                assert!(p.is_finite() && o.is_finite(), "{pill:?} r {radius}");
                if pill.w > 0.0 && pill.h > 0.0 {
                    assert!(!p.is_empty(), "{pill:?} r {radius}");
                }
                if !p.is_empty() {
                    let b = *p.bounds();
                    let max = scaled(notch_bounds(pill), s).with_outset(Vector::new(1e-3, 1e-3));
                    let inside = b.left >= max.left
                        && b.top >= max.top
                        && b.right <= max.right
                        && b.bottom <= max.bottom;
                    assert!(inside, "{pill:?} r {radius}: {b:?} outside {max:?}");
                    assert_eq!(b.top, 0.0);
                }
                let mut surface = surface(s);
                draw_body(surface.canvas(), rect, rad, IslandStyle::Notch, s);
            }
        }
    }
}
