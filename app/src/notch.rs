//! Where the island's body sits for the `[island] style`. The pill rect, its content and
//! dj-core's hit testing are the same in both styles; in notch style the body also covers
//! the neck above the pill and a shoulder on each side of its top (`layout::notch_bounds`).
//!
//! The window region follows the body: in notch style it is the pill's column from the
//! top edge down plus a strip as high as a shoulder, so the 8 px beside the pill below
//! the shoulders stay click-through.

use dj_core::config::IslandStyle;
use dj_core::layout::{self, Rect, NOTCH_SHOULDER};

/// Device-px edges `(left, top, right, bottom)` in the window.
pub type Edges = (i32, i32, i32, i32);

/// How far inside the pill's right and bottom edges a clamped point lands:
/// `Rect::contains` leaves those edges out.
const INSET: f32 = 0.01;

/// The island's column in window-logical px: the pill itself, or in notch style the pill
/// stretched up to the top edge (the shoulders are added by [`shoulder_strip`]).
pub fn body_rect(style: IslandStyle, pill: Rect) -> Rect {
    match style {
        IslandStyle::Pill => pill,
        IslandStyle::Notch => Rect::new(pill.x, 0.0, pill.w, pill.y + pill.h),
    }
}

/// `r` (window-logical px) in device px, each edge rounded.
pub fn device_edges(r: Rect, scale: f32) -> Edges {
    (
        (r.x * scale).round() as i32,
        (r.y * scale).round() as i32,
        ((r.x + r.w) * scale).round() as i32,
        ((r.y + r.h) * scale).round() as i32,
    )
}

/// Notch style: the shoulders' strip for a device-px column (as [`body_rect`] gives it):
/// as wide as the column plus a shoulder each side, from the top edge down one shoulder.
pub fn shoulder_strip((l, _, r, _): Edges, scale: f32) -> Edges {
    let sh = (NOTCH_SHOULDER * scale).round() as i32;
    (l - sh, 0, r + sh, sh)
}

/// Window-logical `(x, y)` as the island should see it. In notch style the neck and the
/// shoulders are part of the island, so a point there acts as the nearest point of the
/// pill. Anywhere else, and in pill style, the point passes through untouched.
pub fn to_pill(style: IslandStyle, pill: Rect, x: f32, y: f32) -> (f32, f32) {
    let on_top = y < NOTCH_SHOULDER.max(pill.y);
    if style != IslandStyle::Notch
        || pill.contains(x, y)
        || !on_top
        || !layout::notch_bounds(pill).contains(x, y)
    {
        return (x, y);
    }
    (clamp_into(x, pill.x, pill.w), clamp_into(y, pill.y, pill.h))
}

/// The strip reserved along the top of a monitor (`[island] reserve_top`), in physical
/// desktop px `(left, top, right, bottom)`: the monitor's full width (Windows only
/// reserves whole edges), `layout::reserve_h(idle_h)` logical px tall (the resting notch
/// height `idle_h`, never less than the compact Music and Timer views).
pub fn reserve_strip(left: i32, top: i32, width: i32, scale: f32, idle_h: f32) -> Edges {
    let h = (layout::reserve_h(idle_h) * scale).ceil() as i32;
    (left, top, left + width, top + h)
}

/// `v` clamped into the half-open span `[start, start + len)`.
fn clamp_into(v: f32, start: f32, len: f32) -> f32 {
    v.clamp(start, start + (len - INSET).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::layout::{IDLE_H, IDLE_W, TOP_MARGIN};

    /// The idle pill, centred in the window as `Island::pill_rect` puts it.
    fn idle() -> Rect {
        let (win_w, _) = layout::window_size();
        Rect::new((win_w - IDLE_W) / 2.0, TOP_MARGIN, IDLE_W, IDLE_H)
    }

    #[test]
    fn the_reserved_strip_spans_the_monitor_and_holds_the_resting_notch() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let (l, t, r, b) = reserve_strip(-1920, 0, 1920, scale, layout::IDLE_H);
            assert_eq!((l, t, r), (-1920, 0, 0));
            // At least as tall as the notch's column in every rest state.
            let col = device_edges(body_rect(IslandStyle::Notch, idle()), scale);
            assert!(b - t >= col.3, "{scale}: {} < {}", b - t, col.3);
            let compact = Rect::new(0.0, TOP_MARGIN, 224.0, layout::MUSIC_COMPACT_H);
            assert!(b - t >= device_edges(compact, scale).3, "{scale}");
        }
        // A monitor that doesn't start at the origin keeps its own top.
        assert_eq!(
            reserve_strip(0, -1080, 1920, 1.0, layout::IDLE_H),
            (0, -1080, 1920, -1038)
        );
    }

    #[test]
    fn pill_style_body_is_the_pill() {
        assert_eq!(body_rect(IslandStyle::Pill, idle()), idle());
    }

    #[test]
    fn notch_column_reaches_the_top_edge() {
        let p = idle();
        let b = body_rect(IslandStyle::Notch, p);
        assert_eq!((b.x, b.y, b.w), (p.x, 0.0, p.w));
        assert_eq!(b.y + b.h, p.y + p.h);
    }

    #[test]
    fn device_edges_scale_and_round() {
        let r = Rect::new(10.2, 6.0, 100.0, 32.0);
        assert_eq!(device_edges(r, 1.0), (10, 6, 110, 38));
        assert_eq!(device_edges(r, 1.5), (15, 9, 165, 57));
    }

    #[test]
    fn notch_region_is_the_column_plus_the_shoulders() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let pill = device_edges(idle(), scale);
            let col = device_edges(body_rect(IslandStyle::Notch, idle()), scale);
            assert_eq!(col, (pill.0, 0, pill.2, pill.3));
            let (l, t, r, b) = shoulder_strip(col, scale);
            let sh = (NOTCH_SHOULDER * scale).round() as i32;
            assert_eq!((l, t, r, b), (pill.0 - sh, 0, pill.2 + sh, sh));
            // The strip matches the drawn shoulders: `notch_bounds` in device px.
            let nb = device_edges(layout::notch_bounds(idle()), scale);
            assert!((l - nb.0).abs() <= 1 && (r - nb.2).abs() <= 1);
        }
    }

    #[test]
    fn pill_style_passes_every_point_through() {
        let p = idle();
        for (x, y) in [(p.x + 10.0, 1.0), (p.x - 3.0, 10.0), (p.x + 10.0, 10.0)] {
            assert_eq!(to_pill(IslandStyle::Pill, p, x, y), (x, y));
        }
    }

    #[test]
    fn notch_points_on_the_pill_pass_through() {
        let p = idle();
        let (x, y) = (p.x + 40.0, p.y + 12.0);
        assert_eq!(to_pill(IslandStyle::Notch, p, x, y), (x, y));
    }

    #[test]
    fn notch_neck_maps_to_the_pill_top() {
        let p = idle();
        let x = p.x + 50.0;
        assert_eq!(to_pill(IslandStyle::Notch, p, x, 0.0), (x, p.y));
        assert_eq!(to_pill(IslandStyle::Notch, p, x, p.y - 0.5), (x, p.y));
    }

    #[test]
    fn notch_shoulders_map_to_the_pill_sides() {
        let p = idle();
        let y = p.y + 1.0;
        assert_eq!(
            to_pill(IslandStyle::Notch, p, p.x - NOTCH_SHOULDER / 2.0, y),
            (p.x, y)
        );
        let (x, y2) = to_pill(IslandStyle::Notch, p, p.x + p.w + 2.0, y);
        assert_eq!(y2, y);
        assert!(p.contains(x, y), "the right shoulder lands inside the pill");
        // The top corners of the notch go to the pill's top corners.
        let (cx, cy) = to_pill(IslandStyle::Notch, p, p.x - 1.0, 1.0);
        assert_eq!((cx, cy), (p.x, p.y));
    }

    #[test]
    fn notch_every_body_point_lands_in_the_pill() {
        let p = idle();
        let b = layout::notch_bounds(p);
        let steps = 40;
        for i in 0..steps {
            for j in 0..steps {
                let x = b.x + b.w * i as f32 / steps as f32;
                let y = NOTCH_SHOULDER * j as f32 / steps as f32;
                let (px, py) = to_pill(IslandStyle::Notch, p, x, y);
                assert!(p.contains(px, py), "({x}, {y}) -> ({px}, {py})");
            }
        }
    }

    #[test]
    fn notch_points_off_the_body_pass_through() {
        let p = idle();
        let outside = [
            (p.x - NOTCH_SHOULDER - 1.0, 2.0),
            (p.x + p.w + NOTCH_SHOULDER + 1.0, 2.0),
            (p.x + 20.0, p.y + p.h + 1.0),
            (p.x - 4.0, p.y + p.h + 1.0),
            (p.x + 20.0, -3.0),
            // Beside the pill below the shoulders: not part of the notch.
            (p.x - 4.0, NOTCH_SHOULDER + 2.0),
            (p.x + p.w + 4.0, p.y + p.h - 2.0),
        ];
        for (x, y) in outside {
            assert_eq!(to_pill(IslandStyle::Notch, p, x, y), (x, y));
        }
    }
}
