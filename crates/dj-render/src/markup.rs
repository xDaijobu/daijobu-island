//! Snap markup: the one painter that draws annotation shapes, used by both the overlay
//! and the copied/saved output (so the output is what the overlay shows), the text
//! metrics behind the editor's hit tests, and the CPU render of the output.
//!
//! Shapes are in desktop px; callers translate the canvas so desktop px land where they
//! belong (the overlay subtracts its origin, the output the crop's top-left).

use crate::fonts::GEIST_MEDIUM;
use crate::paint::{fill_paint, stroke_paint};
use dj_core::snap::{crop, Bitmap, IRect, MonitorFrame};
use dj_core::snap_edit::{
    arrow_geom, pixelate_cells, step_ink, text_lines, Rgba, Shape, ShapeKind, TextMeasure,
    MARKER_ALPHA,
};
use skia_safe::font::Edging;
use skia_safe::paint::{Cap, Join};
use skia_safe::{
    images, surfaces, AlphaType, Canvas, ClipOp, Color, ColorType, Data, FilterMode, Font,
    FontHinting, FontMgr, Image, ImageInfo, MipmapMode, PathBuilder, Point, Rect, SamplingOptions,
    Typeface,
};

/// A raster BGRA image of `b` (copied).
pub(crate) fn raster(b: &Bitmap) -> Option<Image> {
    let info = ImageInfo::new(
        (b.width as i32, b.height as i32),
        ColorType::BGRA8888,
        AlphaType::Opaque,
        None,
    );
    images::raster_from_data(&info, Data::new_copy(&b.pixels), b.stride())
}

pub(crate) fn color(c: Rgba, alpha: f32) -> Color {
    Color::from_argb(
        ((c.a as f32 / 255.0) * alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
        c.r,
        c.g,
        c.b,
    )
}

pub(crate) fn frect(r: IRect) -> Rect {
    Rect::from_xywh(r.x as f32, r.y as f32, r.w as f32, r.h as f32)
}

/// Markup text is Geist Medium; the editor measures it with this, and the painter draws
/// it with the same font, so hit boxes and the caret match what is drawn.
pub struct TextMetrics {
    face: Typeface,
}

impl TextMetrics {
    pub fn new() -> Result<Self, String> {
        let face = FontMgr::default()
            .new_from_data(Data::new_copy(GEIST_MEDIUM), None)
            .ok_or("embedded font Geist Medium failed to load")?;
        Ok(Self { face })
    }

    pub(crate) fn font(&self, px: f32) -> Font {
        let mut f = Font::from_typeface(self.face.clone(), px);
        f.set_subpixel(true);
        f.set_hinting(FontHinting::None);
        f.set_edging(Edging::AntiAlias);
        f
    }

    /// The first baseline below the text's top.
    pub(crate) fn ascent(&self, px: f32) -> f32 {
        -self.font(px).metrics().1.ascent
    }
}

impl TextMeasure for TextMetrics {
    fn width(&self, text: &str, px: f32) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        self.font(px).measure_str(text, None).0
    }

    fn line_height(&self, px: f32) -> f32 {
        self.font(px).metrics().0
    }
}

/// Pixelate's small images, one per box and block size. Rebuilt only when a box moves or
/// changes size; entries no frame used are dropped.
#[derive(Default)]
pub(crate) struct PixelCache {
    entries: Vec<(IRect, i32, Image, bool)>,
    #[cfg(test)]
    fail_next_raster: bool,
}

impl PixelCache {
    fn get(&mut self, frames: &[MonitorFrame], rect: IRect, block: i32) -> Option<Image> {
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_raster) {
            return None;
        }
        if let Some(e) = self
            .entries
            .iter_mut()
            .find(|e| e.0 == rect && e.1 == block)
        {
            e.3 = true;
            return Some(e.2.clone());
        }
        let img = raster(&pixelate_cells(frames, rect, block))?;
        self.entries.push((rect, block, img.clone(), true));
        Some(img)
    }

    /// Forgets what this frame didn't use.
    fn sweep(&mut self) {
        self.entries.retain(|e| e.3);
        for e in &mut self.entries {
            e.3 = false;
        }
    }
}

/// Draws `shapes` then `draft`, clipped to `clip` (all desktop px).
pub(crate) fn draw_markup(
    canvas: &Canvas,
    shapes: &[Shape],
    draft: Option<&Shape>,
    clip: IRect,
    frames: &[MonitorFrame],
    cache: &mut PixelCache,
    metrics: &TextMetrics,
) -> Result<(), &'static str> {
    canvas.save();
    canvas.clip_rect(frect(clip), ClipOp::Intersect, false);
    let mut result = Ok(());
    for s in shapes.iter().chain(draft) {
        result = result.and(draw_shape(canvas, s, frames, cache, metrics));
    }
    canvas.restore();
    cache.sweep();
    result
}

/// A freehand stroke, smoothed through the midpoints. One point is a dot.
fn stroke_points(canvas: &Canvas, pts: &[(f32, f32)], width: f32, ink: Color) {
    match pts {
        [] => {}
        [p] => {
            canvas.draw_circle(*p, width / 2.0, &fill_paint(ink));
        }
        [a, b] => {
            canvas.draw_line(*a, *b, &stroke_paint(ink, width));
        }
        _ => {
            let mut path = PathBuilder::new();
            path.move_to(pts[0]);
            for w in pts[1..].windows(2) {
                let mid = ((w[0].0 + w[1].0) / 2.0, (w[0].1 + w[1].1) / 2.0);
                path.quad_to(w[0], mid);
            }
            path.line_to(pts[pts.len() - 1]);
            canvas.draw_path(&path.detach(), &stroke_paint(ink, width));
        }
    }
}

fn draw_shape(
    canvas: &Canvas,
    s: &Shape,
    frames: &[MonitorFrame],
    cache: &mut PixelCache,
    metrics: &TextMetrics,
) -> Result<(), &'static str> {
    let ink = color(s.color, 1.0);
    let w = s.width();
    match &s.kind {
        ShapeKind::Pen(pts) => stroke_points(canvas, pts, w, ink),
        ShapeKind::Marker(pts) => {
            // Drawn opaque into a layer that is then blended at 40%, so a stroke never
            // darkens where it crosses itself.
            let b = s.bounds(metrics).inflate(1.0);
            canvas.save_layer_alpha_f(Rect::from_xywh(b.x, b.y, b.w, b.h), MARKER_ALPHA);
            stroke_points(canvas, pts, w, ink);
            canvas.restore();
        }
        ShapeKind::Line(a, b) => {
            canvas.draw_line(*a, *b, &stroke_paint(ink, w));
        }
        ShapeKind::Arrow(a, b) => {
            let g = arrow_geom(*a, *b, w, s.scale);
            let mut shaft = stroke_paint(ink, w);
            shaft.set_stroke_cap(Cap::Round);
            canvas.draw_line(*a, g.shaft_end, &shaft);
            let mut head = PathBuilder::new();
            head.move_to(g.head[0]);
            head.line_to(g.head[1]);
            head.line_to(g.head[2]);
            head.close();
            canvas.draw_path(&head.detach(), &fill_paint(ink));
        }
        ShapeKind::Rect(r) => {
            let mut p = stroke_paint(ink, w);
            p.set_stroke_join(Join::Miter);
            p.set_stroke_cap(Cap::Square);
            canvas.draw_rect(Rect::from_xywh(r.x, r.y, r.w, r.h), &p);
        }
        ShapeKind::Text { at, text } => {
            let px = s.font_px();
            let font = metrics.font(px);
            let lh = metrics.line_height(px);
            let base = at.1 + metrics.ascent(px);
            let paint = fill_paint(ink);
            for (i, line) in text_lines(text).enumerate() {
                if !line.is_empty() {
                    canvas.draw_str(line, (at.0, base + i as f32 * lh), &font, &paint);
                }
            }
        }
        ShapeKind::Pixelate(r) => {
            let ir = r.to_irect();
            let block = s.block();
            if ir.is_empty() {
                return Ok(());
            }
            if let Some(img) = cache.get(frames, ir, block) {
                // Each cell becomes a `block` px square, nearest sampled, from the box's
                // top-left; whole-pixel offsets keep the overlay and the output equal.
                canvas.save();
                canvas.clip_rect(frect(ir), ClipOp::Intersect, false);
                canvas.translate((ir.x as f32, ir.y as f32));
                canvas.scale((block as f32, block as f32));
                canvas.draw_image_with_sampling_options(
                    &img,
                    (0.0, 0.0),
                    SamplingOptions::new(FilterMode::Nearest, MipmapMode::None),
                    None,
                );
                canvas.restore();
            } else {
                let mut cover = fill_paint(Color::BLACK);
                cover.set_anti_alias(false);
                canvas.draw_rect(frect(ir), &cover);
                return Err("cannot render pixelate");
            }
        }
        ShapeKind::Step { at, n } => {
            let d = s.step_diameter();
            canvas.draw_circle(*at, d / 2.0, &fill_paint(ink));
            let label = n.to_string();
            let px = d * 0.55;
            let font = metrics.font(px);
            let (tw, bounds) = font.measure_str(&label, None);
            let y = at.1 - bounds.center_y();
            canvas.draw_str(
                &label,
                Point::new(at.0 - tw / 2.0, y),
                &font,
                &fill_paint(color(step_ink(s.color), 1.0)),
            );
        }
    }
    Ok(())
}

/// The copied/saved image: the crop of `rect` with the markup drawn over it, on a CPU
/// surface the size of the crop, in physical px. With no shapes it is the 7a crop,
/// byte for byte.
pub fn render_output(
    frames: &[MonitorFrame],
    rect: IRect,
    shapes: &[Shape],
) -> Result<Bitmap, String> {
    render_output_with_cache(frames, rect, shapes, &mut PixelCache::default())
}

fn render_output_with_cache(
    frames: &[MonitorFrame],
    rect: IRect,
    shapes: &[Shape],
    cache: &mut PixelCache,
) -> Result<Bitmap, String> {
    let base = crop(frames, rect);
    if shapes.is_empty() || base.width == 0 || base.height == 0 {
        return Ok(base);
    }
    let (w, h) = (base.width as i32, base.height as i32);
    let info = ImageInfo::new((w, h), ColorType::BGRA8888, AlphaType::Premul, None);
    let mut surface = surfaces::raster(&info, None, None)
        .ok_or_else(|| format!("cannot make a {w} x {h} surface"))?;
    let metrics = TextMetrics::new()?;
    {
        let canvas = surface.canvas();
        let img = raster(&base).ok_or("cannot wrap the crop")?;
        canvas.draw_image(&img, (0, 0), None);
        canvas.translate((-rect.x as f32, -rect.y as f32));
        draw_markup(canvas, shapes, None, rect, frames, cache, &metrics)?;
    }
    let mut out = Bitmap {
        width: base.width,
        height: base.height,
        pixels: vec![0; base.pixels.len()],
    };
    let stride = out.stride();
    if !surface.read_pixels(&info, &mut out.pixels, stride, (0, 0)) {
        return Err("cannot read the output back".into());
    }
    // The crop is opaque, so the result is too; make it exact.
    for px in out.pixels.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::snap_edit::{FRect, Size, SWATCHES};

    /// Pixel (x, y) of a monitor is [x, y, seed, 255] (wrapping).
    fn frames() -> Vec<MonitorFrame> {
        let mon = |rect: IRect, seed: u8| {
            let mut bitmap = Bitmap::black(rect.w as u32, rect.h as u32);
            for y in 0..rect.h {
                for x in 0..rect.w {
                    let i = (y * rect.w + x) as usize * 4;
                    bitmap.pixels[i] = x as u8;
                    bitmap.pixels[i + 1] = y as u8;
                    bitmap.pixels[i + 2] = seed;
                }
            }
            MonitorFrame {
                rect,
                dpi: 96,
                bitmap,
            }
        };
        vec![
            mon(IRect::new(0, 0, 200, 120), 10),
            mon(IRect::new(-100, 0, 100, 120), 20),
        ]
    }

    fn shape(kind: ShapeKind, color: Rgba) -> Shape {
        Shape {
            kind,
            color,
            size: Size::M,
            scale: 1.0,
        }
    }

    fn px(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * b.width + x) * 4) as usize;
        [
            b.pixels[i],
            b.pixels[i + 1],
            b.pixels[i + 2],
            b.pixels[i + 3],
        ]
    }

    #[test]
    fn an_output_with_no_markup_is_the_crop_byte_for_byte() {
        let f = frames();
        let rect = IRect::new(-30, 10, 80, 50);
        let out = render_output(&f, rect, &[]).unwrap();
        assert_eq!(out, crop(&f, rect));
        // Markup elsewhere leaves the rest of the crop exactly as it was.
        let tiny = shape(
            ShapeKind::Rect(FRect::new(40.0, 50.0, 6.0, 6.0)),
            SWATCHES[0],
        );
        let out = render_output(&f, rect, &[tiny]).unwrap();
        let base = crop(&f, rect);
        assert_eq!((out.width, out.height), (base.width, base.height));
        for y in 0..20 {
            for x in 0..40 {
                assert_eq!(px(&out, x, y), px(&base, x, y), "{x},{y}");
            }
        }
    }

    #[test]
    fn a_rect_colours_its_border_and_leaves_the_inside_alone() {
        let f = frames();
        let rect = IRect::new(0, 0, 100, 100);
        let red = SWATCHES[0];
        // M = 4 px, centred on the edges: 18..22 and 58..62.
        let r = shape(ShapeKind::Rect(FRect::new(20.0, 20.0, 40.0, 40.0)), red);
        let out = render_output(&f, rect, &[r]).unwrap();
        let bgra = [red.b, red.g, red.r, 255];
        assert_eq!(px(&out, 20, 40), bgra, "left edge");
        assert_eq!(px(&out, 40, 59), bgra, "bottom edge");
        assert_eq!(px(&out, 60, 20), bgra, "corner (mitred)");
        assert_eq!(px(&out, 40, 40), [40, 40, 10, 255], "inside untouched");
        assert_eq!(px(&out, 10, 40), [10, 40, 10, 255], "outside untouched");
    }

    #[test]
    fn pixelate_gives_uniform_blocks_anchored_at_the_box() {
        let f = frames();
        let rect = IRect::new(-50, 0, 150, 100);
        // Blocks of 12 (M at 100% is 14): a 36 x 24 box from (-41, 5).
        let mut p = shape(
            ShapeKind::Pixelate(FRect::new(-41.0, 5.0, 36.0, 24.0)),
            SWATCHES[0],
        );
        p.size = Size::S;
        assert_eq!(p.block(), 12);
        let out = render_output(&f, rect, &[p]).unwrap();
        // Box (-41, 5) is output (9, 5). Each 12 x 12 block is one colour: the frozen
        // pixel at its centre, (-41 + 6, 5 + 6) = left monitor (65, 11).
        for by in 0..2u32 {
            for bx in 0..3u32 {
                let (ox, oy) = (9 + bx * 12, 5 + by * 12);
                let want = px(&out, ox, oy);
                for y in 0..12 {
                    for x in 0..12 {
                        assert_eq!(px(&out, ox + x, oy + y), want, "block {bx},{by}");
                    }
                }
            }
        }
        assert_eq!(px(&out, 9, 5), [65, 11, 20, 255]);
        // The next block reads 12 px on.
        assert_eq!(px(&out, 21, 5), [77, 11, 20, 255]);
        // Just outside the box: untouched.
        assert_eq!(px(&out, 8, 5), [58, 5, 20, 255]);
        assert_eq!(px(&out, 45, 5), [(-5i32 + 100) as u8, 5, 20, 255]);
    }

    #[test]
    fn pixelate_failure_refuses_output() {
        let f = frames();
        let rect = IRect::new(-50, 0, 150, 100);
        let p = shape(
            ShapeKind::Pixelate(FRect::new(-41.0, 5.0, 36.0, 24.0)),
            SWATCHES[0],
        );
        let mut cache = PixelCache {
            fail_next_raster: true,
            ..PixelCache::default()
        };
        assert!(matches!(
            render_output_with_cache(&f, rect, &[p], &mut cache),
            Err(e) if e == "cannot render pixelate"
        ));
    }

    #[test]
    fn pixelate_failure_covers_preview_and_restores_canvas() {
        let f = frames();
        let p = shape(
            ShapeKind::Pixelate(FRect::new(0.0, 0.0, 40.0, 40.0)),
            SWATCHES[0],
        );
        let mut surface = surfaces::raster_n32_premul((64, 64)).unwrap();
        let canvas = surface.canvas();
        canvas.clear(Color::WHITE);
        let mut cache = PixelCache {
            fail_next_raster: true,
            ..PixelCache::default()
        };
        assert_eq!(
            draw_markup(
                canvas,
                &[p],
                None,
                IRect::new(5, 5, 20, 20),
                &f,
                &mut cache,
                &TextMetrics::new().unwrap(),
            ),
            Err("cannot render pixelate")
        );
        canvas.draw_rect(
            Rect::from_xywh(40.0, 40.0, 10.0, 10.0),
            &fill_paint(Color::RED),
        );
        let mut out = Bitmap::black(64, 64);
        let info = ImageInfo::new((64, 64), ColorType::BGRA8888, AlphaType::Premul, None);
        let stride = out.stride();
        assert!(surface.read_pixels(&info, &mut out.pixels, stride, (0, 0)));
        for y in 5..25 {
            for x in 5..25 {
                assert_eq!(px(&out, x, y), [0, 0, 0, 255]);
            }
        }
        assert_eq!(px(&out, 4, 5), [255, 255, 255, 255]);
        assert_eq!(px(&out, 25, 5), [255, 255, 255, 255]);
        assert_eq!(px(&out, 45, 45), [0, 0, 255, 255]);
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn markup_is_clipped_to_the_selection() {
        let f = frames();
        // A line that runs far outside the crop on both sides.
        let rect = IRect::new(10, 10, 50, 40);
        let l = shape(ShapeKind::Line((-100.0, 30.0), (300.0, 30.0)), SWATCHES[3]);
        let out = render_output(&f, rect, std::slice::from_ref(&l)).unwrap();
        assert_eq!((out.width, out.height), (50, 40));
        let blue = [SWATCHES[3].b, SWATCHES[3].g, SWATCHES[3].r, 255];
        assert_eq!(px(&out, 0, 20), blue);
        assert_eq!(px(&out, 49, 20), blue);
        // On a bigger canvas the same draw stops at the clip.
        let mut surface = surfaces::raster_n32_premul((200, 100)).unwrap();
        let canvas = surface.canvas();
        canvas.clear(Color::BLACK);
        let metrics = TextMetrics::new().unwrap();
        let mut cache = PixelCache::default();
        draw_markup(canvas, &[l], None, rect, &f, &mut cache, &metrics).unwrap();
        let img = surface.image_snapshot();
        let info = ImageInfo::new((1, 1), ColorType::BGRA8888, AlphaType::Premul, None);
        let read = |x: i32, y: i32| {
            let mut p = [0u8; 4];
            assert!(img.read_pixels(
                &info,
                &mut p,
                4,
                (x, y),
                skia_safe::image::CachingHint::Disallow
            ));
            p
        };
        assert_eq!(read(30, 30), blue);
        assert_eq!(read(5, 30), [0, 0, 0, 255], "left of the clip");
        assert_eq!(read(60, 30), [0, 0, 0, 255], "right of the clip");
    }

    #[test]
    fn every_shape_draws_and_the_marker_does_not_darken_where_it_crosses() {
        let f = frames();
        let rect = IRect::new(0, 0, 200, 120);
        let white = Rgba::rgb(255, 255, 255);
        // A marker that crosses itself over a flat grey patch.
        let grey = Bitmap {
            width: 200,
            height: 120,
            pixels: [100u8, 100, 100, 255].repeat(200 * 120),
        };
        let flat = [MonitorFrame {
            rect,
            dpi: 96,
            bitmap: grey,
        }];
        let m = shape(
            ShapeKind::Marker(vec![(20.0, 20.0), (80.0, 80.0), (80.0, 20.0), (20.0, 80.0)]),
            white,
        );
        let out = render_output(&flat, rect, &[m]).unwrap();
        let cross = px(&out, 50, 50);
        let single = px(&out, 30, 30);
        assert_eq!(cross, single, "no darker (or lighter) at the crossing");
        // 100 + (255 - 100) * 0.4 = 162.
        assert!(single[0].abs_diff(162) <= 2, "{single:?}");
        // The rest draw without trouble and change the picture.
        let all = vec![
            shape(ShapeKind::Pen(vec![(10.0, 10.0)]), white),
            shape(
                ShapeKind::Pen(vec![(10.0, 20.0), (30.0, 25.0), (50.0, 20.0)]),
                white,
            ),
            shape(ShapeKind::Arrow((10.0, 100.0), (90.0, 60.0)), white),
            shape(
                ShapeKind::Text {
                    at: (100.0, 10.0),
                    text: "hi\nthere".into(),
                },
                white,
            ),
            shape(
                ShapeKind::Step {
                    at: (150.0, 80.0),
                    n: 7,
                },
                SWATCHES[1],
            ),
        ];
        let out = render_output(&f, rect, &all).unwrap();
        assert_ne!(out, crop(&f, rect));
        assert_eq!(px(&out, 10, 10), [255, 255, 255, 255], "the dot");
        assert_eq!(
            px(&out, 150, 72),
            [SWATCHES[1].b, SWATCHES[1].g, SWATCHES[1].r, 255],
            "the step's disc"
        );
    }

    #[test]
    fn text_metrics_measure_geist() {
        let m = TextMetrics::new().unwrap();
        assert_eq!(m.width("", 20.0), 0.0);
        let one = m.width("abc", 20.0);
        assert!(one > 10.0 && one < 60.0, "{one}");
        assert!((m.width("abc", 40.0) - 2.0 * one).abs() < 0.5);
        let lh = m.line_height(20.0);
        assert!(lh > 20.0 && lh < 32.0, "{lh}");
        assert!(m.ascent(20.0) > 10.0 && m.ascent(20.0) < lh);
    }
}
