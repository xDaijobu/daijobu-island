//! Logical-pixel geometry (100% DPI). Positions in `home_*` are relative to the
//! pill's top-left corner.

use crate::features::FEATURES;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// Idle pill: 128 x 32, radius 16.
pub const IDLE_W: f32 = 128.0;
pub const IDLE_H: f32 = 32.0;
pub const IDLE_R: f32 = 16.0;

/// Expanded home grid.
pub const HOME_W: f32 = 520.0;
pub const HOME_H: f32 = 340.0;
pub const HOME_R: f32 = 32.0;
/// Expanded Vitals panel. plan.html draws it 400 x 150; Stage 5 adds disk I/O, the top
/// processes and the Sweep button, so it is taller.
pub const VITALS_W: f32 = 400.0;
pub const VITALS_H: f32 = 220.0;
pub const VITALS_R: f32 = 32.0;
/// The "X reclaimable" button that opens Sweep (pill-local).
pub const VITALS_SWEEP: Rect = Rect::new(236.0, 182.0, 148.0, 26.0);

/// The Sweep view inside Vitals (plan.html: 400 x 200; a little taller for the
/// quick actions row).
pub const SWEEP_W: f32 = 400.0;
pub const SWEEP_H: f32 = 236.0;
pub const SWEEP_R: f32 = 32.0;
/// Sweep elements, relative to the pill's top-left.
pub const SW_BACK: Rect = Rect::new(12.0, 8.0, 100.0, 28.0);
const SW_TOP: f32 = 40.0;
const SW_ROW_H: f32 = 24.0;
const SW_LINE_H: f32 = 20.0;
/// A click right of this x on a category row opens its list; left of it toggles the tick.
pub const SW_ROW_SPLIT: f32 = 276.0;
pub const SW_DNS: Rect = Rect::new(16.0, 166.0, 92.0, 24.0);
pub const SW_BIN: Rect = Rect::new(116.0, 166.0, 92.0, 24.0);
pub const SW_PREV: Rect = Rect::new(12.0, 166.0, 80.0, 24.0);
pub const SW_NEXT: Rect = Rect::new(308.0, 166.0, 80.0, 24.0);
pub const SW_CLEAN: Rect = Rect::new(236.0, 198.0, 148.0, 28.0);

/// Music compact: art, title and a 5-column VU.
pub const MUSIC_COMPACT_W: f32 = 224.0;
pub const MUSIC_COMPACT_H: f32 = 36.0;
pub const MUSIC_COMPACT_R: f32 = 18.0;
/// Music expanded (also the sign-in view).
pub const MUSIC_EXP_W: f32 = 400.0;
pub const MUSIC_EXP_H: f32 = 184.0;
pub const MUSIC_EXP_R: f32 = 32.0;

/// Compact elements, relative to the pill's top-left.
pub const MC_ART: Rect = Rect::new(6.0, 6.0, 24.0, 24.0);
pub const MC_TITLE: Rect = Rect::new(38.0, 0.0, 118.0, 36.0);
pub const MC_VU: Rect = Rect::new(166.0, 9.0, 46.0, 18.0);

/// Expanded elements, relative to the pill's top-left.
pub const ME_ART: Rect = Rect::new(16.0, 14.0, 88.0, 88.0);
pub const ME_HEADER: Rect = Rect::new(120.0, 14.0, 264.0, 12.0);
pub const ME_TITLE: Rect = Rect::new(120.0, 30.0, 264.0, 20.0);
pub const ME_ARTIST: Rect = Rect::new(120.0, 52.0, 264.0, 16.0);
pub const ME_PREV: Rect = Rect::new(120.0, 72.0, 34.0, 34.0);
pub const ME_PLAY: Rect = Rect::new(162.0, 72.0, 34.0, 34.0);
pub const ME_NEXT: Rect = Rect::new(204.0, 72.0, 34.0, 34.0);
/// Scrubber hit area; the bar itself is drawn 4 px tall in its middle.
pub const ME_SCRUB: Rect = Rect::new(16.0, 108.0, 368.0, 14.0);
pub const ME_VU: Rect = Rect::new(16.0, 138.0, 368.0, 16.0);
pub const ME_BTN_OPEN: Rect = Rect::new(16.0, 158.0, 234.0, 20.0);
pub const ME_BTN_HOME: Rect = Rect::new(258.0, 158.0, 126.0, 20.0);
/// Sign-in view only.
pub const ME_SIGN_IN: Rect = Rect::new(80.0, 66.0, 240.0, 36.0);

/// Timer panel (opened from the Home tile or Timer compact).
pub const TIMER_W: f32 = 400.0;
pub const TIMER_H: f32 = 184.0;
pub const TIMER_R: f32 = 32.0;
/// Panel elements, relative to the pill's top-left. The header carries the phase label.
pub const TP_HEADER: Rect = Rect::new(20.0, 12.0, 360.0, 16.0);
pub const TP_RING: Rect = Rect::new(20.0, 36.0, 92.0, 92.0);
pub const TP_DIGITS: Rect = Rect::new(128.0, 36.0, 252.0, 56.0);
pub const TP_CHIP_Y: f32 = 102.0;
pub const TP_CHIP_W: f32 = 64.0;
pub const TP_CHIP_H: f32 = 24.0;
pub const TP_CHIP_GAP: f32 = 8.0;
/// Start / pause / resume (continue while Done).
pub const TP_TOGGLE: Rect = Rect::new(20.0, 140.0, 172.0, 30.0);
pub const TP_SKIP: Rect = Rect::new(200.0, 140.0, 86.0, 30.0);
/// Reset (stop while Done).
pub const TP_RESET: Rect = Rect::new(294.0, 140.0, 86.0, 30.0);

/// Preset chip `i` of the Timer panel.
pub fn tp_chip(i: usize) -> Rect {
    Rect::new(
        TP_DIGITS.x + i as f32 * (TP_CHIP_W + TP_CHIP_GAP),
        TP_CHIP_Y,
        TP_CHIP_W,
        TP_CHIP_H,
    )
}

/// Settings panel (opened from the gear on Home): the Timer panel's width, two rows taller.
pub const SETTINGS_W: f32 = 400.0;
pub const SETTINGS_H: f32 = 256.0;
pub const SETTINGS_R: f32 = 32.0;
/// Panel elements, relative to the pill's top-left. The header text sits left of back.
pub const ST_HEADER: Rect = Rect::new(20.0, 12.0, 260.0, 16.0);
pub const ST_BACK: Rect = Rect::new(304.0, 8.0, 84.0, 24.0);
/// Row labels on the left; the controls are right-aligned at x = 380.
pub const ST_ROW_AUTOSTART: Rect = Rect::new(20.0, 44.0, 220.0, 28.0);
pub const ST_ROW_HOTKEY: Rect = Rect::new(20.0, 80.0, 220.0, 28.0);
pub const ST_ROW_ALT: Rect = Rect::new(20.0, 116.0, 220.0, 28.0);
pub const ST_ROW_STYLE: Rect = Rect::new(20.0, 152.0, 220.0, 28.0);
pub const ST_ROW_RESERVE: Rect = Rect::new(20.0, 188.0, 220.0, 28.0);
pub const ST_TOGGLE: Rect = Rect::new(328.0, 46.0, 52.0, 24.0);
pub const ST_HOTKEY: Rect = Rect::new(248.0, 82.0, 132.0, 24.0);
pub const ST_ALT: Rect = Rect::new(248.0, 118.0, 132.0, 24.0);
/// The island style chip (`notch` / `pill`).
pub const ST_STYLE: Rect = Rect::new(248.0, 154.0, 132.0, 24.0);
/// The "reserve top space" switch (same size as the autostart one).
pub const ST_RESERVE: Rect = Rect::new(328.0, 190.0, 52.0, 24.0);
/// The `taken by another app` line.
pub const ST_STATUS: Rect = Rect::new(20.0, 226.0, 360.0, 16.0);

/// Tile panel (window zones): the layout picker the island opens while a window is being
/// dragged, or from the Home tile. Its content (header, thumbnails, footer) is laid out
/// by `tile::panel_thumbs` in panel-local px; the island only owns the size and routes
/// clicks. Not `TILE_W` / `TILE_H`: those are the Home grid's tile size.
pub const TILE_PANEL_W: f32 = 400.0;
pub const TILE_PANEL_H: f32 = 168.0;
pub const TILE_PANEL_R: f32 = 32.0;

/// Terminal panel: a shell inside the island. The largest state, so the window (and its
/// swap chain) is sized for it. Everything below is panel-local px.
pub const TERM_W: f32 = 760.0;
pub const TERM_H: f32 = 420.0;
pub const TERM_R: f32 = 22.0;
/// The header row: a lamp, the shell's title and the close button.
pub const TERM_HEADER_H: f32 = 36.0;
pub const TERM_CLOSE: Rect = Rect::new(TERM_W - 14.0 - 24.0, 6.0, 24.0, 24.0);
/// Where the grid's cells are drawn and hit-tested. The grid is as many whole cells as
/// fit; what's left over stays empty on the right and at the bottom.
pub const TERM_TEXT: Rect = Rect::new(14.0, 40.0, TERM_W - 28.0, TERM_H - 40.0 - 12.0);

/// The gear in the Home header, left of the clock (pill-local on the Home grid).
pub const HOME_GEAR: Rect = Rect::new(384.0, 16.0, 28.0, 28.0);

/// Timer compact (plan.html: 172 x 36, ring 18, Doto 18).
pub const TIMER_COMPACT_W: f32 = 172.0;
pub const TIMER_COMPACT_H: f32 = 36.0;
pub const TIMER_COMPACT_R: f32 = 18.0;
pub const TC_RING: Rect = Rect::new(9.0, 9.0, 18.0, 18.0);
pub const TC_LABEL_X: f32 = 36.0;
/// Right edge of the digits.
pub const TC_DIGITS_RIGHT: f32 = 158.0;

/// Timer alert (the notification size in plan.html: 360 x 72, radius 28).
pub const ALERT_W: f32 = 360.0;
pub const ALERT_H: f32 = 72.0;
pub const ALERT_R: f32 = 28.0;
/// The `focus done · break 5:00` line.
pub const AL_TEXT: Rect = Rect::new(24.0, 20.0, 180.0, 32.0);
pub const AL_CONTINUE: Rect = Rect::new(212.0, 20.0, 88.0, 32.0);
pub const AL_STOP: Rect = Rect::new(308.0, 20.0, 32.0, 32.0);

/// Snap thumb (plan.html: 330 x 72, radius 26): the shot, its size, the copy status and
/// the copy / save buttons.
pub const SNAP_W: f32 = 330.0;
pub const SNAP_H: f32 = 72.0;
pub const SNAP_R: f32 = 26.0;
/// Thumb elements, relative to the pill's top-left. The shot is aspect-fit in `SN_SHOT`.
pub const SN_SHOT: Rect = Rect::new(10.0, 12.0, 76.0, 48.0);
/// `snap · 612×388` on the left, `✓ copied` right-aligned.
pub const SN_CAPTION: Rect = Rect::new(98.0, 14.0, 218.0, 16.0);
pub const SN_COPY: Rect = Rect::new(98.0, 38.0, 56.0, 22.0);
pub const SN_SAVE: Rect = Rect::new(160.0, 38.0, 56.0, 22.0);

/// The small timer on Music compact while a timer runs: mono mm:ss before the VU. The
/// title gives up this much room.
pub const MC_TIMER: Rect = Rect::new(118.0, 0.0, 42.0, 36.0);

/// Category row `i` of the Sweep checklist.
pub fn sw_row(i: usize) -> Rect {
    Rect::new(12.0, SW_TOP + i as f32 * SW_ROW_H, 376.0, SW_ROW_H)
}

/// Line `i` of a Sweep preview page.
pub fn sw_line(i: usize) -> Rect {
    Rect::new(12.0, SW_TOP + i as f32 * SW_LINE_H, 376.0, SW_LINE_H)
}

/// "Reveal" chip on big-file line `i`.
pub fn sw_reveal(i: usize) -> Rect {
    let l = sw_line(i);
    Rect::new(264.0, l.y + 2.0, 56.0, l.h - 4.0)
}

/// "Recycle" chip on big-file line `i`.
pub fn sw_recycle(i: usize) -> Rect {
    let l = sw_line(i);
    Rect::new(324.0, l.y + 2.0, 60.0, l.h - 4.0)
}

/// Gap between the top of the screen and the pill. In notch style the shape fills this
/// gap (it reaches y = 0), but the pill rect, its content and its hit testing stay put.
pub const TOP_MARGIN: f32 = 6.0;
/// Notch style: radius of the concave shoulders where the notch meets the top edge.
/// They stick out this far past each side of the pill.
pub const NOTCH_SHOULDER: f32 = 8.0;

/// Notch style with `[island] reserve_top`: the height of the strip reserved along the
/// top of the screen (logical px). It holds the notch in every rest state (idle, Music
/// and Timer compact), so only the expanded panels hang over other windows.
pub const RESERVE_H: f32 = TOP_MARGIN + 36.0;

/// Notch style: the box the notch shape covers for a pill rect (window-logical px):
/// from the top edge (y = 0) down to the pill's bottom, widened by a shoulder on each
/// side. The pill rect itself, and so the content and hit testing, does not move.
pub fn notch_bounds(pill: Rect) -> Rect {
    Rect::new(
        pill.x - NOTCH_SHOULDER,
        0.0,
        pill.w + 2.0 * NOTCH_SHOULDER,
        pill.y + pill.h,
    )
}

/// Extra window rows below the largest pill so the AA edge is never clipped.
const BOTTOM_SLACK: f32 = 2.0;

pub const COLS: usize = 5;
pub const TILE_W: f32 = 88.0;
pub const TILE_H: f32 = 76.0;
pub const TILE_GAP: f32 = 8.0;
const PAD_X: f32 = 24.0;
const GRID_Y: f32 = 52.0;

pub const HEADER: Rect = Rect::new(PAD_X, 16.0, HOME_W - 2.0 * PAD_X, 28.0);
pub const FOOTER: Rect = Rect::new(PAD_X, 304.0, HOME_W - 2.0 * PAD_X, 20.0);

/// Window size (logical) that holds the largest state; the swap chain is this size.
/// It is a notch shoulder wider than Home on each side, so Home's notch is not clipped.
/// The pill is centred in it, so the extra width doesn't move anything on screen.
pub fn window_size() -> (f32, f32) {
    (
        HOME_W + 2.0 * NOTCH_SHOULDER,
        TOP_MARGIN + HOME_H + BOTTOM_SLACK,
    )
}

/// One rect per entry of `FEATURES`, in order. A short last row is centred.
pub fn home_tiles() -> Vec<Rect> {
    let n = FEATURES.len();
    let row_w = |count: usize| count as f32 * TILE_W + (count as f32 - 1.0) * TILE_GAP;
    (0..n)
        .map(|i| {
            let (row, col) = (i / COLS, i % COLS);
            let in_row = (n - row * COLS).min(COLS);
            let x0 = PAD_X + (row_w(COLS) - row_w(in_row)) / 2.0;
            Rect::new(
                x0 + col as f32 * (TILE_W + TILE_GAP),
                GRID_Y + row as f32 * (TILE_H + TILE_GAP),
                TILE_W,
                TILE_H,
            )
        })
        .collect()
}

pub fn hit_tile(tiles: &[Rect], x: f32, y: f32) -> Option<usize> {
    tiles.iter().position(|t| t.contains(x, y))
}

/// On-screen volume pill: 220 x 40, radius 20.
pub const OSD_W: f32 = 220.0;
pub const OSD_H: f32 = 40.0;
pub const OSD_R: f32 = 20.0;
/// Speaker icon centre x (y is OSD_H / 2) and its full width.
pub const OSD_ICON_CX: f32 = 22.0;
pub const OSD_ICON_SIZE: f32 = 16.0;
/// Level track: 4 px high, drawn at y = 18.
pub const OSD_BAR: Rect = Rect::new(44.0, 18.0, 124.0, 4.0);
/// Right edge of the mono value text.
pub const OSD_VALUE_RIGHT: f32 = 198.0;
/// Click and drag area on the bar (pill-local, generous vertically).
pub const OSD_HIT: Rect = Rect::new(36.0, 6.0, 140.0, 28.0);

/// The OSD pill's rect in window-logical px: centred horizontally, at TOP_MARGIN.
pub fn osd_rect() -> Rect {
    let (ww, _) = window_size();
    Rect::new((ww - OSD_W) / 2.0, TOP_MARGIN, OSD_W, OSD_H)
}

/// Pill-local (x, y): Some(level in 0..=1) when inside OSD_HIT, mapped linearly onto
/// OSD_BAR and clamped; None outside.
pub fn osd_level_at(x: f32, y: f32) -> Option<f32> {
    if !OSD_HIT.contains(x, y) {
        return None;
    }
    let t = (x - OSD_BAR.x) / OSD_BAR.w;
    Some(t.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twelve_tiles_inside_home_and_disjoint() {
        let tiles = home_tiles();
        assert_eq!(tiles.len(), 12);
        let bounds = Rect::new(0.0, 0.0, HOME_W, HOME_H);
        for (i, a) in tiles.iter().enumerate() {
            assert!(
                a.x >= PAD_X - 0.01 && a.x + a.w <= HOME_W - PAD_X + 0.01,
                "tile {i} x"
            );
            assert!(a.y >= GRID_Y && a.y + a.h <= FOOTER.y, "tile {i} y");
            assert!(bounds.contains(a.x, a.y));
            for b in &tiles[i + 1..] {
                let apart =
                    a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;
                assert!(apart);
            }
        }
    }

    #[test]
    fn last_row_is_centred() {
        let tiles = home_tiles();
        let left = tiles[10].x;
        let right = tiles[11].x + TILE_W;
        assert!((left + right - HOME_W).abs() < 0.01);
    }

    #[test]
    fn hit_testing() {
        let tiles = home_tiles();
        let t = tiles[7];
        assert_eq!(hit_tile(&tiles, t.x + 1.0, t.y + 1.0), Some(7));
        assert_eq!(
            hit_tile(&tiles, t.x + t.w + TILE_GAP / 2.0, t.y + 1.0),
            None
        );
        assert_eq!(hit_tile(&tiles, 1.0, 1.0), None);
    }

    #[test]
    fn window_holds_home() {
        let (w, h) = window_size();
        assert!(w >= HOME_W && h >= TOP_MARGIN + HOME_H);
    }

    #[test]
    fn window_holds_vitals_and_sweep() {
        let (w, h) = window_size();
        for (pw, ph) in [(VITALS_W, VITALS_H), (SWEEP_W, SWEEP_H)] {
            assert!(pw <= w && TOP_MARGIN + ph <= h);
        }
    }

    #[test]
    fn sweep_controls_fit_and_do_not_overlap() {
        let mut rects = vec![SW_BACK, SW_DNS, SW_BIN, SW_CLEAN];
        rects.extend((0..5).map(sw_row));
        let bounds =
            |r: &Rect| r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= SWEEP_W && r.y + r.h <= SWEEP_H;
        let apart = |a: &Rect, b: &Rect| {
            a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y
        };
        for (i, a) in rects.iter().enumerate() {
            assert!(bounds(a), "{a:?}");
            for b in &rects[i + 1..] {
                assert!(apart(a, b), "{a:?} overlaps {b:?}");
            }
        }
        let mut list = vec![SW_BACK, SW_PREV, SW_NEXT, SW_CLEAN];
        for i in 0..crate::sweep::PAGE {
            assert!(bounds(&sw_line(i)));
            list.push(sw_reveal(i));
            list.push(sw_recycle(i));
        }
        for (i, a) in list.iter().enumerate() {
            assert!(bounds(a), "{a:?}");
            for b in &list[i + 1..] {
                assert!(apart(a, b), "{a:?} overlaps {b:?}");
            }
        }
        const {
            assert!(VITALS_SWEEP.x + VITALS_SWEEP.w <= VITALS_W);
            assert!(VITALS_SWEEP.y + VITALS_SWEEP.h <= VITALS_H);
        }
    }

    fn apart(a: &Rect, b: &Rect) -> bool {
        a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y
    }

    fn inside(r: &Rect, w: f32, h: f32) -> bool {
        r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w && r.y + r.h <= h
    }

    #[test]
    fn window_holds_the_timer_views_and_home_is_still_the_largest() {
        let (w, h) = window_size();
        for (pw, ph) in [
            (TIMER_W, TIMER_H),
            (TIMER_COMPACT_W, TIMER_COMPACT_H),
            (ALERT_W, ALERT_H),
        ] {
            assert!(pw <= HOME_W && ph <= HOME_H);
            assert!(pw <= w && TOP_MARGIN + ph <= h);
        }
        assert_eq!(
            window_size(),
            (
                HOME_W + 2.0 * NOTCH_SHOULDER,
                TOP_MARGIN + HOME_H + BOTTOM_SLACK
            )
        );
    }

    #[test]
    fn timer_panel_controls_fit_and_do_not_overlap() {
        let mut rects = vec![TP_HEADER, TP_RING, TP_DIGITS, TP_TOGGLE, TP_SKIP, TP_RESET];
        rects.extend((0..crate::timer::PRESETS.len()).map(tp_chip));
        for (i, a) in rects.iter().enumerate() {
            assert!(inside(a, TIMER_W, TIMER_H), "{a:?}");
            for b in &rects[i + 1..] {
                assert!(apart(a, b), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn settings_panel_controls_fit_and_do_not_overlap() {
        let (w, h) = window_size();
        assert!(SETTINGS_W <= w && TOP_MARGIN + SETTINGS_H <= h);
        let rects = [
            ST_HEADER,
            ST_BACK,
            ST_ROW_AUTOSTART,
            ST_ROW_HOTKEY,
            ST_ROW_ALT,
            ST_ROW_STYLE,
            ST_ROW_RESERVE,
            ST_TOGGLE,
            ST_HOTKEY,
            ST_ALT,
            ST_STYLE,
            ST_RESERVE,
            ST_STATUS,
        ];
        for (i, a) in rects.iter().enumerate() {
            assert!(inside(a, SETTINGS_W, SETTINGS_H), "{a:?}");
            for b in &rects[i + 1..] {
                assert!(apart(a, b), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn the_style_row_lines_up_with_the_rows_above() {
        const PITCH: f32 = 36.0;
        let rows = [
            ST_ROW_AUTOSTART,
            ST_ROW_HOTKEY,
            ST_ROW_ALT,
            ST_ROW_STYLE,
            ST_ROW_RESERVE,
        ];
        for pair in rows.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            assert_eq!((b.x, b.w, b.h), (a.x, a.w, a.h), "{b:?}");
            assert_eq!(b.y - a.y, PITCH, "{b:?}");
        }
        let chips = [ST_HOTKEY, ST_ALT, ST_STYLE];
        for pair in chips.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            assert_eq!((b.x, b.w, b.h), (a.x, a.w, a.h), "{b:?}");
            assert_eq!(b.y - a.y, PITCH, "{b:?}");
        }
        // Each control is centred on its row and right-aligned at x = 380.
        for (row, ctl) in [
            (ST_ROW_AUTOSTART, ST_TOGGLE),
            (ST_ROW_HOTKEY, ST_HOTKEY),
            (ST_ROW_ALT, ST_ALT),
            (ST_ROW_STYLE, ST_STYLE),
            (ST_ROW_RESERVE, ST_RESERVE),
        ] {
            assert_eq!(ctl.y + ctl.h / 2.0, row.y + row.h / 2.0, "{ctl:?}");
            assert_eq!(ctl.x + ctl.w, 380.0, "{ctl:?}");
        }
        // The reserve switch is the autostart switch's size.
        assert_eq!((ST_RESERVE.w, ST_RESERVE.h), (ST_TOGGLE.w, ST_TOGGLE.h));
        // The status line sits below the last row, and the panel is two rows taller
        // than the Timer panel it used to match.
        const {
            assert!(ST_STATUS.y >= ST_ROW_RESERVE.y + ST_ROW_RESERVE.h);
            assert!(SETTINGS_H == TIMER_H + 2.0 * PITCH);
            assert!(SETTINGS_W == TIMER_W);
        }
    }

    #[test]
    fn the_tile_panel_is_400_by_168_and_fits_the_window() {
        assert_eq!(
            (TILE_PANEL_W, TILE_PANEL_H, TILE_PANEL_R),
            (400.0, 168.0, 32.0)
        );
        let (w, h) = window_size();
        assert!(TILE_PANEL_W <= w && TOP_MARGIN + TILE_PANEL_H <= h);
        // Home stays the largest view, and the Home grid's own tile size is untouched.
        const { assert!(TILE_PANEL_W <= HOME_W && TILE_PANEL_H <= HOME_H) };
        assert_eq!((TILE_W, TILE_H), (88.0, 76.0));
    }

    #[test]
    fn the_reserved_strip_holds_every_rest_state() {
        for h in [IDLE_H, MUSIC_COMPACT_H, TIMER_COMPACT_H] {
            assert!(TOP_MARGIN + h <= RESERVE_H, "{h}");
        }
        // But not more than that: the expanded views hang over other windows.
        assert_eq!(RESERVE_H, TOP_MARGIN + MUSIC_COMPACT_H.max(TIMER_COMPACT_H));
    }

    #[test]
    fn window_holds_the_taller_settings_panel_and_home_is_still_the_largest() {
        let (w, h) = window_size();
        assert!(SETTINGS_W <= w && TOP_MARGIN + SETTINGS_H <= h);
        const { assert!(SETTINGS_W <= HOME_W && SETTINGS_H <= HOME_H) };
    }

    /// Every view size the island morphs between, as `(w, h, corner radius)`.
    const VIEWS: [(f32, f32, f32); 13] = [
        (IDLE_W, IDLE_H, IDLE_R),
        (HOME_W, HOME_H, HOME_R),
        (VITALS_W, VITALS_H, VITALS_R),
        (SWEEP_W, SWEEP_H, SWEEP_R),
        (MUSIC_COMPACT_W, MUSIC_COMPACT_H, MUSIC_COMPACT_R),
        (MUSIC_EXP_W, MUSIC_EXP_H, MUSIC_EXP_R),
        (TIMER_W, TIMER_H, TIMER_R),
        (TIMER_COMPACT_W, TIMER_COMPACT_H, TIMER_COMPACT_R),
        (ALERT_W, ALERT_H, ALERT_R),
        (SNAP_W, SNAP_H, SNAP_R),
        (SETTINGS_W, SETTINGS_H, SETTINGS_R),
        (TILE_PANEL_W, TILE_PANEL_H, TILE_PANEL_R),
        (OSD_W, OSD_H, OSD_R),
    ];

    /// The pill rect the island uses for a view: centred, `TOP_MARGIN` down.
    fn centred_pill(w: f32, h: f32) -> Rect {
        let (ww, _) = window_size();
        Rect::new((ww - w) / 2.0, TOP_MARGIN, w, h)
    }

    #[test]
    fn notch_bounds_reach_the_top_edge_and_add_a_shoulder_each_side() {
        let pill = centred_pill(IDLE_W, IDLE_H);
        let n = notch_bounds(pill);
        assert_eq!(n.y, 0.0);
        assert_eq!(n.x, pill.x - NOTCH_SHOULDER);
        assert_eq!(n.x + n.w, pill.x + pill.w + NOTCH_SHOULDER);
        assert_eq!(n.w, IDLE_W + 2.0 * NOTCH_SHOULDER);
        // Same bottom as the pill: the notch only grows up and sideways.
        assert_eq!(n.y + n.h, pill.y + pill.h);
        assert_eq!(n.h, TOP_MARGIN + IDLE_H);
        // The biggest view (Home) the same way.
        let pill = centred_pill(HOME_W, HOME_H);
        assert_eq!(
            notch_bounds(pill),
            Rect::new(
                pill.x - NOTCH_SHOULDER,
                0.0,
                HOME_W + 2.0 * NOTCH_SHOULDER,
                TOP_MARGIN + HOME_H
            )
        );
        // A pill that is not at TOP_MARGIN (mid-morph or a test rect) still reaches 0.
        let n = notch_bounds(Rect::new(40.0, 20.0, 100.0, 30.0));
        assert_eq!(n, Rect::new(32.0, 0.0, 116.0, 50.0));
    }

    #[test]
    fn the_notch_of_every_view_fits_the_window() {
        let (ww, wh) = window_size();
        for (w, h, r) in VIEWS {
            let pill = centred_pill(w, h);
            let n = notch_bounds(pill);
            assert!(n.x >= 0.0 && n.x + n.w <= ww, "{w}x{h}: {n:?}");
            assert!(n.y >= 0.0 && n.y + n.h <= wh, "{w}x{h}: {n:?}");
            // The pill is inside its notch.
            assert!(n.contains(pill.x, pill.y), "{w}x{h}");
            assert!(pill.x + pill.w <= n.x + n.w && pill.y + pill.h <= n.y + n.h);
            // The shoulder curve ends above where the bottom corners start.
            assert!(NOTCH_SHOULDER <= pill.y + pill.h - r, "{w}x{h}");
        }
        // Home touches both sides: nothing wider would fit.
        let home = notch_bounds(centred_pill(HOME_W, HOME_H));
        assert_eq!((home.x, home.x + home.w), (0.0, ww));
        // The OSD's own rect is a centred pill too.
        assert_eq!(osd_rect(), centred_pill(OSD_W, OSD_H));
    }

    #[test]
    fn the_gear_sits_in_the_home_header_clear_of_the_label_and_the_clock() {
        assert!(inside(&HOME_GEAR, HOME_W, HOME_H));
        // The clock is about 70 px wide, flush with the header's right edge.
        const {
            assert!(HOME_GEAR.x + HOME_GEAR.w <= HEADER.x + HEADER.w - 70.0);
            assert!(HOME_GEAR.y >= HEADER.y);
            assert!(HOME_GEAR.y + HOME_GEAR.h <= HEADER.y + HEADER.h);
            assert!(HOME_GEAR.x > HEADER.x + 160.0);
        }
    }

    #[test]
    fn timer_compact_and_alert_parts_fit() {
        assert!(inside(&TC_RING, TIMER_COMPACT_W, TIMER_COMPACT_H));
        const { assert!(TC_LABEL_X > TC_RING.x + TC_RING.w && TC_DIGITS_RIGHT < TIMER_COMPACT_W) };
        let parts = [AL_TEXT, AL_CONTINUE, AL_STOP];
        for (i, a) in parts.iter().enumerate() {
            assert!(inside(a, ALERT_W, ALERT_H), "{a:?}");
            for b in &parts[i + 1..] {
                assert!(apart(a, b), "{a:?} overlaps {b:?}");
            }
        }
        // The mini timer on Music compact sits between the art and the VU.
        assert!(inside(&MC_TIMER, MUSIC_COMPACT_W, MUSIC_COMPACT_H));
        assert!(apart(&MC_TIMER, &MC_ART) && apart(&MC_TIMER, &MC_VU));
    }

    #[test]
    fn snap_thumb_fits_the_window_and_its_parts_do_not_overlap() {
        let (w, h) = window_size();
        assert!(SNAP_W <= w && TOP_MARGIN + SNAP_H <= h);
        let parts = [SN_SHOT, SN_CAPTION, SN_COPY, SN_SAVE];
        for (i, a) in parts.iter().enumerate() {
            assert!(inside(a, SNAP_W, SNAP_H), "{a:?}");
            for b in &parts[i + 1..] {
                assert!(apart(a, b), "{a:?} overlaps {b:?}");
            }
        }
        // The shot is the plan's 76 x 48, and the window still fits Home (and its notch).
        assert_eq!((SN_SHOT.w, SN_SHOT.h), (76.0, 48.0));
        assert_eq!(
            window_size(),
            (
                HOME_W + 2.0 * NOTCH_SHOULDER,
                TOP_MARGIN + HOME_H + BOTTOM_SLACK
            )
        );
    }

    #[test]
    fn window_holds_osd() {
        let (w, h) = window_size();
        let r = osd_rect();
        assert!(r.x >= 0.0 && r.x + r.w <= w);
        assert!(r.y + r.h <= h);
    }

    #[test]
    fn osd_level_mapping() {
        let y = OSD_BAR.y + 1.0;
        assert_eq!(osd_level_at(OSD_BAR.x, y), Some(0.0));
        assert_eq!(osd_level_at(OSD_BAR.x + OSD_BAR.w, y), Some(1.0));
        assert_eq!(osd_level_at(OSD_HIT.x, y), Some(0.0));
        assert_eq!(osd_level_at(OSD_HIT.x - 0.1, y), None);
        assert_eq!(osd_level_at(OSD_BAR.x, OSD_HIT.y + OSD_HIT.h + 1.0), None);
    }
}
