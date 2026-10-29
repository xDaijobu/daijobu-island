//! Shared pieces of the two popup windows (Settings and Spotlight): the panel chrome
//! (shadow, body, grain, hairline), the text field, the icon cache and `PopupRenderer`,
//! the GPU side that presents either view on its own transparent window.

use crate::fonts::{Fonts, Weight};
use crate::fx::Effects;
use crate::gpu::{shared_gpu, Chain, Gpu, RenderError};
use crate::paint::{col, draw_text, fill_paint, stroke_paint, text_width};
use dj_core::color::Rgb;
use dj_core::layout::Rect as LRect;
use dj_core::settings_ui::{FieldView, ViewModel};
use dj_core::spotlight::SpotlightView;
use dj_core::theme::ResolvedTokens;
use skia_safe::paint::Cap;
use skia_safe::{
    images, AlphaType, BlurStyle, Canvas, ClipOp, Color, ColorType, Data, Image, ImageInfo,
    MaskFilter, Paint, RRect, Rect,
};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

/// Opacity of a control that does nothing right now.
pub(crate) const DIMMED: f32 = 0.4;

/// Logical rect to device px.
pub(crate) fn dev(r: LRect, s: f32) -> Rect {
    Rect::from_xywh(r.x * s, r.y * s, r.w * s, r.h * s)
}

/// Text that reads on `bg`: the theme's text colour, or the pill colour on a light fill.
pub(crate) fn ink_on(t: &ResolvedTokens, bg: Rgb) -> Rgb {
    if t.text.contrast(bg) >= t.pill.contrast(bg) {
        t.text
    } else {
        t.pill
    }
}

thread_local! {
    /// The grain shader, compiled on first use on the thread that paints.
    static EFFECTS: RefCell<Option<Effects>> = const { RefCell::new(None) };
}

/// Film grain (and scanlines) over `area`, the same shader the island uses. A still
/// frame: the popups only redraw on input. Skipped when the theme has none.
fn grain(canvas: &Canvas, t: &ResolvedTokens, area: &RRect) {
    if t.grain <= 0.001 && t.scanlines <= 0.001 {
        return;
    }
    EFFECTS.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Effects::new().ok();
        }
        let Some(fx) = slot.as_mut() else { return };
        if let Some(shader) = fx.texture(7.0, t.grain, t.scanlines) {
            let mut p = Paint::default();
            p.set_shader(shader);
            canvas.save();
            canvas.clip_rrect(*area, ClipOp::Intersect, true);
            canvas.draw_rect(area.rect(), &p);
            canvas.restore();
        }
    });
}

/// Shadow, body in the pill colour, grain and a hairline for `panel` (logical px).
pub(crate) fn paint_panel(canvas: &Canvas, t: &ResolvedTokens, s: f32, panel: LRect, radius: f32) {
    let r = dev(panel, s);
    let rad = radius * s;
    // Shadow: blurred dark rrect, lower than the panel. Stays inside the 24 px margin.
    let mut sh = fill_paint(Color::from_argb(110, 0, 0, 0));
    sh.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 6.0 * s, false));
    canvas.draw_rrect(
        RRect::new_rect_xy(r.with_offset((0.0, 4.0 * s)), rad, rad),
        &sh,
    );
    let body = RRect::new_rect_xy(r, rad, rad);
    // Knock the part of the shadow under the body out first, so a see-through pill colour
    // would not show it; the pill colour is opaque, so just paint over.
    canvas.draw_rrect(body, &fill_paint(col(t.pill, 1.0)));
    grain(canvas, t, &body);
    let hair = t.hairline.max(0.12);
    canvas.draw_rrect(
        RRect::new_rect_xy(
            r.with_inset((0.5 * s, 0.5 * s)),
            rad - 0.5 * s,
            rad - 0.5 * s,
        ),
        &stroke_paint(col(t.text, hair), s.max(1.0)),
    );
}

/// A magnifier glyph centred on `(cx, cy)`, `size` device px wide.
pub(crate) fn draw_magnifier(canvas: &Canvas, cx: f32, cy: f32, size: f32, color: Color, s: f32) {
    let r = size * 0.36;
    let mut p = stroke_paint(color, 1.6 * s);
    p.set_stroke_cap(Cap::Round);
    let c = (cx - size * 0.1, cy - size * 0.1);
    canvas.draw_circle(c, r, &p);
    let d = r * 0.72;
    canvas.draw_line(
        (c.0 + d, c.1 + d),
        (c.0 + d + size * 0.26, c.1 + d + size * 0.26),
        &p,
    );
}

/// A one-line text field: plate, magnifier (when `glyph`), text or placeholder and a caret
/// while focused. Text starts at `text_x` (device px).
pub(crate) fn paint_field(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    s: f32,
    f: &FieldView,
    plate: bool,
    size_px: f32,
) {
    let r = dev(f.rect, s);
    let rad = (6.0 * s).min(r.height() / 2.0);
    if plate {
        let rr = RRect::new_rect_xy(r, rad + 2.0 * s, rad + 2.0 * s);
        canvas.draw_rrect(rr, &fill_paint(col(t.surface1, 1.0)));
        let edge = if f.focused { t.accent } else { t.surface2 };
        canvas.draw_rrect(
            RRect::new_rect_xy(
                r.with_inset((0.5 * s, 0.5 * s)),
                rad + 1.5 * s,
                rad + 1.5 * s,
            ),
            &stroke_paint(col(edge, 1.0), s.max(1.0)),
        );
    }
    let size = size_px * s;
    let font = fonts.font(&t.font_label, Weight::Medium, size);
    draw_magnifier(
        canvas,
        r.left + 16.0 * s,
        r.center_y(),
        15.0 * s,
        col(t.text_dim, 1.0),
        s,
    );
    let tx = r.left + 32.0 * s;
    let by = r.center_y() + 0.35 * size;
    if f.text.is_empty() {
        draw_text(
            canvas,
            &font,
            f.placeholder,
            tx,
            by,
            0.0,
            &fill_paint(col(t.text_dim, 0.8)),
        );
        if f.focused {
            canvas.draw_rect(
                Rect::from_xywh(tx, r.center_y() - 8.0 * s, s.max(1.0), 16.0 * s),
                &fill_paint(col(t.accent, 1.0)),
            );
        }
        return;
    }
    let max_w = r.right - tx - 12.0 * s;
    canvas.save();
    canvas.clip_rect(
        Rect::from_xywh(tx, r.top, max_w.max(1.0), r.height()),
        ClipOp::Intersect,
        false,
    );
    // Keep the caret visible: scroll the text left when it runs past the field.
    let before: String = f.text.chars().take(f.caret).collect();
    let cw = text_width(&font, &before, 0.0);
    let shift = (cw - max_w + 2.0 * s).max(0.0);
    draw_text(
        canvas,
        &font,
        &f.text,
        tx - shift,
        by,
        0.0,
        &fill_paint(col(t.text, 1.0)),
    );
    if f.focused {
        canvas.draw_rect(
            Rect::from_xywh(
                tx - shift + cw,
                r.center_y() - 8.0 * s,
                s.max(1.0),
                16.0 * s,
            ),
            &fill_paint(col(t.accent, 1.0)),
        );
    }
    canvas.restore();
}

// ---------------------------------------------------------------------------------------

/// Most icons kept; the oldest goes first.
pub const ICON_CACHE_MAX: usize = 512;

/// App and file icons as Skia images, keyed by `Entry::icon_key`.
#[derive(Default)]
pub struct IconCache {
    map: HashMap<String, Image>,
    order: VecDeque<String>,
}

impl IconCache {
    pub fn pixel_bytes_estimate(&self) -> u64 {
        self.map
            .values()
            .map(|i| i.width() as u64 * i.height() as u64 * 4)
            .sum()
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an icon from a `w` x `h` premultiplied BGRA top-down bitmap. Ignored when the
    /// size is 0 or the byte count is wrong. A new key past the cap drops the oldest.
    pub fn insert_bgra(&mut self, key: &str, w: u32, h: u32, bgra: &[u8]) {
        if w == 0 || h == 0 || w > 1024 || h > 1024 || bgra.len() != w as usize * h as usize * 4 {
            return;
        }
        let info = ImageInfo::new(
            (w as i32, h as i32),
            ColorType::BGRA8888,
            AlphaType::Premul,
            None,
        );
        let Some(img) = images::raster_from_data(&info, Data::new_copy(bgra), w as usize * 4)
        else {
            return;
        };
        if self.map.insert(key.to_string(), img).is_none() {
            self.order.push_back(key.to_string());
            while self.map.len() > ICON_CACHE_MAX {
                match self.order.pop_front() {
                    Some(old) => {
                        self.map.remove(&old);
                    }
                    None => break,
                }
            }
        }
    }

    pub fn contains(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    pub fn get(&self, key: &str) -> Option<&Image> {
        self.map.get(key)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

// ---------------------------------------------------------------------------------------

/// The GPU side of a popup window: shared island context, a premultiplied
/// composition swap chain (transparent margins stay transparent) and fonts.
pub struct PopupRenderer {
    // Field order is drop order; `Drop` clears the surfaces first.
    fonts: Fonts,
    chain: Option<Chain>,
    gpu: Rc<RefCell<Gpu>>,
    hwnd: isize,
    size: (u32, u32),
}

fn missing() -> RenderError {
    RenderError::Setup("popup swap chain missing".into())
}

impl PopupRenderer {
    /// `w` x `h` are the window's client size in device px.
    pub fn new(hwnd: isize, w: u32, h: u32) -> Result<Self, RenderError> {
        let fonts = Fonts::new().map_err(RenderError::Setup)?;
        let gpu = shared_gpu()?;
        let chain = Chain::new(&mut gpu.borrow_mut(), hwnd, w, h)?;
        Ok(Self {
            fonts,
            gpu,
            chain: Some(chain),
            hwnd,
            size: (w, h),
        })
    }

    /// Rebuilds the swap chain for a new client size (a DPI change). No-op if unchanged.
    pub fn resize(&mut self, w: u32, h: u32) -> Result<(), RenderError> {
        if (w, h) == self.size && self.chain.is_some() && self.gpu.borrow().is_valid() {
            return Ok(());
        }
        let gpu = shared_gpu()?;
        // The old chain (and its DirectComposition target) must be gone before the new one
        // binds the same window.
        if let Some(mut old) = self.chain.take() {
            old.surfaces.clear();
            self.gpu.borrow_mut().ctx.flush_submit_and_sync_cpu();
        }
        self.gpu = gpu;
        self.chain = Some(Chain::new(&mut self.gpu.borrow_mut(), self.hwnd, w, h)?);
        self.size = (w, h);
        Ok(())
    }

    pub fn render_tray_menu(
        &mut self,
        menu: &dj_core::tray_menu::Menu,
        t: &ResolvedTokens,
        s: f32,
    ) -> Result<(), RenderError> {
        use dj_core::tray_menu::{MARGIN, ROW_HEIGHT, WIDTH};
        self.resize(self.size.0, self.size.1)?;
        let mut gpu = self.gpu.borrow_mut();
        let chain = self.chain.as_mut().ok_or_else(missing)?;
        let canvas = chain.back_buffer().canvas();
        canvas.clear(Color::TRANSPARENT);
        paint_panel(
            canvas,
            t,
            s,
            LRect {
                x: 8.0,
                y: 8.0,
                w: WIDTH - 16.0,
                h: menu.height() - 16.0,
            },
            10.0,
        );
        let font = self.fonts.font(&t.font_label, Weight::Medium, 12.0 * s);
        let title = if menu.is_submenu() {
            format!("‹  {}", menu.title())
        } else {
            menu.title().to_string()
        };
        draw_text(
            canvas,
            &font,
            &title,
            (MARGIN + 8.0) * s,
            (MARGIN + 21.0) * s,
            0.0,
            &fill_paint(col(t.text_dim, 1.0)),
        );
        for (visible, (i, row)) in menu
            .rows()
            .iter()
            .enumerate()
            .skip(menu.offset)
            .take(menu.visible_count)
            .enumerate()
        {
            let y = MARGIN + ROW_HEIGHT * (visible + 1) as f32;
            let selected = menu.selected == Some(i) && row.enabled;
            let ink = if selected {
                ink_on(t, t.surface2)
            } else {
                t.text
            };
            if selected {
                canvas.draw_round_rect(
                    dev(
                        LRect {
                            x: MARGIN,
                            y,
                            w: WIDTH - 2.0 * MARGIN,
                            h: ROW_HEIGHT,
                        },
                        s,
                    ),
                    5.0 * s,
                    5.0 * s,
                    &fill_paint(col(t.surface2, 1.0)),
                );
            }
            let alpha = if row.enabled { 1.0 } else { DIMMED };
            canvas.save();
            canvas.clip_rect(
                dev(
                    LRect {
                        x: MARGIN + 28.0,
                        y,
                        w: WIDTH - 2.0 * MARGIN - 52.0,
                        h: ROW_HEIGHT,
                    },
                    s,
                ),
                ClipOp::Intersect,
                true,
            );
            draw_text(
                canvas,
                &font,
                &row.label,
                (MARGIN + 28.0) * s,
                (y + 21.0) * s,
                0.0,
                &fill_paint(col(ink, alpha)),
            );
            canvas.restore();
            if row.checked {
                draw_text(
                    canvas,
                    &font,
                    "✓",
                    (MARGIN + 8.0) * s,
                    (y + 21.0) * s,
                    0.0,
                    &fill_paint(col(t.accent, alpha)),
                );
            }
            if !row.children.is_empty() {
                draw_text(
                    canvas,
                    &font,
                    "›",
                    (WIDTH - MARGIN - 16.0) * s,
                    (y + 21.0) * s,
                    0.0,
                    &fill_paint(col(ink, alpha)),
                );
            }
        }
        if menu.rows().len() > menu.visible_count {
            let hint = format!(
                "{}–{} / {}",
                menu.offset + 1,
                (menu.offset + menu.visible_count).min(menu.rows().len()),
                menu.rows().len()
            );
            draw_text(
                canvas,
                &font,
                &hint,
                (MARGIN + 8.0) * s,
                (menu.height() - 14.0) * s,
                0.0,
                &fill_paint(col(t.text_dim, 1.0)),
            );
        }
        chain.present(&mut gpu.ctx)
    }

    pub fn memory_counters(&self) -> Vec<dj_core::dev_memory::Row> {
        use dj_core::dev_memory::{bytes, Row};
        let surfaces = self.chain.as_ref().map_or(0, |c| c.surfaces.len());
        vec![
            Row::resource(
                "Display buffers",
                format!(
                    "~{} GPU",
                    bytes(Some(
                        u64::from(self.size.0) * u64::from(self.size.1) * 4 * surfaces as u64
                    ))
                ),
                format!("{surfaces} buffers compose popup; pixel estimate excludes alignment."),
            ),
            self.fonts.memory_counters(),
        ]
    }

    pub fn render_memory(
        &mut self,
        rows: &[dj_core::dev_memory::Row],
        offset: usize,
        t: &ResolvedTokens,
        s: f32,
        logical: (f32, f32),
    ) -> Result<(), RenderError> {
        self.resize(self.size.0, self.size.1)?;
        let mut gpu = self.gpu.borrow_mut();
        let chain = self.chain.as_mut().ok_or_else(missing)?;
        let canvas = chain.back_buffer().canvas();
        canvas.clear(Color::TRANSPARENT);
        paint_panel(
            canvas,
            t,
            s,
            LRect::new(8.0, 8.0, logical.0 - 16.0, logical.1 - 16.0),
            12.0,
        );
        use dj_core::dev_memory::{bytes, visible_rows, BODY_TOP, ROW_HEIGHT};
        let font = self.fonts.font(&t.font_label, Weight::Medium, 12.0 * s);
        let heading = self.fonts.font(&t.font_label, Weight::SemiBold, 18.0 * s);
        let width = logical.0 - 48.0;
        let wide = width >= 740.0;
        let text = |value: &str, x: f32, y: f32, w: f32, ink: Rgb| {
            canvas.save();
            canvas.clip_rect(
                Rect::from_xywh(x * s, (y - 15.0) * s, w.max(1.0) * s, 20.0 * s),
                ClipOp::Intersect,
                true,
            );
            let mut label = value.to_string();
            if text_width(&font, &label, 0.0) > w * s {
                while !label.is_empty() && text_width(&font, &format!("{label}…"), 0.0) > w * s {
                    label.pop();
                }
                label.push('…');
            }
            draw_text(
                canvas,
                &font,
                &label,
                x * s,
                y * s,
                0.0,
                &fill_paint(col(ink, 1.0)),
            );
            canvas.restore();
        };
        draw_text(
            canvas,
            &heading,
            "Memory diagnostics",
            24.0 * s,
            40.0 * s,
            0.0,
            &fill_paint(col(t.text, 1.0)),
        );
        for (label, x) in [
            ("Refresh · R", logical.0 - 218.0),
            ("Close · Esc", logical.0 - 110.0),
        ] {
            canvas.draw_round_rect(
                Rect::from_xywh(x * s, 53.0 * s, 98.0 * s, 30.0 * s),
                5.0 * s,
                5.0 * s,
                &fill_paint(col(t.surface2, 1.0)),
            );
            text(label, x + 8.0, 73.0, 84.0, ink_on(t, t.surface2));
        }
        text(
            "Live · max 1 Hz · read-only",
            24.0,
            70.0,
            (width - 220.0).max(1.0),
            t.accent,
        );
        text("Memory = resident RAM · Commit = private committed backing · Peak = highest resident RAM", 24.0, 100.0, width, t.text_dim);
        let name_w = if wide { width * 0.40 } else { width * 0.55 };
        let memory_x = 24.0 + name_w;
        canvas.draw_rect(
            Rect::from_xywh(20.0 * s, 114.0 * s, (logical.0 - 40.0) * s, 29.0 * s),
            &fill_paint(col(t.surface1, 1.0)),
        );
        text(
            "Name / PID / purpose",
            30.0,
            134.0,
            name_w - 12.0,
            t.text_dim,
        );
        text(
            "Memory / usage ↓",
            memory_x,
            134.0,
            if wide { width * 0.22 } else { width * 0.45 },
            t.text_dim,
        );
        if wide {
            text(
                "Private commit",
                24.0 + width * 0.64,
                134.0,
                width * 0.18,
                t.text_dim,
            );
            text(
                "Peak WS",
                24.0 + width * 0.83,
                134.0,
                width * 0.17,
                t.text_dim,
            );
        }
        let count = visible_rows(logical.1);
        canvas.save();
        canvas.clip_rect(
            Rect::from_xywh(
                20.0 * s,
                BODY_TOP * s,
                (logical.0 - 40.0) * s,
                (logical.1 - BODY_TOP - 35.0).max(1.0) * s,
            ),
            ClipOp::Intersect,
            true,
        );
        for (i, row) in rows.iter().skip(offset).take(count).enumerate() {
            let y = BODY_TOP + i as f32 * ROW_HEIGHT;
            if row.section || i % 2 == 0 {
                canvas.draw_rect(
                    Rect::from_xywh(20.0 * s, y * s, (logical.0 - 40.0) * s, ROW_HEIGHT * s),
                    &fill_paint(col(t.surface1, if row.section { 0.9 } else { 0.45 })),
                );
            }
            let name = row
                .pid
                .map_or_else(|| row.name.clone(), |pid| format!("{}  ·  {pid}", row.name));
            text(
                &name,
                30.0,
                y + 20.0,
                name_w - 16.0,
                if row.section { t.accent } else { t.text },
            );
            if !row.section {
                let usage = if row.pid.is_some() {
                    bytes(row.memory)
                } else {
                    row.usage.clone()
                };
                text(
                    &usage,
                    memory_x,
                    y + 20.0,
                    if wide { width * 0.22 } else { width * 0.45 },
                    t.text,
                );
                if wide && row.pid.is_some() {
                    text(
                        &bytes(row.private_commit),
                        24.0 + width * 0.64,
                        y + 20.0,
                        width * 0.18,
                        t.text,
                    );
                    text(
                        &bytes(row.peak),
                        24.0 + width * 0.83,
                        y + 20.0,
                        width * 0.17,
                        t.text,
                    );
                }
            }
            let detail = if !wide && row.pid.is_some() {
                format!(
                    "{} · Commit {} · Peak {}",
                    row.description,
                    bytes(row.private_commit),
                    bytes(row.peak)
                )
            } else {
                row.description.clone()
            };
            let mut remaining = detail.as_str();
            for line in 0..2 {
                let mut end = 0;
                for (j, ch) in remaining.char_indices() {
                    let next = j + ch.len_utf8();
                    if text_width(&font, &remaining[..next], 0.0) > (width - 16.0) * s {
                        break;
                    }
                    end = next;
                }
                if end == 0 {
                    break;
                }
                text(
                    if line == 1 {
                        remaining
                    } else {
                        &remaining[..end]
                    },
                    30.0,
                    y + 38.0 + line as f32 * 17.0,
                    width - 16.0,
                    t.text_dim,
                );
                remaining = remaining[end..].trim_start();
                if remaining.is_empty() {
                    break;
                }
            }
        }
        canvas.restore();
        text(
            &format!(
                "{}–{} of {} · Wheel / ↑ ↓ / PgUp PgDn · KB = 1,000 bytes",
                if rows.is_empty() { 0 } else { offset + 1 },
                (offset + count).min(rows.len()),
                rows.len()
            ),
            24.0,
            logical.1 - 18.0,
            width,
            t.text_dim,
        );
        if rows.len() > count {
            let track = (logical.1 - BODY_TOP - 40.0).max(1.0);
            let thumb = (track * count as f32 / rows.len() as f32).max(12.0);
            let y = BODY_TOP + (track - thumb) * offset as f32 / (rows.len() - count) as f32;
            canvas.draw_round_rect(
                Rect::from_xywh((logical.0 - 17.0) * s, y * s, 3.0 * s, thumb * s),
                1.5 * s,
                1.5 * s,
                &fill_paint(col(t.accent, 0.8)),
            );
        }
        chain.present(&mut gpu.ctx)
    }

    pub fn render_settings(
        &mut self,
        view: &ViewModel,
        tokens: &ResolvedTokens,
        scale: f32,
    ) -> Result<(), RenderError> {
        self.resize(self.size.0, self.size.1)?;
        let mut gpu = self.gpu.borrow_mut();
        let chain = self.chain.as_mut().ok_or_else(missing)?;
        let canvas = chain.back_buffer().canvas();
        crate::settings_window::paint_settings(canvas, &mut self.fonts, tokens, scale, view);
        chain.present(&mut gpu.ctx)
    }

    pub fn render_spotlight(
        &mut self,
        view: &SpotlightView,
        tokens: &ResolvedTokens,
        scale: f32,
        icons: &IconCache,
    ) -> Result<(), RenderError> {
        self.resize(self.size.0, self.size.1)?;
        let mut gpu = self.gpu.borrow_mut();
        let chain = self.chain.as_mut().ok_or_else(missing)?;
        let canvas = chain.back_buffer().canvas();
        crate::spotlight::paint_spotlight(canvas, &mut self.fonts, tokens, scale, view, icons);
        chain.present(&mut gpu.ctx)
    }
}

impl Drop for PopupRenderer {
    fn drop(&mut self) {
        if let Some(c) = self.chain.as_mut() {
            c.surfaces.clear();
        }
        self.gpu.borrow_mut().ctx.flush_submit_and_sync_cpu();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(n: usize, v: u8) -> Vec<u8> {
        vec![v; n * n * 4]
    }

    #[test]
    fn icon_cache_insert_get_and_reject() {
        let mut c = IconCache::new();
        assert!(!c.contains("a"));
        c.insert_bgra("a", 4, 4, &px(4, 200));
        assert!(c.contains("a"));
        let img = c.get("a").unwrap();
        assert_eq!((img.width(), img.height()), (4, 4));
        c.insert_bgra("bad", 4, 4, &px(3, 1));
        c.insert_bgra("zero", 0, 4, &[]);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn icon_cache_is_capped_and_drops_oldest() {
        let mut c = IconCache::new();
        for i in 0..ICON_CACHE_MAX + 10 {
            c.insert_bgra(&format!("k{i}"), 1, 1, &[1, 2, 3, 255]);
        }
        assert_eq!(c.len(), ICON_CACHE_MAX);
        assert!(!c.contains("k0"));
        assert!(c.contains(&format!("k{}", ICON_CACHE_MAX + 9)));
        // Re-inserting an existing key neither grows nor reorders.
        c.insert_bgra("k20", 1, 1, &[9, 9, 9, 255]);
        assert_eq!(c.len(), ICON_CACHE_MAX);
    }
}
