//! Independent screenshot references. Geometry belongs to `dj-core::snap_pin`;
//! this module owns only a fixed composition chain and a display-sized image.
//! No animation clock, original screenshot, clipboard or file IO lives here.

use crate::fonts::{Fonts, Weight};
use crate::gpu::{Chain, Gpu, RenderError};
use crate::markup::raster;
use crate::paint::{col, draw_text, fill_paint, fit, stroke_paint, text_width};
use dj_core::color::Rgb;
use dj_core::layout::Rect as LRect;
use dj_core::snap::{Bitmap, IRect};
use dj_core::snap_pin::{self, Layout, PinAction};
use dj_core::theme::ResolvedTokens;
use skia_safe::gpu::{self as sk_gpu, Budgeted};
use skia_safe::{
    surfaces, Canvas, ClipOp, Color, FilterMode, Font, Image, MipmapMode, Paint, RRect, Rect,
    SamplingOptions,
};

#[derive(Clone, Copy, Debug)]
pub struct PinScene<'a> {
    pub tokens: &'a ResolvedTokens,
    pub scale: f32,
    pub layout: &'a Layout,
    pub hover: Option<PinAction>,
    pub focus: Option<PinAction>,
    pub status: Option<(&'a str, bool)>,
    pub saved: bool,
    pub busy: bool,
}

pub struct PinSurface {
    // Drop image and wrapped buffers before the GPU context and native window.
    image: Option<Image>,
    fonts: Fonts,
    gpu: Gpu,
    chain: Chain,
    width: u32,
    height: u32,
}

impl PinSurface {
    /// Caller supplies P1's `allocation_size` for the current DPI. A DPI/device
    /// reset creates a replacement; ordinary movement/rendering never resizes it.
    pub fn new(hwnd: isize, width: u32, height: u32) -> Result<Self, RenderError> {
        if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
            return Err(RenderError::Setup("Invalid screen pin allocation".into()));
        }
        let fonts = Fonts::new().map_err(RenderError::Setup)?;
        let mut gpu = Gpu::new()?;
        let chain = Chain::new(&mut gpu, hwnd, width, height)?;
        Ok(Self {
            image: None,
            fonts,
            gpu,
            chain,
            width,
            height,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Copies/resamples once, retaining only the maximum displayed resolution at
    /// this DPI. Failed replacement leaves the previous cache intact.
    pub fn set_image(&mut self, bitmap: &Bitmap, scale: f32) -> bool {
        let Some(small) = display_image(bitmap, scale) else {
            return false;
        };
        self.image = Some(
            sk_gpu::images::texture_from_image(
                &mut self.gpu.ctx,
                &small,
                sk_gpu::Mipmapped::No,
                Budgeted::No,
            )
            .unwrap_or(small),
        );
        true
    }

    pub fn render(&mut self, scene: &PinScene) -> Result<(), RenderError> {
        let s = sane_scale(scene.scale);
        if !scene.layout.w.is_finite()
            || !scene.layout.h.is_finite()
            || scene.layout.w <= 0.0
            || scene.layout.h <= 0.0
            || (scene.layout.w * s).ceil() > self.width as f32
            || (scene.layout.h * s).ceil() > self.height as f32
        {
            return Err(RenderError::Setup(
                "Screen pin layout exceeds its fixed allocation".into(),
            ));
        }
        let canvas = self.chain.back_buffer().canvas();
        canvas.clear(Color::TRANSPARENT);
        draw_scene(canvas, &mut self.fonts, self.image.as_ref(), scene);
        self.chain.present(&mut self.gpu.ctx)
    }
}

impl Drop for PinSurface {
    fn drop(&mut self) {
        self.image = None;
        self.chain.surfaces.clear();
        self.gpu.ctx.flush_submit_and_sync_cpu();
    }
}

// Same normalization as P1: all drawing and cache sizing use the geometry's DPI.
fn sane_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale.clamp(0.5, 8.0)
    } else {
        1.0
    }
}

fn dev(rect: LRect, scale: f32) -> Rect {
    Rect::from_xywh(
        rect.x * scale,
        rect.y * scale,
        rect.w * scale,
        rect.h * scale,
    )
}

fn cache_size(width: u32, height: u32, scale: f32) -> (i32, i32) {
    let s = sane_scale(scale);
    // Obtain the maximum image fit from P1 rather than duplicating its geometry.
    // A constrained real work area may draw this cache smaller, never larger.
    let layout = snap_pin::layout((width, height), s, IRect::new(0, 0, 32768, 32768));
    (
        (layout.image.w * s).round().max(1.0) as i32,
        (layout.image.h * s).round().max(1.0) as i32,
    )
}

fn display_image(bitmap: &Bitmap, scale: f32) -> Option<Image> {
    if bitmap.width == 0
        || bitmap.height == 0
        || bitmap.width > i32::MAX as u32
        || bitmap.height > i32::MAX as u32
    {
        return None;
    }
    let bytes = (bitmap.width as usize)
        .checked_mul(bitmap.height as usize)?
        .checked_mul(4)?;
    if bytes != bitmap.pixels.len() {
        return None;
    }
    let source = raster(bitmap)?;
    let size = cache_size(bitmap.width, bitmap.height, scale);
    if size == (source.width(), source.height()) {
        return Some(source);
    }
    let source = source.with_default_mipmaps().unwrap_or(source);
    let mut surface = surfaces::raster_n32_premul(size)?;
    surface.canvas().clear(Color::TRANSPARENT);
    surface.canvas().draw_image_rect_with_sampling_options(
        &source,
        None,
        Rect::from_wh(size.0 as f32, size.1 as f32),
        SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear),
        &Paint::default(),
    );
    Some(surface.image_snapshot())
}

/// Theme validation guarantees text/pill contrast; accents, warning colours and
/// raised surfaces are not guaranteed to meet the text threshold in every theme.
fn legible(preferred: Rgb, background: Rgb, tokens: &ResolvedTokens) -> Rgb {
    if preferred.contrast(background) >= 4.5 {
        preferred
    } else {
        tokens.text
    }
}

fn label(canvas: &Canvas, font: &Font, text: &str, rect: Rect, ink: Color, s: f32) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let save = canvas.save();
    canvas.clip_rect(rect, ClipOp::Intersect, false);
    let text = fit(font, text, 0.0, (rect.width() - 8.0 * s).max(0.0));
    let (_, metrics) = font.metrics();
    let baseline = rect.center_y() - (metrics.ascent + metrics.descent) / 2.0;
    draw_text(
        canvas,
        font,
        &text,
        rect.center_x() - text_width(font, &text, 0.0) / 2.0,
        baseline,
        0.0,
        &fill_paint(ink),
    );
    canvas.restore_to_count(save);
}

fn draw_scene(canvas: &Canvas, fonts: &mut Fonts, image: Option<&Image>, sc: &PinScene) {
    let s = sane_scale(sc.scale);
    let t = sc.tokens;
    let bounds = Rect::from_wh(sc.layout.w * s, sc.layout.h * s);
    let save = canvas.save();
    canvas.clip_rect(bounds, ClipOp::Intersect, false);
    let radius = (14.0 * t.radius_scale * s).min(bounds.height() / 2.0);
    let body = RRect::new_rect_xy(bounds, radius, radius);
    canvas.clip_rrect(body, ClipOp::Intersect, true);
    canvas.draw_rect(bounds, &fill_paint(col(t.pill, 1.0)));

    // No grain, tint, busy scrim or chrome overlays the actual screenshot.
    let image_rect = dev(sc.layout.image, s);
    if let Some(image) = image {
        if image_rect.width() > 0.0 && image_rect.height() > 0.0 {
            let image_save = canvas.save();
            canvas.clip_rect(image_rect, ClipOp::Intersect, false);
            canvas.draw_image_rect_with_sampling_options(
                image,
                None,
                image_rect,
                SamplingOptions::new(FilterMode::Linear, MipmapMode::None),
                &Paint::default(),
            );
            canvas.restore_to_count(image_save);
        }
    }

    let font = fonts.font(&t.font_label, Weight::Medium, 12.0 * s);
    for (action, logical) in sc.layout.actions {
        let rect = dev(logical, s);
        let disabled = sc.busy && action != PinAction::Close;
        let hovered = sc.hover == Some(action) && !disabled;
        let background = if hovered && t.text.contrast(t.surface1) >= 4.5 {
            t.surface1
        } else {
            t.pill
        };
        let inner = rect.with_inset((s, s));
        let round = RRect::new_rect_xy(inner, 6.0 * s, 6.0 * s);
        canvas.draw_rrect(round, &fill_paint(col(background, 1.0)));
        canvas.draw_rrect(
            round,
            &stroke_paint(col(t.text, if hovered { 0.75 } else { 0.3 }), s),
        );
        let text = match action {
            PinAction::Copy => "Copy",
            PinAction::Save if sc.saved => "Open",
            PinAction::Save => "Save",
            PinAction::Close => "Close",
        };
        label(canvas, &font, text, inner, col(t.text, 1.0), s);
        if sc.focus == Some(action) {
            // Shape and text contrast distinguish keyboard focus from hover.
            canvas.draw_rrect(
                RRect::new_rect_xy(rect.with_inset((3.0 * s, 3.0 * s)), 4.0 * s, 4.0 * s),
                &stroke_paint(col(t.text, 1.0), 2.0 * s),
            );
        }
    }

    let status_font = fonts.font(&t.font_label, Weight::Medium, 11.0 * s);
    let status = sc.status.map(|(text, error)| {
        if error {
            (format!("Error: {text}"), legible(t.warn, t.pill, t))
        } else {
            (text.to_owned(), t.text)
        }
    });
    let status = status.or_else(|| sc.busy.then(|| ("Working...".to_owned(), t.text)));
    if let Some((text, ink)) = status {
        label(
            canvas,
            &status_font,
            &text,
            dev(sc.layout.status, s),
            col(ink, 1.0),
            s,
        );
    }
    canvas.draw_rrect(
        RRect::new_rect_xy(bounds.with_inset((0.5 * s, 0.5 * s)), radius, radius),
        &stroke_paint(col(t.text, 0.3), s),
    );
    canvas.restore_to_count(save);
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::theme::ThemeRegistry;
    use skia_safe::image::CachingHint;
    use skia_safe::{AlphaType, ColorType, ImageInfo};

    const THEMES: [&str; 4] = ["tobacco-tape", "midnight-cassette", "paper-zine", "clean"];
    const SCALES: [f32; 3] = [1.0, 1.5, 2.0];
    const RED: [u8; 4] = [32, 48, 224, 255];
    const BLUE: [u8; 4] = [224, 64, 32, 255];

    struct Shot {
        width: i32,
        pixels: Vec<u8>,
    }

    impl Shot {
        fn at(&self, x: i32, y: i32) -> [u8; 4] {
            let start = ((y * self.width + x) * 4) as usize;
            self.pixels[start..start + 4].try_into().unwrap()
        }

        fn count(&self, rect: Rect, color: [u8; 4]) -> usize {
            let mut count = 0;
            for y in rect.top.ceil() as i32..rect.bottom.floor() as i32 {
                for x in rect.left.ceil() as i32..rect.right.floor() as i32 {
                    count += usize::from(near(self.at(x, y), color, 20));
                }
            }
            count
        }
    }

    fn near(a: [u8; 4], b: [u8; 4], tolerance: u8) -> bool {
        a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= tolerance)
    }

    fn bgra(color: Rgb) -> [u8; 4] {
        let [r, g, b] = color.to_u8();
        [b, g, r, 255]
    }

    fn bitmap(width: u32, height: u32) -> Bitmap {
        let mut result = Bitmap {
            width,
            height,
            pixels: RED.repeat((width * height) as usize),
        };
        for y in 0..height {
            for x in width / 2..width {
                let start = ((y * width + x) * 4) as usize;
                result.pixels[start..start + 4].copy_from_slice(&BLUE);
            }
        }
        result
    }

    fn scene<'a>(tokens: &'a ResolvedTokens, scale: f32, layout: &'a Layout) -> PinScene<'a> {
        PinScene {
            tokens,
            scale,
            layout,
            hover: None,
            focus: None,
            status: None,
            saved: false,
            busy: false,
        }
    }

    fn paint(scene: &PinScene, image: Option<&Image>) -> Shot {
        let (w, h) = snap_pin::allocation_size(scene.scale);
        // Padding also proves the actual scene clips within its fixed allocation.
        let (w, h) = (w as i32 + 4, h as i32 + 4);
        let mut surface = surfaces::raster_n32_premul((w, h)).unwrap();
        surface.canvas().clear(Color::TRANSPARENT);
        draw_scene(surface.canvas(), &mut Fonts::new().unwrap(), image, scene);
        let info = ImageInfo::new((w, h), ColorType::BGRA8888, AlphaType::Premul, None);
        let mut pixels = vec![0; (w * h * 4) as usize];
        assert!(surface.read_pixels(&info, &mut pixels, (w * 4) as usize, (0, 0)));
        Shot { width: w, pixels }
    }

    #[test]
    fn small_tall_and_wide_images_keep_their_fit_and_pixels_in_every_theme_and_scale() {
        let registry = ThemeRegistry::with_builtins();
        for theme in THEMES {
            let tokens = registry.get(theme).unwrap();
            for scale in SCALES {
                for size in [(8, 6), (80, 960), (1280, 80)] {
                    let bitmap = bitmap(size.0, size.1);
                    let original = bitmap.pixels.clone();
                    let image = display_image(&bitmap, scale).unwrap();
                    let layout = snap_pin::layout(size, scale, IRect::new(-1920, 48, 1920, 1032));
                    let shot = paint(&scene(tokens, scale, &layout), Some(&image));
                    let rect = dev(layout.image, scale);
                    assert!(
                        (rect.width() / rect.height() - size.0 as f32 / size.1 as f32).abs() < 0.01
                    );
                    assert!(rect.width() <= size.0 as f32 + 0.01);
                    assert!(rect.height() <= size.1 as f32 + 0.01);
                    for (fraction, expected) in [(0.25, RED), (0.75, BLUE)] {
                        let pixel = shot.at(
                            (rect.left + rect.width() * fraction) as i32,
                            rect.center_y() as i32,
                        );
                        assert!(
                            near(pixel, expected, 3),
                            "{theme} {scale}x {size:?}: {pixel:?}"
                        );
                    }
                    assert_eq!(bitmap.pixels, original, "source pixels are immutable");
                    let width = (layout.w * scale).ceil() as i32;
                    let height = (layout.h * scale).ceil() as i32;
                    for y in 0..shot.pixels.len() as i32 / 4 / shot.width {
                        assert_eq!(shot.at(width, y)[3], 0, "right of actual layout");
                    }
                    for x in 0..shot.width {
                        assert_eq!(shot.at(x, height)[3], 0, "below actual layout");
                    }
                    for (_, action) in layout.actions {
                        assert!(
                            shot.count(
                                dev(action, scale).with_inset((6.0 * scale, 6.0 * scale)),
                                bgra(tokens.text)
                            ) > 8,
                            "{theme} {scale}x: action label is legible"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn hover_focus_saved_busy_and_error_states_are_visible_without_touching_the_image() {
        let registry = ThemeRegistry::with_builtins();
        for theme in THEMES {
            let tokens = registry.get(theme).unwrap();
            for scale in SCALES {
                let bitmap = bitmap(96, 64);
                let image = display_image(&bitmap, scale).unwrap();
                let layout = snap_pin::layout((96, 64), scale, IRect::new(0, 0, 1920, 1080));
                let base = scene(tokens, scale, &layout);
                let normal = paint(&base, Some(&image));
                for variant in 0..6 {
                    let mut state = base;
                    match variant {
                        0 => state.hover = Some(PinAction::Copy),
                        1 => state.focus = Some(PinAction::Close),
                        2 => state.saved = true,
                        3 => state.busy = true,
                        4 => state.status = Some(("Copy failed", true)),
                        _ => state.status = Some(("Copied", false)),
                    }
                    let changed = paint(&state, Some(&image));
                    assert_ne!(
                        normal.pixels, changed.pixels,
                        "{theme} {scale}x state {variant}"
                    );
                    let rect = dev(layout.image, scale).with_inset((1.0, 1.0));
                    for y in rect.top.ceil() as i32..rect.bottom.floor() as i32 {
                        for x in rect.left.ceil() as i32..rect.right.floor() as i32 {
                            assert_eq!(changed.at(x, y), normal.at(x, y), "state altered image");
                        }
                    }
                    if variant >= 3 {
                        let ink = if variant == 4 {
                            legible(tokens.warn, tokens.pill, tokens)
                        } else {
                            tokens.text
                        };
                        assert!(ink.contrast(tokens.pill) >= 4.5);
                        assert!(changed.count(dev(layout.status, scale), bgra(ink)) > 8);
                    }
                }
            }
        }
    }

    #[test]
    fn status_and_focus_stay_inside_p1_rectangles_on_constrained_work_areas() {
        let registry = ThemeRegistry::with_builtins();
        for theme in THEMES {
            for scale in SCALES {
                let layout =
                    snap_pin::layout((3000, 2000), scale, IRect::new(-320, -200, 320, 200));
                let mut state = scene(registry.get(theme).unwrap(), scale, &layout);
                let base = paint(&state, None);
                state.status = Some((
                    "Failed to save: a very long message with Unicode 日本語 and more details",
                    true,
                ));
                let changed = paint(&state, None);
                let rect = dev(layout.status, scale);
                for y in 0..base.pixels.len() as i32 / 4 / base.width {
                    for x in 0..base.width {
                        if (x as f32) < rect.left.floor()
                            || (x as f32) >= rect.right.ceil()
                            || (y as f32) < rect.top.floor()
                            || (y as f32) >= rect.bottom.ceil()
                        {
                            assert_eq!(base.at(x, y), changed.at(x, y), "status overflow");
                        }
                    }
                }
                for (action, rect) in layout.actions {
                    assert_eq!(
                        layout.hit(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0),
                        Some(snap_pin::Hit::Action(action))
                    );
                    state.focus = Some(action);
                    let focused = paint(&state, None);
                    assert_ne!(focused.pixels, changed.pixels);
                }
            }
        }
    }

    #[test]
    fn cache_is_bounded_by_p1_fit_and_small_images_are_bit_exact() {
        for scale in SCALES {
            for size in [(3840, 2160), (120, 8000), (8000, 120), (8, 6)] {
                let bitmap = bitmap(size.0, size.1);
                let image = display_image(&bitmap, scale).unwrap();
                let layout = snap_pin::layout(size, scale, IRect::new(0, 0, 32768, 32768));
                assert!((image.width() as f32 - layout.image.w * scale).abs() <= 0.5);
                assert!((image.height() as f32 - layout.image.h * scale).abs() <= 0.5);
                assert!(
                    image.width() <= bitmap.width as i32 && image.height() <= bitmap.height as i32
                );
                if size == (8, 6) {
                    let info = ImageInfo::new((8, 6), ColorType::BGRA8888, AlphaType::Opaque, None);
                    let mut pixels = vec![0; bitmap.pixels.len()];
                    assert!(image.read_pixels(
                        &info,
                        &mut pixels,
                        8 * 4,
                        (0, 0),
                        CachingHint::Disallow
                    ));
                    assert_eq!(pixels, bitmap.pixels);
                }
            }
        }
    }

    #[test]
    fn malformed_bitmaps_are_rejected_and_dpi_normalization_matches_geometry() {
        for bitmap in [
            Bitmap::default(),
            Bitmap {
                width: 1,
                height: 1,
                pixels: vec![0; 3],
            },
            Bitmap {
                width: u32::MAX,
                height: u32::MAX,
                pixels: vec![],
            },
        ] {
            assert!(display_image(&bitmap, 1.0).is_none());
        }
        for (scale, expected) in [
            (f32::NAN, 1.0),
            (f32::INFINITY, 1.0),
            (0.0, 1.0),
            (-1.0, 1.0),
            (0.1, 0.5),
            (20.0, 8.0),
        ] {
            assert_eq!(
                cache_size(3000, 2000, scale),
                cache_size(3000, 2000, expected)
            );
        }
    }
}
