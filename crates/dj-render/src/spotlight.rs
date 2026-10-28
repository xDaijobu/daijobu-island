//! The Spotlight painter (Stage V3): the panel with its search field and result rows.
//! Draws exactly what `dj_core::spotlight::SpotlightView` describes (window-local logical
//! px).

use crate::fonts::{Fonts, Weight};
use crate::paint::{col, draw_text, fill_paint, fit, stroke_paint, text_width};
use crate::popup::{dev, paint_field, paint_panel, IconCache};
use dj_core::spotlight::{EntryKind, MenuView, ResultRow, SpotlightView, PANEL_R};
use dj_core::theme::ResolvedTokens;
use skia_safe::{
    BlurStyle, Canvas, ClipOp, Color, FilterMode, MaskFilter, MipmapMode, Paint, RRect, Rect,
    SamplingOptions,
};

/// The icon is drawn this many logical px square.
const ICON: f32 = 28.0;

fn base(cy: f32, size: f32) -> f32 {
    cy + 0.35 * size
}

pub fn paint_spotlight(
    canvas: &Canvas,
    fonts: &mut Fonts,
    tokens: &ResolvedTokens,
    scale: f32,
    view: &SpotlightView,
    icons: &IconCache,
) {
    let (t, s) = (tokens, scale);
    canvas.clear(Color::TRANSPARENT);
    paint_panel(canvas, t, s, view.panel, PANEL_R);

    canvas.save();
    let body = RRect::new_rect_xy(dev(view.panel, s), PANEL_R * s, PANEL_R * s);
    canvas.clip_rrect(body, ClipOp::Intersect, true);

    // The field, then the hint chip at its right.
    paint_field(canvas, fonts, t, s, &view.field, false, 16.0);
    let f = dev(view.field.rect, s);
    if !view.hint.is_empty() {
        let size = 10.0 * s;
        let mono = fonts.font("Geist Mono", Weight::Medium, size);
        let tw = text_width(&mono, view.hint, 0.0);
        let w = tw + 14.0 * s;
        let chip = Rect::from_xywh(f.right - w - 16.0 * s, f.center_y() - 10.0 * s, w, 20.0 * s);
        canvas.draw_rrect(
            RRect::new_rect_xy(chip, 6.0 * s, 6.0 * s),
            &fill_paint(col(t.surface1, 1.0)),
        );
        canvas.draw_rrect(
            RRect::new_rect_xy(chip.with_inset((0.5 * s, 0.5 * s)), 5.5 * s, 5.5 * s),
            &stroke_paint(col(t.surface2, 1.0), s.max(1.0)),
        );
        draw_text(
            canvas,
            &mono,
            view.hint,
            chip.center_x() - tw / 2.0,
            base(chip.center_y(), size),
            0.0,
            &fill_paint(col(t.text_dim, 1.0)),
        );
    }

    let has_below = !view.rows.is_empty() || view.note.is_some();
    if has_below {
        let y = f.bottom;
        let p = dev(view.panel, s);
        canvas.draw_line(
            (p.left + 12.0 * s, y),
            (p.right - 12.0 * s, y),
            &stroke_paint(col(t.text, 0.10), s.max(1.0)),
        );
    }

    for row in &view.rows {
        paint_result(canvas, fonts, t, s, row, icons);
    }

    if let Some(note) = view.note {
        let size = 12.5 * s;
        let font = fonts.font(&t.font_label, Weight::Medium, size);
        let p = dev(view.panel, s);
        let tw = text_width(&font, note, 0.0);
        let cy = f.bottom + (p.bottom - f.bottom) / 2.0;
        draw_text(
            canvas,
            &font,
            note,
            p.center_x() - tw / 2.0,
            base(cy, size),
            0.0,
            &fill_paint(col(t.text_dim, 1.0)),
        );
    }
    canvas.restore();

    // The right-click menu floats over everything (it may reach past the panel).
    if let Some(menu) = view.menu.as_ref() {
        paint_menu(canvas, fonts, t, s, menu);
    }
}

/// A row's right-click menu: a pill-coloured rounded card with a soft shadow and a
/// hairline, one label per item, a plate under the hovered one, hairlines between groups,
/// Delete in the warn colour.
fn paint_menu(canvas: &Canvas, fonts: &mut Fonts, t: &ResolvedTokens, s: f32, menu: &MenuView) {
    let r = dev(menu.rect, s);
    let rad = 10.0 * s;
    let body = RRect::new_rect_xy(r, rad, rad);
    let mut shadow = fill_paint(Color::from_argb(70, 0, 0, 0));
    shadow.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, 10.0 * s, false));
    canvas.draw_rrect(body.with_offset((0.0, 4.0 * s)), &shadow);
    canvas.draw_rrect(body, &fill_paint(col(t.pill, 1.0)));
    canvas.draw_rrect(
        RRect::new_rect_xy(r.with_inset((0.5, 0.5)), rad - 0.5, rad - 0.5),
        &stroke_paint(col(t.text, 0.14), 1.0),
    );
    let size = 12.5 * s;
    let font = fonts.font(&t.font_label, Weight::Medium, size);
    let hint_size = 10.0 * s;
    let mono = fonts.font("Geist Mono", Weight::Regular, hint_size);
    for item in &menu.items {
        let row = dev(item.rect, s);
        if item.separator {
            canvas.draw_line(
                (row.left + 10.0 * s, row.top),
                (row.right - 10.0 * s, row.top),
                &stroke_paint(col(t.text, 0.10), s.max(1.0)),
            );
        }
        if item.hovered {
            let plate = Rect::new(
                row.left + 5.0 * s,
                row.top + 2.0 * s,
                row.right - 5.0 * s,
                row.bottom - 2.0 * s,
            );
            canvas.draw_rrect(
                RRect::new_rect_xy(plate, 6.0 * s, 6.0 * s),
                &fill_paint(col(t.surface2, 1.0)),
            );
        }
        let ink = if item.danger {
            col(t.warn, 1.0)
        } else {
            col(t.text, 1.0)
        };
        let mut right = row.right - 14.0 * s;
        if !item.hint.is_empty() {
            let hw = text_width(&mono, item.hint, 0.0);
            right -= hw;
            draw_text(
                canvas,
                &mono,
                item.hint,
                right,
                base(row.center_y(), hint_size),
                0.0,
                &fill_paint(col(t.text_dim, 0.9)),
            );
            right -= 10.0 * s;
        }
        let x = row.left + 14.0 * s;
        let label = fit(&font, item.label, 0.0, (right - x).max(10.0 * s));
        draw_text(
            canvas,
            &font,
            &label,
            x,
            base(row.center_y(), size),
            0.0,
            &fill_paint(ink),
        );
    }
}

fn kind_tag(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::App => "App",
        EntryKind::Folder => "Folder",
        EntryKind::File => "File",
    }
}

fn paint_result(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    s: f32,
    row: &ResultRow,
    icons: &IconCache,
) {
    let r = dev(row.rect, s);
    let plate = r.with_inset((8.0 * s, 2.0 * s));
    if row.selected {
        canvas.draw_rrect(
            RRect::new_rect_xy(plate, 10.0 * s, 10.0 * s),
            &fill_paint(col(t.surface2, 1.0)),
        );
        // A thin accent edge marks the selection.
        canvas.draw_rrect(
            RRect::new_rect_xy(
                Rect::from_xywh(
                    plate.left + 2.0 * s,
                    plate.center_y() - 9.0 * s,
                    2.5 * s,
                    18.0 * s,
                ),
                1.2 * s,
                1.2 * s,
            ),
            &fill_paint(col(t.accent, 1.0)),
        );
    } else if row.hovered {
        canvas.draw_rrect(
            RRect::new_rect_xy(plate, 10.0 * s, 10.0 * s),
            &fill_paint(col(t.surface2, 0.45)),
        );
    }

    // Icon.
    let isz = ICON * s;
    let ir = Rect::from_xywh(r.left + 20.0 * s, r.center_y() - isz / 2.0, isz, isz);
    match icons.get(&row.icon_key) {
        Some(img) => {
            canvas.draw_image_rect_with_sampling_options(
                img,
                None,
                ir,
                SamplingOptions::new(FilterMode::Linear, MipmapMode::Linear),
                &Paint::default(),
            );
        }
        None => placeholder(canvas, fonts, t, s, ir, &row.name),
    }

    // Right-hand tag first, so the texts know how much room they have.
    let tag_size = 10.5 * s;
    let tag_font = fonts.font(&t.font_label, Weight::Medium, tag_size);
    let tag = kind_tag(row.kind);
    let tw = text_width(&tag_font, tag, 0.0);
    let tag_x = plate.right - 14.0 * s - tw;
    let tag_ink = if row.kind == EntryKind::App {
        col(t.text_dim, 0.8)
    } else {
        col(t.text_dim, 1.0)
    };
    draw_text(
        canvas,
        &tag_font,
        tag,
        tag_x,
        base(r.center_y(), tag_size),
        0.0,
        &fill_paint(tag_ink),
    );

    // The size (and a folder's file count), dim, left of the tag; it arrives lazily.
    let mut right = tag_x;
    if let Some(meta) = row.meta.as_deref() {
        let mfont = fonts.font("Geist Mono", Weight::Regular, tag_size);
        let mw = text_width(&mfont, meta, 0.0);
        let mx = tag_x - 12.0 * s - mw;
        draw_text(
            canvas,
            &mfont,
            meta,
            mx,
            base(r.center_y(), tag_size),
            0.0,
            &fill_paint(col(t.text_dim, 0.85)),
        );
        right = mx;
    }

    let tx = ir.right + 12.0 * s;
    let max_w = (right - tx - 14.0 * s).max(20.0 * s);
    let nsize = 13.5 * s;
    let dsize = 11.0 * s;
    let reg = fonts.font(&t.font_label, Weight::Medium, nsize);
    let bold = fonts.font(&t.font_label, Weight::SemiBold, nsize);
    let dim = fonts.font(&t.font_label, Weight::Regular, dsize);
    let has_detail = !row.detail.is_empty() && row.kind != EntryKind::App;
    let (name_y, detail_y) = if has_detail {
        (r.center_y() - 2.0 * s, r.center_y() + 12.0 * s)
    } else {
        (base(r.center_y(), nsize), 0.0)
    };
    draw_name(
        canvas,
        t,
        &reg,
        &bold,
        &row.name,
        &row.matched,
        tx,
        name_y,
        max_w,
    );
    if has_detail {
        let txt = fit_front(&dim, &row.detail, max_w);
        draw_text(
            canvas,
            &dim,
            &txt,
            tx,
            detail_y,
            0.0,
            &fill_paint(col(t.text_dim, 1.0)),
        );
    }
}

/// Like `fit`, but a path keeps its tail ("..\\Documents\\Budgets"): the end tells more.
fn fit_front(font: &skia_safe::Font, s: &str, max_w: f32) -> String {
    if text_width(font, s, 0.0) <= max_w {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    for start in 1..chars.len() {
        let cand: String = std::iter::once("..".to_string())
            .chain(std::iter::once(chars[start..].iter().collect()))
            .collect();
        if text_width(font, &cand, 0.0) <= max_w {
            return cand;
        }
    }
    String::new()
}

/// The name with the matched chars in the accent colour and the bolder weight.
#[allow(clippy::too_many_arguments)]
fn draw_name(
    canvas: &Canvas,
    t: &ResolvedTokens,
    regular: &skia_safe::Font,
    bold: &skia_safe::Font,
    name: &str,
    matched: &[usize],
    x: f32,
    y: f32,
    max_w: f32,
) {
    let text = fit(regular, name, 0.0, max_w);
    let cut = text.chars().count() < name.chars().count();
    let mut cx = x;
    for (i, ch) in text.chars().enumerate() {
        // Past the cut the ".." has no match.
        let tail = if cut { 2 } else { 0 };
        let hit = matched.contains(&i) && i < text.chars().count().saturating_sub(tail);
        let (font, color) = if hit {
            (bold, t.accent)
        } else {
            (regular, t.text)
        };
        let st = ch.to_string();
        canvas.draw_str(&st, (cx, y), font, &fill_paint(col(color, 1.0)));
        cx += font.measure_str(&st, None).0;
    }
}

/// A neutral rounded square with the first letter, until the real icon is loaded.
fn placeholder(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    s: f32,
    r: Rect,
    name: &str,
) {
    canvas.draw_rrect(
        RRect::new_rect_xy(r, 7.0 * s, 7.0 * s),
        &fill_paint(col(t.surface2, 1.0)),
    );
    let Some(first) = name.chars().next() else {
        return;
    };
    let size = 14.0 * s;
    let font = fonts.font(&t.font_label, Weight::SemiBold, size);
    let letter: String = first.to_uppercase().collect();
    let tw = text_width(&font, &letter, 0.0);
    draw_text(
        canvas,
        &font,
        &letter,
        r.center_x() - tw / 2.0,
        base(r.center_y(), size),
        0.0,
        &fill_paint(col(t.text_dim, 1.0)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::layout::Rect as LRect;
    use dj_core::settings_ui::FieldView;
    use dj_core::spotlight::{FIELD_H, MARGIN, PAD, PANEL_W, ROW_H, WIN_H, WIN_W};
    use dj_core::theme::{ThemeRegistry, DEFAULT_THEME};
    use skia_safe::{surfaces, AlphaType, ColorType, ImageInfo};

    fn tokens() -> ResolvedTokens {
        ThemeRegistry::with_builtins()
            .get(DEFAULT_THEME)
            .unwrap()
            .clone()
    }

    fn alpha_at(px: &[u8], w: i32, x: i32, y: i32) -> u8 {
        px[((y * w + x) * 4 + 3) as usize]
    }

    fn render(view: &SpotlightView, icons: &IconCache, scale: f32) -> (Vec<u8>, i32, i32) {
        let (w, h) = ((WIN_W * scale) as i32, (WIN_H * scale) as i32);
        let mut surface = surfaces::raster_n32_premul((w, h)).unwrap();
        let mut fonts = Fonts::new().unwrap();
        paint_spotlight(surface.canvas(), &mut fonts, &tokens(), scale, view, icons);
        let info = ImageInfo::new((w, h), ColorType::BGRA8888, AlphaType::Premul, None);
        let mut px = vec![0u8; (w * h * 4) as usize];
        assert!(surface.image_snapshot().read_pixels(
            &info,
            &mut px,
            (w * 4) as usize,
            (0, 0),
            skia_safe::image::CachingHint::Allow
        ));
        (px, w, h)
    }

    fn rows(n: usize) -> Vec<ResultRow> {
        (0..n)
            .map(|i| ResultRow {
                rect: LRect::new(MARGIN, MARGIN + FIELD_H + i as f32 * ROW_H, PANEL_W, ROW_H),
                kind: [EntryKind::App, EntryKind::File, EntryKind::Folder][i % 3],
                name: format!("Budget plan {i}"),
                detail: "C:\\Users\\someone\\Documents\\a\\very\\long\\folder\\path\\that\\keeps\\going\\and\\going".into(),
                icon_key: format!("key{i}"),
                selected: i == 2,
                hovered: i == 3,
                matched: vec![0, 1, 2],
                target: format!("t{i}"),
                meta: [None, Some("48 KB".to_string()), Some("{2026}".to_string())][i % 3].clone(),
            })
            .collect()
    }

    fn view(n: usize, note: Option<&'static str>, text: &str) -> SpotlightView {
        let h = FIELD_H
            + if n > 0 {
                n as f32 * ROW_H + PAD
            } else if note.is_some() {
                ROW_H
            } else {
                0.0
            };
        let panel = LRect::new(MARGIN, MARGIN, PANEL_W, h);
        SpotlightView {
            panel,
            field: FieldView {
                rect: LRect::new(MARGIN, MARGIN, PANEL_W, FIELD_H),
                text: text.into(),
                caret: text.chars().count(),
                focused: true,
                placeholder: "Search apps and files",
            },
            rows: rows(n),
            note,
            hint: "esc",
            menu: None,
        }
    }

    #[test]
    fn empty_query_and_note_render() {
        let icons = IconCache::new();
        for (v, tall) in [
            (view(0, None, ""), false),
            (view(0, Some("No results"), "zzz"), false),
        ] {
            let (px, w, h) = render(&v, &icons, 1.0);
            assert!(alpha_at(&px, w, w / 2, MARGIN as i32 + 20) > 200);
            assert!(alpha_at(&px, w, 2, 2) <= 2);
            assert!(alpha_at(&px, w, w - 3, h - 3) <= 2);
            // Below a field-only panel the window is transparent.
            if !tall {
                assert!(alpha_at(&px, w, w / 2, h - 30) <= 2);
            }
        }
    }

    #[test]
    fn rows_with_and_without_icons() {
        let mut icons = IconCache::new();
        // A solid red 8x8 icon for row 1 (premultiplied BGRA).
        icons.insert_bgra("key1", 8, 8, &[0, 0, 255, 255].repeat(64));
        for n in [1usize, 8] {
            let v = view(n, None, "bud");
            let (px, w, h) = render(&v, &icons, 1.0);
            assert!(alpha_at(&px, w, w / 2, MARGIN as i32 + 20) > 200);
            assert!(alpha_at(&px, w, w / 2, MARGIN as i32 + (FIELD_H + 10.0) as i32) > 200);
            assert!(alpha_at(&px, w, 2, h - 3) <= 2);
        }
        // Row 1's icon pixel is the icon's red; row 0 has none and is not.
        let v = view(2, None, "bud");
        let (px, w, _) = render(&v, &icons, 1.0);
        let at = |row: i32| {
            let x = (MARGIN + 20.0 + 14.0) as i32;
            let y = (MARGIN + FIELD_H) as i32 + row * ROW_H as i32 + ROW_H as i32 / 2;
            let i = ((y * w + x) * 4) as usize;
            [px[i + 2], px[i + 1], px[i]]
        };
        let r1 = at(1);
        assert!(r1[0] > 200 && r1[1] < 60 && r1[2] < 60, "{r1:?}");
        let r0 = at(0);
        assert!(!(r0[0] > 200 && r0[1] < 60), "{r0:?}");
    }

    #[test]
    fn scaled_render_and_long_text() {
        let icons = IconCache::new();
        let mut v = view(4, None, &"x".repeat(200));
        v.field.caret = 3;
        v.rows[0].name =
            "A very long application name that cannot possibly fit in the row at all, truly".into();
        v.rows[0].matched = vec![0, 70];
        let (px, w, _) = render(&v, &icons, 1.75);
        assert!(alpha_at(&px, w, w / 2, (MARGIN * 1.75) as i32 + 30) > 200);
    }

    #[test]
    fn a_menu_draws_over_the_rows_and_past_the_panel() {
        use dj_core::spotlight::{MenuAction, MenuItemView, MenuView, MENU_ROW_H, MENU_W};
        let icons = IconCache::new();
        let mut v = view(1, None, "budget");
        // A menu that reaches below the one-row panel, into the window's margin area.
        let top = MARGIN + FIELD_H + 20.0;
        let actions = [
            MenuAction::Open,
            MenuAction::Reveal,
            MenuAction::Properties,
            MenuAction::Delete,
        ];
        let items = actions
            .iter()
            .enumerate()
            .map(|(i, a)| MenuItemView {
                action: *a,
                label: a.label(EntryKind::File),
                hint: if i == 0 { "enter" } else { "" },
                rect: LRect::new(
                    MARGIN + 40.0,
                    top + 6.0 + i as f32 * MENU_ROW_H,
                    MENU_W,
                    MENU_ROW_H,
                ),
                hovered: i == 1,
                separator: i > 0,
                danger: *a == MenuAction::Delete,
            })
            .collect();
        let h = 12.0 + actions.len() as f32 * MENU_ROW_H;
        v.menu = Some(MenuView {
            rect: LRect::new(MARGIN + 40.0, top, MENU_W, h),
            items,
        });
        let (px, w, _) = render(&v, &icons, 1.0);
        // Opaque inside the menu even where the panel has ended.
        let below_panel = (v.panel.y + v.panel.h + 10.0) as i32;
        assert!(below_panel < (top + h) as i32);
        assert!(alpha_at(&px, w, (MARGIN + 60.0) as i32, below_panel) > 200);
    }
}
