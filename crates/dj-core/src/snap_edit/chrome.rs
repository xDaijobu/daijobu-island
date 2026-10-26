//! The editor's chrome, as pure geometry: which monitor a selection belongs to, the
//! selection's 8 handles and their resize maths, the toolbar's layout and placement,
//! the HSV picker's layout, and the cursor each spot asks for.
//!
//! Everything is in physical desktop px. Logical sizes are multiplied by the DPI scale
//! of the monitor that holds most of the selection.

use super::model::{FRect, Pt, Size, Tool, SWATCH_COUNT};
use crate::snap::{IRect, MIN_SIZE};

/// One monitor: its desktop rect and DPI scale (device px per logical px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    pub rect: IRect,
    pub scale: f32,
}

/// The monitor that holds most of `sel` (the first on a tie). With no overlap at all,
/// the first monitor, or the desktop at scale 1 if there are none.
pub fn main_display(sel: IRect, displays: &[Display], desktop: IRect) -> Display {
    let area = |d: &Display| {
        d.rect
            .intersect(&sel)
            .map_or(0_i64, |r| r.w as i64 * r.h as i64)
    };
    let mut best: Option<(&Display, i64)> = None;
    for d in displays {
        let a = area(d);
        if best.is_none_or(|(_, b)| a > b) {
            best = Some((d, a));
        }
    }
    best.map_or(
        Display {
            rect: desktop,
            scale: 1.0,
        },
        |(d, _)| *d,
    )
}

/// The cursor the overlay should show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorKind {
    Cross,
    IBeam,
    SizeNwse,
    SizeNesw,
    SizeWe,
    SizeNs,
    Move,
    Arrow,
}

/// Handles are drawn this big (logical px) and hit within `HANDLE_HIT` of their centre.
pub const HANDLE_SIZE: f32 = 7.0;
pub const HANDLE_HIT: f32 = 7.0;

/// The handle centres: 0 top-left, 1 top, 2 top-right, 3 right, 4 bottom-right,
/// 5 bottom, 6 bottom-left, 7 left.
pub fn handle_points(r: IRect) -> [Pt; 8] {
    let (x0, y0) = (r.x as f32, r.y as f32);
    let (x1, y1) = (r.right() as f32, r.bottom() as f32);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    [
        (x0, y0),
        (cx, y0),
        (x1, y0),
        (x1, cy),
        (x1, y1),
        (cx, y1),
        (x0, y1),
        (x0, cy),
    ]
}

/// The handle under desktop `(x, y)`. Corners win over edges when they overlap.
pub fn handle_at(r: IRect, x: f32, y: f32, scale: f32) -> Option<usize> {
    let pts = handle_points(r);
    let reach = HANDLE_HIT * scale;
    [0, 2, 4, 6, 1, 3, 5, 7]
        .into_iter()
        .find(|&i| (pts[i].0 - x).abs() <= reach && (pts[i].1 - y).abs() <= reach)
}

pub fn handle_cursor(h: usize) -> CursorKind {
    match h {
        0 | 4 => CursorKind::SizeNwse,
        2 | 6 => CursorKind::SizeNesw,
        1 | 5 => CursorKind::SizeNs,
        _ => CursorKind::SizeWe,
    }
}

/// Keeps `[lo, hi)` inside `[min, max)` and at least `MIN_SIZE` long.
fn fit_span(lo: i32, hi: i32, min: i32, max: i32) -> (i32, i32) {
    let lo = lo.clamp(min, max);
    let hi = hi.clamp(min, max);
    if hi - lo >= MIN_SIZE {
        (lo, hi)
    } else if lo + MIN_SIZE <= max {
        (lo, lo + MIN_SIZE)
    } else {
        ((max - MIN_SIZE).max(min), max)
    }
}

/// The selection after dragging handle `h` by `(dx, dy)` from `orig`. Dragging past the
/// opposite edge flips it; the result stays on the desktop and is at least 2 x 2.
pub fn resize(orig: IRect, h: usize, dx: i32, dy: i32, desktop: IRect) -> IRect {
    let (mut l, mut t, mut r, mut b) = (orig.x, orig.y, orig.right(), orig.bottom());
    if matches!(h, 0 | 6 | 7) {
        l += dx;
    }
    if matches!(h, 2..=4) {
        r += dx;
    }
    if matches!(h, 0..=2) {
        t += dy;
    }
    if matches!(h, 4..=6) {
        b += dy;
    }
    let (l, r) = fit_span(l.min(r), l.max(r), desktop.x, desktop.right());
    let (t, b) = fit_span(t.min(b), t.max(b), desktop.y, desktop.bottom());
    IRect::new(l, t, r - l, b - t)
}

/// The selection moved by `(dx, dy)`, kept whole on the desktop.
pub fn move_rect(orig: IRect, dx: i32, dy: i32, desktop: IRect) -> IRect {
    let x = (orig.x + dx).clamp(desktop.x, (desktop.right() - orig.w).max(desktop.x));
    let y = (orig.y + dy).clamp(desktop.y, (desktop.bottom() - orig.h).max(desktop.y));
    IRect::new(x, y, orig.w, orig.h)
}

/// A toolbar button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    Tool(Tool),
    /// 0..8; 7 is the accent.
    Swatch(usize),
    Custom,
    Size(Size),
    Undo,
    Redo,
    Copy,
    Save,
    Close,
}

/// Logical px (plan.html `.snapov .tools`): 26 px buttons 4 apart, 5 px padding, and
/// a 1 px rule between groups.
pub const BUTTON: f32 = 26.0;
pub const GAP: f32 = 4.0;
pub const PAD: f32 = 5.0;
pub const GROUP_GAP: f32 = 9.0;
/// Between the selection and the toolbar, and between the toolbar and the picker.
pub const MARGIN: f32 = 8.0;
pub const RADIUS: f32 = 12.0;

/// The buttons in order, by group.
pub fn groups() -> Vec<Vec<Item>> {
    vec![
        Tool::ALL.into_iter().map(Item::Tool).collect(),
        (0..SWATCH_COUNT)
            .map(Item::Swatch)
            .chain([Item::Custom])
            .collect(),
        Size::ALL.into_iter().map(Item::Size).collect(),
        vec![Item::Undo, Item::Redo],
        vec![Item::Copy, Item::Save, Item::Close],
    ]
}

/// Where the toolbar went relative to the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Below,
    Above,
    Inside,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toolbar {
    pub rect: FRect,
    pub buttons: Vec<(Item, FRect)>,
    /// The 1 px rules between groups.
    pub rules: Vec<FRect>,
    pub place: Place,
    pub rows: usize,
    pub scale: f32,
}

fn group_w(n: usize) -> f32 {
    n as f32 * BUTTON + (n.saturating_sub(1)) as f32 * GAP
}

fn row_w(groups: &[Vec<Item>]) -> f32 {
    let inner: f32 = groups.iter().map(|g| group_w(g.len())).sum();
    inner + groups.len().saturating_sub(1) as f32 * GROUP_GAP
}

/// Lays the toolbar out for selection `sel` on monitor `mon` (the one holding most of
/// the selection) at `scale`. It goes under the selection, left-aligned with it; above
/// if there's no room; else inside the selection's bottom edge. It is clamped to the
/// monitor and wraps to two rows (split between groups) if one row is too wide.
pub fn toolbar(sel: IRect, mon: IRect, scale: f32) -> Toolbar {
    let s = scale;
    let gs = groups();
    let one = row_w(&gs) + 2.0 * PAD;
    let rows_of: Vec<&[Vec<Item>]> = if one * s <= mon.w as f32 {
        vec![&gs[..]]
    } else {
        // The split between groups that gives the narrowest toolbar.
        let k = (1..gs.len())
            .min_by(|&a, &b| {
                let wa = row_w(&gs[..a]).max(row_w(&gs[a..]));
                let wb = row_w(&gs[..b]).max(row_w(&gs[b..]));
                wa.total_cmp(&wb)
            })
            .unwrap_or(1);
        vec![&gs[..k], &gs[k..]]
    };
    let w = (rows_of.iter().map(|r| row_w(r)).fold(0.0_f32, f32::max) + 2.0 * PAD) * s;
    let n = rows_of.len() as f32;
    let h = (2.0 * PAD + n * BUTTON + (n - 1.0) * GAP) * s;
    let (w, h) = (w.round(), h.round());

    let m = MARGIN * s;
    let (mon_l, mon_t) = (mon.x as f32, mon.y as f32);
    let (mon_r, mon_b) = (mon.right() as f32, mon.bottom() as f32);
    let below = sel.bottom() as f32 + m;
    let above = sel.y as f32 - m - h;
    let (y, place) = if below + h <= mon_b {
        (below, Place::Below)
    } else if above >= mon_t {
        (above, Place::Above)
    } else {
        let inside = sel.bottom() as f32 - m - h;
        (inside.min(mon_b - h).max(mon_t), Place::Inside)
    };
    let x = (sel.x as f32).min(mon_r - w).max(mon_l);
    let (x, y) = (x.round(), y.round());

    let mut buttons = Vec::new();
    let mut rules = Vec::new();
    for (ri, row) in rows_of.iter().enumerate() {
        let by = y + (PAD + ri as f32 * (BUTTON + GAP)) * s;
        let mut bx = PAD;
        for (gi, g) in row.iter().enumerate() {
            if gi > 0 {
                let rule_x = bx - GROUP_GAP / 2.0 - 0.5;
                rules.push(FRect::new(
                    (x + rule_x * s).round(),
                    (by + 4.0 * s).round(),
                    s.max(1.0).round(),
                    ((BUTTON - 8.0) * s).round(),
                ));
            }
            for (ii, item) in g.iter().enumerate() {
                if ii > 0 {
                    bx += GAP;
                }
                buttons.push((
                    *item,
                    FRect::new(
                        (x + bx * s).round(),
                        by.round(),
                        (BUTTON * s).round(),
                        (BUTTON * s).round(),
                    ),
                ));
                bx += BUTTON;
            }
            bx += GROUP_GAP;
        }
    }
    Toolbar {
        rect: FRect::new(x, y, w, h),
        buttons,
        rules,
        place,
        rows: rows_of.len(),
        scale: s,
    }
}

impl Toolbar {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.rect.contains(x, y)
    }

    /// The button under desktop `(x, y)` (None on the padding and gaps).
    pub fn item_at(&self, x: f32, y: f32) -> Option<Item> {
        self.buttons
            .iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(i, _)| *i)
    }

    pub fn button(&self, item: Item) -> Option<FRect> {
        self.buttons
            .iter()
            .find(|(i, _)| *i == item)
            .map(|(_, r)| *r)
    }
}

/// The HSV picker's parts, logical px: a 128 px saturation/value square, a 16 px hue
/// strip right of it, and under them a preview chip and the hex text.
pub const PICK_PAD: f32 = 10.0;
pub const PICK_SV: f32 = 128.0;
pub const PICK_HUE_W: f32 = 16.0;
pub const PICK_ROW: f32 = 22.0;
pub const PICK_GAP: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Picker {
    pub rect: FRect,
    pub sv: FRect,
    pub hue: FRect,
    pub preview: FRect,
    pub hex: FRect,
}

/// The draggable parts of the picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerPart {
    Sv,
    Hue,
}

/// The picker next to the toolbar: under it (left-aligned with the custom swatch) if it
/// fits on the monitor, else above it, clamped to the monitor.
pub fn picker(bar: &Toolbar, mon: IRect) -> Picker {
    let s = bar.scale;
    let w = ((2.0 * PICK_PAD + PICK_SV + PICK_GAP + PICK_HUE_W) * s).round();
    let h = ((2.0 * PICK_PAD + PICK_SV + PICK_GAP + PICK_ROW) * s).round();
    let anchor = bar.button(Item::Custom).unwrap_or(bar.rect);
    let (mon_l, mon_t) = (mon.x as f32, mon.y as f32);
    let (mon_r, mon_b) = (mon.right() as f32, mon.bottom() as f32);
    let below = bar.rect.bottom() + MARGIN * s;
    let above = bar.rect.y - MARGIN * s - h;
    let y = if below + h <= mon_b {
        below
    } else if above >= mon_t {
        above
    } else {
        below.min(mon_b - h).max(mon_t)
    };
    let x = (anchor.x - PICK_PAD * s).min(mon_r - w).max(mon_l);
    let (x, y) = (x.round(), y.round());
    let at = |lx: f32, ly: f32, lw: f32, lh: f32| {
        FRect::new(
            (x + lx * s).round(),
            (y + ly * s).round(),
            (lw * s).round(),
            (lh * s).round(),
        )
    };
    let row_y = PICK_PAD + PICK_SV + PICK_GAP;
    Picker {
        rect: FRect::new(x, y, w, h),
        sv: at(PICK_PAD, PICK_PAD, PICK_SV, PICK_SV),
        hue: at(PICK_PAD + PICK_SV + PICK_GAP, PICK_PAD, PICK_HUE_W, PICK_SV),
        preview: at(PICK_PAD, row_y, PICK_ROW * 1.6, PICK_ROW),
        hex: at(
            PICK_PAD + PICK_ROW * 1.6 + PICK_GAP,
            row_y,
            PICK_SV + PICK_HUE_W - PICK_ROW * 1.6,
            PICK_ROW,
        ),
    }
}

impl Picker {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.rect.contains(x, y)
    }

    pub fn part_at(&self, x: f32, y: f32) -> Option<PickerPart> {
        if self.sv.contains(x, y) {
            Some(PickerPart::Sv)
        } else if self.hue.contains(x, y) {
            Some(PickerPart::Hue)
        } else {
            None
        }
    }

    /// Saturation (left to right) and value (top to bottom, 1 to 0) at a point, clamped
    /// to the square, so a drag may leave it.
    pub fn sv_at(&self, x: f32, y: f32) -> (f32, f32) {
        let r = self.sv;
        let s = ((x - r.x) / (r.w - 1.0).max(1.0)).clamp(0.0, 1.0);
        let v = 1.0 - ((y - r.y) / (r.h - 1.0).max(1.0)).clamp(0.0, 1.0);
        (s, v)
    }

    /// Hue at a height: 0 at the top to 360 at the bottom, clamped.
    pub fn hue_at(&self, y: f32) -> f32 {
        let r = self.hue;
        ((y - r.y) / (r.h - 1.0).max(1.0)).clamp(0.0, 1.0) * 360.0
    }

    /// Where a (saturation, value) sits in the square (the drawn marker).
    pub fn sv_point(&self, s: f32, v: f32) -> Pt {
        let r = self.sv;
        (
            r.x + s.clamp(0.0, 1.0) * (r.w - 1.0),
            r.y + (1.0 - v.clamp(0.0, 1.0)) * (r.h - 1.0),
        )
    }

    pub fn hue_y(&self, h: f32) -> f32 {
        let r = self.hue;
        r.y + (h / 360.0).clamp(0.0, 1.0) * (r.h - 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESK: IRect = IRect::new(-1920, 0, 3840, 1080);
    const LEFT: IRect = IRect::new(-1920, 0, 1920, 1080);
    const MAIN: IRect = IRect::new(0, 0, 1920, 1080);

    fn displays() -> Vec<Display> {
        vec![
            Display {
                rect: MAIN,
                scale: 1.0,
            },
            Display {
                rect: LEFT,
                scale: 1.5,
            },
        ]
    }

    #[test]
    fn the_main_display_holds_most_of_the_selection() {
        let d = displays();
        assert_eq!(
            main_display(IRect::new(-100, 0, 300, 50), &d, DESK).rect,
            MAIN
        );
        assert_eq!(
            main_display(IRect::new(-300, 0, 299, 50), &d, DESK).scale,
            1.5
        );
        assert_eq!(
            main_display(IRect::new(9000, 0, 10, 10), &d, DESK).rect,
            MAIN,
            "no overlap: the first"
        );
        assert_eq!(
            main_display(IRect::new(0, 0, 10, 10), &[], DESK),
            Display {
                rect: DESK,
                scale: 1.0
            }
        );
    }

    #[test]
    fn handles_sit_on_corners_and_edges_and_corners_win() {
        let r = IRect::new(100, 100, 200, 100);
        let p = handle_points(r);
        assert_eq!(p[0], (100.0, 100.0));
        assert_eq!(p[3], (300.0, 150.0));
        assert_eq!(p[5], (200.0, 200.0));
        assert_eq!(handle_at(r, 104.0, 95.0, 1.0), Some(0));
        assert_eq!(handle_at(r, 200.0, 206.0, 1.0), Some(5));
        assert_eq!(handle_at(r, 200.0, 150.0, 1.0), None);
        assert_eq!(handle_at(r, 290.0, 150.0, 1.0), None);
        assert_eq!(handle_at(r, 290.0, 150.0, 2.0), Some(3), "reach scales");
        // A tiny selection: every handle overlaps; the corner is taken.
        let tiny = IRect::new(0, 0, 4, 4);
        assert_eq!(handle_at(tiny, 2.0, 0.0, 1.0), Some(0));
        assert_eq!(handle_cursor(0), CursorKind::SizeNwse);
        assert_eq!(handle_cursor(6), CursorKind::SizeNesw);
        assert_eq!(handle_cursor(5), CursorKind::SizeNs);
        assert_eq!(handle_cursor(7), CursorKind::SizeWe);
    }

    #[test]
    fn resize_moves_the_dragged_edges_flips_clamps_and_keeps_the_minimum() {
        let d = IRect::new(0, 0, 1000, 800);
        let r = IRect::new(100, 100, 200, 100);
        // Bottom-right corner out.
        assert_eq!(resize(r, 4, 50, 20, d), IRect::new(100, 100, 250, 120));
        // Top edge: only y changes.
        assert_eq!(resize(r, 1, 77, -30, d), IRect::new(100, 70, 200, 130));
        // Left edge past the right edge: flips.
        assert_eq!(resize(r, 7, 300, 0, d), IRect::new(300, 100, 100, 100));
        // Top-left corner past both: flips both ways.
        assert_eq!(resize(r, 0, 250, 150, d), IRect::new(300, 200, 50, 50));
        // Clamped to the desktop.
        assert_eq!(resize(r, 4, 5000, 5000, d), IRect::new(100, 100, 900, 700));
        assert_eq!(resize(r, 0, -500, -500, d), IRect::new(0, 0, 300, 200));
        // Exactly onto the opposite edge: the 2 px minimum.
        assert_eq!(resize(r, 3, -200, 0, d), IRect::new(100, 100, 2, 100));
        // At the desktop's right edge the minimum grows leftwards.
        let edge = IRect::new(990, 0, 10, 10);
        assert_eq!(resize(edge, 7, 10, 0, d), IRect::new(998, 0, 2, 10));
        // Negative desktops.
        let neg = IRect::new(-500, -300, 1000, 600);
        let r = IRect::new(-100, -100, 50, 50);
        assert_eq!(
            resize(r, 0, -1000, -1000, neg),
            IRect::new(-500, -300, 450, 250)
        );
    }

    #[test]
    fn moving_the_selection_keeps_it_whole_on_the_desktop() {
        let d = IRect::new(-100, -50, 400, 300);
        let r = IRect::new(0, 0, 100, 80);
        assert_eq!(move_rect(r, 20, -10, d), IRect::new(20, -10, 100, 80));
        assert_eq!(move_rect(r, 9999, 9999, d), IRect::new(200, 170, 100, 80));
        assert_eq!(
            move_rect(r, -9999, -9999, d),
            IRect::new(-100, -50, 100, 80)
        );
    }

    #[test]
    fn the_toolbar_goes_below_above_or_inside_and_is_clamped() {
        let sel = IRect::new(200, 100, 300, 200);
        let bar = toolbar(sel, MAIN, 1.0);
        assert_eq!(bar.place, Place::Below);
        assert_eq!(bar.rows, 1);
        assert_eq!((bar.rect.x, bar.rect.y), (200.0, 308.0));
        // 26 buttons, 21 gaps of 4, 4 group gaps of 9, 5 px padding.
        assert_eq!(bar.rect.w, 26.0 * 26.0 + 21.0 * 4.0 + 4.0 * 9.0 + 10.0);
        assert_eq!(bar.rect.h, 36.0);
        assert_eq!(bar.buttons.len(), 26);
        assert_eq!(bar.rules.len(), 4);
        assert_eq!(
            bar.buttons[0],
            (
                Item::Tool(Tool::Select),
                FRect::new(205.0, 313.0, 26.0, 26.0)
            )
        );
        assert_eq!(bar.buttons[25].0, Item::Close);
        // No room below: above.
        let low = IRect::new(200, 800, 300, 260);
        let bar = toolbar(low, MAIN, 1.0);
        assert_eq!(bar.place, Place::Above);
        assert_eq!(bar.rect.y, 800.0 - 8.0 - 36.0);
        // A full-monitor selection: inside its bottom edge.
        let bar = toolbar(MAIN, MAIN, 1.0);
        assert_eq!(bar.place, Place::Inside);
        assert_eq!(bar.rect.y, 1080.0 - 8.0 - 36.0);
        assert_eq!(bar.rect.x, 0.0);
        // Near the right edge: shifted left to stay on the monitor.
        let right = IRect::new(1800, 100, 100, 100);
        let bar = toolbar(right, MAIN, 1.0);
        assert_eq!(bar.rect.right(), 1920.0);
        // On a negative monitor at 150%.
        let bar = toolbar(IRect::new(-1900, 10, 50, 50), LEFT, 1.5);
        assert_eq!(bar.rect.x, -1900.0);
        assert_eq!(bar.rect.h, 54.0);
        assert!(bar.rect.right() <= 0.0);
    }

    #[test]
    fn the_toolbar_wraps_to_two_rows_on_a_narrow_monitor() {
        let narrow = IRect::new(0, 0, 640, 480);
        let bar = toolbar(IRect::new(10, 10, 100, 100), narrow, 1.0);
        assert_eq!(bar.rows, 2);
        assert!(bar.rect.w <= 640.0);
        assert_eq!(bar.rect.h, 10.0 + 2.0 * 26.0 + 4.0);
        assert_eq!(bar.buttons.len(), 26);
        // Order is kept: the second row starts where the first stopped.
        let first_row_y = bar.buttons[0].1.y;
        let split = bar
            .buttons
            .iter()
            .position(|(_, r)| r.y > first_row_y)
            .unwrap();
        assert!(split > 0 && split < 26);
        assert_eq!(bar.buttons[split].1.x, bar.buttons[0].1.x);
        // No button overlaps another.
        for (i, (_, a)) in bar.buttons.iter().enumerate() {
            for (_, b) in &bar.buttons[i + 1..] {
                let apart =
                    a.right() <= b.x || b.right() <= a.x || a.bottom() <= b.y || b.bottom() <= a.y;
                assert!(apart, "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn toolbar_hits() {
        let bar = toolbar(IRect::new(200, 100, 300, 200), MAIN, 1.0);
        // The first button and the gap after it.
        assert_eq!(bar.item_at(206.0, 320.0), Some(Item::Tool(Tool::Select)));
        assert_eq!(bar.item_at(232.0, 320.0), None);
        assert!(bar.contains(232.0, 320.0));
        for (item, r) in &bar.buttons {
            let (cx, cy) = r.center();
            assert_eq!(bar.item_at(cx, cy), Some(*item));
            assert!(bar.contains(cx, cy));
        }
        assert!(!bar.contains(100.0, 100.0));
        let order: Vec<Item> = bar.buttons.iter().map(|(i, _)| *i).collect();
        assert_eq!(order[8], Item::Tool(Tool::Step));
        assert_eq!(order[9], Item::Swatch(0));
        assert_eq!(order[17], Item::Custom);
        assert_eq!(order[18], Item::Size(Size::S));
        assert_eq!(
            &order[21..],
            &[Item::Undo, Item::Redo, Item::Copy, Item::Save, Item::Close]
        );
    }

    #[test]
    fn the_picker_sits_next_to_the_toolbar_and_maps_points_to_colours() {
        let bar = toolbar(IRect::new(200, 100, 300, 200), MAIN, 1.0);
        let p = picker(&bar, MAIN);
        let custom = bar.button(Item::Custom).unwrap();
        assert_eq!(p.rect.y, bar.rect.bottom() + 8.0);
        assert_eq!(p.rect.x, custom.x - 10.0);
        assert_eq!((p.rect.w, p.rect.h), (172.0, 178.0));
        assert_eq!(p.part_at(p.sv.x + 5.0, p.sv.y + 5.0), Some(PickerPart::Sv));
        assert_eq!(
            p.part_at(p.hue.x + 2.0, p.hue.y + 64.0),
            Some(PickerPart::Hue)
        );
        assert_eq!(p.part_at(p.preview.x + 1.0, p.preview.y + 1.0), None);
        assert!(p.contains(p.preview.x + 1.0, p.preview.y + 1.0));
        assert_eq!(p.sv_at(p.sv.x, p.sv.y), (0.0, 1.0));
        assert_eq!(p.sv_at(p.sv.right() - 1.0, p.sv.bottom() - 1.0), (1.0, 0.0));
        assert_eq!(
            p.sv_at(-9999.0, 9999.0),
            (0.0, 0.0),
            "clamped while dragging out"
        );
        assert_eq!(p.hue_at(p.hue.y), 0.0);
        assert_eq!(p.hue_at(p.hue.bottom() + 50.0), 360.0);
        let (x, y) = p.sv_point(0.25, 0.75);
        let (s, v) = p.sv_at(x, y);
        assert!((s - 0.25).abs() < 1e-4 && (v - 0.75).abs() < 1e-4);
        assert!((p.hue_at(p.hue_y(200.0)) - 200.0).abs() < 1e-3);
        // A toolbar at the bottom of the monitor: the picker goes above it.
        let bar = toolbar(MAIN, MAIN, 1.0);
        let p = picker(&bar, MAIN);
        assert_eq!(p.rect.bottom(), bar.rect.y - 8.0);
        // And it stays on the monitor horizontally.
        let bar = toolbar(IRect::new(1800, 100, 100, 100), MAIN, 1.0);
        let p = picker(&bar, MAIN);
        assert!(p.rect.right() <= 1920.0);
    }
}
