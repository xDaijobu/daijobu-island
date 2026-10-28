//! Idle terminal shortcuts, painted in the same logical geometry used for hit testing.

use crate::agent_icons;
use crate::paint::{col, fill_paint, stroke_paint};
use dj_core::layout::Rect as LRect;
use dj_core::term_tabs::TabId;
use dj_core::terminal_shortcuts::{self, Shortcut};
use dj_core::theme::ResolvedTokens;
use skia_safe::{Canvas, RRect, Rect};

pub(crate) fn paint(
    canvas: &Canvas,
    tokens: &ResolvedTokens,
    body: LRect,
    shortcuts: &[Shortcut],
    hover: Option<TabId>,
    scale: f32,
) {
    let s = scale;
    for (shortcut, slot) in shortcuts
        .iter()
        .zip(terminal_shortcuts::buttons(body, shortcuts.len()))
    {
        let r = Rect::from_xywh(slot.x * s, slot.y * s, slot.w * s, slot.h * s);
        if hover == Some(shortcut.id) || shortcut.active {
            let rounded = RRect::new_rect_xy(r, 6.0 * s, 6.0 * s);
            let bg = if hover == Some(shortcut.id) {
                tokens.surface2
            } else {
                tokens.surface1
            };
            canvas.draw_rrect(rounded, &fill_paint(col(bg, 1.0)));
        }
        agent_icons::draw(
            canvas,
            shortcut.agent,
            r.center_x(),
            r.center_y(),
            16.0_f32.min(slot.h - 4.0).max(0.0) * s,
            col(tokens.text, 1.0),
        );
        if shortcut.activity {
            canvas.draw_circle(
                (r.right - 3.0 * s, r.top + 3.0 * s),
                2.0 * s,
                &fill_paint(col(tokens.accent, 1.0)),
            );
        } else if shortcut.active {
            canvas.draw_line(
                (r.center_x() - 3.0 * s, r.bottom - 1.5 * s),
                (r.center_x() + 3.0 * s, r.bottom - 1.5 * s),
                &stroke_paint(col(tokens.accent, 1.0), s),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::term_agents::Agent;
    use dj_core::theme::ThemeRegistry;
    use skia_safe::{surfaces, AlphaType, ColorType, ImageInfo};

    #[test]
    fn shortcut_marks_remain_visible_in_all_themes_and_dpi_scales() {
        let registry = ThemeRegistry::with_builtins();
        let shortcuts = [
            Shortcut {
                id: 1,
                agent: None,
                active: false,
                activity: false,
            },
            Shortcut {
                id: 2,
                agent: Some(Agent::Claude),
                active: true,
                activity: false,
            },
            Shortcut {
                id: 3,
                agent: Some(Agent::Codex),
                active: false,
                activity: true,
            },
        ];
        for id in registry.ids() {
            let t = registry.get(id).unwrap();
            for s in [1.0, 1.5, 2.0] {
                let body = LRect::new(0.0, 0.0, terminal_shortcuts::idle_width(96.0, 3), 24.0);
                let (w, h) = ((body.w * s) as i32, (body.h * s) as i32);
                let mut surface = surfaces::raster_n32_premul((w, h)).unwrap();
                surface.canvas().clear(col(t.pill, 1.0));
                paint(surface.canvas(), t, body, &shortcuts, Some(3), s);
                let info = ImageInfo::new((w, h), ColorType::BGRA8888, AlphaType::Premul, None);
                let mut pixels = vec![0u8; (w * h * 4) as usize];
                assert!(surface.image_snapshot().read_pixels(
                    &info,
                    &mut pixels,
                    (w * 4) as usize,
                    (0, 0),
                    skia_safe::image::CachingHint::Allow,
                ));
                let [red, green, blue] = t.text.to_u8();
                for (index, slot) in terminal_shortcuts::buttons(body, shortcuts.len())
                    .into_iter()
                    .enumerate()
                {
                    let mut ink = 0;
                    for y in (slot.y * s).ceil() as i32..((slot.y + slot.h) * s).floor() as i32 {
                        for x in (slot.x * s).ceil() as i32..((slot.x + slot.w) * s).floor() as i32
                        {
                            let p = &pixels[((y * w + x) * 4) as usize..][..4];
                            ink += usize::from(
                                p[0].abs_diff(blue) < 30
                                    && p[1].abs_diff(green) < 30
                                    && p[2].abs_diff(red) < 30,
                            );
                        }
                    }
                    assert!(
                        ink > 5,
                        "{} at {s}x: icon {index} has {ink} ink pixels",
                        t.name
                    );
                }
            }
        }
    }
}
