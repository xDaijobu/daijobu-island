//! The Tile feature's two drawings: the island's Tile panel (layout thumbnails) and the
//! zone overlay (the zones of a layout on the target monitor while a window is dragged).
//!
//! The panel is drawn by the island painter through `paint_tile_panel`: the painter has
//! already drawn the pill body, so only the content goes on top. It follows plan.html's
//! `tile` mock: a mono header, a grid of `.lay` thumbnails (a surface-1 plate, a hairline
//! border, the zones as small cells; the active layout has an accent border and accent
//! tinted cells), and a footer line for a status or a warning. Everything is laid out by
//! `dj-core` in panel-local logical px; this module only multiplies by `scale`. Nothing
//! is drawn outside the panel's rect, a text is cut (`..`) to fit its rect and never
//! spills out, and a thumbnail draws inside its own rect.
//!
//! The overlay (`ZoneOverlay`) is a second D3D12 + DirectComposition chain on its own
//! transparent, click-through window, sized to the target monitor in device px (like
//! `previews::PreviewSurface`). It draws only when the app calls `render`. Each zone is a
//! rounded rect in the pill colour at low alpha with a text-colour outline and its number
//! in a big mono face with a pill-coloured halo, so it reads on any wallpaper; the
//! highlighted zone is accent. A zone draws nothing outside its own rect, so the gaps
//! between zones stay fully transparent (the window is input-transparent anyway).

use crate::fonts::{Fonts, Weight};
use crate::gpu::{Chain, Gpu, RenderError};
use crate::paint::{col, draw_text, fill_paint, fit, stroke_paint, text_width};
use dj_core::layout::Rect as LRect;
use dj_core::theme::ResolvedTokens;
use skia_safe::{Canvas, ClipOp, Color, Font, Paint, RRect, Rect};

/// Header and footer text: `font_label` (mono), medium, this size in logical px.
const TEXT_SIZE: f32 = 11.0;
/// Letter spacing of that text, as a fraction of the size.
const TEXT_TRACKING: f32 = 0.06;
/// Space kept between the header's two texts.
const HEADER_GAP: f32 = 12.0;

/// A thumbnail's corner radius, its cells' corner radius and its border widths.
const THUMB_R: f32 = 6.0;
const CELL_R: f32 = 3.0;
const THUMB_BORDER: f32 = 1.0;
const THUMB_BORDER_ACTIVE: f32 = 1.5;
/// Cell tints: the mock's `.lay i` is text-dim at .35; the active layout's are accent.
const CELL_ALPHA: f32 = 0.35;
const CELL_ALPHA_ACTIVE: f32 = 0.45;

/// The overlay's zone: radius, fills, outline.
const ZONE_R: f32 = 12.0;
const ZONE_FILL: f32 = 0.35;
const ZONE_HOT_FILL: f32 = 0.45;
const ZONE_LINE: f32 = 2.0;
const ZONE_LINE_ALPHA: f32 = 0.5;
/// The zone number: its size, the smallest it shrinks to and its share of the zone's
/// shorter side at most, and the halo (a stroke around the glyphs, as a share of the size).
const NUMBER_SIZE: f32 = 48.0;
const NUMBER_MIN: f32 = 8.0;
const NUMBER_SHARE: f32 = 0.6;
const HALO: f32 = 0.14;
const HALO_ALPHA: f32 = 0.9;

/// One layout thumbnail, in panel-local logical px.
#[derive(Clone, Copy, Debug)]
pub struct TileThumb<'a> {
    pub rect: LRect,
    /// The zones, in reading order, in the same space as `rect` (inside it).
    pub cells: &'a [LRect],
    /// The layout that is active for the island's monitor.
    pub active: bool,
    /// The cell under the dragged window's cursor.
    pub hover: Option<usize>,
}

/// What the Tile panel shows. All rects are panel-local logical px (origin at the panel's
/// top-left).
#[derive(Clone, Copy, Debug)]
pub struct TilePanel<'a> {
    /// The header's left text (`tile · drop on a zone`).
    pub title: &'a str,
    /// The header's right text (`monitor 1 · 3440×1440`).
    pub monitor: &'a str,
    pub thumbs: &'a [TileThumb<'a>],
    /// The footer line and whether it is a warning (`can't move an admin window`).
    pub footer: Option<(&'a str, bool)>,
    pub header: LRect,
    pub footer_rect: LRect,
}

/// A usable device scale: finite and positive, else 1.
fn sane(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// Draws the Tile panel's content inside `panel` (window logical px) at `scale` (device
/// px per logical px). The island painter has already drawn the pill body; this adds the
/// header, the thumbnails and the footer, clipped to the panel's rect. The panel's corner
/// is rounded to a device pixel, like the island's own rect, so the cells stay crisp.
pub fn paint_tile_panel(
    canvas: &Canvas,
    fonts: &mut Fonts,
    tokens: &ResolvedTokens,
    panel: LRect,
    scale: f32,
    p: &TilePanel,
) {
    let s = sane(scale);
    let t = tokens;
    let area = Rect::new(
        (panel.x * s).round(),
        (panel.y * s).round(),
        ((panel.x + panel.w) * s).round(),
        ((panel.y + panel.h) * s).round(),
    );
    let at = |r: LRect| Rect::from_xywh(area.left + r.x * s, area.top + r.y * s, r.w * s, r.h * s);

    let base = canvas.save();
    canvas.clip_rect(area, ClipOp::Intersect, false);

    // Header: the title on the left, the monitor on the right.
    let font = fonts.font(&t.font_label, Weight::Medium, TEXT_SIZE * s);
    let tracking = TEXT_TRACKING * TEXT_SIZE * s;
    let header = at(p.header);
    let monitor = fit(&font, p.monitor, tracking, header.width());
    let mw = text_width(&font, &monitor, tracking);
    let gap = if monitor.is_empty() {
        0.0
    } else {
        HEADER_GAP * s
    };
    let title = fit(&font, p.title, tracking, header.width() - mw - gap);
    let dim = fill_paint(col(t.text_dim, 1.0));
    clipped_text(canvas, &font, &title, &header, header.left, tracking, &dim);
    clipped_text(
        canvas,
        &font,
        &monitor,
        &header,
        header.right - mw,
        tracking,
        &dim,
    );

    for th in p.thumbs {
        draw_thumb(canvas, t, s, th, &at);
    }

    // Footer: a status in the dim colour, a warning in the warn colour.
    if let Some((text, warn)) = p.footer {
        let foot = at(p.footer_rect);
        let text = fit(&font, text, tracking, foot.width());
        let ink = if warn { t.warn } else { t.text_dim };
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
    canvas.restore_to_count(base);
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
    if text.is_empty() {
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

/// One `.lay` thumbnail: a surface-1 plate, its cells (accent tinted when the layout is
/// active, solid accent for the hovered one), then the border over them. The border and
/// the cells stay inside `r`.
fn draw_thumb(
    canvas: &Canvas,
    t: &ResolvedTokens,
    s: f32,
    th: &TileThumb,
    at: &dyn Fn(LRect) -> Rect,
) {
    let r = at(th.rect);
    if !(r.width() >= 2.0 * s && r.height() >= 2.0 * s) {
        return;
    }
    let rad = (THUMB_R * s).min(r.width() / 2.0).min(r.height() / 2.0);
    let plate = RRect::new_rect_xy(r, rad, rad);
    canvas.draw_rrect(plate, &fill_paint(col(t.surface1, 1.0)));

    canvas.save();
    canvas.clip_rrect(plate, ClipOp::Intersect, true);
    for (i, cell) in th.cells.iter().enumerate() {
        let c = at(*cell);
        if !(c.width() > 0.0 && c.height() > 0.0) {
            continue;
        }
        let ink = if th.hover == Some(i) {
            col(t.accent, 1.0)
        } else if th.active {
            col(t.accent, CELL_ALPHA_ACTIVE)
        } else {
            col(t.text_dim, CELL_ALPHA)
        };
        let cr = (CELL_R * s).min(c.width() / 2.0).min(c.height() / 2.0);
        canvas.draw_rrect(RRect::new_rect_xy(c, cr, cr), &fill_paint(ink));
    }
    canvas.restore();

    let (edge, w) = if th.active {
        (t.accent, THUMB_BORDER_ACTIVE * s)
    } else {
        (t.surface2, THUMB_BORDER * s)
    };
    let half = w / 2.0;
    let line = RRect::new_rect_xy(
        r.with_inset((half, half)),
        (rad - half).max(0.0),
        (rad - half).max(0.0),
    );
    canvas.draw_rrect(line, &stroke_paint(col(edge, 1.0), w));
}

/// One frame of the zone overlay. The zones are in the overlay window's client px (device
/// px, origin at the window's top-left); `scale` only sizes the decoration (radius,
/// outline, number).
#[derive(Clone, Copy, Debug)]
pub struct ZoneScene<'a> {
    pub tokens: &'a ResolvedTokens,
    /// Device px per logical px of the monitor the overlay sits on.
    pub scale: f32,
    /// In reading order: the zone at index `i` shows the number `i + 1`.
    pub zones: &'a [LRect],
    /// The zone under the cursor.
    pub highlight: Option<usize>,
}

/// The overlay's GPU side: a swap chain sized once to the target monitor, drawn only on
/// `render`.
pub struct ZoneOverlay {
    // Field order is drop order; `Drop` clears the surfaces first.
    fonts: Fonts,
    gpu: Gpu,
    chain: Chain,
    width: u32,
    height: u32,
}

impl ZoneOverlay {
    /// `width` x `height` is the window's client size in device px.
    pub fn new(hwnd: isize, width: u32, height: u32) -> Result<Self, RenderError> {
        let fonts = Fonts::new().map_err(RenderError::Setup)?;
        let mut gpu = Gpu::new()?;
        let chain = Chain::new(&mut gpu, hwnd, width, height)?;
        Ok(Self {
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

    /// Clears to transparent, draws the zones and presents (`Present(1)`).
    pub fn render(&mut self, scene: &ZoneScene) -> Result<(), RenderError> {
        let canvas = self.chain.back_buffer().canvas();
        canvas.clear(Color::TRANSPARENT);
        paint_zones(canvas, &mut self.fonts, scene);
        self.chain.present(&mut self.gpu.ctx)
    }
}

impl Drop for ZoneOverlay {
    fn drop(&mut self) {
        // The wrapped buffers go first, then wait for the GPU.
        self.chain.surfaces.clear();
        self.gpu.ctx.flush_submit_and_sync_cpu();
    }
}

/// Draws every zone of the scene on `canvas` (already cleared to transparent).
pub(crate) fn paint_zones(canvas: &Canvas, fonts: &mut Fonts, sc: &ZoneScene) {
    let s = sane(sc.scale);
    for (i, z) in sc.zones.iter().enumerate() {
        draw_zone(canvas, fonts, sc.tokens, s, i, z, sc.highlight == Some(i));
    }
}

/// One zone: the fill, the outline just inside the edge, the number. All of it is clipped
/// to the zone's rounded rect.
fn draw_zone(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    s: f32,
    index: usize,
    z: &LRect,
    hot: bool,
) {
    let r = Rect::from_xywh(z.x, z.y, z.w, z.h);
    if !(r.width() >= 2.0 * s && r.height() >= 2.0 * s) {
        return;
    }
    let rad = (ZONE_R * s).min(r.width() / 2.0).min(r.height() / 2.0);
    let body = RRect::new_rect_xy(r, rad, rad);
    let fill = if hot {
        col(t.accent, ZONE_HOT_FILL)
    } else {
        col(t.pill, ZONE_FILL)
    };
    canvas.draw_rrect(body, &fill_paint(fill));

    let w = ZONE_LINE * s;
    let edge = if hot {
        col(t.accent, 1.0)
    } else {
        col(t.text, ZONE_LINE_ALPHA)
    };
    let inner = RRect::new_rect_xy(
        r.with_inset((w / 2.0, w / 2.0)),
        (rad - w / 2.0).max(0.0),
        (rad - w / 2.0).max(0.0),
    );
    canvas.draw_rrect(inner, &stroke_paint(edge, w));

    // The number, shrunk to fit a small zone and skipped when it would be unreadable.
    let size = (NUMBER_SIZE * s).min(r.width().min(r.height()) * NUMBER_SHARE);
    if size < NUMBER_MIN * s {
        return;
    }
    let font = fonts.font(&t.font_label, Weight::Medium, size);
    let label = (index + 1).to_string();
    // Centred by its advance across, and by the ink of a digit up and down, so every
    // number sits on the same line.
    let x = r.center_x() - font.measure_str(&label, None).0 / 2.0;
    let (_, digit) = font.measure_str("0", None);
    let y = r.center_y() - (digit.top + digit.bottom) / 2.0;
    canvas.save();
    canvas.clip_rrect(body, ClipOp::Intersect, true);
    // The halo: the glyph outlines stroked in the pill colour, under the fill.
    canvas.draw_str(
        &label,
        (x, y),
        &font,
        &stroke_paint(col(t.pill, HALO_ALPHA), size * HALO),
    );
    canvas.draw_str(&label, (x, y), &font, &fill_paint(col(t.text, 1.0)));
    canvas.restore();
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::color::Rgb;
    use dj_core::theme::{ThemeRegistry, DEFAULT_THEME};
    use skia_safe::image::CachingHint;
    use skia_safe::{surfaces, AlphaType, ColorType, ImageInfo};

    fn tokens() -> ResolvedTokens {
        ThemeRegistry::with_builtins()
            .get(DEFAULT_THEME)
            .unwrap()
            .clone()
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

    /// A painted CPU surface (BGRA, premultiplied).
    struct Shot {
        w: i32,
        h: i32,
        px: Vec<u8>,
    }

    /// x, y, w, h in device px.
    type IRect = (i32, i32, i32, i32);

    impl Shot {
        fn at(&self, x: i32, y: i32) -> [u8; 4] {
            assert!(x >= 0 && y >= 0 && x < self.w && y < self.h, "({x},{y})");
            let i = ((y * self.w + x) * 4) as usize;
            [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
        }

        /// Pixels within `tol` of `c` inside the rect.
        fn count_in(&self, r: IRect, c: [u8; 4], tol: u8) -> usize {
            self.ink(r, c, tol).len()
        }

        /// Pixels with any coverage inside the rect.
        fn lit_in(&self, r: IRect) -> usize {
            let mut n = 0;
            for y in r.1..r.1 + r.3 {
                for x in r.0..r.0 + r.2 {
                    if self.at(x, y)[3] > 0 {
                        n += 1;
                    }
                }
            }
            n
        }

        /// The first lit pixel outside every rect, if any.
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

        /// The pixels (x, y) inside `r` that are within `tol` of `c`.
        fn ink(&self, r: IRect, c: [u8; 4], tol: u8) -> Vec<(i32, i32)> {
            let mut v = Vec::new();
            for y in r.1..r.1 + r.3 {
                for x in r.0..r.0 + r.2 {
                    if near(self.at(x, y), c, tol) {
                        v.push((x, y));
                    }
                }
            }
            v
        }
    }

    fn snapshot(mut surface: skia_safe::Surface, w: i32, h: i32) -> Shot {
        let info = ImageInfo::new((w, h), ColorType::BGRA8888, AlphaType::Premul, None);
        let mut px = vec![0u8; (w * h * 4) as usize];
        assert!(surface.image_snapshot().read_pixels(
            &info,
            &mut px,
            (w * 4) as usize,
            (0, 0),
            CachingHint::Disallow
        ));
        Shot { w, h, px }
    }

    // ----- the panel -----

    const HEADER: LRect = LRect::new(20.0, 14.0, 360.0, 16.0);
    const FOOTER: LRect = LRect::new(20.0, 128.0, 360.0, 18.0);
    const PANEL: (f32, f32) = (400.0, 168.0);
    const SIZE: (i32, i32) = (460, 228);
    /// The header and footer rects on the surface when the panel is at (30, 30).
    const HEAD_PX: IRect = (50, 44, 360, 16);
    const FOOT_PX: IRect = (50, 158, 360, 18);

    /// A thumbnail's two cells, with a 2 px gap all round.
    fn halves(r: LRect) -> Vec<LRect> {
        let w = (r.w - 6.0) / 2.0;
        vec![
            LRect::new(r.x + 2.0, r.y + 2.0, w, r.h - 4.0),
            LRect::new(r.x + 4.0 + w, r.y + 2.0, w, r.h - 4.0),
        ]
    }

    /// The `1+2` layout's cells: a tall one on the left, two on the right.
    fn one_two(r: LRect) -> Vec<LRect> {
        let (iw, ih) = (r.w - 6.0, r.h - 6.0);
        let (lw, rw) = (iw * 1.4 / 2.4, iw / 2.4);
        let rh = ih / 2.0;
        vec![
            LRect::new(r.x + 2.0, r.y + 2.0, lw, r.h - 4.0),
            LRect::new(r.x + 4.0 + lw, r.y + 2.0, rw, rh),
            LRect::new(r.x + 4.0 + lw, r.y + 4.0 + rh, rw, rh),
        ]
    }

    /// The first row's thumbnails: 52 x 32, 9 px apart.
    fn thumb_rect(i: usize) -> LRect {
        LRect::new(22.0 + 61.0 * i as f32, 42.0, 52.0, 32.0)
    }

    fn panel_shot(
        t: &ResolvedTokens,
        origin: (f32, f32),
        scale: f32,
        p: &TilePanel,
        (w, h): (i32, i32),
    ) -> Shot {
        let mut fonts = Fonts::new().unwrap();
        let mut surface = surfaces::raster_n32_premul((w, h)).unwrap();
        surface.canvas().clear(Color::TRANSPARENT);
        let panel = LRect::new(origin.0, origin.1, PANEL.0, PANEL.1);
        paint_tile_panel(surface.canvas(), &mut fonts, t, panel, scale, p);
        snapshot(surface, w, h)
    }

    /// A panel with only a header (or nothing).
    fn bare<'a>(title: &'a str, monitor: &'a str) -> TilePanel<'a> {
        TilePanel {
            title,
            monitor,
            thumbs: &[],
            footer: None,
            header: HEADER,
            footer_rect: FOOTER,
        }
    }

    #[test]
    fn the_header_has_its_title_left_and_the_monitor_right_inside_its_rect() {
        let t = tokens();
        let p = bare("tile · drop on a zone", "monitor 1 · 3440×1440");
        let shot = panel_shot(&t, (30.0, 30.0), 1.0, &p, SIZE);
        assert_eq!(shot.lit_outside(&[HEAD_PX]), None);
        let dim = bgra(t.text_dim);
        assert!(shot.count_in((50, 44, 180, 16), dim, 50) > 8, "title");
        assert!(shot.count_in((230, 44, 180, 16), dim, 50) > 8, "monitor");
        assert!(shot.lit_in((50, 44, 150, 16)) > 40);
        assert!(shot.lit_in((260, 44, 150, 16)) > 40);
        // The monitor text is flush right, the title flush left, a gap between them.
        assert_eq!(shot.lit_in((215, 44, 30, 16)), 0, "gap");
        // The header is text only: no plate behind it.
        assert_eq!(shot.at(HEAD_PX.0 + 1, HEAD_PX.1 + 1)[3], 0);
    }

    #[test]
    fn long_header_texts_are_cut_and_never_run_into_each_other_or_out_of_the_rect() {
        let t = tokens();
        let long = "tile · drop a window on a zone of any layout you like, anywhere";
        let mon = "monitor 2 · 1920×1080";
        let cut = panel_shot(&t, (30.0, 30.0), 1.0, &bare(long, mon), SIZE);
        assert_eq!(cut.lit_outside(&[HEAD_PX]), None);
        // The same monitor text with no title: the long title leaves it untouched.
        let alone = panel_shot(&t, (30.0, 30.0), 1.0, &bare("", mon), SIZE);
        let mut fonts = Fonts::new().unwrap();
        let font = fonts.font(&t.font_label, Weight::Medium, TEXT_SIZE);
        let mw = text_width(&font, mon, TEXT_TRACKING * TEXT_SIZE).ceil() as i32;
        let from = HEAD_PX.0 + HEAD_PX.2 - mw - 10;
        for y in HEAD_PX.1..HEAD_PX.1 + HEAD_PX.3 {
            for x in from..HEAD_PX.0 + HEAD_PX.2 {
                assert_eq!(cut.at(x, y), alone.at(x, y), "({x},{y})");
            }
        }
        // And the title is still there, cut short.
        assert!(cut.lit_in((50, 44, from - 50, 16)) > 100);
        // A title too long for any room and a monitor wider than the header: still inside.
        let silly = "x".repeat(200);
        let both = panel_shot(&t, (30.0, 30.0), 1.0, &bare(&silly, &silly), SIZE);
        assert_eq!(both.lit_outside(&[HEAD_PX]), None);
        assert!(both.lit_in(HEAD_PX) > 100);
    }

    #[test]
    fn an_active_thumb_has_an_accent_border_and_the_others_a_hairline() {
        let t = tokens();
        let (a, b) = (thumb_rect(0), thumb_rect(1));
        let (ca, cb) = (halves(a), halves(b));
        let thumbs = [
            TileThumb {
                rect: a,
                cells: &ca,
                active: false,
                hover: None,
            },
            TileThumb {
                rect: b,
                cells: &cb,
                active: true,
                hover: None,
            },
        ];
        let p = TilePanel {
            thumbs: &thumbs,
            ..bare("tile", "monitor 1")
        };
        let shot = panel_shot(&t, (30.0, 30.0), 1.0, &p, SIZE);
        let (ax, ay) = (30 + 22, 30 + 42);
        let (bx, by) = (30 + 83, 30 + 42);
        let acc = bgra(t.accent);
        // The border: s2 for the plain thumb, accent for the active one, on every side.
        let edge = bgra(t.surface2);
        assert!(near(shot.at(ax, ay + 16), edge, 3), "left");
        assert!(near(shot.at(ax + 51, ay + 16), edge, 3), "right");
        assert!(near(shot.at(ax + 26, ay), edge, 3), "top");
        assert!(near(shot.at(ax + 26, ay + 31), edge, 3), "bottom");
        assert!(near(shot.at(bx, by + 16), acc, 3), "active left");
        assert!(near(shot.at(bx + 51, by + 16), acc, 3), "active right");
        assert!(near(shot.at(bx + 26, by), acc, 3), "active top");
        assert!(near(shot.at(bx + 26, by + 31), acc, 3), "active bottom");
        // 1.5 px: the second pixel in is only half accent.
        assert!(!near(shot.at(bx + 1, by + 16), acc, 3));
        // The plain thumb has no accent anywhere; both are rounded (r 6).
        assert_eq!(shot.count_in((ax, ay, 52, 32), acc, 6), 0);
        assert_eq!(shot.at(ax, ay)[3], 0, "corner is cut");
        assert_eq!(shot.at(bx + 51, by + 31)[3], 0, "corner is cut");
        // Cells: dim on the plain thumb, accent tinted on the active one.
        let plate = premul(t.surface1, 1.0);
        let dim = blend(plate, t.text_dim, CELL_ALPHA);
        let tint = blend(plate, t.accent, CELL_ALPHA_ACTIVE);
        for i in 0..2 {
            let (c, d) = (ca[i], cb[i]);
            let plain = shot.at(30 + (c.x + c.w / 2.0) as i32, 30 + (c.y + c.h / 2.0) as i32);
            let active = shot.at(30 + (d.x + d.w / 2.0) as i32, 30 + (d.y + d.h / 2.0) as i32);
            assert!(near(plain, dim, 3), "plain cell {i}: {plain:?}");
            assert!(near(active, tint, 3), "active cell {i}: {active:?}");
        }
        // The gap between cells is the plate.
        assert!(near(shot.at(ax + 26, ay + 16), bgra(t.surface1), 3));
    }

    #[test]
    fn the_hovered_cell_is_solid_accent_on_any_thumb() {
        let t = tokens();
        let (a, b) = (thumb_rect(0), thumb_rect(1));
        let (ca, cb) = (one_two(a), one_two(b));
        let thumbs = [
            TileThumb {
                rect: a,
                cells: &ca,
                active: false,
                hover: Some(1),
            },
            TileThumb {
                rect: b,
                cells: &cb,
                active: true,
                hover: Some(2),
            },
        ];
        let p = TilePanel {
            thumbs: &thumbs,
            ..bare("tile", "monitor 1")
        };
        let shot = panel_shot(&t, (30.0, 30.0), 1.0, &p, SIZE);
        let acc = bgra(t.accent);
        let centre = |c: LRect| {
            (
                (30.0 + c.x + c.w / 2.0) as i32,
                (30.0 + c.y + c.h / 2.0) as i32,
            )
        };
        let plate = premul(t.surface1, 1.0);
        let dim = blend(plate, t.text_dim, CELL_ALPHA);
        let tint = blend(plate, t.accent, CELL_ALPHA_ACTIVE);
        for (i, c) in ca.iter().enumerate() {
            let (x, y) = centre(*c);
            let want = if i == 1 { acc } else { dim };
            assert!(near(shot.at(x, y), want, 3), "plain thumb cell {i}");
        }
        for (i, c) in cb.iter().enumerate() {
            let (x, y) = centre(*c);
            let want = if i == 2 { acc } else { tint };
            assert!(near(shot.at(x, y), want, 3), "active thumb cell {i}");
        }
        // The hovered cell is a whole block of accent, rounded at its corners.
        let c = ca[1];
        let (cx, cy) = (30 + c.x as i32, 30 + c.y as i32);
        assert!(shot.count_in((cx + 2, cy + 2, c.w as i32 - 4, c.h as i32 - 4), acc, 3) > 40);
        assert!(!near(shot.at(cx, cy), acc, 20), "corner is rounded");
        // A hover index past the last cell does nothing.
        let thumbs = [TileThumb {
            hover: Some(7),
            ..thumbs[0]
        }];
        let p = TilePanel {
            thumbs: &thumbs,
            ..bare("tile", "monitor 1")
        };
        let none = panel_shot(&t, (30.0, 30.0), 1.0, &p, SIZE);
        assert_eq!(none.count_in((52, 72, 52, 32), acc, 6), 0);
    }

    #[test]
    fn the_footer_is_dim_for_a_status_and_warn_for_a_warning() {
        let t = tokens();
        let warn = bgra(t.warn);
        let dim = bgra(t.text_dim);
        let foot = |text: Option<(&str, bool)>| {
            let p = TilePanel {
                footer: text,
                ..bare("", "")
            };
            panel_shot(&t, (30.0, 30.0), 1.0, &p, SIZE)
        };
        let bad = foot(Some(("can't move an admin window", true)));
        assert_eq!(bad.lit_outside(&[FOOT_PX]), None);
        assert!(bad.count_in(FOOT_PX, warn, 40) > 20, "warn text");
        assert_eq!(bad.count_in(FOOT_PX, dim, 6), 0, "no dim text");
        // It starts at the footer's left edge and stays left aligned.
        assert!(bad.lit_in((50, 158, 40, 18)) > 20);
        assert_eq!(bad.lit_in((50 + 250, 158, 110, 18)), 0);

        let ok = foot(Some(("layout 1+2 is active", false)));
        assert_eq!(ok.lit_outside(&[FOOT_PX]), None);
        assert!(ok.count_in(FOOT_PX, dim, 40) > 20, "dim text");
        assert_eq!(ok.count_in(FOOT_PX, warn, 30), 0, "no warn colour");
        assert_ne!(ok.px, bad.px);

        let none = foot(None);
        assert_eq!(none.lit_in((0, 0, SIZE.0, SIZE.1)), 0);
        // A footer too long for its rect is cut inside it.
        let long = foot(Some((&"can't move an admin window ".repeat(8), true)));
        assert_eq!(long.lit_outside(&[FOOT_PX]), None);
        assert!(long.lit_in(FOOT_PX) > 100);
    }

    #[test]
    fn nothing_is_drawn_outside_the_panel() {
        let t = tokens();
        let cells = one_two(LRect::new(380.0, 150.0, 52.0, 32.0));
        let thumbs = [
            TileThumb {
                rect: LRect::new(380.0, 150.0, 52.0, 32.0),
                cells: &cells,
                active: true,
                hover: Some(2),
            },
            TileThumb {
                rect: LRect::new(-30.0, -20.0, 52.0, 32.0),
                cells: &cells,
                active: false,
                hover: Some(0),
            },
            // Cells bigger than their thumbnail never leave it.
            TileThumb {
                rect: LRect::new(100.0, 90.0, 40.0, 20.0),
                cells: &[LRect::new(90.0, 80.0, 200.0, 100.0)],
                active: false,
                hover: Some(0),
            },
        ];
        let p = TilePanel {
            title: "tile · drop on a zone and then some more and then some",
            monitor: "monitor 12 · 15360×8640",
            thumbs: &thumbs,
            footer: Some(("can't move an admin window, really", true)),
            header: LRect::new(-10.0, -10.0, 430.0, 16.0),
            footer_rect: LRect::new(300.0, 150.0, 200.0, 18.0),
        };
        for scale in [1.0, 1.5] {
            let origin = (30.0, 30.0);
            let shot = panel_shot(&t, origin, scale, &p, (SIZE.0 * 2, SIZE.1 * 2));
            let area = (
                (origin.0 * scale).round() as i32,
                (origin.1 * scale).round() as i32,
                (PANEL.0 * scale).round() as i32,
                (PANEL.1 * scale).round() as i32,
            );
            assert_eq!(shot.lit_outside(&[area]), None, "scale {scale}");
            assert!(shot.lit_in(area) > 500, "scale {scale}");
        }
    }

    #[test]
    fn the_panel_follows_its_origin() {
        let t = tokens();
        let cells: Vec<Vec<LRect>> = (0..9).map(|i| one_two(thumb_rect(i % 6))).collect();
        let thumbs: Vec<TileThumb> = cells
            .iter()
            .enumerate()
            .map(|(i, c)| TileThumb {
                rect: thumb_rect(i % 6),
                cells: c,
                active: i == 3,
                hover: (i == 4).then_some(1),
            })
            .collect();
        let p = TilePanel {
            thumbs: &thumbs,
            footer: Some(("can't move an admin window", true)),
            ..bare("tile · drop on a zone", "monitor 1 · 3440×1440")
        };
        let home = panel_shot(&t, (0.0, 0.0), 1.0, &p, (400, 168));
        let moved = panel_shot(&t, (37.0, 11.0), 1.0, &p, (460, 200));
        assert!(home.lit_in((0, 0, 400, 168)) > 1000);
        for y in 0..168 {
            for x in 0..400 {
                // Equal up to float rounding of the shifted coordinates (one level).
                let (a, b) = (home.at(x, y), moved.at(x + 37, y + 11));
                assert!(near(a, b, 2), "({x},{y}): {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn at_150_percent_everything_lands_on_device_pixels() {
        let t = tokens();
        let r = thumb_rect(0);
        let cells = halves(r);
        let thumbs = [TileThumb {
            rect: r,
            cells: &cells,
            active: false,
            hover: Some(1),
        }];
        let p = TilePanel {
            thumbs: &thumbs,
            footer: Some(("layout 50/50", false)),
            ..bare("tile · drop on a zone", "monitor 1 · 3440×1440")
        };
        // The panel's corner (10.5, 7) logical is device (15.75, 10.5): rounded to (16, 11).
        let shot = panel_shot(&t, (10.5, 7.0), 1.5, &p, (700, 300));
        let (ox, oy) = (16, 11);
        // The thumbnail is 78 x 48 device px at (ox + 33, oy + 63).
        let (tx, ty) = (ox + 33, oy + 63);
        assert!(near(shot.at(tx, ty + 24), bgra(t.surface2), 3), "left edge");
        assert!(
            near(shot.at(tx + 77, ty + 24), bgra(t.surface2), 3),
            "right"
        );
        assert_eq!(shot.at(tx - 1, ty + 24)[3], 0, "outside, left");
        assert_eq!(shot.at(tx + 78, ty + 24)[3], 0, "outside, right");
        assert_eq!(shot.at(tx + 39, ty - 1)[3], 0, "outside, top");
        assert_eq!(shot.at(tx + 39, ty + 48)[3], 0, "outside, bottom");
        // The hovered (right) cell is accent at its centre.
        let c = cells[1];
        let (cx, cy) = (
            ox + ((c.x + c.w / 2.0) * 1.5) as i32,
            oy + ((c.y + c.h / 2.0) * 1.5) as i32,
        );
        assert!(near(shot.at(cx, cy), bgra(t.accent), 3));
        // The texts (16.5 px) stay in the header and footer rects, scaled.
        let head = (ox + 30, oy + 21, 540, 24);
        let foot = (ox + 30, oy + 192, 540, 27);
        let thumb = (tx, ty, 78, 48);
        assert_eq!(shot.lit_outside(&[head, foot, thumb]), None);
        assert!(shot.lit_in(head) > 100 && shot.lit_in(foot) > 50);
    }

    #[test]
    fn the_texts_use_glyphs_the_embedded_mono_font_has() {
        let mut fonts = Fonts::new().unwrap();
        let t = tokens();
        let face = fonts.typeface(&t.font_label, Weight::Medium);
        let text =
            "tile · drop on a zone monitor 1 · 3440×1440 can't move an admin window 0123456789.";
        for c in text.chars() {
            assert_ne!(
                face.unichar_to_glyph(c as i32),
                0,
                "{} lacks {c}",
                t.font_label
            );
        }
    }

    // ----- the overlay -----

    const OVERLAY: (i32, i32) = (320, 220);

    fn zones_shot(
        t: &ResolvedTokens,
        scale: f32,
        zones: &[LRect],
        highlight: Option<usize>,
        wallpaper: Color,
        (w, h): (i32, i32),
    ) -> Shot {
        let mut fonts = Fonts::new().unwrap();
        let mut surface = surfaces::raster_n32_premul((w, h)).unwrap();
        surface.canvas().clear(wallpaper);
        let sc = ZoneScene {
            tokens: t,
            scale,
            zones,
            highlight,
        };
        paint_zones(surface.canvas(), &mut fonts, &sc);
        snapshot(surface, w, h)
    }

    fn two_zones() -> [LRect; 2] {
        [
            LRect::new(20.0, 20.0, 120.0, 80.0),
            LRect::new(150.0, 20.0, 150.0, 80.0),
        ]
    }

    fn px(z: LRect) -> IRect {
        (z.x as i32, z.y as i32, z.w as i32, z.h as i32)
    }

    /// Height in px of the solid ink pixels, and their centre of mass.
    fn ink_box(pixels: &[(i32, i32)]) -> (i32, f32, f32) {
        let n = pixels.len() as f32;
        let top = pixels.iter().map(|p| p.1).min().unwrap();
        let bottom = pixels.iter().map(|p| p.1).max().unwrap();
        let mx = pixels.iter().map(|p| p.0 as f32).sum::<f32>() / n;
        let my = pixels.iter().map(|p| p.1 as f32).sum::<f32>() / n;
        (bottom - top + 1, mx, my)
    }

    #[test]
    fn a_zone_is_pill_tinted_with_a_text_coloured_outline() {
        let t = tokens();
        let zones = two_zones();
        let shot = zones_shot(&t, 1.0, &zones, None, Color::TRANSPARENT, OVERLAY);
        let fill = premul(t.pill, ZONE_FILL);
        for (x, y) in [(30, 90), (130, 90), (30, 30), (290, 30), (160, 90)] {
            let p = shot.at(x, y);
            assert!(near(p, fill, 2), "fill at ({x},{y}): {p:?}");
        }
        // The outline is 2 px, just inside the edge: text colour at half over the fill.
        let line = blend(fill, t.text, ZONE_LINE_ALPHA);
        for (x, y) in [
            (20, 60),
            (21, 60),
            (138, 60),
            (139, 60),
            (80, 20),
            (80, 21),
            (80, 98),
            (80, 99),
        ] {
            let p = shot.at(x, y);
            assert!(near(p, line, 3), "line at ({x},{y}): {p:?}");
        }
        assert!(near(shot.at(22, 60), fill, 2), "the line is 2 px");
        // Rounded corners (r 12): the very corner is clear, a point on the arc is not.
        assert_eq!(shot.at(20, 20)[3], 0);
        assert_eq!(shot.at(139, 99)[3], 0);
        assert!(shot.at(24, 24)[3] > 100);
    }

    #[test]
    fn the_highlighted_zone_is_accent_filled_and_outlined() {
        let t = tokens();
        let zones = two_zones();
        let shot = zones_shot(&t, 1.0, &zones, Some(1), Color::TRANSPARENT, OVERLAY);
        let hot = premul(t.accent, ZONE_HOT_FILL);
        for (x, y) in [(160, 90), (290, 30), (290, 90)] {
            let p = shot.at(x, y);
            assert!(near(p, hot, 2), "fill at ({x},{y}): {p:?}");
        }
        // Outlined at full accent, 2 px wide.
        let acc = bgra(t.accent);
        for (x, y) in [
            (150, 60),
            (151, 60),
            (298, 60),
            (299, 60),
            (200, 20),
            (200, 21),
            (200, 98),
            (200, 99),
        ] {
            let p = shot.at(x, y);
            assert!(near(p, acc, 2), "edge at ({x},{y}): {p:?}");
        }
        assert!(near(shot.at(152, 60), hot, 2));
        // The other zone keeps the plain look.
        let plain = zones_shot(&t, 1.0, &zones, None, Color::TRANSPARENT, OVERLAY);
        for y in 0..OVERLAY.1 {
            for x in 0..145 {
                assert_eq!(shot.at(x, y), plain.at(x, y), "({x},{y})");
            }
        }
        assert_ne!(shot.px, plain.px);
        // A highlight past the last zone is no highlight.
        let past = zones_shot(&t, 1.0, &zones, Some(2), Color::TRANSPARENT, OVERLAY);
        assert_eq!(past.px, plain.px);
    }

    #[test]
    fn every_zone_shows_its_number_centred_in_the_text_colour() {
        let t = tokens();
        let zones = two_zones();
        let shot = zones_shot(&t, 1.0, &zones, None, Color::TRANSPARENT, OVERLAY);
        let ink = bgra(t.text);
        let mut shapes = Vec::new();
        for z in zones {
            let r = px(z);
            // Inside the outline: the glyph's solid pixels.
            let pixels = shot.ink((r.0 + 4, r.1 + 4, r.2 - 8, r.3 - 8), ink, 8);
            assert!(pixels.len() > 100, "number ink: {}", pixels.len());
            let (height, mx, my) = ink_box(&pixels);
            let (cx, cy) = (z.x + z.w / 2.0, z.y + z.h / 2.0);
            assert!((mx - cx).abs() < 5.0, "x {mx} vs {cx}");
            assert!((my - cy).abs() < 3.0, "y {my} vs {cy}");
            // 48 px of mono type: a digit is about 34 px tall, not a speck.
            assert!((28..=42).contains(&height), "height {height}");
            shapes.push(
                pixels
                    .iter()
                    .map(|p| (p.0 - r.0, p.1 - r.1))
                    .collect::<Vec<_>>(),
            );
        }
        // A 1 and a 2 are different glyphs.
        assert_ne!(shapes[0], shapes[1]);
    }

    #[test]
    fn the_number_has_a_dark_halo_so_it_reads_on_a_white_wallpaper() {
        let t = tokens();
        let zones = [LRect::new(20.0, 20.0, 200.0, 140.0)];
        let shot = zones_shot(&t, 1.0, &zones, None, Color::WHITE, OVERLAY);
        let ink = bgra(t.text);
        let pixels = shot.ink((24, 24, 192, 132), ink, 8);
        assert!(pixels.len() > 100);
        // The light wallpaper under the zone fill: what the halo is there to keep away.
        let wall = blend([255, 255, 255, 255], t.pill, ZONE_FILL);
        for (x, y) in pixels {
            for (dx, dy) in [
                (-2, 0),
                (2, 0),
                (0, -2),
                (0, 2),
                (-2, -2),
                (2, 2),
                (-2, 2),
                (2, -2),
            ] {
                let p = shot.at(x + dx, y + dy);
                assert!(
                    !near(p, wall, 8),
                    "bare wallpaper at ({},{}): {p:?}",
                    x + dx,
                    y + dy
                );
            }
        }
        // On black the number is just as solid.
        let dark = zones_shot(&t, 1.0, &zones, None, Color::BLACK, OVERLAY);
        assert!(dark.ink((24, 24, 192, 132), ink, 8).len() > 100);
    }

    #[test]
    fn between_zones_and_outside_them_the_overlay_is_transparent() {
        let t = tokens();
        // Three columns with a 10 px gap, the middle one highlighted.
        let zones = [
            LRect::new(10.0, 10.0, 90.0, 200.0),
            LRect::new(110.0, 10.0, 100.0, 200.0),
            LRect::new(220.0, 10.0, 90.0, 200.0),
        ];
        let shot = zones_shot(&t, 1.0, &zones, Some(1), Color::TRANSPARENT, OVERLAY);
        let rects: Vec<IRect> = zones.iter().map(|z| px(*z)).collect();
        assert_eq!(shot.lit_outside(&rects), None);
        assert_eq!(shot.at(105, 100), [0, 0, 0, 0], "gap");
        assert_eq!(shot.at(0, 0), [0, 0, 0, 0]);
        assert_eq!(shot.at(OVERLAY.0 - 1, OVERLAY.1 - 1), [0, 0, 0, 0]);
        for z in &rects {
            assert!(shot.lit_in(*z) > 5000);
        }
        // Nothing at all without zones.
        let empty = zones_shot(&t, 1.0, &[], Some(0), Color::TRANSPARENT, OVERLAY);
        assert_eq!(empty.lit_in((0, 0, OVERLAY.0, OVERLAY.1)), 0);
    }

    #[test]
    fn at_150_percent_the_decoration_scales_and_the_zones_stay_where_they_are() {
        let t = tokens();
        let zones = [LRect::new(30.0, 30.0, 180.0, 120.0)];
        let shot = zones_shot(&t, 1.5, &zones, None, Color::TRANSPARENT, (260, 200));
        let fill = premul(t.pill, ZONE_FILL);
        let line = blend(fill, t.text, ZONE_LINE_ALPHA);
        // A 3 px outline (2 x 1.5), then the fill.
        for x in 30..33 {
            assert!(near(shot.at(x, 90), line, 3), "line x={x}");
        }
        assert!(near(shot.at(33, 90), fill, 2), "fill after the line");
        assert_eq!(shot.at(29, 90)[3], 0, "outside the zone");
        for y in 30..33 {
            assert!(near(shot.at(120, y), line, 3), "line y={y}");
        }
        // The corners are r 18, not r 12.
        assert_eq!(shot.at(33, 33)[3], 0);
        assert_eq!(shot.at(209, 149)[3], 0);
        assert!(shot.at(30 + 9, 30 + 9)[3] > 60, "inside the arc: the fill");
        // The number is 72 px at 150 %: its solid pixels span about 50.
        let pixels = shot.ink((34, 34, 172, 112), bgra(t.text), 8);
        let (height, _, _) = ink_box(&pixels);
        assert!((42..=58).contains(&height), "height {height}");
        // A nonsense scale falls back to 100 %.
        let a = zones_shot(&t, 1.0, &zones, Some(0), Color::TRANSPARENT, (260, 200));
        for bad in [0.0, -2.0, f32::NAN, f32::INFINITY] {
            let b = zones_shot(&t, bad, &zones, Some(0), Color::TRANSPARENT, (260, 200));
            assert_eq!(a.px, b.px, "scale {bad}");
        }
    }

    #[test]
    fn small_zones_shrink_their_number_and_tiny_ones_skip_it() {
        let t = tokens();
        let ink = bgra(t.text);
        // 30 x 30: the number shrinks to 18 px and still fits inside the zone.
        let small = [LRect::new(20.0, 20.0, 30.0, 30.0)];
        let shot = zones_shot(&t, 1.0, &small, None, Color::TRANSPARENT, OVERLAY);
        assert_eq!(shot.lit_outside(&[(20, 20, 30, 30)]), None);
        assert!(
            shot.count_in((22, 22, 26, 26), ink, 8) > 10,
            "a small number"
        );
        // 12 x 12: no number, just the plate.
        let tiny = [LRect::new(20.0, 20.0, 12.0, 12.0)];
        let shot = zones_shot(&t, 1.0, &tiny, None, Color::TRANSPARENT, OVERLAY);
        assert_eq!(shot.lit_outside(&[(20, 20, 12, 12)]), None);
        assert_eq!(shot.count_in((20, 20, 12, 12), ink, 20), 0);
        assert!(shot.lit_in((20, 20, 12, 12)) > 100);
        // Degenerate zones draw nothing and do not panic.
        let odd = [
            LRect::new(5.0, 5.0, 1.0, 1.0),
            LRect::new(5.0, 5.0, 0.0, 40.0),
            LRect::new(5.0, 5.0, -10.0, 40.0),
            LRect::new(f32::NAN, 5.0, 40.0, 40.0),
            LRect::new(5.0, 5.0, f32::NAN, 40.0),
        ];
        let shot = zones_shot(&t, 1.0, &odd, Some(0), Color::TRANSPARENT, OVERLAY);
        assert_eq!(shot.lit_in((0, 0, OVERLAY.0, OVERLAY.1)), 0);
    }

    #[test]
    fn nine_zones_are_numbered_one_to_nine_and_a_tenth_is_two_digits() {
        let t = tokens();
        let ink = bgra(t.text);
        let grid: Vec<LRect> = (0..9)
            .map(|i| {
                LRect::new(
                    10.0 + (i % 3) as f32 * 100.0,
                    10.0 + (i / 3) as f32 * 70.0,
                    90.0,
                    60.0,
                )
            })
            .collect();
        let shot = zones_shot(&t, 1.0, &grid, None, Color::TRANSPARENT, (320, 230));
        let mut seen: Vec<Vec<(i32, i32)>> = Vec::new();
        for z in &grid {
            let r = px(*z);
            let pixels: Vec<(i32, i32)> = shot
                .ink((r.0 + 3, r.1 + 3, r.2 - 6, r.3 - 6), ink, 8)
                .into_iter()
                .map(|(x, y)| (x - r.0, y - r.1))
                .collect();
            assert!(pixels.len() > 30, "number ink {}", pixels.len());
            assert!(!seen.contains(&pixels), "two zones share a glyph");
            seen.push(pixels);
        }
        // A tenth zone (no layout makes one) still draws "10" inside its rect.
        let ten: Vec<LRect> = (0..10)
            .map(|i| LRect::new(i as f32 * 31.0, 0.0, 30.0, 60.0))
            .collect();
        let shot = zones_shot(&t, 1.0, &ten, None, Color::TRANSPARENT, (320, 70));
        let rects: Vec<IRect> = ten.iter().map(|z| px(*z)).collect();
        assert_eq!(shot.lit_outside(&rects), None);
        assert!(shot.count_in(rects[9], ink, 8) > 10);
    }
}
