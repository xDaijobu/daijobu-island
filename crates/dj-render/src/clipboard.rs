//! Native Clipboard chrome. Only bounded presentation text and thumbnail pixels
//! enter the painter; originals, filesystem IO and native clipboard access do not.

use crate::fonts::{Fonts, Weight};
use crate::paint::{col, fill_paint, stroke_paint};
use dj_core::clipboard_types::{Content, Filter, Kind, Payload, Row, Thumbnail};
use dj_core::clipboard_ui::{self as ui, Button, Focus, View};
use dj_core::color::Rgb;
use dj_core::layout::Rect as LRect;
use dj_core::theme::ResolvedTokens;
use skia_safe::textlayout::{
    FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, RectHeightStyle, RectWidthStyle,
    TextDirection, TextStyle, TypefaceFontProvider,
};
use skia_safe::{
    images, AlphaType, Canvas, ClipOp, Color, ColorType, Data, FilterMode, FontMgr, Image,
    ImageInfo, MipmapMode, RRect, Rect, SamplingOptions,
};

/// Immutable display snapshot. The payload is an optional selected-item lease;
/// previews render its thumbnail or bounded plain text, never HTML/RTF.
#[derive(Clone, Copy)]
pub struct ClipboardPanel<'a> {
    pub view: &'a View,
    pub payload: Option<&'a Payload>,
    /// Panel-local logical pixels, matching the model's hit testing.
    pub hover: Option<(f32, f32)>,
}

const PREVIEW_BYTES: usize = 16 * 1024;
const ROW_BYTES: usize = 4096;

fn rect(r: LRect) -> Rect {
    Rect::from_xywh(r.x, r.y, r.w, r.h)
}

fn sane_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale.clamp(0.5, 8.0)
    } else {
        1.0
    }
}

/// Prefer the theme's text colour; fall back to its guaranteed text/pill pair
/// where raised surfaces or accents would reduce label contrast.
fn ink(background: Rgb, tokens: &ResolvedTokens) -> Color {
    if tokens.text.contrast(background) >= 4.5 {
        col(tokens.text, 1.0)
    } else if tokens.pill.contrast(background) >= 4.5 {
        col(tokens.pill, 1.0)
    } else {
        let black = Rgb::new(0.0, 0.0, 0.0);
        let white = Rgb::new(1.0, 1.0, 1.0);
        col(
            if black.contrast(background) > white.contrast(background) {
                black
            } else {
                white
            },
            1.0,
        )
    }
}

fn plate(canvas: &Canvas, area: Rect, colour: Color, radius: f32) {
    canvas.draw_rrect(
        RRect::new_rect_xy(area, radius, radius),
        &fill_paint(colour),
    );
}

fn outline(canvas: &Canvas, area: Rect, colour: Color, width: f32) {
    canvas.draw_rrect(
        RRect::new_rect_xy(area.with_inset((width / 2.0, width / 2.0)), 6.0, 6.0),
        &stroke_paint(colour, width),
    );
}

fn contains(area: Rect, point: Option<(f32, f32)>) -> bool {
    point.is_some_and(|(x, y)| x >= area.left && x < area.right && y >= area.top && y < area.bottom)
}

/// Bound shaping work even if a caller accidentally passes original text or a
/// very large platform error. Truncation affects display only, never the lease.
fn presentation(text: &str, limit: usize, single_line: bool) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + 3);
    for ch in text[..end].chars() {
        if ch == '\t' || (single_line && matches!(ch, '\n' | '\r')) {
            out.push(' ');
        } else if !ch.is_control() || ch == '\n' {
            out.push(ch);
        }
    }
    if end < text.len() {
        out.push('…');
    }
    out
}

/// SkParagraph shapes whole runs (including combining marks, bidi and emoji
/// sequences) and uses installed fallback fonts for glyphs the theme lacks.
/// The collection lives for one frame, so sensitive paragraphs are not cached.
struct Text {
    collection: FontCollection,
}

impl Text {
    fn new(fonts: &mut Fonts, tokens: &ResolvedTokens) -> Self {
        let mut provider = TypefaceFontProvider::new();
        provider.register_typeface(
            fonts.typeface(&tokens.font_title, Weight::Regular),
            Some("clipboard-body"),
        );
        provider.register_typeface(
            fonts.typeface(&tokens.font_label, Weight::Medium),
            Some("clipboard-label"),
        );
        let mut collection = FontCollection::new();
        collection.set_asset_font_manager(FontMgr::from(provider));
        collection.set_default_font_manager(FontMgr::default(), None);
        Self { collection }
    }

    fn paragraph(
        &self,
        text: &str,
        width: f32,
        size: f32,
        label: bool,
        colour: Color,
        lines: usize,
    ) -> Paragraph {
        let mut style = TextStyle::new();
        style
            .set_font_families(&[if label {
                "clipboard-label"
            } else {
                "clipboard-body"
            }])
            .set_font_size(size)
            .set_color(colour);
        let mut paragraph_style = ParagraphStyle::new();
        paragraph_style
            .set_text_style(&style)
            .set_max_lines(lines)
            .set_ellipsis("…");
        let mut builder = ParagraphBuilder::new(&paragraph_style, self.collection.clone());
        builder.add_text(text);
        let mut paragraph = builder.build();
        paragraph.layout(width.max(1.0));
        paragraph
    }

    // Keep each call's font, colour and line budget beside its target rectangle.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        canvas: &Canvas,
        text: &str,
        area: Rect,
        size: f32,
        label: bool,
        colour: Color,
        lines: usize,
    ) {
        let text = presentation(
            text,
            if lines == 1 { ROW_BYTES } else { PREVIEW_BYTES },
            lines == 1,
        );
        let paragraph = self.paragraph(&text, area.width(), size, label, colour, lines);
        let saved = canvas.save();
        canvas.clip_rect(area, ClipOp::Intersect, true);
        paragraph.paint(canvas, (area.left, area.top));
        canvas.restore_to_count(saved);
    }
}

/// The only bitmap allocation in paint is bounded to 57,600 bytes. Do not
/// substitute original ImageData.bgra here: it may contain 64 MiB of pixels.
fn thumbnail(thumb: &Thumbnail) -> Option<Image> {
    if thumb.width == 0 || thumb.height == 0 || thumb.width > 160 || thumb.height > 90 {
        return None;
    }
    let stride = (thumb.width as usize).checked_mul(4)?;
    if stride.checked_mul(thumb.height as usize)? != thumb.bgra.len() {
        return None;
    }
    let info = ImageInfo::new(
        (thumb.width as i32, thumb.height as i32),
        ColorType::BGRA8888,
        AlphaType::Unpremul,
        None,
    );
    images::raster_from_data(&info, Data::new_copy(&thumb.bgra), stride)
}

fn aspect_fit(size: (f32, f32), area: Rect) -> Rect {
    let factor = (area.width() / size.0).min(area.height() / size.1);
    let (w, h) = (size.0 * factor, size.1 * factor);
    Rect::from_xywh(area.center_x() - w / 2.0, area.center_y() - h / 2.0, w, h)
}

fn checker(canvas: &Canvas, area: Rect, tokens: &ResolvedTokens) {
    let saved = canvas.save();
    canvas.clip_rect(area, ClipOp::Intersect, true);
    canvas.draw_rect(area, &fill_paint(col(tokens.surface1, 1.0)));
    let mut y = area.top;
    let mut row = 0;
    while y < area.bottom {
        let mut x = area.left + if row % 2 == 0 { 0.0 } else { 6.0 };
        while x < area.right {
            canvas.draw_rect(
                Rect::from_xywh(x, y, 6.0, 6.0),
                &fill_paint(col(tokens.surface2, 1.0)),
            );
            x += 12.0;
        }
        y += 6.0;
        row += 1;
    }
    canvas.restore_to_count(saved);
}

fn draw_thumbnail(canvas: &Canvas, thumb: &Thumbnail, area: Rect, tokens: &ResolvedTokens) -> bool {
    let Some(image) = thumbnail(thumb) else {
        return false;
    };
    checker(canvas, area, tokens);
    let saved = canvas.save();
    canvas.clip_rect(area, ClipOp::Intersect, true);
    canvas.draw_image_rect_with_sampling_options(
        &image,
        None,
        aspect_fit((image.width() as f32, image.height() as f32), area),
        SamplingOptions::new(FilterMode::Linear, MipmapMode::None),
        &fill_paint(Color::WHITE),
    );
    canvas.restore_to_count(saved);
    true
}

fn colour(text: &str) -> Option<Color> {
    let h = text.trim().strip_prefix('#')?;
    if !matches!(h.len(), 3 | 4 | 6 | 8) || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let short = h.len() <= 4;
    let component = |i: usize| -> Option<u8> {
        if short {
            u8::from_str_radix(&h[i..i + 1], 16).ok().map(|n| n * 17)
        } else {
            u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).ok()
        }
    };
    Some(Color::from_argb(
        if matches!(h.len(), 4 | 8) {
            component(3)?
        } else {
            255
        },
        component(0)?,
        component(1)?,
        component(2)?,
    ))
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Text => "Text",
        Kind::Link => "Link",
        Kind::Colour => "Colour",
        Kind::Image => "Image",
        Kind::Files => "Files",
    }
}

fn link_domain(text: &str) -> &str {
    let text = text.trim();
    let authority = text.split_once("://").map_or(text, |(_, rest)| rest);
    let authority = authority.split(['/', '?', '#']).next().unwrap_or("");
    authority.rsplit('@').next().unwrap_or("")
}

fn button(
    canvas: &Canvas,
    text: &Text,
    tokens: &ResolvedTokens,
    panel: ClipboardPanel<'_>,
    which: Button,
    area: LRect,
) {
    let area = rect(area);
    let enabled = ui::button_enabled(panel.view, which);
    let focus = panel.view.focus == Focus::Button(which);
    let background = if enabled {
        tokens.surface1
    } else {
        tokens.pill
    };
    plate(canvas, area, col(background, 1.0), 6.0);
    if enabled && contains(area, panel.hover) {
        outline(canvas, area, ink(background, tokens), 1.0);
    }
    if focus {
        outline(canvas, area, ink(background, tokens), 1.5);
    }
    // Close uses an ASCII cross so its 32px target remains legible at every DPI.
    let label = if which == Button::Close {
        "x"
    } else {
        ui::button_label(panel.view, which)
    };
    let label = if which == Button::IncludePinned {
        if panel.view.include_pinned {
            "[x] Include pinned items"
        } else {
            "[ ] Include pinned items"
        }
    } else {
        label
    };
    let p = text.paragraph(
        label,
        area.width() - 8.0,
        11.0,
        true,
        ink(background, tokens),
        1,
    );
    let x = area.left + (area.width() - p.longest_line()).max(8.0) / 2.0;
    let y = area.top + (area.height() - p.height()) / 2.0;
    let saved = canvas.save();
    canvas.clip_rect(area.with_inset((3.0, 1.0)), ClipOp::Intersect, true);
    p.paint(canvas, (x, y));
    if !enabled {
        // An understated bottom rule distinguishes unavailable actions without
        // making their labels unreadable on a light theme.
        canvas.draw_line(
            (area.left + 8.0, area.bottom - 3.0),
            (area.right - 8.0, area.bottom - 3.0),
            &stroke_paint(col(tokens.surface2, 1.0), 1.0),
        );
    }
    canvas.restore_to_count(saved);
}

fn search(canvas: &Canvas, text: &Text, tokens: &ResolvedTokens, view: &View) {
    let area = rect(ui::SEARCH);
    plate(canvas, area, col(tokens.surface1, 1.0), 8.0);
    let focused = view.focus == Focus::Search && !view.confirm_clear_all;
    if focused {
        outline(canvas, area, ink(tokens.surface1, tokens), 1.0);
    }
    let content = area.with_inset((10.0, 5.0));
    let saved = canvas.save();
    canvas.clip_rect(content, ClipOp::Intersect, true);
    let value = presentation(&view.query, ROW_BYTES, true);
    if value.is_empty() {
        text.draw(
            canvas,
            "Search clipboard…",
            content,
            12.0,
            false,
            ink(tokens.surface1, tokens),
            1,
        );
        if focused {
            canvas.draw_line(
                (content.left, content.top + 1.0),
                (content.left, content.bottom - 1.0),
                &stroke_paint(ink(tokens.surface1, tokens), 1.0),
            );
        }
    } else {
        // Layout on one wide line, then shift to keep the actual edit caret
        // visible. UTF-32 model indices are converted to SkParagraph's UTF-16.
        let p = text.paragraph(
            &value,
            32768.0,
            12.0,
            false,
            ink(tokens.surface1, tokens),
            1,
        );
        let utf16_at = |index: usize| {
            value
                .chars()
                .take(index)
                .map(char::len_utf16)
                .sum::<usize>()
        };
        let caret_index = view.caret();
        let range = if caret_index == 0 {
            0..utf16_at(1)
        } else {
            utf16_at(caret_index - 1)..utf16_at(caret_index)
        };
        let boxes = p.get_rects_for_range(range, RectHeightStyle::Tight, RectWidthStyle::Tight);
        let caret_x = boxes.last().map_or(0.0, |b| {
            if (b.direct == TextDirection::LTR) == (caret_index != 0) {
                b.rect.right
            } else {
                b.rect.left
            }
        });
        let offset = if focused {
            (caret_x - content.width() + 2.0).max(0.0)
        } else {
            0.0
        };
        if focused {
            if let Some((a, b)) = view.search_selection() {
                for b in p.get_rects_for_range(
                    utf16_at(a)..utf16_at(b),
                    RectHeightStyle::Tight,
                    RectWidthStyle::Tight,
                ) {
                    canvas.draw_rect(
                        b.rect.with_offset((content.left - offset, content.top)),
                        &fill_paint(col(tokens.highlight, 0.22)),
                    );
                }
            }
        }
        p.paint(canvas, (content.left - offset, content.top));
        if focused {
            let x = content.left + caret_x - offset;
            canvas.draw_line(
                (x, content.top + 1.0),
                (x, content.bottom - 1.0),
                &stroke_paint(ink(tokens.surface1, tokens), 1.0),
            );
        }
    }
    canvas.restore_to_count(saved);
}

fn filters(canvas: &Canvas, text: &Text, tokens: &ResolvedTokens, panel: ClipboardPanel<'_>) {
    for (filter, area) in ui::FILTERS {
        let background = if panel.view.filter == filter {
            tokens.accent
        } else {
            tokens.surface1
        };
        let area = rect(area);
        plate(canvas, area, col(background, 1.0), 7.0);
        if panel.view.focus == Focus::Filter(filter) || contains(area, panel.hover) {
            outline(canvas, area, ink(background, tokens), 1.0);
        }
        let label = match filter {
            Filter::All => "All",
            Filter::Text => "Text",
            Filter::Images => "Images",
            Filter::Files => "Files",
            Filter::Pinned => "Pinned",
        };
        text.draw(
            canvas,
            label,
            area.with_inset((9.0, 5.0)),
            10.0,
            true,
            ink(background, tokens),
            1,
        );
    }
}

fn draw_row(
    canvas: &Canvas,
    text: &Text,
    tokens: &ResolvedTokens,
    panel: ClipboardPanel<'_>,
    slot: usize,
    row: &Row,
) {
    let Some(area) = ui::row_rect(slot).map(rect) else {
        return;
    };
    let selected = panel.view.selected == Some(row.id);
    let background = if selected {
        tokens.surface2
    } else {
        tokens.pill
    };
    plate(
        canvas,
        area.with_inset((0.0, 1.0)),
        col(background, 1.0),
        7.0,
    );
    if selected {
        canvas.draw_rect(
            Rect::from_xywh(area.left + 1.0, area.top + 8.0, 2.0, 28.0),
            &fill_paint(ink(background, tokens)),
        );
        if panel.view.focus == Focus::List {
            outline(
                canvas,
                area.with_inset((0.0, 1.0)),
                ink(background, tokens),
                1.0,
            );
        }
    } else if contains(area, panel.hover) {
        outline(
            canvas,
            area.with_inset((0.0, 1.0)),
            col(tokens.surface2, 1.0),
            1.0,
        );
    }
    let foreground = ink(background, tokens);
    text.draw(
        canvas,
        &(slot + 1).to_string(),
        Rect::from_xywh(area.left + 7.0, area.top + 15.0, 10.0, 16.0),
        9.0,
        true,
        foreground,
        1,
    );
    let thumb_box = Rect::from_xywh(area.left + 23.0, area.top + 6.0, 46.0, 32.0);
    let mut thumb_drawn = false;
    if !row.corrupt {
        if row.kind == Kind::Image {
            if let Some(thumb) = &row.thumbnail {
                thumb_drawn = draw_thumbnail(canvas, thumb, thumb_box, tokens);
            }
        } else if row.kind == Kind::Colour {
            if let Some(c) = colour(&row.preview) {
                checker(canvas, thumb_box, tokens);
                canvas.draw_rect(thumb_box, &fill_paint(c));
                outline(canvas, thumb_box, foreground, 1.0);
                thumb_drawn = true;
            }
        }
    }
    if !thumb_drawn {
        plate(canvas, thumb_box, col(tokens.surface1, 1.0), 4.0);
        let label = if row.corrupt {
            "!"
        } else {
            match row.kind {
                Kind::Image => "IMG",
                Kind::Files => "FILE",
                Kind::Link => "URL",
                Kind::Colour => "HEX",
                Kind::Text => "TXT",
            }
        };
        text.draw(
            canvas,
            label,
            thumb_box.with_inset((5.0, 10.0)),
            9.0,
            true,
            ink(tokens.surface1, tokens),
            1,
        );
    }
    let copy = if row.corrupt {
        "Item unavailable"
    } else if row.preview.is_empty() {
        kind_label(row.kind)
    } else {
        &row.preview
    };
    let title_area = Rect::from_xywh(area.left + 78.0, area.top + 5.0, area.width() - 106.0, 17.0);
    text.draw(canvas, copy, title_area, 12.0, false, foreground, 1);
    let metadata = if row.corrupt {
        "Could not load · Delete to remove".to_owned()
    } else if row.kind == Kind::Link {
        format!(
            "Link · {}",
            presentation(link_domain(&row.preview), 256, true)
        )
    } else {
        let source = row.source.as_deref().map(|s| presentation(s, 256, true));
        match source {
            Some(source) if !source.is_empty() => format!("{} · {source}", kind_label(row.kind)),
            _ => format!("{} · {}", kind_label(row.kind), byte_label(row.bytes)),
        }
    };
    text.draw(
        canvas,
        &metadata,
        Rect::from_xywh(title_area.left, area.top + 25.0, title_area.width(), 14.0),
        9.0,
        true,
        foreground,
        1,
    );
    if row.pinned {
        // A vector pin avoids depending on an emoji glyph or its colour font.
        let x = area.right - 15.0;
        let y = area.top + 13.0;
        let paint = stroke_paint(foreground, 1.2);
        canvas.draw_line((x - 3.0, y), (x + 3.0, y), &paint);
        canvas.draw_line((x - 2.0, y), (x - 2.0, y + 6.0), &paint);
        canvas.draw_line((x + 2.0, y), (x + 2.0, y + 6.0), &paint);
        canvas.draw_line((x - 4.0, y + 6.0), (x + 4.0, y + 6.0), &paint);
        canvas.draw_line((x, y + 6.0), (x, y + 14.0), &paint);
    }
}

fn byte_label(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn message(canvas: &Canvas, text: &Text, tokens: &ResolvedTokens, title: &str, detail: &str) {
    text.draw(
        canvas,
        title,
        Rect::from_xywh(30.0, 128.0, 340.0, 27.0),
        16.0,
        false,
        col(tokens.text, 1.0),
        1,
    );
    text.draw(
        canvas,
        detail,
        Rect::from_xywh(30.0, 163.0, 340.0, 108.0),
        12.0,
        false,
        col(tokens.text, 1.0),
        6,
    );
}

fn preview(canvas: &Canvas, text: &Text, tokens: &ResolvedTokens, panel: ClipboardPanel<'_>) {
    let area = rect(ui::PREVIEW);
    let saved = canvas.save();
    canvas.clip_rect(area, ClipOp::Intersect, true);
    plate(canvas, area, col(tokens.surface1, 1.0), 8.0);
    let foreground = ink(tokens.surface1, tokens);
    let Some(row) = panel.view.selected_row() else {
        message(
            canvas,
            text,
            tokens,
            "No selected item",
            "Return to the list and select an item to preview.",
        );
        canvas.restore_to_count(saved);
        return;
    };
    if row.corrupt {
        message(
            canvas,
            text,
            tokens,
            "Item unavailable",
            "This item could not be loaded safely. You can delete it from history.",
        );
        canvas.restore_to_count(saved);
        return;
    }
    text.draw(
        canvas,
        &format!("{} preview · Esc to close", kind_label(row.kind)),
        area.with_inset((10.0, 8.0)),
        10.0,
        true,
        foreground,
        1,
    );
    let body = Rect::from_xywh(
        area.left + 10.0,
        area.top + 30.0,
        area.width() - 20.0,
        area.height() - 54.0,
    );
    let mut caption = "Plain text preview · Copy keeps the complete item".to_owned();
    match panel.payload.map(|p| &p.content) {
        Some(Content::Image(image)) if row.kind == Kind::Image => {
            if !draw_thumbnail(canvas, &image.thumbnail, body, tokens) {
                text.draw(
                    canvas,
                    "Image preview unavailable",
                    body,
                    12.0,
                    false,
                    foreground,
                    2,
                );
            }
            caption = format!("{} × {} · thumbnail preview", image.width, image.height);
        }
        _ if row.kind == Kind::Image => {
            if !row
                .thumbnail
                .as_ref()
                .is_some_and(|thumb| draw_thumbnail(canvas, thumb, body, tokens))
            {
                text.draw(
                    canvas,
                    "Loading image preview…",
                    body,
                    12.0,
                    false,
                    foreground,
                    2,
                );
            }
            caption = "Thumbnail preview · Copy keeps original resolution".to_owned();
        }
        Some(Content::Text { text: value, .. })
            if matches!(row.kind, Kind::Text | Kind::Link | Kind::Colour) =>
        {
            if let Some(c) = (row.kind == Kind::Colour).then(|| colour(value)).flatten() {
                let swatch = Rect::from_xywh(body.left, body.top, 80.0, 80.0);
                checker(canvas, swatch, tokens);
                canvas.draw_rect(swatch, &fill_paint(c));
                outline(canvas, swatch, foreground, 1.0);
                text.draw(
                    canvas,
                    value,
                    Rect::from_xywh(
                        body.left + 92.0,
                        body.top + 8.0,
                        body.width() - 92.0,
                        body.height(),
                    ),
                    13.0,
                    false,
                    foreground,
                    8,
                );
            } else {
                text.draw(canvas, value, body, 13.0, false, foreground, 9);
            }
        }
        Some(Content::Files(paths)) if row.kind == Kind::Files => {
            let mut value = String::new();
            for path in paths.iter().take(12) {
                value.push_str(&presentation(path, 1024, true));
                value.push('\n');
            }
            text.draw(canvas, &value, body, 12.0, false, foreground, 9);
            caption = format!("{} paths · files are not opened for preview", paths.len());
        }
        _ => {
            text.draw(canvas, &row.preview, body, 13.0, false, foreground, 9);
            caption = if panel.view.loading {
                "Loading preview…"
            } else {
                "Summary preview · Copy keeps the complete item"
            }
            .to_owned();
        }
    }
    text.draw(
        canvas,
        &caption,
        Rect::from_xywh(
            area.left + 10.0,
            area.bottom - 18.0,
            area.width() - 20.0,
            15.0,
        ),
        9.0,
        true,
        foreground,
        1,
    );
    canvas.restore_to_count(saved);
}

fn confirmation(canvas: &Canvas, text: &Text, tokens: &ResolvedTokens, panel: ClipboardPanel<'_>) {
    let area = rect(ui::LIST);
    plate(canvas, area, col(tokens.pill, 1.0), 8.0);
    text.draw(
        canvas,
        "Clear history?",
        Rect::from_xywh(40.0, 116.0, 320.0, 26.0),
        17.0,
        false,
        col(tokens.text, 1.0),
        1,
    );
    text.draw(
        canvas,
        if panel.view.include_pinned {
            "All history items, including pins, will be removed."
        } else {
            "Unpinned items will be removed. Pins are kept."
        },
        Rect::from_xywh(40.0, 148.0, 320.0, 36.0),
        12.0,
        false,
        col(tokens.text, 1.0),
        2,
    );
    button(
        canvas,
        text,
        tokens,
        panel,
        Button::IncludePinned,
        ui::CLEAR_INCLUDE_PINNED,
    );
    button(
        canvas,
        text,
        tokens,
        panel,
        Button::CancelClear,
        ui::CLEAR_CANCEL,
    );
    button(
        canvas,
        text,
        tokens,
        panel,
        Button::ConfirmClear,
        ui::CLEAR_CONFIRM,
    );
    text.draw(
        canvas,
        "Clipboard and original files stay intact.",
        Rect::from_xywh(40.0, 286.0, 320.0, 20.0),
        10.0,
        true,
        col(tokens.text, 1.0),
        1,
    );
}

fn tooltip(canvas: &Canvas, text: &Text, tokens: &ResolvedTokens, panel: ClipboardPanel<'_>) {
    if panel.view.confirm_clear_all {
        return;
    }
    let value = if contains(Rect::from_xywh(12.0, 8.0, 152.0, 24.0), panel.hover) {
        panel.view.status.as_deref()
    } else {
        ui::FOOTER_BUTTONS
            .iter()
            .chain(ui::HEADER_BUTTONS.iter())
            .find(|(_, area)| contains(rect(*area), panel.hover))
            .map(|(b, _)| ui::button_tooltip(panel.view, *b))
            .or_else(|| match panel.view.focus {
                Focus::Button(b) => Some(ui::button_tooltip(panel.view, b)),
                _ => None,
            })
    };
    let Some(value) = value else {
        return;
    };
    // A bounded in-panel tooltip may cover row metadata while hovered, but
    // never the action targets and never the host application's other panels.
    let area = Rect::from_xywh(20.0, 262.0, 360.0, 58.0);
    plate(canvas, area, col(tokens.pill, 1.0), 7.0);
    outline(canvas, area, col(tokens.text, 1.0), 1.0);
    text.draw(
        canvas,
        value,
        area.with_inset((8.0, 5.0)),
        11.0,
        false,
        col(tokens.text, 1.0),
        3,
    );
}

/// The island painter supplies the pill body. All content stays inside the
/// fixed panel, and all target rectangles come from the model's hit helpers.
pub(crate) fn paint_clipboard(
    canvas: &Canvas,
    fonts: &mut Fonts,
    tokens: &ResolvedTokens,
    at: (f32, f32),
    scale: f32,
    panel: ClipboardPanel<'_>,
) {
    if !at.0.is_finite() || !at.1.is_finite() {
        return;
    }
    let saved = canvas.save();
    let s = sane_scale(scale);
    canvas.translate((at.0 * s, at.1 * s));
    canvas.scale((s, s));
    canvas.clip_rect(
        Rect::from_wh(ui::PANEL_W, ui::PANEL_H),
        ClipOp::Intersect,
        true,
    );
    let text = Text::new(fonts, tokens);
    let view = panel.view;
    let title = view.status.as_deref().unwrap_or(if view.loading {
        "Loading history…"
    } else if !view.enabled {
        "History off"
    } else if view.paused {
        "History paused"
    } else {
        "Clipboard"
    });
    text.draw(
        canvas,
        title,
        Rect::from_xywh(12.0, 14.0, 152.0, 17.0),
        11.0,
        true,
        col(tokens.text, 1.0),
        1,
    );
    for (b, area) in ui::HEADER_BUTTONS {
        button(canvas, &text, tokens, panel, b, area);
    }
    search(canvas, &text, tokens, view);
    filters(canvas, &text, tokens, panel);
    if view.confirm_clear_all {
        confirmation(canvas, &text, tokens, panel);
    } else if view.preview {
        preview(canvas, &text, tokens, panel);
    } else if !view.enabled && view.rows.is_empty() {
        message(
            canvas,
            &text,
            tokens,
            "Clipboard history is off",
            "Enable to remember future copies on this device.",
        );
        button(canvas, &text, tokens, panel, Button::Enable, ui::ENABLE);
        text.draw(
            canvas,
            view.status
                .as_deref()
                .unwrap_or("History is encrypted for your Windows account."),
            Rect::from_xywh(30.0, 246.0, 340.0, 36.0),
            11.0,
            false,
            col(tokens.text, 1.0),
            2,
        );
    } else if view.rows.is_empty() {
        let (title, detail) = if view.loading {
            (
                "Loading history…",
                "Your saved items will appear when loading finishes.",
            )
        } else if !view.query.is_empty() || view.filter != Filter::All {
            (
                "No results",
                "Try another search or filter. Saved items are kept.",
            )
        } else if view.paused {
            (
                "History paused",
                "Existing history is kept. Resume to remember future copies.",
            )
        } else if let Some(status) = view.status.as_deref() {
            ("History status", status)
        } else {
            (
                "No history yet",
                "Copy text, an image or files to add your first item.",
            )
        };
        message(canvas, &text, tokens, title, detail);
    } else {
        let list_saved = canvas.save();
        canvas.clip_rect(rect(ui::LIST), ClipOp::Intersect, true);
        for (slot, row) in view.visible_rows().iter().enumerate() {
            draw_row(canvas, &text, tokens, panel, slot, row);
        }
        if view.rows.len() > ui::VISIBLE_ROWS {
            let track = rect(ui::LIST);
            let count = view.rows.len() as f32;
            let h = track.height() * ui::VISIBLE_ROWS as f32 / count;
            let top = track.top
                + (track.height() - h) * view.scroll.min(view.rows.len() - ui::VISIBLE_ROWS) as f32
                    / (count - ui::VISIBLE_ROWS as f32);
            plate(
                canvas,
                Rect::from_xywh(track.right - 2.0, top, 2.0, h),
                col(tokens.text, 1.0),
                1.0,
            );
        }
        canvas.restore_to_count(list_saved);
    }
    for (b, area) in ui::FOOTER_BUTTONS {
        button(canvas, &text, tokens, panel, b, area);
    }
    tooltip(canvas, &text, tokens, panel);
    canvas.restore_to_count(saved);
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::clipboard_types::ItemId;
    use dj_core::theme::ThemeRegistry;
    use skia_safe::surfaces;

    const THEMES: [&str; 4] = ["tobacco-tape", "midnight-cassette", "paper-zine", "clean"];
    const SCALES: [f32; 3] = [1.0, 1.5, 2.0];
    const PAD: f32 = 8.0;

    struct Raster {
        bytes: Vec<u8>,
        width: usize,
        scale: f32,
    }

    impl Raster {
        fn pixel(&self, x: f32, y: f32) -> [u8; 4] {
            let x = ((x + PAD) * self.scale).floor() as usize;
            let y = ((y + PAD) * self.scale).floor() as usize;
            self.bytes[(y * self.width + x) * 4..(y * self.width + x) * 4 + 4]
                .try_into()
                .unwrap()
        }

        fn ink(&self, area: Rect, colour: Color) -> usize {
            let mut count = 0;
            for y in ((area.top + PAD) * self.scale).ceil() as usize
                ..((area.bottom + PAD) * self.scale).floor() as usize
            {
                for x in ((area.left + PAD) * self.scale).ceil() as usize
                    ..((area.right + PAD) * self.scale).floor() as usize
                {
                    if self.bytes[(y * self.width + x) * 4..(y * self.width + x) * 4 + 4]
                        == [colour.b(), colour.g(), colour.r(), colour.a()]
                    {
                        count += 1;
                    }
                }
            }
            count
        }

        fn margins_are_empty(&self) {
            let left = (PAD * self.scale) as usize;
            let right = ((PAD + ui::PANEL_W) * self.scale) as usize;
            let bottom = ((PAD + ui::PANEL_H) * self.scale) as usize;
            let height = self.bytes.len() / (self.width * 4);
            for y in 0..height {
                for x in 0..self.width {
                    if x < left || x >= right || y < left || y >= bottom {
                        assert_eq!(
                            self.bytes[(y * self.width + x) * 4 + 3],
                            0,
                            "paint escaped panel"
                        );
                    }
                }
            }
        }
    }

    fn raster(
        fonts: &mut Fonts,
        t: &ResolvedTokens,
        scale: f32,
        view: &View,
        payload: Option<&Payload>,
        hover: Option<(f32, f32)>,
    ) -> Raster {
        let width = ((ui::PANEL_W + 2.0 * PAD) * scale) as i32;
        let height = ((ui::PANEL_H + 2.0 * PAD) * scale) as i32;
        let mut surface = surfaces::raster_n32_premul((width, height)).unwrap();
        surface.canvas().clear(Color::TRANSPARENT);
        surface.canvas().draw_rect(
            Rect::from_xywh(
                PAD * scale,
                PAD * scale,
                ui::PANEL_W * scale,
                ui::PANEL_H * scale,
            ),
            &fill_paint(col(t.pill, 1.0)),
        );
        paint_clipboard(
            surface.canvas(),
            fonts,
            t,
            (PAD, PAD),
            scale,
            ClipboardPanel {
                view,
                payload,
                hover,
            },
        );
        let info = ImageInfo::new(
            (width, height),
            ColorType::BGRA8888,
            AlphaType::Premul,
            None,
        );
        let mut bytes = vec![0; width as usize * height as usize * 4];
        assert!(surface.read_pixels(&info, &mut bytes, width as usize * 4, (0, 0)));
        Raster {
            bytes,
            width: width as usize,
            scale,
        }
    }

    fn thumb() -> Thumbnail {
        Thumbnail {
            width: 160,
            height: 40,
            bgra: [18, 220, 30, 255].repeat(160 * 40),
        }
    }

    fn row(id: u64, kind: Kind, preview: &str) -> Row {
        Row {
            id: ItemId(id),
            created_ms: 1,
            used_ms: 1,
            pinned: false,
            kind,
            preview: preview.to_owned(),
            source: Some("Synthetic fixture".to_owned()),
            thumbnail: (kind == Kind::Image).then(thumb),
            bytes: 4096,
            corrupt: false,
        }
    }

    fn populated() -> View {
        let mut view = View::default();
        view.enabled = true;
        view.set_rows(vec![
            row(
                1,
                Kind::Text,
                &"Very long text 日本語 e\u{301} 🦊 👩‍💻 ".repeat(200),
            ),
            row(2, Kind::Link, "https://example.invalid/synthetic?q=test"),
            row(3, Kind::Colour, "#3A9F"),
            row(4, Kind::Files, "two synthetic paths"),
            row(5, Kind::Image, "Synthetic wide image"),
            row(6, Kind::Text, "Sixth row after scrolling"),
        ]);
        view.rows[0].pinned = true;
        view.focus = Focus::List;
        view
    }

    #[test]
    fn five_rows_actions_selection_and_pin_are_readable_in_all_themes_and_dpis() {
        let registry = ThemeRegistry::with_builtins();
        let mut fonts = Fonts::new().unwrap();
        let view = populated();
        for theme in THEMES {
            let t = registry.get(theme).unwrap();
            for scale in SCALES {
                let image = raster(&mut fonts, t, scale, &view, None, None);
                image.margins_are_empty();
                for slot in 0..5 {
                    let r = rect(ui::row_rect(slot).unwrap());
                    let background = if slot == 0 { t.surface2 } else { t.pill };
                    assert!(
                        image.ink(
                            Rect::from_xywh(r.left + 78.0, r.top + 5.0, 200.0, 16.0),
                            ink(background, t)
                        ) > 12,
                        "row text: {theme} {scale} {slot}"
                    );
                }
                for (button, area) in ui::FOOTER_BUTTONS {
                    let background = if ui::button_enabled(&view, button) {
                        t.surface1
                    } else {
                        t.pill
                    };
                    let count = image.ink(rect(area).with_inset((4.0, 4.0)), ink(background, t));
                    assert!(
                        count > 5,
                        "action text: {theme} {scale} {button:?} ({count})"
                    );
                }
                let pin_area = Rect::from_xywh(365.0, 112.0, 16.0, 18.0);
                let mut unpinned = view.clone();
                unpinned.rows[0].pinned = false;
                let without_pin = raster(&mut fonts, t, scale, &unpinned, None, None);
                assert!(
                    image.ink(pin_area, col(t.surface2, 1.0))
                        < without_pin.ink(pin_area, col(t.surface2, 1.0)),
                    "antialiased vector pin: {theme} {scale}"
                );
                let mut unselected = view.clone();
                unselected.selected = None;
                unselected.rows[0].pinned = false;
                let other = raster(&mut fonts, t, scale, &unselected, None, None);
                assert_ne!(
                    image.pixel(15.0, 120.0),
                    other.pixel(15.0, 120.0),
                    "selection rail"
                );
                assert_ne!(image.bytes, other.bytes, "selection and pin change raster");
            }
        }
    }

    #[test]
    fn states_and_clear_confirmation_remain_clipped_and_have_actual_glyphs() {
        let registry = ThemeRegistry::with_builtins();
        let mut fonts = Fonts::new().unwrap();
        for theme in THEMES {
            let t = registry.get(theme).unwrap();
            for scale in SCALES {
                for state in 0..10 {
                    let mut view = View::default();
                    view.enabled = state != 0;
                    match state {
                        1 => view.loading = true,
                        2 => view.paused = true,
                        3 => view.query = "no synthetic match".into(),
                        4 => {
                            view.status =
                                Some("History full. Unpin or delete an item to continue.".into())
                        }
                        5 => {
                            view.status =
                                Some("Write failed. Your selected history item was kept.".into())
                        }
                        6 | 7 => {
                            view.confirm_clear_all = true;
                            view.include_pinned = state == 7;
                            view.focus = Focus::Button(Button::CancelClear);
                        }
                        8 => {
                            view.set_rows(vec![row(1, Kind::Text, "unavailable")]);
                            view.rows[0].corrupt = true;
                        }
                        9 => {
                            view.set_rows(vec![row(
                                1,
                                Kind::Text,
                                "Preview with no payload lease",
                            )]);
                            view.preview = true;
                        }
                        _ => {}
                    }
                    let image = raster(&mut fonts, t, scale, &view, None, None);
                    image.margins_are_empty();
                    assert!(
                        image.ink(Rect::from_xywh(30.0, 110.0, 340.0, 200.0), col(t.text, 1.0))
                            > 15,
                        "state text: {theme} {scale} {state}"
                    );
                    if view.confirm_clear_all {
                        for area in [
                            ui::CLEAR_CANCEL,
                            ui::CLEAR_CONFIRM,
                            ui::CLEAR_INCLUDE_PINNED,
                        ] {
                            assert!(
                                image.ink(rect(area).with_inset((4.0, 4.0)), ink(t.surface1, t))
                                    > 5,
                                "clear choice text"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn contained_image_previews_preserve_aspect_ratio_at_every_dpi() {
        let registry = ThemeRegistry::with_builtins();
        let mut fonts = Fonts::new().unwrap();
        let mut view = View::default();
        view.enabled = true;
        view.set_rows(vec![row(1, Kind::Image, "Synthetic image")]);
        view.preview = true;
        let payload = Payload {
            no_cloud: false,
            content: Content::Image(dj_core::clipboard_types::ImageData {
                width: 16000,
                height: 4000,
                // Original pixels are deliberately absent: drawing must use only
                // the thumbnail, even if an original lease would be very large.
                bgra: Vec::new(),
                png: Vec::new(),
                thumbnail: thumb(),
            }),
        };
        for theme in THEMES {
            let t = registry.get(theme).unwrap();
            for scale in SCALES {
                let image = raster(&mut fonts, t, scale, &view, Some(&payload), None);
                assert_eq!(image.pixel(200.0, 213.0), [18, 220, 30, 255]);
                assert_ne!(
                    image.pixel(200.0, 135.0),
                    [18, 220, 30, 255],
                    "letterbox above image"
                );
                assert_ne!(
                    image.pixel(200.0, 290.0),
                    [18, 220, 30, 255],
                    "letterbox below image"
                );
                image.margins_are_empty();
            }
        }
    }

    #[test]
    fn long_plain_text_emoji_files_and_colour_previews_never_escape_the_list() {
        let registry = ThemeRegistry::with_builtins();
        let mut fonts = Fonts::new().unwrap();
        let payloads = [
            (
                Kind::Text,
                Content::Text {
                    text: "日本語 e\u{301} 🦊 👩‍💻 <b>literal plain text</b>\n".repeat(30_000),
                    html: Some(b"<script>never rendered</script>".to_vec()),
                    rtf: None,
                },
            ),
            (
                Kind::Files,
                Content::Files(vec![
                    "C:\\Synthetic\\a.txt".into(),
                    "C:\\Synthetic\\b.txt".into(),
                ]),
            ),
            (
                Kind::Colour,
                Content::Text {
                    text: "#3A98".into(),
                    html: None,
                    rtf: None,
                },
            ),
        ];
        for theme in THEMES {
            let t = registry.get(theme).unwrap();
            for scale in SCALES {
                for (kind, content) in &payloads {
                    let mut view = View::default();
                    view.enabled = true;
                    view.set_rows(vec![row(1, *kind, "Synthetic preview")]);
                    let before = raster(&mut fonts, t, scale, &view, None, None);
                    view.preview = true;
                    let payload = Payload {
                        content: content.clone(),
                        no_cloud: false,
                    };
                    let after = raster(&mut fonts, t, scale, &view, Some(&payload), None);
                    after.margins_are_empty();
                    // Preview replaces only LIST. Search/filter/footer geometry
                    // remains byte-for-byte identical on the CPU surface.
                    for y in [40.0, 75.0, 335.0] {
                        for x in [18.0, 60.0, 140.0, 260.0, 380.0] {
                            assert_eq!(before.pixel(x, y), after.pixel(x, y));
                        }
                    }
                    assert!(
                        after.ink(
                            Rect::from_xywh(24.0, 132.0, 348.0, 150.0),
                            ink(t.surface1, t)
                        ) > 10,
                        "preview text"
                    );
                }
            }
        }
    }

    #[test]
    fn scrolling_focus_search_selection_and_export_tooltip_have_visible_raster_effects() {
        let registry = ThemeRegistry::with_builtins();
        let mut fonts = Fonts::new().unwrap();
        for theme in THEMES {
            let t = registry.get(theme).unwrap();
            for scale in SCALES {
                let mut view = populated();
                let first = raster(&mut fonts, t, scale, &view, None, None);
                view.scroll = 1;
                view.selected = Some(ItemId(6));
                let scrolled = raster(&mut fonts, t, scale, &view, None, None);
                assert_ne!(first.bytes, scrolled.bytes, "scroll paints different rows");
                view.insert_search("A🦊e\u{301} synthetic search that is long enough to scroll the caret into view");
                view.loading = false;
                view.key(0x41, true, false, false);
                let selected = raster(&mut fonts, t, scale, &view, None, None);
                view.key(0x27, false, false, false);
                let caret = raster(&mut fonts, t, scale, &view, None, None);
                assert_ne!(selected.bytes, caret.bytes, "selection highlight");
                view.set_rows(vec![row(1, Kind::Image, "Synthetic image")]);
                view.focus = Focus::Button(Button::Drag);
                let focused = raster(&mut fonts, t, scale, &view, None, Some((350.0, 338.0)));
                focused.margins_are_empty();
                assert!(
                    focused.ink(Rect::from_xywh(28.0, 270.0, 344.0, 43.0), col(t.text, 1.0)) > 20,
                    "export destination tooltip text"
                );
            }
        }
    }

    #[test]
    fn status_chrome_keeps_all_five_rows_visible_and_does_not_mislabel_no_results() {
        let registry = ThemeRegistry::with_builtins();
        let mut fonts = Fonts::new().unwrap();
        for theme in THEMES {
            let t = registry.get(theme).unwrap();
            for scale in SCALES {
                let mut view = populated();
                let baseline = raster(&mut fonts, t, scale, &view, None, None);
                for status in [
                    "Recording future clipboard changes",
                    "Copied",
                    "History full",
                    "Paste failed: target unavailable",
                ] {
                    view.status = Some(status.to_owned());
                    let status_image = raster(&mut fonts, t, scale, &view, None, None);
                    let start = ((ui::LIST.y + PAD) * scale) as usize * baseline.width * 4;
                    let end =
                        ((ui::LIST.y + ui::LIST.h + PAD) * scale) as usize * baseline.width * 4;
                    assert_eq!(
                        &baseline.bytes[start..end],
                        &status_image.bytes[start..end],
                        "status must not cover clickable rows: {theme} {scale}"
                    );
                }
                view.insert_search("no synthetic match");
                view.loading = false;
                view.status = None;
                let empty = raster(&mut fonts, t, scale, &view, None, None);
                view.status = Some("Recording future clipboard changes".to_owned());
                let with_info = raster(&mut fonts, t, scale, &view, None, None);
                let start = ((ui::LIST.y + PAD) * scale) as usize * empty.width * 4;
                let end = ((ui::LIST.y + ui::LIST.h + PAD) * scale) as usize * empty.width * 4;
                assert_eq!(
                    &empty.bytes[start..end],
                    &with_info.bytes[start..end],
                    "informational status must preserve No results"
                );
            }
        }
    }

    #[test]
    fn malformed_thumbnails_are_rejected_and_straight_alpha_is_composited() {
        assert!(thumbnail(&Thumbnail {
            width: u32::MAX,
            height: 90,
            bgra: vec![]
        })
        .is_none());
        assert!(thumbnail(&Thumbnail {
            width: 161,
            height: 1,
            bgra: vec![0; 644]
        })
        .is_none());
        assert!(thumbnail(&Thumbnail {
            width: 1,
            height: 91,
            bgra: vec![0; 364]
        })
        .is_none());
        assert!(thumbnail(&Thumbnail {
            width: 1,
            height: 1,
            bgra: vec![0; 3]
        })
        .is_none());
        let thumb = Thumbnail {
            width: 1,
            height: 1,
            bgra: vec![0, 0, 255, 128],
        };
        let image = thumbnail(&thumb).unwrap();
        let mut surface = surfaces::raster_n32_premul((1, 1)).unwrap();
        surface.canvas().clear(Color::WHITE);
        surface.canvas().draw_image(&image, (0, 0), None);
        let info = ImageInfo::new((1, 1), ColorType::BGRA8888, AlphaType::Premul, None);
        let mut pixel = [0; 4];
        assert!(surface.read_pixels(&info, &mut pixel, 4, (0, 0)));
        assert!(pixel[0].abs_diff(127) <= 1 && pixel[1].abs_diff(127) <= 1);
        assert_eq!(&pixel[2..], &[255, 255]);
    }

    #[test]
    fn display_bounds_are_utf8_safe_and_colours_and_domains_are_literal() {
        let value = "🦊".repeat(PREVIEW_BYTES);
        let display = presentation(&value, PREVIEW_BYTES - 1, false);
        assert!(display.len() <= PREVIEW_BYTES + 3);
        assert!(display.ends_with('…'));
        assert_eq!(presentation("a\r\nb\t\0c", 100, true), "a  b c");
        assert_eq!(colour("#f08").unwrap(), Color::from_argb(255, 255, 0, 136));
        assert_eq!(colour("#F08A").unwrap().a(), 170);
        assert_eq!(colour("#FF008880").unwrap().a(), 128);
        assert!(colour("#red").is_none());
        assert!(colour("#ffffff<script>").is_none());
        assert_eq!(
            link_domain("https://user@example.invalid/path?q=a"),
            "example.invalid"
        );
        assert_eq!(
            aspect_fit((400.0, 100.0), Rect::from_wh(200.0, 200.0)),
            Rect::from_xywh(0.0, 75.0, 200.0, 50.0)
        );
    }

    #[test]
    fn system_fallback_shapes_emoji_cjk_and_combining_sequences() {
        let registry = ThemeRegistry::with_builtins();
        let mut fonts = Fonts::new().unwrap();
        let text = Text::new(&mut fonts, registry.get("clean").unwrap());
        let mut p = text.paragraph(
            "Hello 日本語 e\u{301} 🦊 👩‍💻",
            350.0,
            13.0,
            false,
            Color::BLACK,
            2,
        );
        assert_eq!(
            p.unresolved_glyphs(),
            Some(0),
            "installed Windows fallback fonts must cover synthetic CJK/emoji"
        );
        assert!(p.longest_line() > 50.0 && p.height() > 10.0);
    }
}
