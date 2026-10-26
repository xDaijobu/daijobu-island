//! Snap drawing: the full-screen overlay (its own swap chain, made per snap) and the
//! snap thumb in the island (330 x 72).
//!
//! The overlay follows plan.html's `.snapov`: the frozen screen, a 55% dim outside the
//! selection, a dashed highlight outline, the size label above the selection, and a
//! round 8x loupe with a crosshair and `#E0A458 · 812,344` under it. Before a drag, the
//! window under the cursor is cut out of the dim with a solid outline. It is drawn only
//! when the app asks (on input), never in a loop.
//!
//! Once the selection is released the overlay is in its editing phase: the selection
//! stays (solid outline, 8 handles, size label), the markup is drawn by the same painter
//! as the output, and the toolbar (plan.html `.snapov .tools`) and the HSV picker sit
//! next to it. There is no loupe while editing.

use crate::fonts::{Fonts, Weight};
use crate::gpu::{Chain, Gpu, RenderError};
use crate::icons::draw_toolbar_icon;
use crate::markup::{color, draw_markup, frect, raster, PixelCache, TextMetrics};
use crate::paint::{col, draw_text, fill_paint, fit, stroke_paint, text_width, Frame, Painter};
use dj_core::layout::{self as lay, Rect as LRect};
use dj_core::snap::{
    aspect_fit, thumb_label, Bitmap, IRect, Loupe, MonitorFrame, ThumbStatus, LOUPE_CELLS,
    LOUPE_ZOOM,
};
use dj_core::snap_edit::{
    self as ed, caret_line_col, handle_points, live_text_rect, swatch_color, text_lines,
    ColorChoice, Editor, FRect, Hsv, Item, Picker, Place, Rgba, ShapeKind, TextMeasure, Toolbar,
};
use dj_core::theme::ResolvedTokens;
use skia_safe::gpu::{self as sk_gpu, Budgeted};
use skia_safe::gradient::{self, Gradient, Interpolation};
use skia_safe::{
    surfaces, BlurStyle, Canvas, ClipOp, Color, Color4f, FilterMode, Image, MaskFilter, MipmapMode,
    Paint, PathEffect, Point, RRect, Rect, SamplingOptions, Shader, TileMode,
};

/// A linear gradient from `a` to `b` through evenly spaced `colors`.
fn linear(a: Point, b: Point, colors: &[Color]) -> Option<Shader> {
    let c: Vec<Color4f> = colors.iter().map(|c| Color4f::from(*c)).collect();
    let g = Gradient::new(
        gradient::Colors::new_evenly_spaced(&c, TileMode::Clamp, None),
        Interpolation::default(),
    );
    gradient::shaders::linear_gradient((a, b), &g, None)
}

/// A full-turn sweep gradient around `center`.
fn sweep(center: Point, colors: &[Color]) -> Option<Shader> {
    let c: Vec<Color4f> = colors.iter().map(|c| Color4f::from(*c)).collect();
    let g = Gradient::new(
        gradient::Colors::new_evenly_spaced(&c, TileMode::Clamp, None),
        Interpolation::default(),
    );
    gradient::shaders::sweep_gradient(center, (0.0, 360.0), &g, None)
}

/// The thumb keeps its copy of the shot at this size (2x the 76 x 48 box), so it stays
/// sharp up to 200% scale.
const THUMB_PX: (f32, f32) = (152.0, 96.0);
/// Gap between the cursor and the loupe, in logical px.
const LOUPE_GAP: f32 = 24.0;
/// Outside the selection: plan.html's `rgba(0,0,0,.55)`.
const DIM: f32 = 0.55;

/// What the snap thumb shows: the shot's size and the copy/save status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapFrame {
    pub width: u32,
    pub height: u32,
    pub status: ThumbStatus,
    /// Saved already: the save button reads "open" (reveal in Explorer).
    pub saved: bool,
}

impl Painter {
    /// Downscales the shot once into the thumb's small copy (mipmapped, so a big shot
    /// still looks smooth). False if Skia refused it; the thumb then shows a blank box.
    pub(crate) fn set_snap_shot(&mut self, shot: &Bitmap) -> bool {
        self.snap_shot = None;
        let Some(src) = raster(shot) else {
            return false;
        };
        let src = src.with_default_mipmaps().unwrap_or(src);
        let (w, h) = aspect_fit(shot.width, shot.height, THUMB_PX.0, THUMB_PX.1);
        let (w, h) = (w.round().max(1.0) as i32, h.round().max(1.0) as i32);
        let Some(mut surface) = surfaces::raster_n32_premul((w, h)) else {
            return false;
        };
        surface.canvas().draw_image_rect_with_sampling_options(
            &src,
            None,
            Rect::from_wh(w as f32, h as f32),
            SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear),
            &Paint::default(),
        );
        self.snap_shot = Some(surface.image_snapshot());
        true
    }

    pub(crate) fn clear_snap_shot(&mut self) {
        self.snap_shot = None;
    }

    /// The snap thumb: the shot (aspect-fit, r 8), `snap · 612×388`, the status, and the
    /// copy / save buttons.
    pub(crate) fn snap_thumb(&mut self, canvas: &Canvas, f: &Frame, rect: &Rect) {
        let (s, t) = (f.scale, f.tokens);
        let ox = rect.center_x() - lay::SNAP_W * s / 2.0;
        let oy = rect.top;
        let at = |r: LRect| Rect::from_xywh(ox + r.x * s, oy + r.y * s, r.w * s, r.h * s);

        // The shot, centred in its box on surface1.
        let boxr = at(lay::SN_SHOT);
        let rad = 8.0 * s;
        let rr = RRect::new_rect_xy(boxr, rad, rad);
        canvas.draw_rrect(rr, &fill_paint(col(t.surface1, 1.0)));
        if let (Some(img), Some(info)) = (&self.snap_shot, f.snap) {
            let (w, h) = aspect_fit(info.width, info.height, boxr.width(), boxr.height());
            let dst = Rect::from_xywh(boxr.center_x() - w / 2.0, boxr.center_y() - h / 2.0, w, h);
            canvas.save();
            canvas.clip_rrect(rr, ClipOp::Intersect, true);
            canvas.draw_image_rect_with_sampling_options(
                img,
                None,
                dst,
                SamplingOptions::new(FilterMode::Linear, MipmapMode::None),
                &Paint::default(),
            );
            canvas.restore();
        }
        canvas.draw_rrect(
            RRect::new_rect_xy(boxr.with_inset((0.5, 0.5)), rad - 0.5, rad - 0.5),
            &stroke_paint(col(t.surface2, 1.0), 1.0),
        );

        // Caption: size on the left, status on the right.
        let cap = at(lay::SN_CAPTION);
        let small = self.fonts.font(&t.font_label, Weight::Medium, 9.5 * s);
        let base = cap.center_y() + 0.35 * 9.5 * s;
        if let Some(info) = f.snap {
            let label = info.status.label();
            let ink = if info.status.is_error() {
                t.warn
            } else {
                t.accent
            };
            let lw = text_width(&small, label, 0.0);
            draw_text(
                canvas,
                &small,
                label,
                cap.right - lw,
                base,
                0.0,
                &fill_paint(col(ink, 1.0)),
            );
            let size = fit(
                &small,
                &thumb_label(info.width, info.height),
                0.0,
                cap.width() - lw - 8.0 * s,
            );
            draw_text(
                canvas,
                &small,
                &size,
                cap.left,
                base,
                0.0,
                &fill_paint(col(t.text_dim, 1.0)),
            );
        }

        // Buttons: surface2 chips; the hovered one turns accent.
        let btn = self.fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
        let hover = f.view.snap_hover;
        let save = if f.snap.is_some_and(|i| i.saved) {
            "open"
        } else {
            "save"
        };
        for (r, label, action) in [
            (lay::SN_COPY, "copy", dj_core::island::SnapAction::Copy),
            (lay::SN_SAVE, save, dj_core::island::SnapAction::Save),
        ] {
            let br = at(r);
            let on = hover == Some(action);
            let (body, ink) = if on {
                (t.accent, t.pill)
            } else {
                (t.surface2, t.text)
            };
            let rad = 7.0 * s;
            canvas.draw_rrect(
                RRect::new_rect_xy(br, rad, rad),
                &fill_paint(col(body, 1.0)),
            );
            let tracking = 0.6 * s;
            let tw = text_width(&btn, label, tracking);
            draw_text(
                canvas,
                &btn,
                label,
                br.center_x() - tw / 2.0,
                br.center_y() + 0.35 * 10.0 * s,
                tracking,
                &fill_paint(col(ink, 1.0)),
            );
        }
    }
}

/// One frame of the overlay, in desktop px (the renderer subtracts its origin).
pub struct SnapScene<'a> {
    pub tokens: &'a ResolvedTokens,
    pub cursor: (i32, i32),
    /// The live drag rect, once a drag has started.
    pub selection: Option<IRect>,
    /// Before a drag: the window (or monitor) a click would take.
    pub hover: Option<IRect>,
    pub loupe: Loupe,
    /// `#E0A458 · 812,344`.
    pub loupe_text: &'a str,
    /// `612 × 388` for the selection or the hovered window.
    pub size_text: Option<&'a str>,
    /// The monitor under the cursor: the loupe and labels stay on it.
    pub monitor: IRect,
    /// Device px per logical px on that monitor.
    pub scale: f32,
    /// The editing phase. When set, the fields above (selecting phase) are not used.
    pub edit: Option<EditScene<'a>>,
}

/// The overlay while editing: everything comes from the editor.
pub struct EditScene<'a> {
    pub editor: &'a Editor,
    /// The frozen monitors (pixelate reads them, not the canvas).
    pub frames: &'a [MonitorFrame],
    /// `612 × 388` for the selection.
    pub size_text: &'a str,
}

/// The overlay's GPU side: a swap chain the size of the virtual desktop, and the frozen
/// monitors as GPU images. Made when a snap starts, dropped when it ends.
pub struct SnapOverlay {
    // Field order is drop order; `Drop` clears the surfaces and images first.
    images: Vec<(Image, IRect)>,
    pixels: PixelCache,
    fonts: Fonts,
    metrics: TextMetrics,
    gpu: Gpu,
    chain: Chain,
    origin: (i32, i32),
}

impl SnapOverlay {
    /// `origin` and `width` x `height` are the overlay window's desktop rect.
    pub fn new(
        hwnd: isize,
        origin: (i32, i32),
        width: u32,
        height: u32,
        frames: &[MonitorFrame],
    ) -> Result<Self, RenderError> {
        let mut gpu = Gpu::new()?;
        let chain = Chain::new(&mut gpu, hwnd, width, height)?;
        let mut images = Vec::with_capacity(frames.len());
        for f in frames {
            let Some(cpu) = raster(&f.bitmap) else {
                continue;
            };
            // Upload once; the raster copy goes at the end of this iteration.
            let img = sk_gpu::images::texture_from_image(
                &mut gpu.ctx,
                &cpu,
                sk_gpu::Mipmapped::No,
                Budgeted::No,
            )
            .unwrap_or(cpu);
            images.push((img, f.rect));
        }
        let fonts = Fonts::new().map_err(RenderError::Setup)?;
        let metrics = TextMetrics::new().map_err(RenderError::Setup)?;
        Ok(Self {
            images,
            pixels: PixelCache::default(),
            fonts,
            metrics,
            gpu,
            chain,
            origin,
        })
    }

    /// Draws the scene and presents it (`Present(1)`).
    pub fn render(&mut self, scene: &SnapScene) -> Result<(), RenderError> {
        let origin = self.origin;
        // A desktop rect in window px.
        let local = move |r: IRect| {
            Rect::from_xywh(
                (r.x - origin.0) as f32,
                (r.y - origin.1) as f32,
                r.w as f32,
                r.h as f32,
            )
        };
        let canvas = self.chain.back_buffer().canvas();
        let mut ctx = SceneCtx {
            fonts: &mut self.fonts,
            metrics: &self.metrics,
            pixels: &mut self.pixels,
        };
        draw_scene(canvas, &self.images, &mut ctx, origin, scene, local);
        self.chain.present(&mut self.gpu.ctx)
    }
}

impl Drop for SnapOverlay {
    fn drop(&mut self) {
        // The frozen monitors and the wrapped buffers go first, then wait for the GPU.
        self.images.clear();
        self.pixels = PixelCache::default();
        self.chain.surfaces.clear();
        self.gpu.ctx.flush_submit_and_sync_cpu();
    }
}

/// What drawing the scene needs besides the canvas and the frozen images.
struct SceneCtx<'a> {
    fonts: &'a mut Fonts,
    metrics: &'a TextMetrics,
    pixels: &'a mut PixelCache,
}

fn draw_scene(
    canvas: &Canvas,
    images: &[(Image, IRect)],
    ctx: &mut SceneCtx,
    origin: (i32, i32),
    sc: &SnapScene,
    local: impl Fn(IRect) -> Rect,
) {
    let t = sc.tokens;
    let s = sc.scale;
    canvas.clear(Color::BLACK);
    for (img, r) in images {
        let lr = local(*r);
        canvas.draw_image(img, (lr.left, lr.top), None);
    }
    if let Some(edit) = &sc.edit {
        canvas.save();
        canvas.translate((-origin.0 as f32, -origin.1 as f32));
        draw_edit(canvas, ctx, origin, t, edit);
        canvas.restore();
        return;
    }
    let fonts = &mut *ctx.fonts;

    // Dim everything but the selection (or the window a click would take).
    let focus = sc.selection.or(sc.hover);
    let full = Rect::from_wh(
        canvas.base_layer_size().width as f32,
        canvas.base_layer_size().height as f32,
    );
    canvas.save();
    if let Some(f) = focus.filter(|f| !f.is_empty()) {
        canvas.clip_rect(local(f), ClipOp::Difference, false);
    }
    canvas.draw_rect(
        full,
        &fill_paint(Color::from_argb((DIM * 255.0) as u8, 0, 0, 0)),
    );
    canvas.restore();

    if let Some(f) = focus.filter(|f| !f.is_empty()) {
        let r = local(f);
        let w = 1.5 * s;
        let mut p = stroke_paint(col(t.highlight, 1.0), w);
        p.set_stroke_cap(skia_safe::paint::Cap::Butt);
        if sc.selection.is_some() {
            p.set_path_effect(PathEffect::dash(&[6.0 * s, 4.0 * s], 0.0));
        } else {
            p.set_stroke_width(2.0 * s);
        }
        canvas.draw_rect(r.with_inset((w / 2.0, w / 2.0)), &p);

        if let Some(text) = sc.size_text {
            let font = fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
            let tw = text_width(&font, text, 0.0);
            let (pad_x, h) = (6.0 * s, 16.0 * s);
            // Kept on the overlay; the corner may sit on another monitor than the cursor.
            let mon = full;
            let mut x = r.left;
            let mut y = r.top - h - 6.0 * s;
            if y < mon.top {
                // No room above: tuck it inside the top-left corner.
                y = r.top + 6.0 * s;
                x = r.left + 6.0 * s;
            }
            x = x.min(mon.right - tw - pad_x * 2.0).max(mon.left);
            let label = Rect::from_xywh(x, y, tw + pad_x * 2.0, h);
            let rad = 5.0 * s;
            canvas.draw_rrect(
                RRect::new_rect_xy(label, rad, rad),
                &fill_paint(col(t.pill, 1.0)),
            );
            draw_text(
                canvas,
                &font,
                text,
                label.left + pad_x,
                label.center_y() + 0.35 * 10.0 * s,
                0.0,
                &fill_paint(col(t.highlight, 1.0)),
            );
        }
    }

    loupe(canvas, images, fonts, origin, sc, &local);
}

fn rect_of(r: FRect) -> Rect {
    Rect::from_xywh(r.x, r.y, r.w, r.h)
}

/// A soft drop shadow under a floating panel.
fn shadow(canvas: &Canvas, rr: RRect, s: f32) {
    let mut p = fill_paint(Color::from_argb(120, 0, 0, 0));
    p.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 8.0 * s, false));
    canvas.draw_rrect(rr.with_offset((0.0, 4.0 * s)), &p);
}

fn dashed(c: Color, width: f32, s: f32) -> Paint {
    let mut p = stroke_paint(c, width);
    p.set_stroke_cap(skia_safe::paint::Cap::Butt);
    p.set_path_effect(PathEffect::dash(&[4.0 * s, 3.0 * s], 0.0));
    p
}

/// The editing phase, in desktop px (the canvas is already translated).
fn draw_edit(
    canvas: &Canvas,
    ctx: &mut SceneCtx,
    origin: (i32, i32),
    t: &ResolvedTokens,
    sc: &EditScene,
) {
    let e = sc.editor;
    let s = e.scale();
    let sel = e.selection();
    let sr = frect(sel);
    let size = canvas.base_layer_size();
    let full = Rect::from_xywh(
        origin.0 as f32,
        origin.1 as f32,
        size.width as f32,
        size.height as f32,
    );

    // Dim outside the selection, then the markup inside it.
    canvas.save();
    canvas.clip_rect(sr, ClipOp::Difference, false);
    canvas.draw_rect(
        full,
        &fill_paint(Color::from_argb((DIM * 255.0) as u8, 0, 0, 0)),
    );
    canvas.restore();
    let _ = draw_markup(
        canvas,
        e.shapes(),
        e.draft(),
        sel,
        sc.frames,
        ctx.pixels,
        ctx.metrics,
    );

    // The selected shape's dashed box.
    if let Some(shape) = e.selected().and_then(|i| e.shapes().get(i)) {
        let b = shape.bounds(ctx.metrics).inflate(4.0 * s);
        canvas.draw_rect(rect_of(b), &dashed(col(t.highlight, 1.0), 1.0 * s, s));
    }

    // The text being typed: a faint box around it and the caret (no blink).
    if let Some(c) = e.caret() {
        if let Some(shape) = e.shapes().get(c.shape) {
            if let (ShapeKind::Text { at, text }, Some(r)) =
                (&shape.kind, shape.text_rect(ctx.metrics))
            {
                let boxr = live_text_rect(r, shape, ctx.metrics);
                canvas.draw_rect(rect_of(boxr), &dashed(col(t.highlight, 0.7), 1.0 * s, s));
                let px = shape.font_px();
                let lh = ctx.metrics.line_height(px);
                let (line, colm) = caret_line_col(text, c.index);
                let prefix: String = text_lines(text)
                    .nth(line)
                    .unwrap_or("")
                    .chars()
                    .take(colm)
                    .collect();
                let x = at.0 + ctx.metrics.width(&prefix, px);
                let y = at.1 + line as f32 * lh;
                let mut caret = stroke_paint(color(shape.color, 1.0), (1.5 * s).max(1.0));
                caret.set_stroke_cap(skia_safe::paint::Cap::Butt);
                canvas.draw_line((x, y), (x, y + lh), &caret);
            }
        }
    }

    // The selection: a solid outline and its 8 handles.
    let w = 1.5 * s;
    let mut outline = stroke_paint(col(t.highlight, 1.0), w);
    outline.set_stroke_cap(skia_safe::paint::Cap::Butt);
    outline.set_stroke_join(skia_safe::paint::Join::Miter);
    canvas.draw_rect(sr.with_inset((w / 2.0, w / 2.0)), &outline);
    let hs = (ed::HANDLE_SIZE * s).round();
    for (hx, hy) in handle_points(sel) {
        let r = Rect::from_xywh((hx - hs / 2.0).round(), (hy - hs / 2.0).round(), hs, hs);
        canvas.draw_rect(r, &fill_paint(col(t.highlight, 1.0)));
        canvas.draw_rect(
            r.with_inset((0.5, 0.5)),
            &stroke_paint(col(t.pill, 1.0), 1.0),
        );
    }

    let bar = e.toolbar();
    let mon = frect(e.display().rect);
    size_label(canvas, ctx.fonts, t, sr, mon, bar.place, sc.size_text, s);
    draw_toolbar(canvas, ctx.fonts, t, e, &bar);
    if let Some((p, hsv)) = e.picker() {
        draw_picker(canvas, ctx.fonts, t, &p, hsv, s);
    }
}

/// `612 × 388` above the selection's top-left, or inside it when there's no room (or
/// the toolbar took the space above).
#[allow(clippy::too_many_arguments)]
fn size_label(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    r: Rect,
    mon: Rect,
    place: Place,
    text: &str,
    s: f32,
) {
    let font = fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
    let tw = text_width(&font, text, 0.0);
    let (pad_x, h) = (6.0 * s, 16.0 * s);
    let mut x = r.left;
    let mut y = r.top - h - 6.0 * s;
    if y < mon.top || place == Place::Above {
        y = r.top + 6.0 * s;
        x = r.left + 6.0 * s;
    }
    x = x.min(mon.right - tw - pad_x * 2.0).max(mon.left);
    let label = Rect::from_xywh(x, y, tw + pad_x * 2.0, h);
    let rad = 5.0 * s;
    canvas.draw_rrect(
        RRect::new_rect_xy(label, rad, rad),
        &fill_paint(col(t.pill, 1.0)),
    );
    draw_text(
        canvas,
        &font,
        text,
        label.left + pad_x,
        label.center_y() + 0.35 * 10.0 * s,
        0.0,
        &fill_paint(col(t.highlight, 1.0)),
    );
}

/// The toolbar: pill body, r 12, 26 px buttons with r 8, the active tool and size in
/// accent, the active swatch ringed in accent, the hovered button on surface2.
fn draw_toolbar(canvas: &Canvas, fonts: &mut Fonts, t: &ResolvedTokens, e: &Editor, bar: &Toolbar) {
    let s = bar.scale;
    let body = rect_of(bar.rect);
    let rad = ed::RADIUS * s;
    let rr = RRect::new_rect_xy(body, rad, rad);
    shadow(canvas, rr, s);
    canvas.draw_rrect(rr, &fill_paint(col(t.pill, 1.0)));
    canvas.draw_rrect(
        RRect::new_rect_xy(body.with_inset((0.5, 0.5)), rad - 0.5, rad - 0.5),
        &stroke_paint(col(t.surface2, 1.0), 1.0),
    );
    for r in &bar.rules {
        canvas.draw_rect(rect_of(*r), &fill_paint(col(t.surface2, 1.0)));
    }
    let size_font = fonts.font(&t.font_label, Weight::SemiBold, 12.0 * s);
    for (item, r) in &bar.buttons {
        let br = rect_of(*r);
        let (cx, cy) = r.center();
        let filled = match item {
            Item::Tool(tool) => *tool == e.tool(),
            Item::Size(z) => *z == e.size(),
            _ => false,
        };
        let enabled = match item {
            Item::Undo => e.can_undo(),
            Item::Redo => e.can_redo(),
            _ => true,
        };
        let hovered = e.hover() == Some(*item) && enabled;
        let brad = 8.0 * s;
        if filled {
            canvas.draw_rrect(
                RRect::new_rect_xy(br, brad, brad),
                &fill_paint(col(t.accent, 1.0)),
            );
        } else if hovered {
            canvas.draw_rrect(
                RRect::new_rect_xy(br, brad, brad),
                &fill_paint(col(t.surface2, 1.0)),
            );
        }
        let ink = if filled {
            col(t.pill, 1.0)
        } else if enabled {
            col(t.text, 1.0)
        } else {
            col(t.text_dim, 0.45)
        };
        match item {
            Item::Swatch(i) => {
                let c = swatch_color(*i, e.accent());
                swatch(canvas, t, c, cx, cy, s);
                if e.color_choice() == ColorChoice::Swatch(*i) {
                    ring(canvas, t, cx, cy, s);
                }
            }
            Item::Custom => {
                // The custom colour inside a thin hue ring.
                let hues = [
                    Color::from_rgb(255, 0, 0),
                    Color::from_rgb(255, 255, 0),
                    Color::from_rgb(0, 255, 0),
                    Color::from_rgb(0, 255, 255),
                    Color::from_rgb(0, 0, 255),
                    Color::from_rgb(255, 0, 255),
                    Color::from_rgb(255, 0, 0),
                ];
                let mut p = stroke_paint(Color::WHITE, 2.0 * s);
                p.set_shader(sweep(Point::new(cx, cy), &hues));
                canvas.draw_circle((cx, cy), 8.0 * s, &p);
                canvas.draw_circle((cx, cy), 5.0 * s, &fill_paint(color(e.custom(), 1.0)));
                if e.color_choice() == ColorChoice::Custom {
                    ring(canvas, t, cx, cy, s);
                }
            }
            Item::Size(z) => {
                let label = z.name();
                let tw = text_width(&size_font, label, 0.0);
                draw_text(
                    canvas,
                    &size_font,
                    label,
                    cx - tw / 2.0,
                    cy + 0.36 * 12.0 * s,
                    0.0,
                    &fill_paint(ink),
                );
            }
            _ => {
                let mut p = stroke_paint(ink, 1.5 * s);
                p.set_stroke_join(skia_safe::paint::Join::Round);
                draw_toolbar_icon(canvas, *item, cx, cy, 16.0 * s, &p);
            }
        }
    }
}

/// A colour dot with a faint edge, so white and black show on the pill.
fn swatch(canvas: &Canvas, t: &ResolvedTokens, c: Rgba, cx: f32, cy: f32, s: f32) {
    canvas.draw_circle((cx, cy), 6.5 * s, &fill_paint(color(c, 1.0)));
    canvas.draw_circle((cx, cy), 6.5 * s, &stroke_paint(col(t.surface2, 1.0), 1.0));
}

/// The active swatch's accent ring.
fn ring(canvas: &Canvas, t: &ResolvedTokens, cx: f32, cy: f32, s: f32) {
    canvas.draw_circle(
        (cx, cy),
        10.5 * s,
        &stroke_paint(col(t.accent, 1.0), 1.5 * s),
    );
}

/// The HSV picker: the saturation/value square for the current hue, the hue strip, a
/// preview of the colour and its hex.
fn draw_picker(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    p: &Picker,
    hsv: Hsv,
    s: f32,
) {
    let body = rect_of(p.rect);
    let rad = ed::RADIUS * s;
    let rr = RRect::new_rect_xy(body, rad, rad);
    shadow(canvas, rr, s);
    canvas.draw_rrect(rr, &fill_paint(col(t.pill, 1.0)));
    canvas.draw_rrect(
        RRect::new_rect_xy(body.with_inset((0.5, 0.5)), rad - 0.5, rad - 0.5),
        &stroke_paint(col(t.surface2, 1.0), 1.0),
    );

    // Saturation left to right, value top to bottom, over the pure hue.
    let sv = rect_of(p.sv);
    let pure = Hsv {
        h: hsv.h,
        s: 1.0,
        v: 1.0,
    }
    .to_rgba();
    canvas.draw_rect(sv, &fill_paint(color(pure, 1.0)));
    let mut white = Paint::default();
    white.set_shader(linear(
        Point::new(sv.left, sv.top),
        Point::new(sv.right, sv.top),
        &[Color::WHITE, Color::from_argb(0, 255, 255, 255)],
    ));
    canvas.draw_rect(sv, &white);
    let mut black = Paint::default();
    black.set_shader(linear(
        Point::new(sv.left, sv.top),
        Point::new(sv.left, sv.bottom),
        &[Color::from_argb(0, 0, 0, 0), Color::BLACK],
    ));
    canvas.draw_rect(sv, &black);
    let (mx, my) = p.sv_point(hsv.s, hsv.v);
    canvas.draw_circle((mx, my), 5.0 * s, &stroke_paint(Color::BLACK, 3.0 * s));
    canvas.draw_circle((mx, my), 5.0 * s, &stroke_paint(Color::WHITE, 1.5 * s));

    // The hue strip, top (0°) to bottom (360°).
    let hue = rect_of(p.hue);
    let stops: Vec<Color> = (0..=6)
        .map(|i| {
            let c = Hsv {
                h: i as f32 * 60.0,
                s: 1.0,
                v: 1.0,
            }
            .to_rgba();
            color(c, 1.0)
        })
        .collect();
    let mut strip = Paint::default();
    strip.set_shader(linear(
        Point::new(hue.left, hue.top),
        Point::new(hue.left, hue.bottom),
        &stops,
    ));
    let hr = 4.0 * s;
    canvas.draw_rrect(RRect::new_rect_xy(hue, hr, hr), &strip);
    let hy = p.hue_y(hsv.h);
    let marker = Rect::from_xywh(
        hue.left - 2.0 * s,
        hy - 2.5 * s,
        hue.width() + 4.0 * s,
        5.0 * s,
    );
    canvas.draw_rrect(
        RRect::new_rect_xy(marker, 2.0 * s, 2.0 * s),
        &stroke_paint(Color::BLACK, 3.0 * s),
    );
    canvas.draw_rrect(
        RRect::new_rect_xy(marker, 2.0 * s, 2.0 * s),
        &stroke_paint(Color::WHITE, 1.5 * s),
    );

    // Preview and hex.
    let c = hsv.to_rgba();
    let pv = rect_of(p.preview);
    let prad = 6.0 * s;
    canvas.draw_rrect(
        RRect::new_rect_xy(pv, prad, prad),
        &fill_paint(color(c, 1.0)),
    );
    canvas.draw_rrect(
        RRect::new_rect_xy(pv.with_inset((0.5, 0.5)), prad, prad),
        &stroke_paint(col(t.surface2, 1.0), 1.0),
    );
    let font = fonts.font(&t.font_label, Weight::Medium, 11.0 * s);
    let hex = rect_of(p.hex);
    draw_text(
        canvas,
        &font,
        &c.to_hex(),
        hex.left,
        hex.center_y() + 0.35 * 11.0 * s,
        0.0,
        &fill_paint(col(t.text, 1.0)),
    );
}

/// The round loupe next to the cursor, flipped to the other side near the monitor's
/// edges, with the colour and position under it.
fn loupe(
    canvas: &Canvas,
    images: &[(Image, IRect)],
    fonts: &mut Fonts,
    origin: (i32, i32),
    sc: &SnapScene,
    local: &impl Fn(IRect) -> Rect,
) {
    let t = sc.tokens;
    let s = sc.scale;
    let cell = LOUPE_ZOOM as f32 * s;
    let d = LOUPE_CELLS as f32 * cell;
    let gap = LOUPE_GAP * s;
    let caption_h = 22.0 * s;
    let mon = local(sc.monitor);
    let (cx, cy) = (
        (sc.cursor.0 - origin.0) as f32,
        (sc.cursor.1 - origin.1) as f32,
    );
    let mut x = cx + gap;
    if x + d > mon.right {
        x = cx - gap - d;
    }
    let mut y = cy + gap;
    if y + d + caption_h > mon.bottom {
        y = cy - gap - d - caption_h;
    }
    let area = Rect::from_xywh(x, y, d, d);
    let circle = RRect::new_oval(area);

    // A soft shadow, then the magnified pixels clipped to the circle.
    let mut shadow = fill_paint(Color::from_argb(150, 0, 0, 0));
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 8.0 * s, false));
    canvas.draw_rrect(RRect::new_oval(area.with_offset((0.0, 6.0 * s))), &shadow);
    canvas.save();
    canvas.clip_rrect(circle, ClipOp::Intersect, true);
    canvas.draw_rect(area, &fill_paint(Color::BLACK));
    canvas.save();
    canvas.translate((x, y));
    canvas.scale((cell, cell));
    canvas.translate((-(sc.loupe.origin.0 as f32), -(sc.loupe.origin.1 as f32)));
    // Only the 15 x 15 grid is visible; clip to it so nothing else is sampled.
    canvas.clip_rect(
        Rect::from_xywh(
            sc.loupe.origin.0 as f32,
            sc.loupe.origin.1 as f32,
            LOUPE_CELLS as f32,
            LOUPE_CELLS as f32,
        ),
        ClipOp::Intersect,
        false,
    );
    let nearest = SamplingOptions::new(FilterMode::Nearest, MipmapMode::None);
    for (img, r) in images {
        canvas.draw_image_with_sampling_options(img, (r.x as f32, r.y as f32), nearest, None);
    }
    canvas.restore();

    // Crosshair: the cursor's row and column, faint, and its cell outlined.
    let (ccx, ccy) = sc.loupe.cursor_cell;
    let cell_r = Rect::from_xywh(x + ccx as f32 * cell, y + ccy as f32 * cell, cell, cell);
    let faint = fill_paint(col(t.highlight, 0.18));
    canvas.draw_rect(Rect::from_xywh(x, cell_r.top, d, cell), &faint);
    canvas.draw_rect(Rect::from_xywh(cell_r.left, y, cell, d), &faint);
    canvas.restore();
    let mut edge = stroke_paint(col(t.highlight, 1.0), 1.0 * s);
    edge.set_stroke_cap(skia_safe::paint::Cap::Butt);
    canvas.draw_rect(cell_r.with_inset((0.5 * s, 0.5 * s)), &edge);
    canvas.draw_rrect(circle, &stroke_paint(col(t.highlight, 1.0), 2.0 * s));

    // Caption under the loupe.
    let font = fonts.font(&t.font_label, Weight::Medium, 9.0 * s);
    let tw = text_width(&font, sc.loupe_text, 0.0);
    let (pad_x, h) = (6.0 * s, 16.0 * s);
    let label = Rect::from_xywh(
        area.center_x() - tw / 2.0 - pad_x,
        area.bottom + 6.0 * s,
        tw + pad_x * 2.0,
        h,
    );
    let rad = 5.0 * s;
    canvas.draw_rrect(
        RRect::new_rect_xy(label, rad, rad),
        &fill_paint(col(t.pill, 1.0)),
    );
    draw_text(
        canvas,
        &font,
        sc.loupe_text,
        label.left + pad_x,
        label.center_y() + 0.35 * 9.0 * s,
        0.0,
        &fill_paint(col(t.text, 1.0)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::snap::Selector;
    use dj_core::theme::{ThemeRegistry, DEFAULT_THEME};
    use skia_safe::image::CachingHint;
    use skia_safe::{AlphaType, ColorType, ImageInfo};

    fn tokens() -> ResolvedTokens {
        ThemeRegistry::with_builtins()
            .get(DEFAULT_THEME)
            .unwrap()
            .clone()
    }

    /// An opaque bitmap of one colour (BGRA).
    fn solid(w: u32, h: u32, bgra: [u8; 4]) -> Bitmap {
        Bitmap {
            width: w,
            height: h,
            pixels: bgra.repeat(w as usize * h as usize),
        }
    }

    /// Draws a scene onto a CPU surface the way the overlay does.
    fn draw_cpu(
        surface: &mut skia_safe::Surface,
        images: &[(Image, IRect)],
        origin: (i32, i32),
        scene: &SnapScene,
    ) {
        let mut fonts = Fonts::new().unwrap();
        let metrics = TextMetrics::new().unwrap();
        let mut pixels = PixelCache::default();
        let mut ctx = SceneCtx {
            fonts: &mut fonts,
            metrics: &metrics,
            pixels: &mut pixels,
        };
        draw_scene(surface.canvas(), images, &mut ctx, origin, scene, |r| {
            Rect::from_xywh(
                (r.x - origin.0) as f32,
                (r.y - origin.1) as f32,
                r.w as f32,
                r.h as f32,
            )
        });
    }

    fn read(surface: &mut skia_safe::Surface, x: i32, y: i32) -> [u8; 4] {
        let info = ImageInfo::new((1, 1), ColorType::BGRA8888, AlphaType::Premul, None);
        let mut px = [0u8; 4];
        let img = surface.image_snapshot();
        assert!(img.read_pixels(&info, &mut px, 4, (x, y), CachingHint::Disallow));
        px
    }

    #[test]
    fn the_thumb_copy_is_aspect_fit_at_twice_the_box() {
        let mut p = Painter::new().unwrap();
        assert!(p.set_snap_shot(&solid(612, 388, [10, 20, 30, 255])));
        let img = p.snap_shot.clone().unwrap();
        assert_eq!((img.width(), img.height()), (151, 96));
        assert!(p.set_snap_shot(&solid(3000, 20, [0, 0, 0, 255])));
        let img = p.snap_shot.clone().unwrap();
        assert_eq!((img.width(), img.height()), (152, 1));
        p.clear_snap_shot();
        assert!(p.snap_shot.is_none());
    }

    /// Draws the overlay onto a CPU surface (no GPU, no screen): the frame shows
    /// through inside the selection and is dimmed by about 55% outside it.
    #[test]
    fn the_overlay_dims_outside_the_selection_only() {
        let t = tokens();
        // Two monitors side by side, the left one at a negative x.
        let frames = [
            MonitorFrame {
                rect: IRect::new(-200, 0, 200, 150),
                dpi: 96,
                bitmap: solid(200, 150, [200, 200, 200, 255]),
            },
            MonitorFrame {
                rect: IRect::new(0, 0, 300, 150),
                dpi: 96,
                bitmap: solid(300, 150, [200, 200, 200, 255]),
            },
        ];
        let images: Vec<(Image, IRect)> = frames
            .iter()
            .map(|f| (raster(&f.bitmap).unwrap(), f.rect))
            .collect();
        let origin = (-200, 0);
        let desktop = IRect::new(-200, 0, 500, 150);
        let mut sel = Selector::new(desktop, vec![], (-150, 20));
        sel.press(-150, 20);
        sel.move_to(50, 100);
        let selection = sel.selection();
        assert_eq!(selection, Some(IRect::new(-150, 20, 200, 80)));
        let cursor = (280, 140);
        let scene = SnapScene {
            tokens: &t,
            cursor,
            selection,
            hover: None,
            loupe: Loupe::at(cursor, desktop),
            loupe_text: "#C8C8C8 · 280,140",
            size_text: Some("200 × 80"),
            monitor: IRect::new(0, 0, 300, 150),
            scale: 1.0,
            edit: None,
        };
        let mut surface = surfaces::raster_n32_premul((500, 150)).unwrap();
        draw_cpu(&mut surface, &images, origin, &scene);
        // Inside the selection, across the monitor seam: untouched.
        assert_eq!(read(&mut surface, 100, 60), [200, 200, 200, 255]);
        assert_eq!(read(&mut surface, 220, 60), [200, 200, 200, 255]);
        // Outside: 200 * 0.45 = 90.
        let dim = read(&mut surface, 10, 140);
        assert!(dim[0].abs_diff(90) <= 2 && dim[3] == 255, "{dim:?}");
        let dim = read(&mut surface, 300, 10);
        assert!(dim[0].abs_diff(90) <= 2, "{dim:?}");
    }

    fn bgra(c: Color) -> [u8; 4] {
        [c.b(), c.g(), c.r(), c.a()]
    }

    /// The editing phase on a CPU surface: the selection shows the frame and the markup,
    /// outside is dimmed, and the handles and the toolbar are drawn; no loupe.
    #[test]
    fn the_editing_scene_draws_markup_handles_and_the_toolbar() {
        use dj_core::snap_edit::{Display, EditInput, Prefs, Tool};
        let t = tokens();
        let mon = IRect::new(0, 0, 800, 500);
        let frames = [MonitorFrame {
            rect: mon,
            dpi: 96,
            bitmap: solid(800, 500, [200, 200, 200, 255]),
        }];
        let images: Vec<(Image, IRect)> = frames
            .iter()
            .map(|f| (raster(&f.bitmap).unwrap(), f.rect))
            .collect();
        let metrics = TextMetrics::new().unwrap();
        let mut e = Editor::new(
            IRect::new(100, 100, 300, 200),
            mon,
            vec![Display {
                rect: mon,
                scale: 1.0,
            }],
            Prefs {
                tool: Tool::Rect,
                ..Prefs::default()
            },
            Rgba::from_rgb(t.accent),
            (0, 0),
        );
        for ev in [
            EditInput::Press { x: 150, y: 150 },
            EditInput::Motion { x: 250, y: 250 },
            EditInput::Release { x: 250, y: 250 },
            EditInput::Motion { x: 700, y: 50 },
        ] {
            e.input(ev, &metrics);
        }
        assert_eq!(e.shapes().len(), 1);
        let scene = SnapScene {
            tokens: &t,
            cursor: (700, 50),
            selection: None,
            hover: None,
            loupe: Loupe::at((700, 50), mon),
            loupe_text: "",
            size_text: None,
            monitor: mon,
            scale: 1.0,
            edit: Some(EditScene {
                editor: &e,
                frames: &frames,
                size_text: "300 × 200",
            }),
        };
        let mut surface = surfaces::raster_n32_premul((800, 500)).unwrap();
        draw_cpu(&mut surface, &images, (0, 0), &scene);
        // Inside the selection, off the markup: the frame as it was.
        assert_eq!(read(&mut surface, 300, 280), [200, 200, 200, 255]);
        // The rect's left edge, in red.
        let red = dj_core::snap_edit::SWATCHES[0];
        assert_eq!(read(&mut surface, 150, 200), [red.b, red.g, red.r, 255]);
        // Outside, away from the toolbar: dimmed, and no loupe at the cursor.
        let dim = read(&mut surface, 700, 50);
        assert!(dim[0].abs_diff(90) <= 2, "{dim:?}");
        let dim = read(&mut surface, 730, 80);
        assert!(dim[0].abs_diff(90) <= 2, "no loupe: {dim:?}");
        // The top-left handle.
        assert_eq!(read(&mut surface, 99, 99), bgra(col(t.highlight, 1.0)));
        // The toolbar (two rows: one row is wider than this 800 px monitor) with the
        // active tool in accent.
        let bar = e.toolbar();
        assert_eq!(bar.rows, 2);
        let body = read(
            &mut surface,
            bar.rect.x as i32 + 3,
            bar.rect.center().1 as i32,
        );
        assert_eq!(body, bgra(col(t.pill, 1.0)));
        let b = bar.button(Item::Tool(Tool::Rect)).unwrap();
        let on = read(&mut surface, b.x as i32 + 3, b.center().1 as i32);
        assert_eq!(on, bgra(col(t.accent, 1.0)));
    }
}
