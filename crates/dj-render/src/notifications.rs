//! Plain-text native notification views, painted inside the existing island body.
use crate::fonts::{Fonts, Weight};
use crate::paint::{col, fill_paint, Frame, Painter};
use dj_core::notifications::{self as n, Access, Activation, NotificationCenter};
use dj_core::theme::ResolvedTokens;
use skia_safe::textlayout::{
    FontCollection, ParagraphBuilder, ParagraphStyle, TextStyle, TypefaceFontProvider,
};
use skia_safe::{Canvas, ClipOp, RRect, Rect};
use skia_safe::{Color, FontMgr};

fn readable_surface(t: &ResolvedTokens, surface: dj_core::color::Rgb) -> dj_core::color::Rgb {
    if t.text.contrast(surface) >= 4.5 {
        surface
    } else {
        t.pill
    }
}

#[derive(Clone, Copy)]
pub struct NotificationPanel<'a> {
    pub center: &'a NotificationCenter,
    pub access: Access,
    pub enabled: bool,
    pub notice: Option<&'a str>,
    pub setup: &'a dj_core::notification_setup::Setup,
    pub setup_notice: Option<&'a str>,
}

struct Typography {
    collection: FontCollection,
    ink: Color,
}
impl Typography {
    fn new(fonts: &mut Fonts, t: &ResolvedTokens) -> Self {
        let mut provider = TypefaceFontProvider::new();
        provider.register_typeface(
            fonts.typeface(&t.font_label, Weight::Regular),
            Some("notification-text"),
        );
        let mut collection = FontCollection::new();
        collection.set_asset_font_manager(FontMgr::from(provider));
        collection.set_default_font_manager(FontMgr::default(), None);
        Self {
            collection,
            ink: col(t.text, 1.0),
        }
    }
}

fn text(
    canvas: &Canvas,
    typography: &Typography,
    value: &str,
    at: (f32, f32),
    width: f32,
    size: f32,
) {
    let mut style = TextStyle::new();
    style
        .set_font_families(&["notification-text"])
        .set_font_size(size)
        .set_color(typography.ink);
    let mut paragraph_style = ParagraphStyle::new();
    paragraph_style
        .set_text_style(&style)
        .set_max_lines(1)
        .set_ellipsis("\u{2026}");
    let mut builder = ParagraphBuilder::new(&paragraph_style, typography.collection.clone());
    builder.add_text(value);
    let mut paragraph = builder.build();
    paragraph.layout(width.max(1.0));
    paragraph.paint(canvas, (at.0, at.1 - size));
}

fn paint_panel(
    canvas: &Canvas,
    fonts: &mut Fonts,
    t: &ResolvedTokens,
    panel: NotificationPanel<'_>,
) {
    let typography = &Typography::new(fonts, t);
    text(
        canvas,
        typography,
        "NOTIFICATIONS",
        (18.0, 30.0),
        350.0,
        12.0,
    );
    text(canvas, typography, "X", (405.0, 31.0), 20.0, 13.0);
    text(
        canvas,
        typography,
        panel.access.label(),
        (18.0, 44.0),
        405.0,
        9.0,
    );
    canvas.draw_rrect(
        RRect::new_rect_xy(Rect::from_xywh(16.0, 50.0, 408.0, 30.0), 8.0, 8.0),
        &fill_paint(col(readable_surface(t, t.surface2), 1.0)),
    );
    let button = panel.setup.button(panel.enabled, panel.access);
    text(canvas, typography, button, (27.0, 70.0), 330.0, 11.0);
    if panel.enabled {
        text(canvas, typography, "Off", (380.0, 70.0), 40.0, 11.0);
    }
    for (i, item) in panel
        .center
        .history()
        .iter()
        .skip(panel.center.page * n::PAGE_SIZE)
        .take(n::PAGE_SIZE)
        .enumerate()
        .filter(|_| !(panel.enabled && panel.access == Access::MissingIdentity))
    {
        let r = n::row(i);
        canvas.draw_rrect(
            RRect::new_rect_xy(Rect::from_xywh(r.x, r.y, r.w, r.h), 8.0, 8.0),
            &fill_paint(col(readable_surface(t, t.surface1), 1.0)),
        );
        text(
            canvas,
            typography,
            &item.source,
            (r.x + 10.0, r.y + 14.0),
            260.0,
            9.0,
        );
        if item.activation != Activation::None {
            text(
                canvas,
                typography,
                "Open app",
                (r.x + 300.0, r.y + 14.0),
                66.0,
                9.0,
            );
        }
        text(
            canvas,
            typography,
            &item.title,
            (r.x + 10.0, r.y + 32.0),
            357.0,
            12.0,
        );
        text(
            canvas,
            typography,
            &item.body,
            (r.x + 10.0, r.y + 50.0),
            357.0,
            10.0,
        );
        text(
            canvas,
            typography,
            "X",
            (r.x + r.w - 23.0, r.y + 35.0),
            20.0,
            12.0,
        );
    }
    if panel.enabled && panel.access == Access::MissingIdentity {
        // Setup state and actionable errors remain visible even when internal
        // history exists. Use a bounded native paragraph rather than truncating
        // the useful Windows error to a single line.
        canvas.draw_rrect(
            RRect::new_rect_xy(Rect::from_xywh(16.0, 91.0, 408.0, 202.0), 8.0, 8.0),
            &fill_paint(col(readable_surface(t, t.surface1), 1.0)),
        );
        let mut style = TextStyle::new();
        style
            .set_font_families(&["notification-text"])
            .set_font_size(12.0)
            .set_color(typography.ink);
        let mut paragraph_style = ParagraphStyle::new();
        paragraph_style
            .set_text_style(&style)
            .set_max_lines(10)
            .set_ellipsis("\u{2026}");
        let mut builder = ParagraphBuilder::new(&paragraph_style, typography.collection.clone());
        builder.add_text(
            panel
                .setup_notice
                .unwrap_or_else(|| panel.setup.explanation()),
        );
        let mut paragraph = builder.build();
        paragraph.layout(384.0);
        paragraph.paint(canvas, (28.0, 106.0));
    } else if panel.center.history().is_empty() {
        text(
            canvas,
            typography,
            "No notifications in memory",
            (26.0, 144.0),
            380.0,
            12.0,
        );
        text(
            canvas,
            typography,
            "History clears when disabled or the app exits.",
            (26.0, 168.0),
            380.0,
            11.0,
        );
    }
    text(
        canvas,
        typography,
        panel
            .notice
            .unwrap_or("RAM only · dismiss also removes the Windows toast"),
        (18.0, 297.0),
        406.0,
        9.0,
    );
    text(canvas, typography, "< Prev", (22.0, 321.0), 70.0, 11.0);
    text(
        canvas,
        typography,
        &format!("{} / {}", panel.center.page + 1, panel.center.pages()),
        (200.0, 321.0),
        120.0,
        11.0,
    );
    text(canvas, typography, "Next >", (366.0, 321.0), 64.0, 11.0);
}

impl Painter {
    pub(crate) fn notifications(&mut self, canvas: &Canvas, f: &Frame, rect: &Rect, toast: bool) {
        let Some(panel) = f.notifications else {
            return;
        };
        let w = if toast { n::TOAST_W } else { n::PANEL_W };
        canvas.save();
        canvas.translate((rect.center_x() - w * f.scale / 2.0, rect.top));
        canvas.scale((f.scale, f.scale));
        canvas.clip_rect(
            Rect::from_xywh(0.0, 0.0, w, if toast { n::TOAST_H } else { n::PANEL_H }),
            ClipOp::Intersect,
            true,
        );
        if toast {
            if let Some(item) = panel.center.toast() {
                let t = f.tokens;
                let typography = &Typography::new(&mut self.fonts, t);
                text(canvas, typography, &item.source, (20.0, 20.0), 288.0, 9.0);
                text(canvas, typography, &item.title, (20.0, 40.0), 294.0, 12.0);
                text(canvas, typography, &item.body, (20.0, 58.0), 294.0, 10.0);
                text(canvas, typography, "X", (333.0, 40.0), 16.0, 12.0);
            }
        } else {
            paint_panel(canvas, &mut self.fonts, f.tokens, panel);
        }
        canvas.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::notifications::{NotificationInput, NotificationKey};
    use dj_core::theme::{ThemeRegistry, DEFAULT_THEME};
    use std::time::Instant;
    #[test]
    fn synthetic_native_panel_paints_without_a_window_or_permission() {
        let mut surface = skia_safe::surfaces::raster_n32_premul((440, 340)).unwrap();
        let mut fonts = Fonts::new().unwrap();
        let registry = ThemeRegistry::with_builtins();
        let tokens = registry.get(DEFAULT_THEME).unwrap();
        let mut center = NotificationCenter::default();
        center.configure_internal(true);
        center.admit(
            NotificationInput {
                key: NotificationKey::Internal(1),
                source: "Synthetic app".into(),
                title: "Native notification smoke".into(),
                body: "Memory only, no listener consent".into(),
                activation: Activation::None,
            },
            Instant::now(),
        );
        surface.canvas().clear(col(tokens.pill, 1.0));
        paint_panel(
            surface.canvas(),
            &mut fonts,
            tokens,
            NotificationPanel {
                center: &center,
                access: Access::MissingIdentity,
                enabled: true,
                notice: None,
                setup: &dj_core::notification_setup::Setup::default(),
                setup_notice: None,
            },
        );
        let bytes = surface
            .image_snapshot()
            .encode(None, skia_safe::EncodedImageFormat::PNG, 100)
            .unwrap();
        assert!(bytes.size() > 1000);
        // Synthetic presentation only; never encode actual notification contents.
        if let Some(path) = std::env::var_os("DAIJOBU_NOTIFICATION_SMOKE_PNG") {
            std::fs::write(path, bytes.as_bytes()).unwrap();
        }
    }
}
