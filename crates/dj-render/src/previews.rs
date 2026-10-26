//! The Snap preview stack: the floating cards under the island, their selection bar and
//! the "Preview stack full" notice, drawn on their own transparent window.
//!
//! The surface is a second D3D12 + DirectComposition chain (like `snap::SnapOverlay`),
//! sized once to the largest layout in device px. It draws only when the app calls
//! `render` (input, a spring step, a countdown tick), never in a loop.
//!
//! Cards follow plan.html's `.shot`: 124 x 80, r 12, a 1 px hairline, aspect-fit thumbnail,
//! an 18 px check box top-left, a 16 px countdown ring top-right and a row of action
//! chips on hover. Selected, pinned, focused and failed cards differ by shape or text,
//! not by colour alone: a drawn check mark, a "PIN" badge in place of the ring, a dashed
//! focus ring outside the selection outline, and the status text itself.
//!
//! Everything is laid out by `dj-core` in logical px; this module only multiplies by the
//! scene's `scale`. Card-local rects (`check`, `ring`, `status_rect`, the actions) are
//! relative to the card's top-left, the rest are window coordinates. Nothing is drawn
//! more than 9 logical px outside a card, except the shadow, which reaches about 11 px
//! below it, so the window's transparent margin (16 px) holds the shadow, the selection
//! outline and the focus ring even with a hover lift.

use crate::fonts::{Fonts, Weight};
use crate::gpu::{Chain, Gpu, RenderError};
use crate::markup::raster;
use crate::paint::{col, draw_text, fill_paint, fit, stroke_paint, text_width};
use dj_core::layout::Rect as LRect;
use dj_core::snap::{aspect_fit, Bitmap};
use dj_core::theme::ResolvedTokens;
use skia_safe::gpu::{self as sk_gpu, Budgeted};
use skia_safe::paint::Cap;
use skia_safe::{
    surfaces, BlurStyle, Canvas, ClipOp, Color, FilterMode, Font, Image, MaskFilter, MipmapMode,
    Paint, PathEffect, RRect, Rect, SamplingOptions,
};
use std::collections::HashMap;

// Mirrors dj_core::snap_stack::{CARD_W, CARD_H}; the integrator swaps them.
const CARD_W: f32 = 124.0;
const CARD_H: f32 = 80.0;
/// Mirrors dj_core::snap_stack::CARD_R.
const CARD_R: f32 = 12.0;

/// The card's body: `.shot`'s border is `color-mix(t1 14%)`; hover brightens it.
const BORDER: f32 = 0.14;
const BORDER_HOVER: f32 = 0.32;

/// One frame of the stack, in window logical px.
#[derive(Clone, Copy, Debug)]
pub struct PreviewScene<'a> {
    pub tokens: &'a ResolvedTokens,
    /// Device px per logical px.
    pub scale: f32,
    /// Back to front.
    pub cards: &'a [PreviewCard<'a>],
    pub bar: Option<PreviewBar<'a>>,
    /// The notice chip and its text (`Preview stack full`).
    pub notice: Option<(LRect, &'a str)>,
}

/// One card. `rect` is where the (animated) card sits; `alpha` and `scale` are the pop
/// in/out, applied about the card's centre. The rects below it are relative to the
/// card's top-left corner and are not affected by `scale` (the canvas transform is).
#[derive(Clone, Copy, Debug)]
pub struct PreviewCard<'a> {
    pub id: u64,
    pub rect: LRect,
    pub alpha: f32,
    pub scale: f32,
    /// The countdown ring's remaining fraction, 0..=1. `None`: no ring (pinned).
    pub remaining: Option<f32>,
    pub selected: bool,
    pub pinned: bool,
    pub hovered: bool,
    /// Keyboard focus.
    pub focused: bool,
    pub busy: bool,
    /// An empty check box when not selected (hover, or any selection).
    pub show_check: bool,
    pub show_actions: bool,
    /// The status chip: its label and whether it is an error.
    pub status: Option<(&'a str, bool)>,
    pub check: LRect,
    pub ring: LRect,
    pub status_rect: LRect,
    pub actions: [PreviewButton<'a>; 4],
}

/// A labelled chip: a card action (rect relative to the card) or a bar button (absolute).
#[derive(Clone, Copy, Debug)]
pub struct PreviewButton<'a> {
    pub rect: LRect,
    pub label: &'a str,
    /// Keyboard focus: drawn with an outline.
    pub focused: bool,
    /// Accent chip (pill-coloured text on accent).
    pub primary: bool,
}

/// The selection bar: the count and the batch buttons. All rects are window coordinates;
/// nothing is drawn outside `rect`.
#[derive(Clone, Copy, Debug)]
pub struct PreviewBar<'a> {
    pub rect: LRect,
    pub count: (LRect, &'a str),
    pub buttons: &'a [PreviewButton<'a>],
}

/// The stack's GPU side: a swap chain sized once for the largest layout and one small
/// image per card (downscaled when it arrives; the shot itself is not kept).
pub struct PreviewSurface {
    // Field order is drop order; `Drop` clears the images and surfaces first.
    images: HashMap<u64, Image>,
    fonts: Fonts,
    gpu: Gpu,
    chain: Chain,
    width: u32,
    height: u32,
}

impl PreviewSurface {
    /// `width` x `height` is the window's client size in device px.
    pub fn new(hwnd: isize, width: u32, height: u32) -> Result<Self, RenderError> {
        let fonts = Fonts::new().map_err(RenderError::Setup)?;
        let mut gpu = Gpu::new()?;
        let chain = Chain::new(&mut gpu, hwnd, width, height)?;
        Ok(Self {
            images: HashMap::new(),
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

    /// Downscales the shot once to the card's aspect-fit box at `scale` (device px per
    /// logical px), uploads that small copy and caches it under `id`. The full-size
    /// pixels are not kept. False if Skia refused the bitmap; the card then shows a
    /// blank thumbnail.
    pub fn set_image(&mut self, id: u64, bitmap: &Bitmap, scale: f32) -> bool {
        self.images.remove(&id);
        let Some(small) = shrink(bitmap, scale) else {
            return false;
        };
        // Upload once; the raster copy goes at the end of this call.
        let img = sk_gpu::images::texture_from_image(
            &mut self.gpu.ctx,
            &small,
            sk_gpu::Mipmapped::No,
            Budgeted::No,
        )
        .unwrap_or(small);
        self.images.insert(id, img);
        true
    }

    pub fn has_image(&self, id: u64) -> bool {
        self.images.contains_key(&id)
    }

    pub fn drop_image(&mut self, id: u64) {
        self.images.remove(&id);
    }

    pub fn clear_images(&mut self) {
        self.images.clear();
    }

    /// Clears to transparent, draws the scene and presents it (`Present(1)`).
    pub fn render(&mut self, scene: &PreviewScene) -> Result<(), RenderError> {
        let canvas = self.chain.back_buffer().canvas();
        canvas.clear(Color::TRANSPARENT);
        draw_scene(canvas, &mut self.fonts, &self.images, scene);
        self.chain.present(&mut self.gpu.ctx)
    }
}

impl Drop for PreviewSurface {
    fn drop(&mut self) {
        // The textures and the wrapped buffers go first, then wait for the GPU.
        self.images.clear();
        self.chain.surfaces.clear();
        self.gpu.ctx.flush_submit_and_sync_cpu();
    }
}

/// The size of the cached copy of a `w` x `h` shot at `scale`: the aspect-fit box of a
/// card in device px, rounded, at least 1 px. A shot that already fits is kept as it is
/// (the draw scales it up), never resampled.
fn cache_size(w: u32, h: u32, scale: f32) -> (i32, i32) {
    let scale = sane(scale);
    let (bw, bh) = (CARD_W * scale, CARD_H * scale);
    if w as f32 <= bw && h as f32 <= bh {
        return (w.max(1) as i32, h.max(1) as i32);
    }
    let (fw, fh) = aspect_fit(w, h, bw, bh);
    (fw.round().max(1.0) as i32, fh.round().max(1.0) as i32)
}

/// The shot's small copy: a CPU image of `cache_size`, filtered through mipmaps so a
/// 4K shot still looks smooth at 124 x 70. None if Skia refused the bitmap.
fn shrink(bitmap: &Bitmap, scale: f32) -> Option<Image> {
    if bitmap.width == 0 || bitmap.height == 0 {
        return None;
    }
    let src = raster(bitmap)?;
    let (w, h) = cache_size(bitmap.width, bitmap.height, scale);
    if (w, h) == (src.width(), src.height()) {
        return Some(src);
    }
    let src = src.with_default_mipmaps().unwrap_or(src);
    let mut surface = surfaces::raster_n32_premul((w, h))?;
    surface.canvas().draw_image_rect_with_sampling_options(
        &src,
        None,
        Rect::from_wh(w as f32, h as f32),
        SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear),
        &Paint::default(),
    );
    Some(surface.image_snapshot())
}

/// A usable device scale: finite and positive, else 1.
fn sane(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// A logical rect in device px.
fn dev(r: LRect, s: f32) -> Rect {
    Rect::from_xywh(r.x * s, r.y * s, r.w * s, r.h * s)
}

/// A card-local logical rect in device px, from the card's device top-left.
fn rel(card: &Rect, r: LRect, s: f32) -> Rect {
    Rect::from_xywh(card.left + r.x * s, card.top + r.y * s, r.w * s, r.h * s)
}

/// `rect` grown by `by` on every side, with the corner radius grown with it.
fn grown(rect: Rect, rad: f32, by: f32) -> RRect {
    RRect::new_rect_xy(rect.with_outset((by, by)), rad + by, rad + by)
}

fn draw_scene(canvas: &Canvas, fonts: &mut Fonts, images: &HashMap<u64, Image>, sc: &PreviewScene) {
    let s = sane(sc.scale);
    for card in sc.cards {
        draw_card(canvas, fonts, images.get(&card.id), sc.tokens, s, card);
    }
    if let Some(bar) = &sc.bar {
        draw_bar(canvas, fonts, sc.tokens, s, bar);
    }
    if let Some((rect, text)) = sc.notice {
        draw_notice(canvas, fonts, sc.tokens, s, rect, text);
    }
}

/// `text` centred in `rect`, baseline from the font size. A lone dismiss glyph is drawn
/// as two strokes when the font has no glyph for it, so it never shows as a box.
fn label_in(
    canvas: &Canvas,
    font: &Font,
    size: f32,
    text: &str,
    rect: &Rect,
    tracking: f32,
    ink: Color,
) {
    let mut chars = text.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if matches!(c, '✕' | '✖' | '×') && font.typeface().unichar_to_glyph(c as i32) == 0 {
            let d = size * 0.3;
            let (cx, cy) = (rect.center_x(), rect.center_y());
            let p = stroke_paint(ink, (size * 0.14).max(1.0));
            canvas.draw_line((cx - d, cy - d), (cx + d, cy + d), &p);
            canvas.draw_line((cx - d, cy + d), (cx + d, cy - d), &p);
            return;
        }
    }
    let tw = text_width(font, text, tracking);
    draw_text(
        canvas,
        font,
        text,
        rect.center_x() - tw / 2.0,
        rect.center_y() + 0.35 * size,
        tracking,
        &fill_paint(ink),
    );
}

fn draw_card(
    canvas: &Canvas,
    fonts: &mut Fonts,
    img: Option<&Image>,
    t: &ResolvedTokens,
    s: f32,
    c: &PreviewCard,
) {
    let alpha = c.alpha.clamp(0.0, 1.0);
    let zoom = c.scale.max(0.0);
    if alpha <= 0.0 || zoom <= 0.0 {
        return;
    }
    let r = dev(c.rect, s);
    let rad = CARD_R * s;
    let rr = RRect::new_rect_xy(r, rad, rad);
    let base = canvas.save();
    if zoom != 1.0 {
        canvas.translate((r.center_x(), r.center_y()));
        canvas.scale((zoom, zoom));
        canvas.translate((-r.center_x(), -r.center_y()));
    }
    if alpha < 1.0 {
        // The whole card (shadow and outlines too) fades as one.
        canvas.save_layer_alpha_f(r.with_outset((16.0 * s, 16.0 * s)), alpha);
    }

    // A soft drop shadow, mostly below: the blur stays inside the window's margin
    // (about 11 px under, about 3 px to the sides) and the 10 px gap between cards
    // stays clear.
    let mut sp = fill_paint(Color::from_argb(170, 0, 0, 0));
    sp.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 3.0 * s, false));
    canvas.draw_rrect(
        RRect::new_rect_xy(
            r.with_inset((4.0 * s, 4.0 * s)).with_offset((0.0, 6.0 * s)),
            rad - 4.0 * s,
            rad - 4.0 * s,
        ),
        &sp,
    );

    canvas.save();
    canvas.clip_rrect(rr, ClipOp::Intersect, true);
    canvas.draw_rect(r, &fill_paint(col(t.pill, 1.0)));
    if let Some(img) = img {
        let (w, h) = aspect_fit(
            img.width().max(1) as u32,
            img.height().max(1) as u32,
            r.width(),
            r.height(),
        );
        let dst = Rect::from_xywh(r.center_x() - w / 2.0, r.center_y() - h / 2.0, w, h);
        canvas.draw_image_rect_with_sampling_options(
            img,
            None,
            dst,
            SamplingOptions::new(FilterMode::Linear, MipmapMode::None),
            &Paint::default(),
        );
    }
    if c.busy {
        busy_mark(canvas, t, r, s);
    }
    status_and_ring(canvas, fonts, t, s, c, &r);
    if c.selected || c.show_check {
        check_box(canvas, t, rel(&r, c.check, s), s, c.selected);
    }
    if c.show_actions || c.actions.iter().any(|a| a.focused) {
        let font = fonts.font(&t.font_label, Weight::Medium, 9.0 * s);
        for a in &c.actions {
            card_button(canvas, &font, t, s, a, rel(&r, a.rect, s));
        }
    }
    canvas.restore();

    // The hairline over the clipped content.
    let line = if c.hovered { BORDER_HOVER } else { BORDER };
    canvas.draw_rrect(
        RRect::new_rect_xy(
            r.with_inset((0.5 * s, 0.5 * s)),
            rad - 0.5 * s,
            rad - 0.5 * s,
        ),
        &stroke_paint(col(t.text, line), 1.0 * s),
    );

    // Selected: a solid accent outline, 2 px wide and 2 px off the card (the mock's
    // `outline: 2px; outline-offset: 2px`).
    if c.selected {
        canvas.draw_rrect(
            grown(r, rad, 3.0 * s),
            &stroke_paint(col(t.accent, 1.0), 2.0 * s),
        );
    }
    // Keyboard focus: a dashed ring in the text colour on a pill halo, outside the
    // selection outline, so it reads on any desktop and differs from selection by shape.
    if c.focused {
        let ring = grown(r, rad, 6.5 * s);
        canvas.draw_rrect(ring, &stroke_paint(col(t.pill, 1.0), 3.5 * s));
        let mut dash = stroke_paint(col(t.text, 1.0), 1.5 * s);
        dash.set_stroke_cap(Cap::Butt);
        dash.set_path_effect(PathEffect::dash(&[5.0 * s, 3.0 * s], 0.0));
        canvas.draw_rrect(ring, &dash);
    }
    canvas.restore_to_count(base);
}

/// A busy card (copying or saving): dimmed, with three dots over the thumbnail.
fn busy_mark(canvas: &Canvas, t: &ResolvedTokens, r: Rect, s: f32) {
    canvas.draw_rect(r, &fill_paint(col(t.pill, 0.45)));
    let dot = fill_paint(col(t.text, 1.0));
    for i in -1..=1 {
        canvas.draw_circle(
            (r.center_x() + i as f32 * 7.0 * s, r.center_y()),
            1.7 * s,
            &dot,
        );
    }
}

/// The status chip and the countdown ring or "PIN" badge in the top row.
fn status_and_ring(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    s: f32,
    c: &PreviewCard,
    card: &Rect,
) {
    let ring = rel(card, c.ring, s);
    // Where the badge (if any) starts, so the status chip keeps clear of it.
    let mut right = f32::INFINITY;
    if c.pinned {
        let font = fonts.font(&t.font_label, Weight::Medium, 8.0 * s);
        let tracking = 0.04 * 8.0 * s;
        let tw = text_width(&font, "PIN", tracking);
        let w = tw + 6.0 * s;
        let h = 12.0 * s;
        let badge = Rect::from_xywh(ring.right - w, ring.center_y() - h / 2.0, w, h);
        right = badge.left - 2.0 * s;
        canvas.draw_rrect(
            RRect::new_rect_xy(badge, 4.0 * s, 4.0 * s),
            &fill_paint(col(t.accent, 1.0)),
        );
        label_in(
            canvas,
            &font,
            8.0 * s,
            "PIN",
            &badge,
            tracking,
            col(t.pill, 1.0),
        );
    } else if let Some(f) = c.remaining {
        countdown_ring(canvas, t, ring, f);
    }

    if let Some((label, error)) = c.status {
        let mut area = rel(card, c.status_rect, s);
        area.right = area.right.min(right);
        if area.width() > 4.0 * s {
            status_chip(canvas, fonts, t, s, label, error, area);
        }
    }
}

/// The 16 px ring: a dark disc for legibility over any screenshot, the track
/// (`rgba(0,0,0,.45)`), and the remaining fraction in the highlight colour from 12
/// o'clock, clockwise.
fn countdown_ring(canvas: &Canvas, t: &ResolvedTokens, ring: Rect, fraction: f32) {
    let k = ring.width().min(ring.height()) / 16.0;
    let (cx, cy) = (ring.center_x(), ring.center_y());
    let (radius, width) = (6.0 * k, 2.5 * k);
    canvas.draw_circle((cx, cy), 8.0 * k, &fill_paint(col(t.pill, 0.78)));
    canvas.draw_circle(
        (cx, cy),
        radius,
        &stroke_paint(Color::from_argb(115, 0, 0, 0), width),
    );
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        let oval = Rect::from_xywh(cx - radius, cy - radius, radius * 2.0, radius * 2.0);
        canvas.draw_arc(
            oval,
            -90.0,
            360.0 * f,
            false,
            &stroke_paint(col(t.highlight, 1.0), width),
        );
    }
}

/// The status text on a pill chip, centred in `area`: normal ones in the text colour,
/// errors in the warn colour with a warn outline. The label says what happened
/// (`copy failed`), so colour is never the only signal. The text shrinks a little
/// before it is cut.
fn status_chip(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    s: f32,
    label: &str,
    error: bool,
    area: Rect,
) {
    let pad = 4.0 * s;
    let mut size = 9.0 * s;
    let mut font = fonts.font(&t.font_label, Weight::Medium, size);
    while size > 7.0 * s && text_width(&font, label, 0.0) > area.width() - pad * 2.0 {
        size -= 0.5 * s;
        font = fonts.font(&t.font_label, Weight::Medium, size);
    }
    let text = fit(&font, label, 0.0, area.width() - pad * 2.0);
    let w = (text_width(&font, &text, 0.0) + pad * 2.0).min(area.width());
    let h = area.height().min(16.0 * s);
    let chip = Rect::from_xywh(area.center_x() - w / 2.0, area.center_y() - h / 2.0, w, h);
    let rad = 6.0 * s;
    canvas.draw_rrect(
        RRect::new_rect_xy(chip, rad, rad),
        &fill_paint(col(t.pill, 0.92)),
    );
    let ink = col(if error { t.warn } else { t.text }, 1.0);
    if error {
        canvas.draw_rrect(
            RRect::new_rect_xy(chip.with_inset((0.5 * s, 0.5 * s)), rad, rad),
            &stroke_paint(ink, 1.0 * s),
        );
    }
    label_in(canvas, &font, size, &text, &chip, 0.0, ink);
}

/// The 18 px check box: empty (dark, text-coloured border) or, when selected, filled
/// with the accent and a drawn check mark in the pill colour.
fn check_box(canvas: &Canvas, t: &ResolvedTokens, b: Rect, s: f32, selected: bool) {
    let rad = (6.0 * s).min(b.width() / 2.0);
    let body = RRect::new_rect_xy(b, rad, rad);
    let edge = RRect::new_rect_xy(b.with_inset((0.75 * s, 0.75 * s)), rad, rad);
    if selected {
        canvas.draw_rrect(body, &fill_paint(col(t.accent, 1.0)));
        canvas.draw_rrect(edge, &stroke_paint(col(t.accent, 1.0), 1.5 * s));
        let mark = stroke_paint(col(t.pill, 1.0), 2.0 * s);
        let at = |x: f32, y: f32| (b.left + x * b.width(), b.top + y * b.height());
        canvas.draw_line(at(0.24, 0.52), at(0.43, 0.70), &mark);
        canvas.draw_line(at(0.43, 0.70), at(0.76, 0.32), &mark);
    } else {
        canvas.draw_rrect(body, &fill_paint(col(t.pill, 0.6)));
        canvas.draw_rrect(edge, &stroke_paint(col(t.text, 1.0), 1.5 * s));
    }
}

/// An action chip on a card: `rgba(10,9,8,.78)` with `#eee` mono 9px text, r 6; a
/// primary one is accent with pill-coloured text. A focused one gets an outline.
fn card_button(
    canvas: &Canvas,
    font: &Font,
    t: &ResolvedTokens,
    s: f32,
    b: &PreviewButton,
    r: Rect,
) {
    let rad = 6.0 * s;
    let (body, ink) = if b.primary {
        (col(t.accent, 1.0), col(t.pill, 1.0))
    } else {
        (
            Color::from_argb(if b.focused { 235 } else { 199 }, 10, 9, 8),
            Color::from_rgb(0xee, 0xee, 0xee),
        )
    };
    canvas.draw_rrect(RRect::new_rect_xy(r, rad, rad), &fill_paint(body));
    label_in(canvas, font, 9.0 * s, b.label, &r, 0.04 * 9.0 * s, ink);
    if b.focused {
        let edge = if b.primary { t.text } else { t.accent };
        canvas.draw_rrect(
            grown(r, rad, 1.5 * s),
            &stroke_paint(col(edge, 1.0), 1.5 * s),
        );
    }
}

/// The selection bar: a count chip and the buttons (pill, hairline, r 8, mono 10px; the
/// primary one accent). Only the chips are drawn, and only inside the bar's rect.
fn draw_bar(canvas: &Canvas, fonts: &mut Fonts, t: &ResolvedTokens, s: f32, bar: &PreviewBar) {
    let font = fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
    let rad = 8.0 * s;
    let tracking = 0.06 * 10.0 * s;
    canvas.save();
    canvas.clip_rect(dev(bar.rect, s), ClipOp::Intersect, true);

    let count = dev(bar.count.0, s);
    canvas.draw_rrect(
        RRect::new_rect_xy(count, rad, rad),
        &fill_paint(col(t.pill, 1.0)),
    );
    let text = fit(&font, bar.count.1, tracking, count.width() - 8.0 * s);
    label_in(
        canvas,
        &font,
        10.0 * s,
        &text,
        &count,
        tracking,
        col(t.text, 1.0),
    );

    for b in bar.buttons {
        let r = dev(b.rect, s);
        let (body, ink, edge) = if b.primary {
            (t.accent, t.pill, t.accent)
        } else {
            (t.pill, t.text, t.surface2)
        };
        let rr = RRect::new_rect_xy(r, rad, rad);
        canvas.draw_rrect(rr, &fill_paint(col(body, 1.0)));
        canvas.draw_rrect(
            RRect::new_rect_xy(
                r.with_inset((0.5 * s, 0.5 * s)),
                rad - 0.5 * s,
                rad - 0.5 * s,
            ),
            &stroke_paint(col(edge, 1.0), 1.0 * s),
        );
        let text = fit(&font, b.label, tracking, r.width() - 8.0 * s);
        label_in(canvas, &font, 10.0 * s, &text, &r, tracking, col(ink, 1.0));
        if b.focused {
            // Inside the chip, so it stays within the bar's rect.
            let ring = if b.primary { t.text } else { t.accent };
            canvas.draw_rrect(
                RRect::new_rect_xy(
                    r.with_inset((1.0 * s, 1.0 * s)),
                    rad - 1.0 * s,
                    rad - 1.0 * s,
                ),
                &stroke_paint(col(ring, 1.0), 2.0 * s),
            );
        }
    }
    canvas.restore();
}

/// The notice chip (the mock's `.toast`): accent, pill-coloured mono 11px text, r 8.
fn draw_notice(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    s: f32,
    rect: LRect,
    text: &str,
) {
    let r = dev(rect, s);
    let font = fonts.font(&t.font_label, Weight::Medium, 11.0 * s);
    let rad = 8.0 * s;
    canvas.draw_rrect(
        RRect::new_rect_xy(r, rad, rad),
        &fill_paint(col(t.accent, 1.0)),
    );
    let text = fit(&font, text, 0.0, r.width() - 12.0 * s);
    label_in(canvas, &font, 11.0 * s, &text, &r, 0.0, col(t.pill, 1.0));
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::color::Rgb;
    use dj_core::theme::{ThemeRegistry, DEFAULT_THEME};
    use skia_safe::image::CachingHint;
    use skia_safe::{AlphaType, ColorType, ImageInfo};

    fn tokens() -> ResolvedTokens {
        ThemeRegistry::with_builtins()
            .get(DEFAULT_THEME)
            .unwrap()
            .clone()
    }

    /// `[b, g, r, a]` of a theme colour, as the CPU surface stores it.
    fn bgra(c: Rgb) -> [u8; 4] {
        let [r, g, b] = c.to_u8();
        [b, g, r, 255]
    }

    fn near(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
        a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= tol)
    }

    fn solid(w: u32, h: u32, bgra: [u8; 4]) -> Bitmap {
        Bitmap {
            width: w,
            height: h,
            pixels: bgra.repeat(w as usize * h as usize),
        }
    }

    /// A painted CPU surface.
    struct Shot {
        w: i32,
        h: i32,
        px: Vec<u8>,
    }

    impl Shot {
        fn at(&self, x: i32, y: i32) -> [u8; 4] {
            assert!(x >= 0 && y >= 0 && x < self.w && y < self.h, "({x},{y})");
            let i = ((y * self.w + x) * 4) as usize;
            [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
        }

        /// Pixels within `tol` of `c` (opaque) that lie outside `inside` (x, y, w, h).
        fn count_outside(&self, inside: (i32, i32, i32, i32), c: [u8; 4], tol: u8) -> usize {
            let mut n = 0;
            for y in 0..self.h {
                for x in 0..self.w {
                    let (ix, iy, iw, ih) = inside;
                    if x >= ix && x < ix + iw && y >= iy && y < iy + ih {
                        continue;
                    }
                    if near(self.at(x, y), c, tol) {
                        n += 1;
                    }
                }
            }
            n
        }

        /// Pixels within `tol` of `c` inside the rect (x, y, w, h).
        fn count_in(&self, r: (i32, i32, i32, i32), c: [u8; 4], tol: u8) -> usize {
            let mut n = 0;
            for y in r.1..r.1 + r.3 {
                for x in r.0..r.0 + r.2 {
                    if near(self.at(x, y), c, tol) {
                        n += 1;
                    }
                }
            }
            n
        }

        /// Opaque pixels inside the rect (x, y, w, h) that are NOT within `tol` of `c`.
        fn count_far(&self, r: (i32, i32, i32, i32), c: [u8; 4], tol: u8) -> usize {
            let mut n = 0;
            for y in r.1..r.1 + r.3 {
                for x in r.0..r.0 + r.2 {
                    let p = self.at(x, y);
                    if p[3] == 255 && !near(p, c, tol) {
                        n += 1;
                    }
                }
            }
            n
        }
    }

    fn paint(sc: &PreviewScene, images: &HashMap<u64, Image>, w: i32, h: i32) -> Shot {
        let mut fonts = Fonts::new().unwrap();
        let mut surface = surfaces::raster_n32_premul((w, h)).unwrap();
        surface.canvas().clear(Color::TRANSPARENT);
        draw_scene(surface.canvas(), &mut fonts, images, sc);
        let info = ImageInfo::new((w, h), ColorType::BGRA8888, AlphaType::Premul, None);
        let mut px = vec![0u8; (w * h * 4) as usize];
        let img = surface.image_snapshot();
        assert!(img.read_pixels(
            &info,
            &mut px,
            (w * 4) as usize,
            (0, 0),
            CachingHint::Disallow
        ));
        Shot { w, h, px }
    }

    const LABELS: [&str; 4] = ["copy", "save", "pin", "✕"];

    /// A card at (x, y) with the contract's card-local rects (A's layout: 18 px check at
    /// 6,6; 16 px ring 5 px in from the top right; actions on the bottom row).
    fn card(id: u64, x: f32, y: f32) -> PreviewCard<'static> {
        let widths = [27.0, 27.0, 33.0, 18.0];
        let mut left = 5.0;
        let actions = std::array::from_fn(|i| {
            let r = LRect::new(left, 59.0, widths[i], 16.0);
            left += widths[i] + 3.0;
            PreviewButton {
                rect: r,
                label: LABELS[i],
                focused: false,
                primary: false,
            }
        });
        PreviewCard {
            id,
            rect: LRect::new(x, y, 124.0, 80.0),
            alpha: 1.0,
            scale: 1.0,
            remaining: Some(1.0),
            selected: false,
            pinned: false,
            hovered: false,
            focused: false,
            busy: false,
            show_check: false,
            show_actions: false,
            status: None,
            check: LRect::new(6.0, 6.0, 18.0, 18.0),
            ring: LRect::new(103.0, 5.0, 16.0, 16.0),
            status_rect: LRect::new(28.0, 6.0, 70.0, 18.0),
            actions,
        }
    }

    fn scene<'a>(
        t: &'a ResolvedTokens,
        scale: f32,
        cards: &'a [PreviewCard<'a>],
    ) -> PreviewScene<'a> {
        PreviewScene {
            tokens: t,
            scale,
            cards,
            bar: None,
            notice: None,
        }
    }

    fn one_image(id: u64, bitmap: &Bitmap, scale: f32) -> HashMap<u64, Image> {
        HashMap::from([(id, shrink(bitmap, scale).unwrap())])
    }

    const RED: [u8; 4] = [30, 60, 200, 255];
    const GREEN: [u8; 4] = [60, 190, 70, 255];

    // The two-card window: cards at x = 20 and x = 154 (the 10 px gap), y = 20.
    const W: i32 = 320;
    const H: i32 = 140;

    #[test]
    fn a_card_shows_its_image_and_the_gap_stays_clear() {
        let t = tokens();
        let cards = [card(1, 20.0, 20.0), card(2, 154.0, 20.0)];
        let mut images = one_image(1, &solid(300, 200, RED), 1.0);
        images.insert(2, shrink(&solid(300, 200, GREEN), 1.0).unwrap());
        let shot = paint(&scene(&t, 1.0, &cards), &images, W, H);
        assert!(near(shot.at(82, 60), RED, 2), "{:?}", shot.at(82, 60));
        assert!(near(shot.at(216, 60), GREEN, 2), "{:?}", shot.at(216, 60));
        // The middle of the gap, and the far corners, are untouched.
        assert_eq!(shot.at(149, 60)[3], 0, "gap");
        assert_eq!(shot.at(2, 2), [0, 0, 0, 0]);
        assert_eq!(shot.at(W - 3, H - 3), [0, 0, 0, 0]);
        // Everything beyond the 16 px margin is clear too (a hover lift of a few px
        // stays inside it).
        for x in 0..W {
            assert_eq!(shot.at(x, 2)[3], 0);
            assert_eq!(shot.at(x, 20 + 80 + 16 + 2)[3], 0);
        }
    }

    #[test]
    fn a_card_without_its_image_is_a_pill_coloured_box() {
        let t = tokens();
        let cards = [card(1, 20.0, 20.0)];
        let shot = paint(&scene(&t, 1.0, &cards), &HashMap::new(), W, H);
        assert!(near(shot.at(82, 60), bgra(t.pill), 1));
    }

    #[test]
    fn a_wide_image_is_aspect_fit_with_pill_bands() {
        let t = tokens();
        let cards = [card(1, 20.0, 20.0)];
        // 4:1 in a 124 x 80 card: 124 x 31, centred, so 24 px bands above and below.
        let images = one_image(1, &solid(400, 100, RED), 1.0);
        let shot = paint(&scene(&t, 1.0, &cards), &images, W, H);
        assert!(near(shot.at(82, 60), RED, 2));
        assert!(near(shot.at(82, 20 + 10), bgra(t.pill), 1), "top band");
        assert!(near(shot.at(82, 20 + 70), bgra(t.pill), 1), "bottom band");
        // And a tall one leaves bands left and right.
        let images = one_image(1, &solid(100, 400, GREEN), 1.0);
        let shot = paint(&scene(&t, 1.0, &cards), &images, W, H);
        assert!(near(shot.at(82, 60), GREEN, 2));
        assert!(near(shot.at(20 + 15, 60), bgra(t.pill), 1), "left band");
    }

    #[test]
    fn the_ring_runs_clockwise_from_twelve() {
        let t = tokens();
        let hi = bgra(t.highlight);
        let ring_at = |frac: Option<f32>, pinned: bool| {
            let mut c = card(1, 20.0, 20.0);
            c.remaining = frac;
            c.pinned = pinned;
            let cards = [c];
            paint(&scene(&t, 1.0, &cards), &HashMap::new(), W, H)
        };
        // The ring's centre is (111, 13) in the card, so (131, 33) on the surface.
        let (cx, cy) = (20 + 111, 20 + 13);
        let half = ring_at(Some(0.5), false);
        assert!(near(half.at(cx + 6, cy), hi, 8), "right side");
        assert!(near(half.at(cx + 4, cy - 4), hi, 24), "upper right");
        assert!(near(half.at(cx + 4, cy + 4), hi, 24), "lower right");
        assert!(!near(half.at(cx - 6, cy), hi, 40), "left side");
        assert!(!near(half.at(cx - 4, cy - 4), hi, 40), "upper left");
        assert!(!near(half.at(cx - 4, cy + 4), hi, 40), "lower left");

        // A quarter is the top right only.
        let quarter = ring_at(Some(0.25), false);
        assert!(near(quarter.at(cx + 4, cy - 4), hi, 24));
        assert!(!near(quarter.at(cx + 4, cy + 4), hi, 40));
        assert!(!near(quarter.at(cx - 4, cy - 4), hi, 40));

        // Full: all around. Empty: none.
        let full = ring_at(Some(1.0), false);
        assert!(near(full.at(cx - 6, cy), hi, 8) && near(full.at(cx + 6, cy), hi, 8));
        let area = (cx - 8, cy - 8, 16, 16);
        assert_eq!(ring_at(Some(0.0), false).count_in(area, hi, 8), 0);
        // No ring when there is no fraction.
        assert_eq!(ring_at(None, false).count_in(area, hi, 8), 0);
    }

    #[test]
    fn a_pinned_card_has_a_pin_badge_instead_of_the_ring() {
        let t = tokens();
        let hi = bgra(t.highlight);
        let area = (20 + 100, 20 + 2, 24, 22);
        let mut c = card(1, 20.0, 20.0);
        c.remaining = Some(0.5); // ignored when pinned
        c.pinned = true;
        let cards = [c];
        let pinned = paint(&scene(&t, 1.0, &cards), &HashMap::new(), W, H);
        assert_eq!(pinned.count_in(area, hi, 8), 0, "no ring");
        // The badge: an accent plate with pill-coloured letters, right-aligned to the
        // ring's spot (x up to 119, y 7..19).
        let acc = bgra(t.accent);
        assert!(
            near(pinned.at(20 + 117, 20 + 9), acc, 4),
            "{:?}",
            pinned.at(20 + 117, 20 + 9)
        );
        let letters = pinned.count_far((20 + 103, 20 + 8, 14, 10), acc, 60);
        assert!(letters > 10, "PIN text: {letters}");
        // The same card unpinned has the ring, not the plate.
        let mut c = card(1, 20.0, 20.0);
        c.remaining = Some(0.5);
        let cards = [c];
        let ringed = paint(&scene(&t, 1.0, &cards), &HashMap::new(), W, H);
        assert!(ringed.count_in(area, hi, 8) > 10);
        assert_eq!(ringed.count_in(area, acc, 4), 0);
    }

    #[test]
    fn selection_is_an_accent_outline_and_a_drawn_check_mark() {
        let t = tokens();
        let acc = bgra(t.accent);
        let mut c = card(1, 20.0, 20.0);
        c.selected = true;
        c.show_check = true;
        let cards = [c];
        let sel = paint(&scene(&t, 1.0, &cards), &HashMap::new(), W, H);
        // The outline sits 2 to 4 px outside the card on every side.
        assert!(
            near(sel.at(20 - 3, 60), acc, 4),
            "left {:?}",
            sel.at(17, 60)
        );
        assert!(near(sel.at(20 + 124 + 2, 60), acc, 4), "right");
        assert!(near(sel.at(82, 20 - 3), acc, 4), "top");
        assert!(near(sel.at(82, 20 + 80 + 2), acc, 4), "bottom");
        // The check box is filled, and its mark is drawn in the pill colour.
        assert!(near(sel.at(20 + 8, 20 + 15), acc, 4), "box fill");
        assert!(
            near(sel.at(20 + 12, 20 + 17), bgra(t.pill), 8),
            "check mark"
        );

        // Unselected with the empty box: no outline, no fill, a text-coloured border.
        let mut c = card(1, 20.0, 20.0);
        c.show_check = true;
        let cards = [c];
        let empty = paint(&scene(&t, 1.0, &cards), &HashMap::new(), W, H);
        assert!(empty.at(20 - 3, 60)[3] < 12, "only a faint shadow");
        assert_eq!(empty.count_outside((20, 20, 124, 80), acc, 4), 0);
        assert!(!near(empty.at(20 + 15, 20 + 15), acc, 40));
        assert!(
            near(empty.at(20 + 6, 20 + 15), bgra(t.text), 12),
            "box border"
        );
        // And without `show_check` there is no box at all.
        let cards = [card(1, 20.0, 20.0)];
        let none = paint(&scene(&t, 1.0, &cards), &HashMap::new(), W, H);
        assert!(near(none.at(20 + 6, 20 + 15), bgra(t.pill), 1));
    }

    #[test]
    fn keyboard_focus_is_a_dashed_ring_that_differs_from_selection() {
        let t = tokens();
        let (acc, txt) = (bgra(t.accent), bgra(t.text));
        let card_rect = (20, 20, 124, 80);
        let make = |selected: bool, focused: bool| {
            let mut c = card(1, 20.0, 20.0);
            c.selected = selected;
            c.focused = focused;
            let cards = [c];
            paint(&scene(&t, 1.0, &cards), &HashMap::new(), W, H)
        };
        let (sel, foc, both) = (make(true, false), make(false, true), make(true, true));
        // Selection: accent outside the card, no text-coloured ring.
        assert!(sel.count_outside(card_rect, acc, 6) > 200);
        assert_eq!(sel.count_outside(card_rect, txt, 6), 0);
        // Focus: a text-coloured ring, no accent outline.
        assert!(foc.count_outside(card_rect, txt, 6) > 100);
        assert_eq!(foc.count_outside(card_rect, acc, 6), 0);
        // Both: both rings.
        assert!(both.count_outside(card_rect, acc, 6) > 200);
        assert!(both.count_outside(card_rect, txt, 6) > 100);
        assert_ne!(sel.px, foc.px);
        assert_ne!(foc.px, both.px);
        // The focus ring is dashed: along the top edge both its dashes and its gaps show.
        let row: Vec<bool> = (30..130).map(|x| near(foc.at(x, 20 - 7), txt, 6)).collect();
        assert!(row.contains(&true) && row.contains(&false), "{row:?}");
        // It stays within 9 px of the card (12 below, with the shadow), inside the margin.
        for y in 0..H {
            for x in 0..W {
                // The shadow reaches a little further below than the ring does to the sides.
                let outside = (20 - x).max(x - 143).max(20 - y).max(y - 99 - 3);
                if outside > 9 {
                    assert_eq!(foc.at(x, y)[3], 0, "({x},{y})");
                }
            }
        }
    }

    #[test]
    fn a_failed_status_is_a_warn_coloured_chip_with_text() {
        let t = tokens();
        let warn = bgra(t.warn);
        let area = (20 + 28, 20 + 4, 72, 22);
        let make = |label: &'static str, error: bool| {
            let mut c = card(1, 20.0, 20.0);
            c.status = Some((label, error));
            let cards = [c];
            paint(&scene(&t, 1.0, &cards), &HashMap::new(), W, H)
        };
        let failed = make("copy failed", true);
        assert!(failed.count_in(area, warn, 8) > 30, "outline and text");
        // The chip is a pill plate in the middle of the status rect.
        assert!(near(failed.at(20 + 29 + 35, 20 + 8), bgra(t.pill), 40));
        // A calm status draws the label but no warn pixels.
        let ok = make("copied", false);
        assert_eq!(ok.count_in(area, warn, 8), 0);
        assert!(ok.count_in(area, bgra(t.text), 40) > 5, "text");
        // No status, no chip: the status area is empty.
        let cards = [card(1, 20.0, 20.0)];
        let none = paint(&scene(&t, 1.0, &cards), &HashMap::new(), W, H);
        assert_eq!(none.count_in(area, warn, 8), 0);
        assert_ne!(none.px, ok.px);
        // A long label is shrunk/cut to fit instead of spilling out of the rect.
        let long = make("file missing and not coming back", true);
        assert_eq!(long.count_in((20 + 99, 20 + 4, 5, 22), warn, 8), 0);
    }

    #[test]
    fn a_pinned_card_keeps_its_status_clear_of_the_badge() {
        let t = tokens();
        let mut c = card(1, 20.0, 20.0);
        c.pinned = true;
        c.status = Some(("file missing", true));
        let cards = [c];
        let shot = paint(&scene(&t, 1.0, &cards), &HashMap::new(), W, H);
        // The badge plate (x 100..119) is intact: no warn pixels over it.
        assert_eq!(
            shot.count_in((20 + 102, 20 + 7, 17, 12), bgra(t.warn), 8),
            0
        );
        assert!(shot.count_in((20 + 28, 20 + 4, 72, 22), bgra(t.warn), 8) > 20);
    }

    #[test]
    fn a_busy_card_is_dimmed_and_marked() {
        let t = tokens();
        let images = one_image(1, &solid(124, 80, RED), 1.0);
        let make = |busy: bool| {
            let mut c = card(1, 20.0, 20.0);
            c.busy = busy;
            let cards = [c];
            paint(&scene(&t, 1.0, &cards), &images, W, H)
        };
        let (idle, busy) = (make(false), make(true));
        assert!(near(idle.at(20 + 30, 20 + 40), RED, 2));
        let dim = busy.at(20 + 30, 20 + 40);
        assert!(!near(dim, RED, 20), "{dim:?}");
        // Dimmed towards the pill: darker in the red channel.
        assert!(dim[2] < RED[2] && dim[2] > bgra(t.pill)[2]);
        // Three dots in the text colour at the centre.
        assert!(busy.count_in((20 + 50, 20 + 36, 26, 8), bgra(t.text), 20) > 8);
    }

    #[test]
    fn actions_show_with_labels_and_a_focused_one_is_outlined() {
        let t = tokens();
        let images = one_image(1, &solid(124, 80, RED), 1.0);
        let make = |show: bool, focus: Option<usize>, primary: Option<usize>| {
            let mut c = card(1, 20.0, 20.0);
            c.show_actions = show;
            if let Some(i) = focus {
                c.actions[i].focused = true;
            }
            if let Some(i) = primary {
                c.actions[i].primary = true;
            }
            let cards = [c];
            paint(&scene(&t, 1.0, &cards), &images, W, H)
        };
        let hidden = make(false, None, None);
        assert!(near(hidden.at(20 + 8, 20 + 67), RED, 2));
        let shown = make(true, None, None);
        // The chip: dark plate over the image, light label.
        let plate = shown.at(20 + 15, 20 + 61);
        assert!(
            plate[0] < 60 && plate[1] < 60 && plate[2] < 100 && !near(plate, RED, 20),
            "{plate:?}"
        );
        let light = [0xee, 0xee, 0xee, 0xff];
        for i in 0..4 {
            let r = card(1, 20.0, 20.0).actions[i].rect;
            let area = (20 + r.x as i32, 20 + r.y as i32, r.w as i32, r.h as i32);
            assert!(shown.count_in(area, light, 40) > 4, "label {i} drawn");
        }
        // The chips stay inside the card.
        assert_eq!(shown.count_outside((20, 20, 124, 80), light, 40), 0);
        // A focused chip has an accent outline; primary has an accent plate.
        let acc = bgra(t.accent);
        assert_eq!(shown.count_in((20, 20, 124, 80), acc, 4), 0);
        let focused = make(true, Some(1), None);
        assert!(focused.count_in((20 + 30, 20 + 55, 36, 24), acc, 4) > 40);
        let primary = make(true, None, Some(1));
        assert!(near(primary.at(20 + 37, 20 + 61), acc, 4));
        // A focused action shows even if the chips are otherwise hidden.
        let forced = make(false, Some(2), None);
        assert!(forced.count_in((20 + 60, 20 + 55, 42, 24), acc, 4) > 40);
    }

    #[test]
    fn pop_in_scales_about_the_centre_and_fades_as_one() {
        let t = tokens();
        let images = one_image(1, &solid(124, 80, RED), 1.0);
        let make = |alpha: f32, scale: f32| {
            let mut c = card(1, 100.0, 30.0);
            c.alpha = alpha;
            c.scale = scale;
            let cards = [c];
            paint(&scene(&t, 1.0, &cards), &images, 320, 160)
        };
        // Half size: the corner region of the full card is clear, the centre is not.
        let small = make(1.0, 0.5);
        assert!(near(small.at(162, 70), RED, 2));
        assert_eq!(small.at(100 + 4, 30 + 4)[3], 0);
        assert!(near(small.at(162 - 25, 70), RED, 2), "still 62 px wide");
        assert_eq!(small.at(162 - 40, 70)[3], 0);
        // Half transparent: premultiplied, so every channel is about half.
        let faint = make(0.5, 1.0);
        let p = faint.at(162, 70);
        assert!(p[3].abs_diff(128) <= 2, "{p:?}");
        assert!(p[2].abs_diff(RED[2] / 2) <= 3, "{p:?}");
        // Invisible: nothing at all (not even a shadow).
        let gone = make(0.0, 1.0);
        assert!(gone.px.iter().all(|b| *b == 0));
        assert!(make(1.0, 0.0).px.iter().all(|b| *b == 0));
    }

    #[test]
    fn a_150_percent_scene_lands_on_device_pixels() {
        let t = tokens();
        let images = one_image(1, &solid(300, 200, RED), 1.5);
        let mut c = card(1, 20.0, 20.0);
        c.selected = true;
        c.show_check = true;
        let cards = [c];
        let sc = scene(&t, 1.5, &cards);
        let shot = paint(&sc, &images, 330, 210);
        let acc = bgra(t.accent);
        // The card is at device (30, 30), 186 x 120.
        assert!(near(shot.at(30 + 93, 30 + 60), RED, 2));
        assert!(near(shot.at(30 + 180, 30 + 60), RED, 2));
        assert_eq!(shot.at(30 + 186 + 20, 30 + 60)[3], 0);
        // The outline is 3 to 6 device px outside, and the check box 27 px at (39, 39).
        assert!(
            near(shot.at(30 - 5, 30 + 60), acc, 4),
            "{:?}",
            shot.at(25, 90)
        );
        assert!(near(shot.at(30 + 186 + 4, 30 + 60), acc, 4));
        assert!(near(shot.at(39 + 4, 39 + 13), acc, 4), "box fill");
        assert!(!near(shot.at(39 - 3, 39 + 13), acc, 40), "outside the box");
        // The ring (radius 9 device px at (30 + 166.5, 30 + 19.5)) is not painted at
        // 100% positions.
        let hi = bgra(t.highlight);
        assert!(near(shot.at(30 + 166 + 9, 30 + 19), hi, 24), "ring at 1.5x");
        assert_eq!(shot.count_in((30 + 103, 30 + 5, 14, 12), hi, 8), 0);
    }

    #[test]
    fn the_bar_and_notice_draw_inside_their_rects_only() {
        let t = tokens();
        let buttons = [
            PreviewButton {
                rect: LRect::new(110.0, 110.0, 60.0, 24.0),
                label: "Select all",
                focused: false,
                primary: false,
            },
            PreviewButton {
                rect: LRect::new(176.0, 110.0, 70.0, 24.0),
                label: "Save selected",
                focused: true,
                primary: true,
            },
        ];
        let bar = PreviewBar {
            rect: LRect::new(40.0, 110.0, 206.0, 24.0),
            count: (LRect::new(40.0, 110.0, 64.0, 24.0), "3 selected"),
            buttons: &buttons,
        };
        let notice = (LRect::new(60.0, 20.0, 160.0, 24.0), "Preview stack full");
        let sc = PreviewScene {
            tokens: &t,
            scale: 1.0,
            cards: &[],
            bar: Some(bar),
            notice: Some(notice),
        };
        let shot = paint(&sc, &HashMap::new(), 300, 160);
        for y in 0..160 {
            for x in 0..300 {
                let in_bar = (40..246).contains(&x) && (110..134).contains(&y);
                let in_notice = (60..220).contains(&x) && (20..44).contains(&y);
                if !in_bar && !in_notice {
                    assert_eq!(shot.at(x, y), [0, 0, 0, 0], "({x},{y})");
                }
            }
        }
        // The chips are there: pill for the count and the plain button, accent for the
        // primary one, accent for the notice, and the gaps between chips stay clear.
        assert!(near(shot.at(44, 114), bgra(t.pill), 8) || shot.at(44, 114)[3] > 0);
        assert_eq!(shot.at(107, 122)[3], 0, "gap between count and button");
        assert!(near(shot.at(114, 122), bgra(t.pill), 8));
        let acc = bgra(t.accent);
        assert!(near(shot.at(180, 122), acc, 8), "primary button");
        assert!(near(shot.at(64, 32), acc, 8), "notice");
        // Both carry their text (pill-coloured on accent for the notice).
        assert!(shot.count_in((60, 20, 160, 24), bgra(t.pill), 30) > 20);
        assert!(shot.count_in((110, 110, 60, 24), bgra(t.text), 40) > 20);
        // The focused primary button has an outline in the text colour, inside its rect.
        assert!(shot.count_in((176, 110, 70, 24), bgra(t.text), 8) > 40);
    }

    #[test]
    fn the_bar_never_paints_outside_its_rect_even_if_buttons_overflow() {
        let t = tokens();
        let buttons = [PreviewButton {
            rect: LRect::new(10.0, 10.0, 100.0, 40.0),
            label: "Dismiss selected",
            focused: true,
            primary: false,
        }];
        let bar = PreviewBar {
            rect: LRect::new(10.0, 10.0, 60.0, 24.0),
            count: (LRect::new(10.0, 10.0, 30.0, 24.0), "12 selected, a lot"),
            buttons: &buttons,
        };
        let sc = PreviewScene {
            tokens: &t,
            scale: 1.0,
            cards: &[],
            bar: Some(bar),
            notice: None,
        };
        let shot = paint(&sc, &HashMap::new(), 140, 80);
        for y in 0..80 {
            for x in 0..140 {
                if !((10..70).contains(&x) && (10..34).contains(&y)) {
                    assert_eq!(shot.at(x, y)[3], 0, "({x},{y})");
                }
            }
        }
    }

    #[test]
    fn cards_draw_back_to_front() {
        let t = tokens();
        let cards = [card(1, 20.0, 20.0), card(2, 60.0, 20.0)];
        let mut images = one_image(1, &solid(124, 80, RED), 1.0);
        images.insert(2, shrink(&solid(124, 80, GREEN), 1.0).unwrap());
        let shot = paint(&scene(&t, 1.0, &cards), &images, W, H);
        // x = 100 is inside both: the later (front) card wins.
        assert!(near(shot.at(100, 60), GREEN, 2));
        assert!(near(shot.at(40, 60), RED, 2));
    }

    #[test]
    fn the_cached_copy_is_the_aspect_fit_box_at_the_display_scale() {
        // A 1080p shot in a 124 x 80 card: 124 x 70 at 100%, 186 x 105 at 150%, 248 x 140 at 200%.
        assert_eq!(cache_size(1920, 1080, 1.0), (124, 70));
        assert_eq!(cache_size(1920, 1080, 1.5), (186, 105));
        assert_eq!(cache_size(1920, 1080, 2.0), (248, 140));
        // Tall and very thin shots keep their ratio, never reaching zero.
        assert_eq!(cache_size(1000, 2000, 1.0), (40, 80));
        assert_eq!(cache_size(3000, 20, 1.0), (124, 1));
        assert_eq!(cache_size(20, 3000, 1.0), (1, 80));
        // A shot that already fits is not resampled or enlarged.
        assert_eq!(cache_size(50, 30, 1.0), (50, 30));
        assert_eq!(cache_size(124, 80, 1.0), (124, 80));
        assert_eq!(cache_size(125, 80, 1.0), (124, 79));
        // A nonsense scale falls back to 100%.
        assert_eq!(cache_size(1920, 1080, 0.0), (124, 70));
        assert_eq!(cache_size(1920, 1080, f32::NAN), (124, 70));
    }

    #[test]
    fn the_shot_is_shrunk_once_and_the_copy_is_small() {
        let big = solid(3840, 2160, RED);
        let small = shrink(&big, 1.0).unwrap();
        assert_eq!((small.width(), small.height()), (124, 70));
        let small = shrink(&big, 2.0).unwrap();
        assert_eq!((small.width(), small.height()), (248, 140));
        // The copy keeps the colour (the mipmapped filter does not shift it).
        let one = one_image(1, &big, 1.0);
        let t = tokens();
        let cards = [card(1, 20.0, 20.0)];
        let shot = paint(&scene(&t, 1.0, &cards), &one, W, H);
        assert!(near(shot.at(82, 60), RED, 3), "{:?}", shot.at(82, 60));
        // A shot that fits is kept as it is.
        let tiny = shrink(&solid(40, 24, GREEN), 1.0).unwrap();
        assert_eq!((tiny.width(), tiny.height()), (40, 24));
    }

    #[test]
    fn a_two_tone_shot_survives_the_downscale() {
        // Left half red, right half green: still split in the middle after 3840 -> 124.
        let (w, h) = (3840usize, 200usize);
        let mut pixels = Vec::with_capacity(w * h * 4);
        for _ in 0..h {
            for x in 0..w {
                pixels.extend_from_slice(if x < w / 2 { &RED } else { &GREEN });
            }
        }
        let bitmap = Bitmap {
            width: w as u32,
            height: h as u32,
            pixels,
        };
        let t = tokens();
        let cards = [card(1, 20.0, 20.0)];
        let images = one_image(1, &bitmap, 1.0);
        let shot = paint(&scene(&t, 1.0, &cards), &images, W, H);
        assert!(near(shot.at(20 + 30, 60), RED, 6), "{:?}", shot.at(50, 60));
        assert!(
            near(shot.at(20 + 94, 60), GREEN, 6),
            "{:?}",
            shot.at(114, 60)
        );
    }

    #[test]
    fn broken_bitmaps_are_refused_without_a_panic() {
        assert!(shrink(&solid(0, 0, RED), 1.0).is_none());
        let short = Bitmap {
            width: 100,
            height: 100,
            pixels: vec![0; 100],
        };
        assert!(shrink(&short, 1.0).is_none());
    }

    #[test]
    fn the_dismiss_glyph_is_drawn_even_without_a_font_glyph() {
        // Whether or not Geist Mono has U+2715, the chip shows a mark in its middle.
        let mut fonts = Fonts::new().unwrap();
        let t = tokens();
        let font = fonts.font(&t.font_label, Weight::Medium, 9.0);
        let mut surface = surfaces::raster_n32_premul((18, 16)).unwrap();
        surface.canvas().clear(Color::TRANSPARENT);
        label_in(
            surface.canvas(),
            &font,
            9.0,
            "✕",
            &Rect::from_wh(18.0, 16.0),
            0.0,
            Color::WHITE,
        );
        let info = ImageInfo::new((18, 16), ColorType::BGRA8888, AlphaType::Premul, None);
        let mut px = vec![0u8; 18 * 16 * 4];
        assert!(surface.image_snapshot().read_pixels(
            &info,
            &mut px,
            72,
            (0, 0),
            CachingHint::Disallow
        ));
        let lit = px.chunks(4).filter(|p| p[3] > 128).count();
        assert!(lit >= 6, "{lit} lit pixels");
        // And it is not a notdef box: the box would light its whole outline.
        let corner = px[(2 * 18 + 3) * 4 + 3];
        assert!(corner < 255);
    }
}
