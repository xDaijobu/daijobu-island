//! The window picker ("assist") of the Tile feature: a topmost window that covers exactly
//! one lit zone and shows the open windows as cards, so one can be picked for the zone.
//!
//! `AssistSurface` is a D3D12 + DirectComposition chain on that window (like
//! `tile::ZoneOverlay`), sized in device px. It draws only when the app calls `render`
//! (input, a new zone, a changed window list), never in a loop. `resize` makes the chain
//! again at a new size, because the next zone may be another size: that happens once per
//! zone, never per frame.
//!
//! Everything is laid out by `dj-core` in zone-local logical px (the window's client
//! origin is the zone's top-left); this module only multiplies by the scene's `scale`.
//!
//! The look: the whole zone is a rounded rect of the pill colour at 88 % with a 2 px
//! accent outline (the lit zone). A mono header (title left, the step right in accent),
//! cards in a `surface1` body (a `surface2` hairline just outside it) with the window's
//! icon and title on top, and a mono footer.
//!
//! **The live thumbnail is not ours.** Windows composes each DWM thumbnail on top of the
//! card's `thumb_rect`, so that rect only ever gets a plain darker plate: no text, no
//! outline, no hairline, nothing that would be covered or that would leak around the
//! thumbnail. The hover and keyboard outlines are drawn outside the card for the same
//! reason (a thumb rect may reach the card's edges). Only a card without a thumbnail
//! (`no_thumb`: minimized, or refused by Windows) shows its icon large on the plate with
//! a short hint.
//!
//! Every text is clipped to its rect and cut with `..` to fit, and nothing is drawn
//! outside the zone's rounded rect.

use crate::fonts::{Fonts, Weight};
use crate::gpu::{Chain, Gpu, RenderError};
use crate::paint::{col, draw_text, fill_paint, fit, stroke_paint, text_width};
use dj_core::layout::Rect as LRect;
use dj_core::theme::ResolvedTokens;
use skia_safe::gpu::{self as sk_gpu, Budgeted};
use skia_safe::paint::Cap;
use skia_safe::{
    images, AlphaType, Canvas, ClipOp, Color, ColorType, Data, FilterMode, Font, Image, ImageInfo,
    MipmapMode, Paint, PathEffect, RRect, Rect, SamplingOptions,
};
use std::collections::HashMap;

/// The zone: corner radius, the pill's alpha and the accent outline's width.
const ZONE_R: f32 = 16.0;
const ZONE_FILL: f32 = 0.88;
const ZONE_LINE: f32 = 2.0;

/// A card's corner radius, and the thumbnail plate's (clipped to the card).
const CARD_R: f32 = 10.0;
const PLATE_R: f32 = 6.0;
/// The plate is black at this alpha over the card: darker than the card in every theme.
const PLATE_SHADE: f32 = 0.32;

/// The hairline just outside a card's edge, in `surface2`.
const HAIRLINE: f32 = 1.0;

/// The outlines around a card: width, and the distance of their centre line from the
/// card's edge (the first slot, and the second one when two outlines show at once).
const RING_W: f32 = 2.0;
const RING_NEAR: f32 = 3.0;
const RING_FAR: f32 = 7.0;
/// The keyboard outline's dashes: drawn / skipped, in logical px.
const DASH_ON: f32 = 6.0;
const DASH_OFF: f32 = 4.0;

/// Text sizes in logical px: header (`font_label`), card title (`font_title`), footer
/// (`font_label`), the admin chip and the no-thumbnail hint (`font_label`).
const HEADER_SIZE: f32 = 12.0;
const TITLE_SIZE: f32 = 12.0;
const FOOTER_SIZE: f32 = 11.0;
const CHIP_SIZE: f32 = 9.0;
const HINT_SIZE: f32 = 10.0;
/// Letter spacing of the mono texts as a fraction of their size (the chip's is smaller).
const TRACKING: f32 = 0.06;
const CHIP_TRACKING: f32 = 0.04;
/// Space kept between the header's two texts, and between a title and the admin chip.
const HEADER_GAP: f32 = 12.0;
const CHIP_GAP: f32 = 6.0;

/// The admin chip: its label, height and the padding either side of the label.
const ADMIN_LABEL: &str = "admin";
const CHIP_H: f32 = 14.0;
const CHIP_PAD: f32 = 4.0;

/// What a card without a live thumbnail says under its large icon.
const NO_THUMB_HINT: &str = "no preview";
/// The large icon: its share of the thumb rect's height and width, and its largest size.
const BIG_ICON_H: f32 = 0.42;
const BIG_ICON_W: f32 = 0.5;
const BIG_ICON_MAX: f32 = 48.0;
/// The gap between the large icon and the hint, and the thumb height needed for the hint.
const BIG_GAP: f32 = 8.0;
const HINT_MIN_H: f32 = 52.0;

/// The largest icon bitmap `set_icon` takes (px per side).
const MAX_ICON: u32 = 1024;

/// One card of the picker. The rects are zone-local logical px (the same space as
/// `AssistScene::size`).
#[derive(Clone, Copy, Debug)]
pub struct AssistCard<'a> {
    pub rect: LRect,
    pub icon_rect: LRect,
    pub title_rect: LRect,
    /// Where Windows draws the live thumbnail. Only a plate is drawn here.
    pub thumb_rect: LRect,
    pub title: &'a str,
    /// A key into the surface's icon cache (`set_icon`). A missing one draws a blank tile.
    pub icon: Option<u64>,
    /// The mouse is over the card: a solid accent outline.
    pub hovered: bool,
    /// The keyboard selection: a dashed outline in the text colour.
    pub selected: bool,
    /// The window is elevated: an "admin" chip at the right of the title row.
    pub elevated: bool,
    /// No live thumbnail (minimized or refused): the icon is drawn large in the thumb
    /// area instead.
    pub no_thumb: bool,
}

/// One frame of the picker.
#[derive(Clone, Copy, Debug)]
pub struct AssistScene<'a> {
    pub tokens: &'a ResolvedTokens,
    /// Device px per logical px.
    pub scale: f32,
    /// The window in logical px (the zone).
    pub size: (f32, f32),
    pub header: LRect,
    /// The header's left text.
    pub title: &'a str,
    /// The header's right text (`1 / 2`), right-aligned in accent.
    pub step: &'a str,
    pub footer: LRect,
    pub footer_text: &'a str,
    /// The footer is a warning (`warn` colour) instead of a hint (dim).
    pub warn: bool,
    pub cards: &'a [AssistCard<'a>],
}

/// The picker's GPU side: a swap chain at the window's size, redone by `resize`, and the
/// window icons as small images (kept across a resize).
pub struct AssistSurface {
    // Field order is drop order; `Drop` clears the images and surfaces first.
    icons: HashMap<u64, Image>,
    fonts: Fonts,
    /// None only between a failed `resize` and the next one that works.
    chain: Option<Chain>,
    gpu: Gpu,
    hwnd: isize,
    width: u32,
    height: u32,
}

impl AssistSurface {
    /// `width` x `height` is the window's client size in device px.
    pub fn new(hwnd: isize, width: u32, height: u32) -> Result<Self, RenderError> {
        let fonts = Fonts::new().map_err(RenderError::Setup)?;
        let mut gpu = Gpu::new()?;
        let chain = Chain::new(&mut gpu, hwnd, width, height)?;
        Ok(Self {
            icons: HashMap::new(),
            fonts,
            chain: Some(chain),
            gpu,
            hwnd,
            width,
            height,
        })
    }

    /// The swap chain's size in device px.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Makes the swap chain again at the new client size (the next zone may be another
    /// size). The icons stay. Call it after the window has been moved or resized, and
    /// before the next `render`. The old chain goes first (the GPU is waited for, so no
    /// frame is in flight): a window takes one composition target at a time. A size that
    /// is already the current one does nothing. If making the new chain fails, `render`
    /// fails until a `resize` works.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), RenderError> {
        if width == 0 || height == 0 {
            return Err(RenderError::Setup(format!(
                "assist surface: empty size {width}x{height}"
            )));
        }
        if self.chain.is_some() && (width, height) == (self.width, self.height) {
            return Ok(());
        }
        if let Some(mut old) = self.chain.take() {
            old.surfaces.clear();
            self.gpu.ctx.flush_submit_and_sync_cpu();
            drop(old);
        }
        self.chain = Some(Chain::new(&mut self.gpu, self.hwnd, width, height)?);
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// Caches a window's icon under `key`: `size` x `size` premultiplied BGRA (so
    /// `bgra.len() == size * size * 4`), made into a small texture with mipmaps once, so
    /// 20 px and large draws both stay smooth. False if the bitmap is empty, too big or
    /// the wrong length, or Skia refused it; the card then draws a blank tile. Replaces
    /// an icon under the same key.
    pub fn set_icon(&mut self, key: u64, bgra: &[u8], size: u32) -> bool {
        self.icons.remove(&key);
        let Some(raster) = icon_raster(bgra, size) else {
            return false;
        };
        // Upload once; the raster copy goes at the end of this call.
        let img = match sk_gpu::images::texture_from_image(
            &mut self.gpu.ctx,
            &raster,
            sk_gpu::Mipmapped::Yes,
            Budgeted::No,
        ) {
            Some(texture) => texture,
            None => raster.with_default_mipmaps().unwrap_or(raster),
        };
        self.icons.insert(key, img);
        true
    }

    pub fn has_icon(&self, key: u64) -> bool {
        self.icons.contains_key(&key)
    }

    pub fn clear_icons(&mut self) {
        self.icons.clear();
    }

    /// Clears to transparent, draws the scene and presents (`Present(1)`).
    pub fn render(&mut self, scene: &AssistScene) -> Result<(), RenderError> {
        let Some(chain) = self.chain.as_mut() else {
            return Err(RenderError::Setup(
                "assist surface has no swap chain".into(),
            ));
        };
        let canvas = chain.back_buffer().canvas();
        canvas.clear(Color::TRANSPARENT);
        paint_assist(canvas, &mut self.fonts, &self.icons, scene);
        chain.present(&mut self.gpu.ctx)
    }
}

impl Drop for AssistSurface {
    fn drop(&mut self) {
        // The textures and the wrapped buffers go first, then wait for the GPU.
        self.icons.clear();
        if let Some(chain) = self.chain.as_mut() {
            chain.surfaces.clear();
        }
        self.gpu.ctx.flush_submit_and_sync_cpu();
    }
}

/// A raster image of a `size` x `size` premultiplied BGRA bitmap (copied). None if the
/// size is 0 or past `MAX_ICON`, or the bytes are not exactly `size * size * 4`.
fn icon_raster(bgra: &[u8], size: u32) -> Option<Image> {
    if size == 0 || size > MAX_ICON || bgra.len() != size as usize * size as usize * 4 {
        return None;
    }
    let info = ImageInfo::new(
        (size as i32, size as i32),
        ColorType::BGRA8888,
        AlphaType::Premul,
        None,
    );
    images::raster_from_data(&info, Data::new_copy(bgra), size as usize * 4)
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

/// A rect with a real, finite area.
fn solid(r: &Rect) -> bool {
    r.is_finite() && r.width() > 0.0 && r.height() > 0.0
}

/// `rect` grown by `by` on every side, with the corner radius grown with it.
fn grown(rect: Rect, rad: f32, by: f32) -> RRect {
    RRect::new_rect_xy(rect.with_outset((by, by)), rad + by, rad + by)
}

/// `text` with its left edge at `x`, centred vertically in `rect`, clipped to `rect`.
fn clipped_text(
    canvas: &Canvas,
    font: &Font,
    text: &str,
    rect: &Rect,
    x: f32,
    tracking: f32,
    paint: &Paint,
) {
    if text.is_empty() || !solid(rect) {
        return;
    }
    canvas.save();
    canvas.clip_rect(*rect, ClipOp::Intersect, true);
    draw_text(
        canvas,
        font,
        text,
        x,
        rect.center_y() + 0.35 * font.size(),
        tracking,
        paint,
    );
    canvas.restore();
}

/// Draws the picker on `canvas` (already cleared to transparent): the zone, its header
/// and footer and the cards. `icons` is the surface's icon cache.
pub(crate) fn paint_assist(
    canvas: &Canvas,
    fonts: &mut Fonts,
    icons: &HashMap<u64, Image>,
    sc: &AssistScene,
) {
    let s = sane(sc.scale);
    let t = sc.tokens;
    let zone = Rect::from_wh(sc.size.0 * s, sc.size.1 * s);
    if !(solid(&zone) && zone.width() >= 4.0 * s && zone.height() >= 4.0 * s) {
        return;
    }
    let rad = (ZONE_R * s)
        .min(zone.width() / 2.0)
        .min(zone.height() / 2.0);
    let body = RRect::new_rect_xy(zone, rad, rad);
    canvas.draw_rrect(body, &fill_paint(col(t.pill, ZONE_FILL)));

    // Everything inside is clipped to the rounded zone, so a card that reaches an edge
    // never leaves it.
    let base = canvas.save();
    canvas.clip_rrect(body, ClipOp::Intersect, true);
    draw_header(canvas, fonts, t, s, sc);
    draw_footer(canvas, fonts, t, s, sc);
    for card in sc.cards {
        draw_card(canvas, fonts, icons, t, s, card);
    }
    canvas.restore_to_count(base);

    // The lit zone's outline, just inside the edge, over everything.
    let w = ZONE_LINE * s;
    let inner = RRect::new_rect_xy(
        zone.with_inset((w / 2.0, w / 2.0)),
        (rad - w / 2.0).max(0.0),
        (rad - w / 2.0).max(0.0),
    );
    canvas.draw_rrect(inner, &stroke_paint(col(t.accent, 1.0), w));
}

/// The title on the left in the text colour, the step on the right in accent. The title
/// is cut so the two never meet.
fn draw_header(canvas: &Canvas, fonts: &mut Fonts, t: &ResolvedTokens, s: f32, sc: &AssistScene) {
    let head = dev(sc.header, s);
    if !solid(&head) {
        return;
    }
    let font = fonts.font(&t.font_label, Weight::Medium, HEADER_SIZE * s);
    let tracking = TRACKING * HEADER_SIZE * s;
    let step = fit(&font, sc.step, tracking, head.width());
    let sw = text_width(&font, &step, tracking);
    let gap = if step.is_empty() { 0.0 } else { HEADER_GAP * s };
    let title = fit(&font, sc.title, tracking, head.width() - sw - gap);
    clipped_text(
        canvas,
        &font,
        &title,
        &head,
        head.left,
        tracking,
        &fill_paint(col(t.text, 1.0)),
    );
    clipped_text(
        canvas,
        &font,
        &step,
        &head,
        head.right - sw,
        tracking,
        &fill_paint(col(t.accent, 1.0)),
    );
}

/// The footer line, left aligned: dim for a hint, the warn colour for a warning.
fn draw_footer(canvas: &Canvas, fonts: &mut Fonts, t: &ResolvedTokens, s: f32, sc: &AssistScene) {
    let foot = dev(sc.footer, s);
    if !solid(&foot) {
        return;
    }
    let font = fonts.font(&t.font_label, Weight::Medium, FOOTER_SIZE * s);
    let tracking = TRACKING * FOOTER_SIZE * s;
    let text = fit(&font, sc.footer_text, tracking, foot.width());
    let ink = if sc.warn { t.warn } else { t.text_dim };
    clipped_text(
        canvas,
        &font,
        &text,
        &foot,
        foot.left,
        tracking,
        &fill_paint(col(ink, 1.0)),
    );
}

/// One card: the body, the thumbnail plate (and, without a live thumbnail, the large
/// icon and a hint on it), the icon and title row, then the outlines outside the card.
fn draw_card(
    canvas: &Canvas,
    fonts: &mut Fonts,
    icons: &HashMap<u64, Image>,
    t: &ResolvedTokens,
    s: f32,
    c: &AssistCard,
) {
    let r = dev(c.rect, s);
    if !(solid(&r) && r.width() >= 2.0 * s && r.height() >= 2.0 * s) {
        return;
    }
    let rad = (CARD_R * s).min(r.width() / 2.0).min(r.height() / 2.0);
    let body = RRect::new_rect_xy(r, rad, rad);
    canvas.draw_rrect(body, &fill_paint(col(t.surface1, 1.0)));
    // A hairline just outside the body (never over it, where the thumbnail may be), so a
    // card reads against the zone in a dark theme too.
    canvas.draw_rrect(
        grown(r, rad, HAIRLINE * s / 2.0),
        &stroke_paint(col(t.surface2, 1.0), HAIRLINE * s),
    );

    // The plate is clipped to the card, so a thumb rect that reaches the card's corner
    // follows the card's rounding.
    let thumb = dev(c.thumb_rect, s);
    if solid(&thumb) {
        canvas.save();
        canvas.clip_rrect(body, ClipOp::Intersect, true);
        let pr = (PLATE_R * s)
            .min(thumb.width() / 2.0)
            .min(thumb.height() / 2.0);
        canvas.draw_rrect(
            RRect::new_rect_xy(thumb, pr, pr),
            &fill_paint(Color::from_argb((PLATE_SHADE * 255.0 + 0.5) as u8, 0, 0, 0)),
        );
        if c.no_thumb {
            draw_no_thumb(canvas, fonts, icons, t, s, c.icon, &thumb);
        }
        canvas.restore();
    }

    draw_title_row(canvas, fonts, icons, t, s, c);

    // Outside the card: the thumb rect may touch the card's edges, and Windows covers it.
    // The mouse: a solid accent outline. The keyboard: a dashed one in the text colour,
    // in the same place, or one slot further out when both show.
    if c.hovered {
        canvas.draw_rrect(
            grown(r, rad, RING_NEAR * s),
            &stroke_paint(col(t.accent, 1.0), RING_W * s),
        );
    }
    if c.selected {
        let slot = if c.hovered { RING_FAR } else { RING_NEAR };
        let mut dash = stroke_paint(col(t.text, 1.0), RING_W * s);
        dash.set_stroke_cap(Cap::Butt);
        dash.set_path_effect(PathEffect::dash(&[DASH_ON * s, DASH_OFF * s], 0.0));
        canvas.draw_rrect(grown(r, rad, slot * s), &dash);
    }
}

/// The icon at the left of the title row, the title, and the admin chip at the right.
/// All of it stays inside the icon's and the title's rects.
fn draw_title_row(
    canvas: &Canvas,
    fonts: &mut Fonts,
    icons: &HashMap<u64, Image>,
    t: &ResolvedTokens,
    s: f32,
    c: &AssistCard,
) {
    let icon = dev(c.icon_rect, s);
    if solid(&icon) {
        canvas.save();
        canvas.clip_rect(icon, ClipOp::Intersect, true);
        draw_icon(canvas, icons, t, c.icon, icon);
        canvas.restore();
    }

    let row = dev(c.title_rect, s);
    if !solid(&row) {
        return;
    }
    // The title gets what the chip leaves.
    let mut room = row.width();
    if c.elevated {
        let chip = fonts.font(&t.font_label, Weight::Medium, CHIP_SIZE * s);
        let tracking = CHIP_TRACKING * CHIP_SIZE * s;
        let (w, label) = admin_chip(&chip, tracking, s, row.width());
        let h = (CHIP_H * s).min(row.height());
        let rect = Rect::from_xywh(row.right - w, row.center_y() - h / 2.0, w, h);
        let rad = (4.0 * s).min(h / 2.0);
        canvas.draw_rrect(
            RRect::new_rect_xy(rect, rad, rad),
            &fill_paint(col(t.warn, 1.0)),
        );
        let lw = text_width(&chip, &label, tracking);
        clipped_text(
            canvas,
            &chip,
            &label,
            &rect,
            rect.center_x() - lw / 2.0,
            tracking,
            &fill_paint(col(t.pill, 1.0)),
        );
        room = (row.width() - w - CHIP_GAP * s).max(0.0);
    }
    let font = fonts.font(&t.font_title, Weight::Medium, TITLE_SIZE * s);
    let title = fit(&font, c.title, 0.0, room);
    let area = Rect::from_ltrb(row.left, row.top, row.left + room, row.bottom);
    clipped_text(
        canvas,
        &font,
        &title,
        &area,
        row.left,
        0.0,
        &fill_paint(col(t.text, 1.0)),
    );
}

/// The admin chip's width and label (device px) in a title row `room` wide: the label
/// and its padding when they fit, else the whole row with the label cut to it. The label
/// is only cut when it really does not fit, never by float rounding.
fn admin_chip(font: &Font, tracking: f32, s: f32, room: f32) -> (f32, String) {
    let pad = CHIP_PAD * s;
    let wanted = text_width(font, ADMIN_LABEL, tracking) + 2.0 * pad;
    if wanted <= room {
        return (wanted, ADMIN_LABEL.to_string());
    }
    (
        room.max(0.0),
        fit(font, ADMIN_LABEL, tracking, room - 2.0 * pad),
    )
}

/// A window's icon centred in `dst` (as a square) or, when the cache has none under the
/// key, a blank tile so the row keeps its shape.
fn draw_icon(
    canvas: &Canvas,
    icons: &HashMap<u64, Image>,
    t: &ResolvedTokens,
    key: Option<u64>,
    dst: Rect,
) {
    let side = dst.width().min(dst.height());
    let square = Rect::from_xywh(
        dst.center_x() - side / 2.0,
        dst.center_y() - side / 2.0,
        side,
        side,
    );
    match key.and_then(|k| icons.get(&k)) {
        Some(img) => {
            canvas.draw_image_rect_with_sampling_options(
                img,
                None,
                square,
                SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear),
                &Paint::default(),
            );
        }
        None => {
            let tile = square.with_inset((side * 0.1, side * 0.1));
            let rad = side * 0.2;
            canvas.draw_rrect(
                RRect::new_rect_xy(tile, rad, rad),
                &fill_paint(col(t.text_dim, 0.35)),
            );
        }
    }
}

/// What a card without a live thumbnail shows on its plate: the icon large and centred
/// with a short hint under it (the hint only when the plate is tall enough). Clipped to
/// the thumb rect.
fn draw_no_thumb(
    canvas: &Canvas,
    fonts: &mut Fonts,
    icons: &HashMap<u64, Image>,
    t: &ResolvedTokens,
    s: f32,
    key: Option<u64>,
    thumb: &Rect,
) {
    canvas.save();
    canvas.clip_rect(*thumb, ClipOp::Intersect, true);
    let side = (thumb.height() * BIG_ICON_H)
        .min(thumb.width() * BIG_ICON_W)
        .min(BIG_ICON_MAX * s);
    let font = fonts.font(&t.font_label, Weight::Medium, HINT_SIZE * s);
    let hint_h = HINT_SIZE * s * 1.2;
    let with_hint = thumb.height() >= HINT_MIN_H * s;
    let stack = if with_hint {
        side + BIG_GAP * s + hint_h
    } else {
        side
    };
    let top = thumb.center_y() - stack / 2.0;
    let icon = Rect::from_xywh(thumb.center_x() - side / 2.0, top, side, side);
    if side > 0.0 {
        draw_icon(canvas, icons, t, key, icon);
    }
    if with_hint {
        let tracking = TRACKING * HINT_SIZE * s;
        let text = fit(&font, NO_THUMB_HINT, tracking, thumb.width() - 8.0 * s);
        let tw = text_width(&font, &text, tracking);
        let line = Rect::from_xywh(thumb.left, top + side + BIG_GAP * s, thumb.width(), hint_h);
        // The text colour, not the dim one: the plate is darker than the card, and a
        // light theme's dim text would not reach 4.5:1 on it.
        clipped_text(
            canvas,
            &font,
            &text,
            &line,
            thumb.center_x() - tw / 2.0,
            tracking,
            &fill_paint(col(t.text, 1.0)),
        );
    }
    canvas.restore();
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::color::Rgb;
    use dj_core::theme::{ThemeRegistry, DEFAULT_THEME};
    use skia_safe::image::CachingHint;
    use skia_safe::surfaces;

    fn tokens() -> ResolvedTokens {
        ThemeRegistry::with_builtins()
            .get(DEFAULT_THEME)
            .unwrap()
            .clone()
    }

    fn theme(id: &str) -> ResolvedTokens {
        ThemeRegistry::with_builtins().get(id).unwrap().clone()
    }

    /// `[b, g, r, a]` of an opaque theme colour, as the CPU surface stores it.
    fn bgra(c: Rgb) -> [u8; 4] {
        let [r, g, b] = c.to_u8();
        [b, g, r, 255]
    }

    fn near(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
        a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= tol)
    }

    /// A theme colour at `alpha` on nothing: premultiplied, as the surface stores it.
    fn premul(c: Rgb, alpha: f32) -> [u8; 4] {
        blend([0, 0, 0, 0], c, alpha)
    }

    /// `c` at `alpha` drawn over the premultiplied pixel `dst`.
    fn blend(dst: [u8; 4], c: Rgb, alpha: f32) -> [u8; 4] {
        let a = (alpha * 255.0 + 0.5).floor() / 255.0;
        let [r, g, b] = c.to_u8();
        let mix = |src: u8, d: u8| (src as f32 * a + d as f32 * (1.0 - a)).round() as u8;
        [
            mix(b, dst[0]),
            mix(g, dst[1]),
            mix(r, dst[2]),
            mix(255, dst[3]),
        ]
    }

    /// The thumbnail plate on top of the card body.
    fn plate(t: &ResolvedTokens) -> [u8; 4] {
        blend(bgra(t.surface1), Rgb::new(0.0, 0.0, 0.0), PLATE_SHADE)
    }

    /// x, y, w, h in device px.
    type IRect = (i32, i32, i32, i32);

    /// A painted CPU surface (BGRA, premultiplied).
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

        /// Pixels within `tol` of `c` inside the rect.
        fn count_in(&self, r: IRect, c: [u8; 4], tol: u8) -> usize {
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

        /// The first pixel with any coverage outside every rect, if any.
        fn lit_outside(&self, inside: &[IRect]) -> Option<(i32, i32)> {
            (0..self.h)
                .flat_map(|y| (0..self.w).map(move |x| (x, y)))
                .find(|&(x, y)| {
                    let within = inside
                        .iter()
                        .any(|r| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3);
                    !within && self.at(x, y)[3] > 0
                })
        }

        /// The bounding box (left, top, right, bottom; exclusive) of the pixels that
        /// differ from `other`, if any.
        fn diff(&self, other: &Shot) -> Option<(i32, i32, i32, i32)> {
            assert_eq!((self.w, self.h), (other.w, other.h));
            let mut bb: Option<(i32, i32, i32, i32)> = None;
            for y in 0..self.h {
                for x in 0..self.w {
                    if self.at(x, y) != other.at(x, y) {
                        bb = Some(match bb {
                            None => (x, y, x + 1, y + 1),
                            Some((l, t, r, b)) => (l.min(x), t.min(y), r.max(x + 1), b.max(y + 1)),
                        });
                    }
                }
            }
            bb
        }

        /// The pixels inside `r` are the same in both shots.
        fn same_in(&self, other: &Shot, r: IRect) -> bool {
            (r.1..r.1 + r.3).all(|y| (r.0..r.0 + r.2).all(|x| self.at(x, y) == other.at(x, y)))
        }
    }

    fn inside(bb: (i32, i32, i32, i32), r: IRect) -> bool {
        bb.0 >= r.0 && bb.1 >= r.1 && bb.2 <= r.0 + r.2 && bb.3 <= r.1 + r.3
    }

    /// The zone: 400 x 300, the header and the footer as `dj_core::tile_assist` puts them.
    const ZONE: (f32, f32) = (400.0, 300.0);
    const HEADER: LRect = LRect::new(24.0, 24.0, 352.0, 32.0);
    const FOOTER: LRect = LRect::new(24.0, 248.0, 352.0, 28.0);
    const HEAD_PX: IRect = (24, 24, 352, 32);
    const FOOT_PX: IRect = (24, 248, 352, 28);
    /// The surface is bigger than the zone, to see that nothing is drawn outside it.
    const SURFACE: (i32, i32) = (440, 340);
    const ZONE_PX: IRect = (0, 0, 400, 300);

    /// A card as the core lays it out: a 28 px title row (icon at the left) above a 16:10
    /// thumbnail area, here inset by 6 px.
    fn card<'a>(x: f32, y: f32, w: f32, title: &'a str) -> AssistCard<'a> {
        let h = 28.0 + w / 1.6;
        AssistCard {
            rect: LRect::new(x, y, w, h),
            icon_rect: LRect::new(x + 8.0, y + 4.0, 20.0, 20.0),
            title_rect: LRect::new(x + 34.0, y, w - 42.0, 28.0),
            thumb_rect: LRect::new(x + 6.0, y + 28.0, w - 12.0, h - 34.0),
            title,
            icon: None,
            hovered: false,
            selected: false,
            elevated: false,
            no_thumb: false,
        }
    }

    fn px(r: LRect) -> IRect {
        (r.x as i32, r.y as i32, r.w as i32, r.h as i32)
    }

    fn paint<'a>(
        t: &'a ResolvedTokens,
        scale: f32,
        build: impl Fn(&mut AssistScene<'a>),
        icons: &HashMap<u64, Image>,
        (w, h): (i32, i32),
        cards: &'a [AssistCard<'a>],
    ) -> Shot {
        let mut fonts = Fonts::new().unwrap();
        let mut surface = surfaces::raster_n32_premul((w, h)).unwrap();
        surface.canvas().clear(Color::TRANSPARENT);
        let mut sc = AssistScene {
            tokens: t,
            scale,
            size: ZONE,
            header: HEADER,
            title: "",
            step: "",
            footer: FOOTER,
            footer_text: "",
            warn: false,
            cards,
        };
        build(&mut sc);
        paint_assist(surface.canvas(), &mut fonts, icons, &sc);
        let info = ImageInfo::new((w, h), ColorType::BGRA8888, AlphaType::Premul, None);
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        assert!(surface.image_snapshot().read_pixels(
            &info,
            &mut pixels,
            (w * 4) as usize,
            (0, 0),
            CachingHint::Disallow
        ));
        Shot { w, h, px: pixels }
    }

    /// A scene at 100 % with these cards and nothing else.
    fn cards_shot<'a>(t: &'a ResolvedTokens, cards: &'a [AssistCard<'a>]) -> Shot {
        paint(t, 1.0, |_| {}, &HashMap::new(), SURFACE, cards)
    }

    /// A scene at 100 % with these texts and no cards.
    fn text_shot<'a>(
        t: &'a ResolvedTokens,
        title: &'a str,
        step: &'a str,
        foot: &'a str,
        warn: bool,
    ) -> Shot {
        paint(
            t,
            1.0,
            |sc| {
                sc.title = title;
                sc.step = step;
                sc.footer_text = foot;
                sc.warn = warn;
            },
            &HashMap::new(),
            SURFACE,
            &[],
        )
    }

    /// An opaque icon of one colour, as the BGRA bytes `set_icon` takes.
    fn flat_icon(size: u32, bgra: [u8; 4]) -> Image {
        let bytes = bgra.repeat((size * size) as usize);
        icon_raster(&bytes, size)
            .unwrap()
            .with_default_mipmaps()
            .unwrap()
    }

    const RED: [u8; 4] = [0, 0, 255, 255];

    // ----- the zone -----

    #[test]
    fn the_zone_is_the_pill_at_88_percent_with_a_two_pixel_accent_outline() {
        let t = tokens();
        let shot = cards_shot(&t, &[]);
        let fill = premul(t.pill, ZONE_FILL);
        for (x, y) in [(200, 150), (30, 30), (370, 270), (3, 150), (200, 3)] {
            let p = shot.at(x, y);
            assert!(near(p, fill, 2), "fill at ({x},{y}): {p:?}");
        }
        assert!(
            (222..=226).contains(&shot.at(200, 150)[3]),
            "88 % of 255 is 224"
        );
        // The outline: 2 px of accent just inside the edge, on all four sides.
        let acc = bgra(t.accent);
        for (x, y) in [
            (0, 150),
            (1, 150),
            (398, 150),
            (399, 150),
            (200, 0),
            (200, 1),
            (200, 298),
            (200, 299),
        ] {
            let p = shot.at(x, y);
            assert!(near(p, acc, 2), "outline at ({x},{y}): {p:?}");
        }
        assert!(near(shot.at(2, 150), fill, 2), "the outline is 2 px");
        assert!(near(shot.at(397, 150), fill, 2));
        // Rounded (r 16): the very corners are clear, a point on the arc is lit.
        for (x, y) in [(0, 0), (399, 0), (0, 299), (399, 299)] {
            assert_eq!(shot.at(x, y)[3], 0, "corner ({x},{y})");
        }
        assert!(shot.at(5, 5)[3] > 200);
        // Nothing outside the zone.
        assert_eq!(shot.lit_outside(&[ZONE_PX]), None);
        assert_eq!(shot.at(SURFACE.0 - 1, SURFACE.1 - 1), [0, 0, 0, 0]);
    }

    #[test]
    fn at_150_percent_the_decoration_scales_and_the_zone_keeps_its_logical_size() {
        let t = tokens();
        // 400 x 300 logical is 600 x 450 device px.
        let shot = paint(&t, 1.5, |_| {}, &HashMap::new(), (640, 490), &[]);
        let fill = premul(t.pill, ZONE_FILL);
        let acc = bgra(t.accent);
        for x in 0..3 {
            assert!(near(shot.at(x, 225), acc, 2), "outline x={x}");
            assert!(near(shot.at(599 - x, 225), acc, 2), "outline x={}", 599 - x);
        }
        assert!(
            near(shot.at(3, 225), fill, 2),
            "3 px outline, then the fill"
        );
        assert_eq!(shot.at(0, 0)[3], 0, "corner");
        assert!(shot.at(8, 8)[3] > 150, "r 24: this point is on the arc");
        assert_eq!(shot.lit_outside(&[(0, 0, 600, 450)]), None);
        // A nonsense scale falls back to 100 %.
        let one = cards_shot(&t, &[]);
        for bad in [0.0, -2.0, f32::NAN, f32::INFINITY] {
            let b = paint(&t, bad, |_| {}, &HashMap::new(), SURFACE, &[]);
            assert_eq!(one.px, b.px, "scale {bad}");
        }
    }

    #[test]
    fn a_degenerate_zone_draws_nothing_and_does_not_panic() {
        let t = tokens();
        for size in [
            (0.0, 0.0),
            (3.0, 300.0),
            (400.0, -5.0),
            (f32::NAN, 300.0),
            (400.0, f32::INFINITY),
        ] {
            let shot = paint(
                &t,
                1.0,
                |sc| {
                    sc.size = size;
                    sc.title = "title";
                },
                &HashMap::new(),
                SURFACE,
                &[card(30.0, 76.0, 160.0, "x")],
            );
            assert_eq!(shot.lit_outside(&[]), None, "{size:?}");
        }
    }

    // ----- header and footer -----

    #[test]
    fn the_header_has_its_title_left_in_text_and_the_step_right_in_accent() {
        let t = tokens();
        let base = text_shot(&t, "", "", "", false);
        let both = text_shot(&t, "pick a window · zone 1 of 2", "1 / 2", "", false);
        let bb = both.diff(&base).expect("something is drawn");
        assert!(inside(bb, HEAD_PX), "header ink {bb:?}");
        // The title is flush left, the step flush right.
        let title = text_shot(&t, "pick a window · zone 1 of 2", "", "", false);
        let step = text_shot(&t, "", "1 / 2", "", false);
        let (tb, sb) = (title.diff(&base).unwrap(), step.diff(&base).unwrap());
        assert!(
            (24..28).contains(&tb.0),
            "title starts at the left edge: {tb:?}"
        );
        assert!(
            (372..=376).contains(&sb.2),
            "step ends at the right edge: {sb:?}"
        );
        assert!(tb.2 < sb.0, "a gap between them: {tb:?} {sb:?}");
        // Colours: text for the title, accent for the step, nothing else.
        assert!(title.count_in(HEAD_PX, bgra(t.text), 40) > 30);
        assert_eq!(title.count_in(HEAD_PX, bgra(t.accent), 6), 0);
        assert!(step.count_in(HEAD_PX, bgra(t.accent), 40) > 8);
        assert_eq!(step.count_in(HEAD_PX, bgra(t.text), 6), 0);
        // The title is 12 px mono: a line of a glyph's height, not a speck.
        assert!((7..=14).contains(&(tb.3 - tb.1)), "title height {tb:?}");
    }

    #[test]
    fn a_long_header_is_cut_and_never_runs_into_the_step_or_out_of_its_rect() {
        let t = tokens();
        let long = "pick a window for this zone of the layout, any window you like at all";
        let step_only = text_shot(&t, "", "12 / 12", "", false);
        let base = text_shot(&t, "", "", "", false);
        let both = text_shot(&t, long, "12 / 12", "", false);
        let bb = both.diff(&base).unwrap();
        assert!(inside(bb, HEAD_PX), "{bb:?}");
        // Where the step is, the long title adds nothing.
        let sb = step_only.diff(&base).unwrap();
        assert!(both.same_in(&step_only, (sb.0 - 8, 24, 376 - (sb.0 - 8), 32)));
        // The title is still there, cut short.
        let t_only = text_shot(&t, long, "12 / 12", "", false)
            .diff(&step_only)
            .unwrap();
        assert!(
            t_only.2 < sb.0,
            "title ends before the step: {t_only:?} vs {sb:?}"
        );
        assert!(t_only.2 - t_only.0 > 150);
        // Silly: nothing fits.
        let silly = "x".repeat(300);
        let shot = text_shot(&t, &silly, &silly, "", false);
        let bb = shot.diff(&base).unwrap();
        assert!(inside(bb, HEAD_PX), "{bb:?}");
    }

    #[test]
    fn the_footer_is_dim_for_a_hint_and_warn_for_a_warning() {
        let t = tokens();
        let base = text_shot(&t, "", "", "", false);
        let hint = text_shot(&t, "", "", "enter pick · esc cancel", false);
        let warn = text_shot(&t, "", "", "can't move an admin window", true);
        for (shot, ink, other) in [(&hint, t.text_dim, t.warn), (&warn, t.warn, t.text_dim)] {
            let bb = shot.diff(&base).expect("the footer is drawn");
            assert!(inside(bb, FOOT_PX), "{bb:?}");
            assert!((24..28).contains(&bb.0), "left aligned: {bb:?}");
            assert!(shot.count_in(FOOT_PX, bgra(ink), 40) > 20);
            assert_eq!(shot.count_in(FOOT_PX, bgra(other), 6), 0);
        }
        assert_ne!(hint.px, warn.px);
        // Too long for its rect: cut inside it.
        let long = text_shot(&t, "", "", &"can't move an admin window ".repeat(8), true);
        let bb = long.diff(&base).unwrap();
        assert!(inside(bb, FOOT_PX), "{bb:?}");
        assert!(bb.2 - bb.0 > 300, "it fills the rect: {bb:?}");
    }

    // ----- cards -----

    #[test]
    fn a_card_is_a_surface1_body_with_rounded_corners_and_a_darker_plate_for_the_thumbnail() {
        let t = tokens();
        let c = card(30.0, 76.0, 160.0, "");
        let shot = cards_shot(&t, &[c]);
        let (x, y) = (30, 76);
        let fill = premul(t.pill, ZONE_FILL);
        // A surface2 hairline just outside the body, on every side (not inside it).
        let line = bgra(t.surface2);
        for (hx, hy) in [
            (x - 1, y + 60),
            (x + 160, y + 60),
            (x + 80, y - 1),
            (x + 80, y + 128),
        ] {
            assert!(near(shot.at(hx, hy), line, 3), "hairline at ({hx},{hy})");
        }
        assert!(!near(shot.at(x - 2, y + 60), line, 6), "one pixel wide");
        // The body, where nothing else is: left of the icon and in the margin of the plate.
        assert!(near(shot.at(x + 3, y + 14), bgra(t.surface1), 2));
        assert!(near(shot.at(x + 3, y + 28 + 40), bgra(t.surface1), 2));
        assert!(near(shot.at(x + 80, y + 126), bgra(t.surface1), 2));
        // The plate: in the thumb rect, darker than the body.
        let tr = px(c.thumb_rect);
        assert!(near(
            shot.at(tr.0 + tr.2 / 2, tr.1 + tr.3 / 2),
            plate(&t),
            2
        ));
        assert!(plate(&t)[2] < bgra(t.surface1)[2]);
        // The card's corners are rounded (r 10): the very corner shows the zone.
        for (cx, cy) in [(x, y), (x + 159, y), (x, y + 127), (x + 159, y + 127)] {
            assert!(near(shot.at(cx, cy), fill, 2), "corner ({cx},{cy})");
        }
        assert!(near(shot.at(x + 4, y + 4), bgra(t.surface1), 4));
        assert_eq!(shot.lit_outside(&[ZONE_PX]), None);
    }

    #[test]
    fn a_thumb_rect_that_reaches_the_cards_edges_follows_the_cards_corners() {
        let t = tokens();
        let mut c = card(30.0, 76.0, 160.0, "");
        // Flush: the whole width, down to the card's bottom edge.
        c.thumb_rect = LRect::new(30.0, 104.0, 160.0, 100.0);
        let shot = cards_shot(&t, &[c]);
        let fill = premul(t.pill, ZONE_FILL);
        assert!(
            near(shot.at(31, 150), plate(&t), 2),
            "the plate reaches the edge"
        );
        assert!(near(shot.at(188, 150), plate(&t), 2));
        assert!(near(shot.at(110, 202), plate(&t), 2));
        // The bottom corners are the card's rounding: the plate does not poke out of them.
        for (cx, cy) in [(30, 203), (189, 203)] {
            let p = shot.at(cx, cy);
            assert!(near(p, fill, 2), "corner ({cx},{cy}): {p:?}");
        }
        assert_eq!(shot.lit_outside(&[ZONE_PX]), None);
    }

    #[test]
    fn nothing_is_drawn_inside_the_thumb_rect_whatever_the_card_state() {
        let t = tokens();
        for scale in [1.0f32, 1.5] {
            for flush in [false, true] {
                let mut c = card(30.0, 76.0, 160.0, "A window with a rather long title");
                if flush {
                    c.thumb_rect = LRect::new(30.0, 104.0, 160.0, 100.0);
                }
                c.hovered = true;
                c.selected = true;
                c.elevated = true;
                let size = ((440.0 * scale) as i32, (340.0 * scale) as i32);
                let shot = paint(&t, scale, |_| {}, &HashMap::new(), size, &[c]);
                // Every pixel of the plate, but the 3 px along its edge (anti-aliasing).
                let r = c.thumb_rect;
                let (x0, y0) = (((r.x * scale) as i32) + 3, ((r.y * scale) as i32) + 3);
                let (x1, y1) = (
                    (((r.x + r.w) * scale) as i32) - 3,
                    (((r.y + r.h) * scale) as i32) - 3,
                );
                // Flush plates meet the card's rounded bottom corners: leave those out.
                let y1 = if flush { y1 - 8 } else { y1 };
                for y in y0..y1 {
                    for x in x0..x1 {
                        assert!(
                            near(shot.at(x, y), plate(&t), 1),
                            "scale {scale} flush {flush} ({x},{y}): {:?}",
                            shot.at(x, y)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn hover_is_a_solid_accent_outline_and_the_keyboard_a_dashed_one_outside_the_card() {
        let t = tokens();
        let plain = card(30.0, 76.0, 160.0, "Notes");
        let hover = AssistCard {
            hovered: true,
            ..plain
        };
        let picked = AssistCard {
            selected: true,
            ..plain
        };
        let both = AssistCard {
            hovered: true,
            selected: true,
            ..plain
        };
        let none = cards_shot(&t, &[plain]);
        let (h, k, hk) = (
            cards_shot(&t, &[hover]),
            cards_shot(&t, &[picked]),
            cards_shot(&t, &[both]),
        );
        let (acc, ink) = (bgra(t.accent), bgra(t.text));
        // The card itself is untouched by either: all drawing is outside its rect. (An
        // outline hugs the rounded corners, so it passes through the corner notches of
        // the card's bounding box: leave the 10 px corners out.)
        let (cx, cy, cw, ch) = px(plain.rect);
        for s in [&h, &k, &hk] {
            assert!(none.same_in(s, (cx + 10, cy, cw - 20, ch)));
            assert!(none.same_in(s, (cx, cy + 10, cw, ch - 20)));
        }
        // Along the top edge, 3 px out (the outline's centre line): the row just above
        // the card's top (y = 76) is 3..=4 px away.
        let row = 76 - 4;
        let span = 60..130; // straight part of the edge
        let on = |s: &Shot, c: [u8; 4]| span.clone().filter(|&x| near(s.at(x, row), c, 3)).count();
        // Hover: continuous accent, no text colour.
        assert_eq!(on(&h, acc), span.len(), "solid");
        assert_eq!(on(&h, ink), 0);
        // Keyboard: dashes in the text colour with gaps, no accent.
        let dashes = on(&k, ink);
        assert!(dashes > 20 && dashes < span.len() - 10, "dashed: {dashes}");
        assert_eq!(on(&k, acc), 0);
        // On the sides too, 3 px out.
        assert!(near(h.at(30 - 4, 140), acc, 3) && near(h.at(30 - 3, 140), acc, 3));
        assert!(near(h.at(189 + 3, 140), acc, 3) && near(h.at(189 + 4, 140), acc, 3));
        // Both at once: the accent outline stays where it was, the dashes go outside it.
        assert_eq!(on(&hk, acc), span.len());
        let outer = 76 - 8;
        let dashes = span
            .clone()
            .filter(|&x| near(hk.at(x, outer), ink, 3))
            .count();
        assert!(
            dashes > 20 && dashes < span.len() - 10,
            "outer dashes: {dashes}"
        );
        // The outlines are rounded and stay inside the zone.
        for s in [&h, &k, &hk] {
            assert_eq!(s.lit_outside(&[ZONE_PX]), None);
        }
        // Not hovered, not selected: no outline.
        assert_eq!(none.count_in((20, 66, 180, 140), acc, 3), 0);
    }

    #[test]
    fn the_admin_chip_is_warn_at_the_right_of_the_title_row_and_the_title_gives_way() {
        let t = tokens();
        let title = "Windows PowerShell (Administrator) - a long title";
        let plain = card(30.0, 76.0, 160.0, title);
        let admin = AssistCard {
            elevated: true,
            ..plain
        };
        let (a, b) = (cards_shot(&t, &[plain]), cards_shot(&t, &[admin]));
        let row = px(plain.title_rect);
        // Only the title row changes, and the chip is warn coloured inside it.
        let bb = b.diff(&a).expect("the chip is drawn");
        assert!(inside(bb, row), "{bb:?} vs {row:?}");
        assert_eq!(a.count_in(row, bgra(t.warn), 3), 0);
        assert!(b.count_in(row, bgra(t.warn), 3) > 150, "a filled chip");
        // At the right end of the row, 14 px tall.
        assert!((row.0 + row.2 - 2..=row.0 + row.2).contains(&bb.2));
        let chip: Vec<i32> = (row.1..row.1 + row.3)
            .filter(|&y| near(b.at(row.0 + row.2 - 3, y), bgra(t.warn), 3))
            .collect();
        assert!(
            (12..=14).contains(&chip.len()),
            "chip height {}",
            chip.len()
        );
        // Its label is in the pill colour on the warn fill.
        assert!(b.count_in(row, bgra(t.pill), 20) > 15, "the label");
        // The title is cut before the chip, never behind it: no title ink in the last
        // 34 px of the row, and some at the left.
        let tail = (row.0 + row.2 - 34, row.1, 34, row.3);
        assert_eq!(b.count_in(tail, bgra(t.text), 6), 0);
        assert!(b.count_in((row.0, row.1, 60, row.3), bgra(t.text), 40) > 30);
        // And a short title keeps its place, left of the chip.
        let short = AssistCard {
            title: "Notes",
            ..admin
        };
        let s = cards_shot(&t, &[short]);
        assert!(s.count_in((row.0, row.1, 40, row.3), bgra(t.text), 40) > 15);
    }

    #[test]
    fn the_admin_label_is_whole_in_every_theme_font_and_cut_only_when_it_cannot_fit() {
        let reg = ThemeRegistry::with_builtins();
        let mut fonts = Fonts::new().unwrap();
        for id in reg.ids() {
            let t = reg.get(id).unwrap();
            for s in [1.0, 1.25, 1.5, 2.0] {
                let font = fonts.font(&t.font_label, Weight::Medium, CHIP_SIZE * s);
                let tracking = CHIP_TRACKING * CHIP_SIZE * s;
                let label_w = text_width(&font, ADMIN_LABEL, tracking);
                // Plenty of room: the label and its padding.
                let (w, label) = admin_chip(&font, tracking, s, 120.0 * s);
                assert_eq!(label, ADMIN_LABEL, "{id} at {s}");
                assert!((w - (label_w + 2.0 * CHIP_PAD * s)).abs() < 0.01, "{id}");
                // Exactly as much room as it needs: still whole (no float rounding cut).
                let (w2, label2) = admin_chip(&font, tracking, s, w);
                assert_eq!((w2, label2.as_str()), (w, ADMIN_LABEL), "{id} at {s}");
                // Less: the whole row, the label cut to fit its padding.
                let room = w - 6.0 * s;
                let (w3, label3) = admin_chip(&font, tracking, s, room);
                assert!((w3 - room).abs() < 0.01);
                assert_ne!(label3, ADMIN_LABEL);
                assert!(text_width(&font, &label3, tracking) <= room - 2.0 * CHIP_PAD * s + 0.01);
            }
        }
        // No room at all: an empty chip, no label.
        let font = fonts.font("Geist Mono", Weight::Medium, CHIP_SIZE);
        assert_eq!(admin_chip(&font, 0.0, 1.0, 0.0), (0.0, String::new()));
        assert_eq!(admin_chip(&font, 0.0, 1.0, -5.0), (0.0, String::new()));
    }

    #[test]
    fn the_title_is_cut_to_its_rect_and_never_spills() {
        let t = tokens();
        let long = "A very long window title that cannot possibly fit in a small card at all";
        let blank = cards_shot(&t, &[card(30.0, 76.0, 160.0, "")]);
        let c = card(30.0, 76.0, 160.0, long);
        let shot = cards_shot(&t, &[c]);
        let row = px(c.title_rect);
        let bb = shot.diff(&blank).expect("a title is drawn");
        assert!(inside(bb, row), "{bb:?} vs {row:?}");
        assert!(bb.2 - bb.0 > 80, "most of the row is used: {bb:?}");
        assert!(shot.count_in(row, bgra(t.text), 40) > 100);
        // A rect shorter than the text: clipped to it, nothing leaves it.
        let mut thin = c;
        thin.title_rect = LRect::new(64.0, 80.0, 100.0, 5.0);
        let s = cards_shot(&t, &[thin]);
        let bb = s.diff(&blank).expect("clipped text");
        assert!(inside(bb, (64, 80, 100, 5)), "{bb:?}");
        // A rect narrower than any glyph run: no text at all.
        let mut tiny = c;
        tiny.title_rect = LRect::new(64.0, 80.0, 2.0, 16.0);
        let s = cards_shot(&t, &[tiny]);
        assert!(s.diff(&blank).is_none_or(|bb| inside(bb, (64, 80, 2, 16))));
    }

    #[test]
    fn a_cached_icon_is_drawn_at_the_icon_rect_and_a_missing_one_is_a_blank_tile() {
        let t = tokens();
        let mut icons = HashMap::new();
        icons.insert(7u64, flat_icon(32, RED));
        let mut c = card(30.0, 76.0, 160.0, "");
        c.icon = Some(7);
        let shot = paint(&t, 1.0, |_| {}, &icons, SURFACE, &[c]);
        let ir = px(c.icon_rect);
        assert!(
            shot.count_in(ir, RED, 12) > 340,
            "a solid 20 px icon: {}",
            shot.count_in(ir, RED, 12)
        );
        assert_eq!(
            shot.count_in((30, 76, 160, 128), RED, 40),
            shot.count_in(ir, RED, 40)
        );
        // Not in the cache: a dim blank tile, no red.
        c.icon = Some(8);
        let blank = paint(&t, 1.0, |_| {}, &icons, SURFACE, &[c]);
        assert_eq!(blank.count_in((30, 76, 160, 128), RED, 40), 0);
        let body = blank.at(ir.0 + 10, ir.1 + 10);
        assert!(
            !near(body, bgra(t.surface1), 4),
            "a tile is drawn: {body:?}"
        );
        assert!(near(body, blend(bgra(t.surface1), t.text_dim, 0.35), 3));
        // No key at all: the same tile.
        c.icon = None;
        let none = paint(&t, 1.0, |_| {}, &icons, SURFACE, &[c]);
        assert_eq!(none.px, blank.px);
        // The icon stays inside its rect.
        let c0 = AssistCard { icon: None, ..c };
        let base = cards_shot(
            &t,
            &[AssistCard {
                icon_rect: LRect::new(0.0, 0.0, 0.0, 0.0),
                ..c0
            }],
        );
        let with = paint(
            &t,
            1.0,
            |_| {},
            &icons,
            SURFACE,
            &[AssistCard {
                icon: Some(7),
                ..c0
            }],
        );
        assert!(inside(with.diff(&base).unwrap(), ir));
    }

    #[test]
    fn a_card_without_a_thumbnail_shows_its_icon_large_with_a_hint() {
        let t = tokens();
        let mut icons = HashMap::new();
        icons.insert(7u64, flat_icon(64, RED));
        let mut c = card(30.0, 76.0, 160.0, "Calculator");
        c.icon = Some(7);
        let live = paint(&t, 1.0, |_| {}, &icons, SURFACE, &[c]);
        let none = paint(
            &t,
            1.0,
            |_| {},
            &icons,
            SURFACE,
            &[AssistCard {
                no_thumb: true,
                ..c
            }],
        );
        let tr = px(c.thumb_rect);
        // With a live thumbnail only the plate: no icon, no hint.
        assert_eq!(live.count_in(tr, RED, 40), 0);
        assert_eq!(live.count_in(tr, bgra(t.text), 60), 0);
        // Without: a big red icon (about 39 px square: 42 % of the 94 px plate) ...
        let red = none.count_in(tr, RED, 12);
        assert!((1100..=1700).contains(&red), "big icon: {red} px");
        // ... centred across the plate and above its middle, with the hint under it.
        let xs: Vec<i32> = (tr.0..tr.0 + tr.2)
            .filter(|&x| (tr.1..tr.1 + tr.3).any(|y| near(none.at(x, y), RED, 12)))
            .collect();
        let mid = (xs[0] + xs[xs.len() - 1]) as f32 / 2.0;
        assert!(
            (mid - (tr.0 + tr.2 / 2) as f32).abs() < 2.0,
            "centred: {mid}"
        );
        let hint = (tr.0, tr.1 + tr.3 / 2 + 15, tr.2, tr.3 / 2 - 15);
        assert!(
            none.count_in(hint, bgra(t.text), 60) > 20,
            "the hint is text coloured"
        );
        // All of it inside the thumb rect: the rest of the card is as with a thumbnail.
        let bb = none.diff(&live).unwrap();
        assert!(inside(bb, tr), "{bb:?} vs {tr:?}");
        // No cached icon: the blank tile, large.
        let blank = paint(
            &t,
            1.0,
            |_| {},
            &HashMap::new(),
            SURFACE,
            &[AssistCard {
                no_thumb: true,
                ..c
            }],
        );
        // (Against a live card with the same, empty, cache: the title row is the same.)
        let live_blank = paint(&t, 1.0, |_| {}, &HashMap::new(), SURFACE, &[c]);
        assert!(inside(blank.diff(&live_blank).unwrap(), tr));
        let tile = blend(plate(&t), t.text_dim, 0.35);
        assert!(blank.count_in(tr, tile, 4) > 800);
    }

    #[test]
    fn a_short_plate_shows_only_the_large_icon() {
        let t = tokens();
        let mut icons = HashMap::new();
        icons.insert(7u64, flat_icon(32, RED));
        let mut c = card(30.0, 76.0, 120.0, "x");
        c.icon = Some(7);
        c.no_thumb = true;
        // A 40 px tall plate: too short for a hint.
        c.thumb_rect = LRect::new(36.0, 104.0, 108.0, 40.0);
        let shot = paint(&t, 1.0, |_| {}, &icons, SURFACE, &[c]);
        let tr = px(c.thumb_rect);
        let red = shot.count_in(tr, RED, 12);
        assert!(red > 150, "icon: {red}");
        assert_eq!(shot.count_in(tr, bgra(t.text), 60), 0, "no hint");
        // The icon is at most 48 px, and on a big plate it stops there.
        let mut big = card(30.0, 76.0, 300.0, "x");
        big.icon = Some(7);
        big.no_thumb = true;
        big.thumb_rect = LRect::new(36.0, 104.0, 288.0, 180.0);
        let mut sc_size = ZONE;
        sc_size.0 = 380.0;
        sc_size.1 = 320.0;
        let shot = paint(&t, 1.0, |sc| sc.size = sc_size, &icons, SURFACE, &[big]);
        let red = shot.count_in(px(big.thumb_rect), RED, 12);
        assert!((2000..=2400).contains(&red), "48 x 48 icon: {red}");
    }

    #[test]
    fn cards_outside_the_zone_or_degenerate_are_clipped_or_skipped() {
        let t = tokens();
        // Half out of the zone on the right and the bottom: clipped to the rounded zone.
        let mut c = card(300.0, 200.0, 160.0, "Partly outside the zone");
        c.hovered = true;
        c.selected = true;
        c.elevated = true;
        c.no_thumb = true;
        let shot = cards_shot(&t, &[c]);
        assert_eq!(shot.lit_outside(&[ZONE_PX]), None);
        // The zone's own outline is on top of the card.
        assert!(near(shot.at(399, 250), bgra(t.accent), 2));
        // Degenerate cards draw nothing and do not panic.
        let odd = [
            AssistCard {
                rect: LRect::new(10.0, 10.0, 0.0, 40.0),
                ..c
            },
            AssistCard {
                rect: LRect::new(10.0, 10.0, -20.0, 40.0),
                ..c
            },
            AssistCard {
                rect: LRect::new(f32::NAN, 10.0, 40.0, 40.0),
                ..c
            },
            AssistCard {
                rect: LRect::new(10.0, 10.0, 1.0, 1.0),
                ..c
            },
            AssistCard {
                thumb_rect: LRect::new(10.0, 10.0, f32::NAN, 40.0),
                ..c
            },
            AssistCard {
                title_rect: LRect::new(10.0, 10.0, 0.0, 0.0),
                ..c
            },
            AssistCard {
                icon_rect: LRect::new(10.0, 10.0, -1.0, 4.0),
                ..c
            },
        ];
        for d in odd {
            let s = cards_shot(&t, &[d]);
            assert_eq!(s.lit_outside(&[ZONE_PX]), None);
        }
        let empty = cards_shot(&t, &[odd[0], odd[1], odd[2], odd[3]]);
        assert_eq!(
            empty.px,
            cards_shot(&t, &[]).px,
            "nothing drawn for those four"
        );
    }

    #[test]
    fn at_150_percent_the_card_scales_with_its_rects() {
        let t = tokens();
        let mut c = card(30.0, 76.0, 160.0, "Notes");
        c.hovered = true;
        let shot = paint(&t, 1.5, |_| {}, &HashMap::new(), (640, 490), &[c]);
        // Card at (45, 114) device px, 240 x 192.
        assert!(near(shot.at(45 + 5, 114 + 21), bgra(t.surface1), 3));
        // The hover outline: 3 px wide, centre line 4.5 px out, so it covers 3 to 6 px
        // out: the pixel columns 39, 40 and 41 (column 44 is the first one out).
        let acc = bgra(t.accent);
        for x in 39..=41 {
            assert!(near(shot.at(x, 200), acc, 3), "ring at column {x}");
        }
        assert!(!near(shot.at(42, 200), acc, 20));
        assert!(!near(shot.at(38, 200), acc, 20));
        // The plate keeps to the (scaled) thumb rect.
        assert!(near(shot.at(45 + 120, 114 + 42 + 70), plate(&t), 2));
        assert_eq!(shot.lit_outside(&[(0, 0, 600, 450)]), None);
    }

    // ----- fonts and themes -----

    #[test]
    fn the_texts_use_glyphs_the_embedded_fonts_have() {
        let mut fonts = Fonts::new().unwrap();
        let t = tokens();
        let mono = fonts.typeface(&t.font_label, Weight::Medium);
        let title = fonts.typeface(&t.font_title, Weight::Medium);
        // The texts this module draws itself, and what the app puts in the header and
        // footer: a middle dot, the arrows and plain ASCII.
        let own = format!("{ADMIN_LABEL}{NO_THUMB_HINT}");
        let app = "pick a window · zone 1 of 2  1 / 2  enter pick · esc cancel  \
                   click or ← ↑ ↓ → · can't move an admin window  3 more not shown";
        for c in own.chars().chain(app.chars()) {
            assert_ne!(
                mono.unichar_to_glyph(c as i32),
                0,
                "{} lacks {c}",
                t.font_label
            );
        }
        // The title row is Geist: letters, digits and the usual title punctuation.
        for c in "Windows PowerShell (Administrator) - Notes.txt 0123456789 ·_:/".chars() {
            assert_ne!(
                title.unichar_to_glyph(c as i32),
                0,
                "{} lacks {c}",
                t.font_title
            );
        }
        // The ellipsis `fit` writes is two ASCII dots, never a missing glyph.
        assert_ne!(mono.unichar_to_glyph('.' as i32), 0);
    }

    #[test]
    fn text_reads_on_the_card_and_on_the_plate_in_every_builtin_theme() {
        let reg = ThemeRegistry::with_builtins();
        for id in reg.ids() {
            let t = reg.get(id).unwrap();
            let body = t.surface1;
            let k = 1.0 - PLATE_SHADE;
            // Skia blends the 8-bit sRGB values: black at PLATE_SHADE over the card.
            let dark = Rgb::new(body.r * k, body.g * k, body.b * k);
            assert!(t.text.contrast(body) >= 4.5, "{id}: title on the card");
            assert!(t.text.contrast(dark) >= 4.5, "{id}: the hint on the plate");
            assert!(
                dark.relative_luminance() < body.relative_luminance(),
                "{id}: the plate is darker than the card"
            );
        }
    }

    #[test]
    fn every_builtin_theme_draws_inside_its_zone() {
        let reg = ThemeRegistry::with_builtins();
        for id in reg.ids() {
            let t = reg.get(id).unwrap();
            let mut c = card(30.0, 76.0, 160.0, "Notes");
            c.hovered = true;
            c.selected = true;
            c.elevated = true;
            c.no_thumb = true;
            let shot = paint(
                t,
                1.0,
                |sc| {
                    sc.title = "pick a window · zone 1 of 2";
                    sc.step = "1 / 2";
                    sc.footer_text = "enter pick · esc cancel";
                },
                &HashMap::new(),
                SURFACE,
                &[c],
            );
            assert_eq!(shot.lit_outside(&[ZONE_PX]), None, "{id}");
            assert!(near(shot.at(200, 5), premul(t.pill, ZONE_FILL), 2), "{id}");
            assert!(near(shot.at(0, 150), bgra(t.accent), 2), "{id}");
        }
        // The light theme's plate is still darker than its card.
        let paper = theme("paper-zine");
        let shot = cards_shot(&paper, &[card(30.0, 76.0, 160.0, "")]);
        let (card_px, plate_px) = (shot.at(33, 90), shot.at(110, 150));
        assert!(plate_px[2] < card_px[2] && plate_px[1] < card_px[1]);
    }

    // ----- the icon bitmap -----

    #[test]
    fn an_icon_bitmap_must_be_exactly_size_squared_premultiplied_bgra() {
        let ok = vec![255u8; 4 * 4 * 4];
        let img = icon_raster(&ok, 4).expect("4 x 4");
        assert_eq!((img.width(), img.height()), (4, 4));
        assert!(icon_raster(&ok, 0).is_none(), "no size");
        assert!(icon_raster(&ok, 3).is_none(), "wrong length");
        assert!(icon_raster(&ok[..63], 4).is_none(), "short");
        let mut long = ok.clone();
        long.push(0);
        assert!(icon_raster(&long, 4).is_none(), "long");
        assert!(icon_raster(&[], 4).is_none());
        assert!(icon_raster(&[], 0).is_none());
        assert!(icon_raster(&ok, MAX_ICON + 1).is_none(), "too big");
        // The pixels are kept as given: a half transparent red stays premultiplied.
        let px = [0u8, 0, 128, 128].repeat(4);
        let img = icon_raster(&px, 2).unwrap();
        let info = ImageInfo::new((2, 2), ColorType::BGRA8888, AlphaType::Premul, None);
        let mut out = vec![0u8; 16];
        assert!(img.read_pixels(&info, &mut out, 8, (0, 0), CachingHint::Disallow));
        assert_eq!(out, px);
    }
}
