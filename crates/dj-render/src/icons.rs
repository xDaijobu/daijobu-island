//! Tiny stroke icons for the feature tiles and the Snap toolbar, drawn in a +-10 unit
//! box.

use dj_core::features::Icon;
use dj_core::osd::VolumeIcon;
use dj_core::snap_edit::{Item, Tool};
use skia_safe::{Canvas, Paint, Path, PathBuilder, Point, RRect, Rect};

fn poly(pts: &[(f32, f32)], close: bool) -> Path {
    let mut b = PathBuilder::new();
    b.move_to(pts[0]);
    for p in &pts[1..] {
        b.line_to(*p);
    }
    if close {
        b.close();
    }
    b.detach()
}

fn line(canvas: &Canvas, a: (f32, f32), b: (f32, f32), paint: &Paint) {
    canvas.draw_line(a, b, paint);
}

fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> RRect {
    RRect::new_rect_xy(Rect::from_xywh(x, y, w, h), r, r)
}

/// `paint` must be a stroke paint; `size` is the icon's full width in px.
pub fn draw_icon(canvas: &Canvas, icon: Icon, cx: f32, cy: f32, size: f32, paint: &Paint) {
    canvas.save();
    canvas.translate((cx, cy));
    let k = size / 22.0;
    canvas.scale((k, k));
    match icon {
        Icon::Media => {
            canvas.draw_path(&poly(&[(-5.0, -7.5), (8.0, 0.0), (-5.0, 7.5)], true), paint);
        }
        Icon::Osd => {
            canvas.draw_path(
                &poly(
                    &[
                        (-9.0, -3.0),
                        (-5.0, -3.0),
                        (0.0, -7.5),
                        (0.0, 7.5),
                        (-5.0, 3.0),
                        (-9.0, 3.0),
                    ],
                    true,
                ),
                paint,
            );
            canvas.draw_arc(
                Rect::from_xywh(-4.0, -5.0, 10.0, 10.0),
                -45.0,
                90.0,
                false,
                paint,
            );
            canvas.draw_arc(
                Rect::from_xywh(-6.5, -9.0, 17.0, 18.0),
                -45.0,
                90.0,
                false,
                paint,
            );
        }
        Icon::Vitals => {
            canvas.draw_path(
                &poly(
                    &[
                        (-10.0, 1.0),
                        (-5.0, 1.0),
                        (-2.5, -7.0),
                        (1.0, 8.0),
                        (3.5, -2.0),
                        (5.0, 1.0),
                        (10.0, 1.0),
                    ],
                    false,
                ),
                paint,
            );
        }
        Icon::Timer => {
            canvas.draw_circle((0.0, 1.5), 8.0, paint);
            line(canvas, (-2.5, -9.5), (2.5, -9.5), paint);
            line(canvas, (0.0, 1.5), (0.0, -3.5), paint);
            line(canvas, (0.0, 1.5), (3.5, 3.5), paint);
        }
        Icon::Snap => {
            for (sx, sy) in [(-1.0_f32, -1.0_f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                canvas.draw_path(
                    &poly(
                        &[
                            (sx * 3.0, sy * 9.0),
                            (sx * 9.0, sy * 9.0),
                            (sx * 9.0, sy * 3.0),
                        ],
                        false,
                    ),
                    paint,
                );
            }
            canvas.draw_circle((0.0, 0.0), 1.4, paint);
        }
        Icon::Clipboard => {
            canvas.draw_rrect(rrect(-7.0, -7.5, 14.0, 17.0, 2.0), paint);
            canvas.draw_rrect(rrect(-3.5, -10.0, 7.0, 4.5, 1.5), paint);
            line(canvas, (-3.5, 0.0), (3.5, 0.0), paint);
            line(canvas, (-3.5, 4.0), (1.5, 4.0), paint);
        }
        Icon::Notifications => {
            let mut b = PathBuilder::new();
            b.move_to((-6.5, 4.5));
            b.line_to((-6.5, -1.0));
            b.cubic_to((-6.5, -9.0), (6.5, -9.0), (6.5, -1.0));
            b.line_to((6.5, 4.5));
            b.close();
            canvas.draw_path(&b.detach(), paint);
            line(canvas, (-9.0, 4.5), (9.0, 4.5), paint);
            canvas.draw_arc(
                Rect::from_xywh(-2.5, 3.0, 5.0, 6.0),
                20.0,
                140.0,
                false,
                paint,
            );
        }
        Icon::Lyrics => {
            line(canvas, (-10.0, -5.0), (3.0, -5.0), paint);
            line(canvas, (-10.0, 0.0), (0.0, 0.0), paint);
            line(canvas, (-10.0, 5.0), (-3.0, 5.0), paint);
            canvas.draw_circle((5.5, 5.0), 2.3, paint);
            line(canvas, (7.8, 5.0), (7.8, -5.5), paint);
        }
        Icon::Cli => {
            canvas.draw_path(
                &poly(&[(-9.0, -5.0), (-3.0, 0.0), (-9.0, 5.0)], false),
                paint,
            );
            line(canvas, (0.0, 6.0), (9.0, 6.0), paint);
        }
        Icon::Tile => {
            canvas.draw_rrect(rrect(-9.5, -8.0, 19.0, 16.0, 2.5), paint);
            line(canvas, (0.0, -8.0), (0.0, 8.0), paint);
            line(canvas, (0.0, 0.0), (9.5, 0.0), paint);
        }
        Icon::Sweep => {
            line(canvas, (8.0, -9.5), (0.5, -1.0), paint);
            canvas.draw_path(
                &poly(&[(-1.5, -1.5), (3.5, 2.5), (-1.5, 9.0), (-9.0, 4.0)], true),
                paint,
            );
            line(canvas, (-4.5, 3.5), (-1.0, 6.5), paint);
        }
        Icon::Shelf => {
            canvas.draw_path(
                &poly(&[(-9.5, 1.0), (-9.5, 8.0), (9.5, 8.0), (9.5, 1.0)], false),
                paint,
            );
            canvas.draw_rrect(rrect(-4.5, -9.0, 9.0, 11.0, 1.5), paint);
        }
        Icon::Ambient => {
            for y in [-5.5_f32, 0.0, 5.5] {
                let mut b = PathBuilder::new();
                b.move_to((-9.5, y));
                b.quad_to((-6.5, y - 3.5), (-3.2, y));
                b.quad_to((0.0, y + 3.5), (3.2, y));
                b.quad_to((6.5, y - 3.5), (9.5, y));
                canvas.draw_path(&b.detach(), paint);
            }
        }
    }
    canvas.restore();
}

/// Speaker with sound arcs (or an X when muted), same box and scaling as `draw_icon`.
pub fn draw_volume_icon(
    canvas: &Canvas,
    icon: VolumeIcon,
    cx: f32,
    cy: f32,
    size: f32,
    paint: &Paint,
) {
    canvas.save();
    canvas.translate((cx, cy));
    let k = size / 22.0;
    canvas.scale((k, k));
    canvas.draw_path(
        &poly(
            &[
                (-9.0, -3.0),
                (-5.0, -3.0),
                (0.0, -7.5),
                (0.0, 7.5),
                (-5.0, 3.0),
                (-9.0, 3.0),
            ],
            true,
        ),
        paint,
    );
    // Concentric arc centred just past the cone, spanning +-45 degrees.
    let arc = |r: f32| {
        canvas.draw_arc(
            Rect::from_xywh(0.5 - r, -r, 2.0 * r, 2.0 * r),
            -45.0,
            90.0,
            false,
            paint,
        );
    };
    match icon {
        VolumeIcon::Muted => {
            line(canvas, (3.5, -3.0), (9.5, 3.0), paint);
            line(canvas, (9.5, -3.0), (3.5, 3.0), paint);
        }
        VolumeIcon::Low => arc(4.0),
        VolumeIcon::Mid => {
            arc(4.0);
            arc(7.5);
        }
        VolumeIcon::High => {
            arc(3.5);
            arc(6.5);
            arc(9.5);
        }
    }
    canvas.restore();
}

/// Small padlock in a +-5 box (stroked shackle, filled body).
pub fn draw_lock(canvas: &Canvas, cx: f32, cy: f32, size: f32, paint: &Paint) {
    canvas.save();
    canvas.translate((cx, cy));
    let k = size / 11.0;
    canvas.scale((k, k));
    let mut stroke = paint.clone();
    stroke.set_style(skia_safe::paint::Style::Stroke);
    stroke.set_stroke_width(1.3);
    let mut b = PathBuilder::new();
    b.move_to(Point::new(-2.4, -0.5));
    b.line_to((-2.4, -2.2));
    b.cubic_to((-2.4, -5.6), (2.4, -5.6), (2.4, -2.2));
    b.line_to((2.4, -0.5));
    canvas.draw_path(&b.detach(), &stroke);
    let mut fill = paint.clone();
    fill.set_style(skia_safe::paint::Style::Fill);
    canvas.draw_rrect(rrect(-4.0, -0.5, 8.0, 6.0, 1.6), &fill);
    canvas.restore();
}

/// Settings gear in the same +-10 box as the tile icons: a toothed outline and a hub.
/// `paint` must be a stroke paint; `size` is the icon's full width in px.
pub fn draw_gear(canvas: &Canvas, cx: f32, cy: f32, size: f32, paint: &Paint) {
    canvas.save();
    canvas.translate((cx, cy));
    let k = size / 22.0;
    canvas.scale((k, k));
    const TEETH: usize = 8;
    let (inner, outer) = (6.2_f32, 8.6_f32);
    // Each tooth spans a quarter of its sector at the tip and half at the root.
    let sector = std::f32::consts::TAU / TEETH as f32;
    let at = |r: f32, a: f32| (r * a.cos(), r * a.sin());
    let mut pts = Vec::with_capacity(TEETH * 4);
    for i in 0..TEETH {
        let a = i as f32 * sector;
        pts.push(at(inner, a - sector * 0.25));
        pts.push(at(outer, a - sector * 0.13));
        pts.push(at(outer, a + sector * 0.13));
        pts.push(at(inner, a + sector * 0.25));
    }
    canvas.draw_path(&poly(&pts, true), paint);
    canvas.draw_circle((0.0, 0.0), 2.6, paint);
    canvas.restore();
}

/// The Snap toolbar's glyphs (tools, undo/redo, copy, save, close), in the same box and
/// stroke style as the tile icons. Swatches and sizes are drawn by the toolbar itself.
pub fn draw_toolbar_icon(canvas: &Canvas, item: Item, cx: f32, cy: f32, size: f32, paint: &Paint) {
    canvas.save();
    canvas.translate((cx, cy));
    let k = size / 22.0;
    canvas.scale((k, k));
    match item {
        Item::Tool(Tool::Select) => {
            canvas.draw_path(
                &poly(
                    &[
                        (-5.0, -8.5),
                        (-5.0, 6.0),
                        (-1.5, 2.5),
                        (1.5, 8.5),
                        (4.0, 7.5),
                        (1.0, 1.5),
                        (5.5, 1.5),
                    ],
                    true,
                ),
                paint,
            );
        }
        Item::Tool(Tool::Pen) => {
            canvas.draw_path(
                &poly(
                    &[
                        (-8.0, 8.0),
                        (-7.0, 3.5),
                        (3.5, -7.0),
                        (7.0, -3.5),
                        (-3.5, 7.0),
                    ],
                    true,
                ),
                paint,
            );
            line(canvas, (1.0, -4.5), (4.5, -1.0), paint);
        }
        Item::Tool(Tool::Line) => line(canvas, (-7.0, 7.0), (7.0, -7.0), paint),
        Item::Tool(Tool::Arrow) => {
            line(canvas, (-7.0, 7.0), (6.5, -6.5), paint);
            canvas.draw_path(
                &poly(&[(-0.5, -7.0), (7.0, -7.0), (7.0, 0.5)], false),
                paint,
            );
        }
        Item::Tool(Tool::Rect) => {
            canvas.draw_rect(Rect::from_xywh(-7.5, -6.0, 15.0, 12.0), paint);
        }
        Item::Tool(Tool::Marker) => {
            canvas.draw_path(
                &poly(&[(-6.5, 3.0), (2.5, -6.0), (6.5, -2.0), (-2.5, 7.0)], true),
                paint,
            );
            line(canvas, (-9.0, 9.0), (1.0, 9.0), paint);
        }
        Item::Tool(Tool::Text) => {
            line(canvas, (-6.5, -7.0), (6.5, -7.0), paint);
            line(canvas, (0.0, -7.0), (0.0, 7.5), paint);
            line(canvas, (-2.5, 7.5), (2.5, 7.5), paint);
        }
        Item::Tool(Tool::Pixelate) => {
            canvas.draw_rect(Rect::from_xywh(-7.5, -7.5, 15.0, 15.0), paint);
            for t in [-2.5, 2.5] {
                line(canvas, (t, -7.5), (t, 7.5), paint);
                line(canvas, (-7.5, t), (7.5, t), paint);
            }
        }
        Item::Tool(Tool::Step) => {
            canvas.draw_circle((0.0, 0.0), 8.0, paint);
            canvas.draw_path(
                &poly(&[(-2.0, -2.0), (0.5, -4.5), (0.5, 4.5)], false),
                paint,
            );
            line(canvas, (-2.0, 4.5), (3.0, 4.5), paint);
        }
        Item::Undo | Item::Redo => {
            if item == Item::Redo {
                canvas.scale((-1.0, 1.0));
            }
            // An arc over the top, with the head at its left end pointing down.
            canvas.draw_arc(
                Rect::from_xywh(-5.0, -4.5, 12.0, 13.0),
                180.0,
                225.0,
                false,
                paint,
            );
            canvas.draw_path(
                &poly(&[(-8.0, -0.5), (-5.0, 3.0), (-2.0, -0.5)], false),
                paint,
            );
        }
        Item::Copy => {
            canvas.draw_rrect(rrect(-7.5, -3.5, 10.5, 11.0, 2.0), paint);
            canvas.draw_path(
                &poly(
                    &[
                        (-3.5, -3.5),
                        (-3.5, -7.5),
                        (7.0, -7.5),
                        (7.0, 3.0),
                        (3.0, 3.0),
                    ],
                    false,
                ),
                paint,
            );
        }
        Item::Save => {
            line(canvas, (0.0, -8.0), (0.0, 3.0), paint);
            canvas.draw_path(
                &poly(&[(-4.0, -1.0), (0.0, 3.0), (4.0, -1.0)], false),
                paint,
            );
            canvas.draw_path(
                &poly(&[(-7.5, 3.0), (-7.5, 7.5), (7.5, 7.5), (7.5, 3.0)], false),
                paint,
            );
        }
        Item::Close => {
            line(canvas, (-5.5, -5.5), (5.5, 5.5), paint);
            line(canvas, (5.5, -5.5), (-5.5, 5.5), paint);
        }
        Item::Swatch(_) | Item::Custom | Item::Size(_) | Item::Pin => {}
    }
    canvas.restore();
}
