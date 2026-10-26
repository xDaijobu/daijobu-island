//! The Settings panel (400 x 256): a header, then five rows: the Start with Windows
//! switch, the two Snap hotkey chips, the island style chip (`notch` or `pill`) and the
//! Reserve top space switch (dimmed for a pill, where it does nothing). Content is
//! anchored to the pill's top-centre like the other views. A hotkey that another app
//! holds draws its chip in the warn colour, with a status line below.

use crate::fonts::Weight;
use crate::music::button;
use crate::paint::{col, draw_text, fill_paint, stroke_paint, text_width, Frame, Painter};
use dj_core::config::IslandStyle;
use dj_core::layout::{self as lay, Rect as LRect};
use skia_safe::{Canvas, RRect, Rect};

/// Opacity of a row that has no effect right now.
const DIMMED: f32 = 0.4;

impl Painter {
    /// The Settings panel: header with back, then one row per setting.
    pub(crate) fn settings_panel(&mut self, canvas: &Canvas, f: &Frame, rect: &Rect) {
        let (s, t, v) = (f.scale, f.tokens, f.settings);
        let ox = rect.center_x() - lay::SETTINGS_W * s / 2.0;
        let oy = rect.top;
        let at = |r: LRect| Rect::from_xywh(ox + r.x * s, oy + r.y * s, r.w * s, r.h * s);

        // Header: lamp + SETTINGS on the left, back on the right.
        let label = self.fonts.font(&t.font_label, Weight::Medium, 8.5 * s);
        let h = at(lay::ST_HEADER);
        let hy = h.center_y();
        canvas.draw_circle(
            (h.left + 3.0 * s, hy),
            2.6 * s,
            &fill_paint(col(t.accent, 1.0)),
        );
        draw_text(
            canvas,
            &label,
            "SETTINGS",
            h.left + 12.0 * s,
            hy + 0.35 * 8.5 * s,
            1.6 * s,
            &fill_paint(col(t.text_dim, 1.0)),
        );
        let btn = self.fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
        button(
            canvas,
            &btn,
            at(lay::ST_BACK),
            "BACK",
            col(t.text, 1.0),
            col(t.surface1, 1.0),
            col(t.surface2, 1.0),
            1.2 * s,
            s,
        );

        // The reserve switch only does something for a notch: dimmed for a pill.
        let reserve_alpha = if v.style == IslandStyle::Notch {
            1.0
        } else {
            DIMMED
        };

        // Row labels.
        let row_font = self.fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
        for (row, text, alpha) in [
            (lay::ST_ROW_AUTOSTART, "START WITH WINDOWS", 1.0),
            (lay::ST_ROW_HOTKEY, "SNAP HOTKEY", 1.0),
            (lay::ST_ROW_ALT, "SNAP ALT HOTKEY", 1.0),
            (lay::ST_ROW_STYLE, "ISLAND STYLE", 1.0),
            (lay::ST_ROW_RESERVE, "RESERVE TOP SPACE", reserve_alpha),
        ] {
            let r = at(row);
            draw_text(
                canvas,
                &row_font,
                text,
                r.left,
                r.center_y() + 0.35 * 10.0 * s,
                1.2 * s,
                &fill_paint(col(t.text, alpha)),
            );
        }

        // The switches: an accent track with the knob right when on.
        for (switch, on, alpha) in [
            (lay::ST_TOGGLE, v.autostart, 1.0),
            (lay::ST_RESERVE, v.reserve, reserve_alpha),
        ] {
            let tr = at(switch);
            let rad = tr.height() / 2.0;
            let (track, edge) = if on {
                (t.accent, t.accent)
            } else {
                (t.surface1, t.surface2)
            };
            canvas.draw_rrect(
                RRect::new_rect_xy(tr, rad, rad),
                &fill_paint(col(track, alpha)),
            );
            canvas.draw_rrect(
                RRect::new_rect_xy(tr.with_inset((0.5, 0.5)), rad - 0.5, rad - 0.5),
                &stroke_paint(col(edge, alpha), 1.0),
            );
            let knob_r = rad - 4.0 * s;
            let knob_x = if on { tr.right - rad } else { tr.left + rad };
            let knob = if on { t.pill } else { t.text_dim };
            canvas.draw_circle(
                (knob_x, tr.center_y()),
                knob_r,
                &fill_paint(col(knob, alpha)),
            );
        }

        // The chips: two hotkeys (warn when taken) and the island style (never warns).
        let chip_font = self.fonts.font(&t.font_label, Weight::Medium, 10.0 * s);
        let chips = [
            (lay::ST_HOTKEY, v.hotkey.label(), v.hotkey_taken),
            (
                lay::ST_ALT,
                dj_core::settings::alt_label(v.alt),
                v.alt_taken,
            ),
            (lay::ST_STYLE, v.style.label(), false),
        ];
        for (chip, text, taken) in chips {
            let r = at(chip);
            let rad = r.height() / 2.0;
            let (edge, ink) = if taken {
                (t.warn, t.warn)
            } else {
                (t.surface2, t.text)
            };
            canvas.draw_rrect(
                RRect::new_rect_xy(r, rad, rad),
                &fill_paint(col(t.surface1, 1.0)),
            );
            canvas.draw_rrect(
                RRect::new_rect_xy(r.with_inset((0.5, 0.5)), rad - 0.5, rad - 0.5),
                &stroke_paint(col(edge, 1.0), 1.0),
            );
            let tw = text_width(&chip_font, text, 0.0);
            draw_text(
                canvas,
                &chip_font,
                text,
                r.center_x() - tw / 2.0,
                r.center_y() + 0.35 * 10.0 * s,
                0.0,
                &fill_paint(col(ink, 1.0)),
            );
        }

        // Status line.
        if let Some(line) = v.status() {
            let r = at(lay::ST_STATUS);
            let small = self.fonts.font(&t.font_label, Weight::Regular, 9.0 * s);
            draw_text(
                canvas,
                &small,
                &line.to_uppercase(),
                r.left,
                r.center_y() + 0.35 * 9.0 * s,
                1.2 * s,
                &fill_paint(col(t.warn, 1.0)),
            );
        }
    }
}
