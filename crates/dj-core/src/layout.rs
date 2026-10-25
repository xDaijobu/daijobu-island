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

/// Gap between the top of the screen and the pill.
pub const TOP_MARGIN: f32 = 6.0;
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
pub fn window_size() -> (f32, f32) {
    (HOME_W, TOP_MARGIN + HOME_H + BOTTOM_SLACK)
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
    fn thirteen_tiles_inside_home_and_disjoint() {
        let tiles = home_tiles();
        assert_eq!(tiles.len(), 13);
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
        let right = tiles[12].x + TILE_W;
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
