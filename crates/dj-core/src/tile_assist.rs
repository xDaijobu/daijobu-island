//! Tile's window picker ("assist"): fills a layout zone by zone. The picker is a window
//! that covers exactly the zone being filled and shows the open windows as cards; picking
//! one fills the zone and moves on to the next.
//!
//! Pure: nothing here lists windows, draws, or registers a thumbnail. `Assist` is the run
//! (the zones, their order, the windows already picked and the keyboard selection);
//! `layout` is the card grid inside one zone; `hit` and `fit` are the small geometry
//! helpers the app needs on top.
//!
//! Units: zones are `IRect`, physical desktop px, as `tile::zones` makes them. The grid is
//! in zone-local **logical** px (the picker window's client origin is the zone's top-left).
//!
//! Layout rules (`layout`): the zone has a `PAD` margin; the header row is `PAD..PAD +
//! HEADER_H` at the top and the footer row ends `PAD` above the bottom; the cards fill the
//! space between them. A card is a `TITLE_H` row (icon and title) above a thumbnail area of
//! aspect `THUMB_ASPECT`; the cards are as wide as the space allows within `CARD_MIN_W..=
//! CARD_MAX_W` (whole logical px) and are centred as one block.

use crate::layout::Rect;
use crate::snap::IRect;

/// The margin inside the zone.
pub const PAD: f32 = 24.0;
/// The header row (title, step) is `PAD..PAD + HEADER_H`.
pub const HEADER_H: f32 = 32.0;
/// The footer row (keys, notices) ends `PAD` above the bottom.
pub const FOOTER_H: f32 = 28.0;
/// A card's top row: the icon and the window title.
pub const TITLE_H: f32 = 28.0;
/// The icon's size in the title row.
pub const ICON: f32 = 20.0;
/// The gap between cards, across and down.
pub const CARD_GAP: f32 = 16.0;
pub const CARD_MIN_W: f32 = 120.0;
pub const CARD_MAX_W: f32 = 320.0;
/// Width / height of a card's thumbnail area.
pub const THUMB_ASPECT: f32 = 1.6;

/// The inset of the icon and the title from the card's sides, and between them.
const INSET: f32 = 8.0;

// ---------------------------------------------------------------------------------------
// The run

/// One "fill every zone" run.
#[derive(Clone, Debug)]
pub struct Assist {
    zones: Vec<IRect>,
    /// Zone indices in fill order: the first one, then the others in reading order.
    order: Vec<usize>,
    /// How many zones are filled (`chose` calls).
    step: usize,
    /// The windows picked so far, in order, without repeats.
    chosen: Vec<u64>,
    selected: usize,
}

impl Assist {
    /// `zones` in reading order (physical px); the run starts at `first`, then the others in
    /// reading order. A `first` past the end starts at the first zone.
    pub fn new(zones: Vec<IRect>, first: usize) -> Self {
        let first = if first < zones.len() { first } else { 0 };
        let order = if zones.is_empty() {
            Vec::new()
        } else {
            std::iter::once(first)
                .chain((0..zones.len()).filter(|&i| i != first))
                .collect()
        };
        Self {
            zones,
            order,
            step: 0,
            chosen: Vec::new(),
            selected: 0,
        }
    }

    /// The zone being filled: (its index in the layout, its rect). None once every zone is
    /// done.
    pub fn zone(&self) -> Option<(usize, IRect)> {
        let i = *self.order.get(self.step)?;
        Some((i, self.zones[i]))
    }

    /// (1-based step, number of zones), for "1 / 2". Once every zone is done the step stays
    /// on the last one; with no zones at all it is (0, 0).
    pub fn progress(&self) -> (usize, usize) {
        let n = self.zones.len();
        ((self.step + 1).min(n), n)
    }

    /// `windows` (newest first) minus the ones already picked in this run.
    pub fn candidates(&self, windows: &[u64]) -> Vec<u64> {
        windows
            .iter()
            .copied()
            .filter(|w| !self.chosen.contains(w))
            .collect()
    }

    /// `window` filled the current zone: remember it, go to the next zone, reset the
    /// selection. Does nothing once the run is done.
    pub fn chose(&mut self, window: u64) {
        if self.done() {
            return;
        }
        if !self.chosen.contains(&window) {
            self.chosen.push(window);
        }
        self.step += 1;
        self.selected = 0;
    }

    /// Every zone is filled (also true for a run with no zones).
    pub fn done(&self) -> bool {
        self.step >= self.order.len()
    }

    /// The windows picked so far, in the order they were picked (the app brings them to the
    /// front in this order at the end).
    pub fn chosen(&self) -> &[u64] {
        &self.chosen
    }

    /// The keyboard selection (index into the current candidates). It can be past the end
    /// of a list that shrank; `key` clamps it.
    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn set_selected(&mut self, i: usize) {
        self.selected = i;
    }

    /// A key with `cols` columns and `count` cards on screen.
    ///
    /// - Left/Right move the selection by 1, Up/Down by `cols`; Home/End go to the first /
    ///   last card. Every move **clamps** at the ends of the list (no wrap), so Up in the
    ///   first row goes to the first card and Down in the last row to the last one.
    /// - Enter gives `Pick(selected)`, Escape `Cancel`.
    /// - With no cards (`count == 0`) everything but Escape gives `None`.
    ///
    /// `Redraw` means the selection moved; the caller applies `Pick` (and calls `chose`).
    pub fn key(&mut self, key: AssistKey, cols: usize, count: usize) -> AssistCmd {
        if key == AssistKey::Escape {
            return AssistCmd::Cancel;
        }
        if count == 0 {
            return AssistCmd::None;
        }
        let last = count - 1;
        let at = self.selected.min(last);
        let next = match key {
            AssistKey::Enter => return AssistCmd::Pick(at),
            AssistKey::Left => at.saturating_sub(1),
            AssistKey::Right => (at + 1).min(last),
            AssistKey::Up => at.saturating_sub(cols.max(1)),
            AssistKey::Down => (at + cols.max(1)).min(last),
            AssistKey::Home => 0,
            AssistKey::End => last,
            AssistKey::Escape => return AssistCmd::Cancel,
        };
        let moved = next != self.selected;
        self.selected = next;
        if moved {
            AssistCmd::Redraw
        } else {
            AssistCmd::None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssistKey {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Enter,
    Escape,
}

/// What the caller does after `Assist::key`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssistCmd {
    /// Nothing changed.
    None,
    /// Only the selection moved: redraw.
    Redraw,
    /// Fill the zone with the card at this index (of the cards on screen).
    Pick(usize),
    /// Stop the run.
    Cancel,
}

// ---------------------------------------------------------------------------------------
// The card grid

/// One card. `icon`, `title` and `thumb` lie inside `rect`: the icon and the title side by
/// side in the top `TITLE_H` row, the thumbnail area below, as wide as the card.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub rect: Rect,
    pub icon: Rect,
    pub title: Rect,
    pub thumb: Rect,
}

/// The picker's geometry for one zone. `cards` are in candidate order, row by row (card `i`
/// is in column `i % cols`, row `i / cols`); `hidden` windows did not fit even at
/// `CARD_MIN_W`. With no cards `cols` is 0.
#[derive(Clone, Debug, PartialEq)]
pub struct AssistLayout {
    pub header: Rect,
    pub footer: Rect,
    pub cards: Vec<Card>,
    pub cols: usize,
    pub hidden: usize,
}

/// A card's height for a width.
fn card_h(w: f32) -> f32 {
    TITLE_H + w / THUMB_ASPECT
}

/// Whether `rows` rows of `cols` cards of width `cw` fit in `aw` × `ah`.
fn fits(aw: f32, ah: f32, cols: usize, rows: usize, cw: f32) -> bool {
    cols as f32 * cw + (cols - 1) as f32 * CARD_GAP <= aw
        && rows as f32 * card_h(cw) + (rows - 1) as f32 * CARD_GAP <= ah
}

/// The widest card (whole px, `CARD_MIN_W..=CARD_MAX_W`) for a grid of `count` cards in
/// `aw` × `ah`, as (columns, rows, width). Ties go to fewer rows, then fewer columns.
fn best_grid(aw: f32, ah: f32, count: usize) -> Option<(usize, usize, f32)> {
    let max_cols = ((aw + CARD_GAP) / (CARD_MIN_W + CARD_GAP)).floor().max(0.0) as usize;
    let mut best: Option<(usize, usize, f32)> = None;
    for cols in 1..=max_cols.min(count) {
        let rows = count.div_ceil(cols);
        // The width the columns and the rows allow; the exact test below fixes any rounding.
        let by_cols = (aw - (cols - 1) as f32 * CARD_GAP) / cols as f32;
        let by_rows = ((ah - (rows - 1) as f32 * CARD_GAP) / rows as f32 - TITLE_H) * THUMB_ASPECT;
        let mut cw = (by_cols.min(by_rows).min(CARD_MAX_W) + 0.01).floor();
        while cw >= CARD_MIN_W && !fits(aw, ah, cols, rows, cw) {
            cw -= 1.0;
        }
        if cw < CARD_MIN_W {
            continue;
        }
        let better = match best {
            None => true,
            Some((_, best_rows, best_w)) => cw > best_w || (cw == best_w && rows < best_rows),
        };
        if better {
            best = Some((cols, rows, cw));
        }
    }
    best
}

/// The card grid for a zone of `w` × `h` logical px and `count` windows (zone-local logical
/// px): the largest card width in `CARD_MIN_W..=CARD_MAX_W` whose grid fits between the
/// header and the footer, centred there. When even `CARD_MIN_W` doesn't fit all of them the
/// first ones that fit are shown and `hidden` counts the rest (all of them in a zone too
/// small for one card).
pub fn layout(w: f32, h: f32, count: usize) -> AssistLayout {
    let aw = (w - 2.0 * PAD).max(0.0);
    let header = Rect::new(PAD, PAD, aw, HEADER_H);
    let footer = Rect::new(PAD, h - PAD - FOOTER_H, aw, FOOTER_H);
    let area = Rect::new(
        PAD,
        PAD + HEADER_H,
        aw,
        (h - 2.0 * PAD - HEADER_H - FOOTER_H).max(0.0),
    );

    // As many as fit at the minimum width, if not all of them.
    let mut shown = count;
    if count > 0 && best_grid(area.w, area.h, count).is_none() {
        let cols = ((area.w + CARD_GAP) / (CARD_MIN_W + CARD_GAP))
            .floor()
            .max(0.0) as usize;
        let rows = ((area.h + CARD_GAP) / (card_h(CARD_MIN_W) + CARD_GAP))
            .floor()
            .max(0.0) as usize;
        shown = count.min(cols * rows);
        while shown > 0 && best_grid(area.w, area.h, shown).is_none() {
            shown -= 1;
        }
    }
    let grid = if shown > 0 {
        best_grid(area.w, area.h, shown)
    } else {
        None
    };
    let Some((cols, rows, cw)) = grid else {
        return AssistLayout {
            header,
            footer,
            cards: Vec::new(),
            cols: 0,
            hidden: count,
        };
    };

    let ch = card_h(cw);
    let grid_w = cols as f32 * cw + (cols - 1) as f32 * CARD_GAP;
    let grid_h = rows as f32 * ch + (rows - 1) as f32 * CARD_GAP;
    let x0 = area.x + ((area.w - grid_w) / 2.0).round();
    let y0 = area.y + ((area.h - grid_h) / 2.0).round();
    let cards = (0..shown)
        .map(|i| {
            let x = x0 + (i % cols) as f32 * (cw + CARD_GAP);
            let y = y0 + (i / cols) as f32 * (ch + CARD_GAP);
            let icon = Rect::new(x + INSET, y + (TITLE_H - ICON) / 2.0, ICON, ICON);
            let title_x = icon.x + ICON + INSET;
            Card {
                rect: Rect::new(x, y, cw, ch),
                icon,
                title: Rect::new(title_x, y, x + cw - INSET - title_x, TITLE_H),
                thumb: Rect::new(x, y + TITLE_H, cw, ch - TITLE_H),
            }
        })
        .collect();
    AssistLayout {
        header,
        footer,
        cards,
        cols,
        hidden: count - shown,
    }
}

/// The card under the point (zone-local logical px), if any: its index in `l.cards`. The
/// gaps between cards, the header and the footer hit nothing.
pub fn hit(l: &AssistLayout, x: f32, y: f32) -> Option<usize> {
    l.cards.iter().position(|c| c.rect.contains(x, y))
}

/// `src` (w, h) aspect-fitted and centred in `area`: as large as fits (a small window is
/// scaled up). A source with no size gives `area` itself.
pub fn fit(area: Rect, src: (f32, f32)) -> Rect {
    let (sw, sh) = src;
    if sw <= 0.0 || sh <= 0.0 || area.w <= 0.0 || area.h <= 0.0 {
        return area;
    }
    // The limiting side is set exactly, so the result never pokes out of `area`.
    let (w, h) = if area.w * sh <= area.h * sw {
        (area.w, sh * area.w / sw)
    } else {
        (sw * area.h / sh, area.h)
    };
    Rect::new(
        area.x + (area.w - w) / 2.0,
        area.y + (area.h - h) / 2.0,
        w,
        h,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile;

    const EPS: f32 = 1e-3;

    fn z(i: i32) -> IRect {
        IRect::new(i * 100, 0, 90, 90)
    }

    fn run3(first: usize) -> Assist {
        Assist::new(vec![z(0), z(1), z(2)], first)
    }

    /// The zones of the preset `name` on a work area, in logical px at scale 1.
    fn zone_sizes(name: &str, work: IRect) -> Vec<(f32, f32)> {
        let presets = tile::presets();
        let i = tile::find(&presets, name).expect(name);
        tile::zones(&presets[i], work, 8)
            .iter()
            .map(|r| (r.w as f32, r.h as f32))
            .collect()
    }

    // --- the run ---

    #[test]
    fn the_run_starts_at_first_then_reading_order() {
        let mut a = run3(1);
        let mut seen = Vec::new();
        while let Some((i, r)) = a.zone() {
            assert_eq!(r, z(i as i32));
            seen.push(i);
            a.chose(i as u64);
        }
        assert_eq!(seen, [1, 0, 2]);

        let mut a = run3(0);
        let mut seen = Vec::new();
        while let Some((i, _)) = a.zone() {
            seen.push(i);
            a.chose(i as u64);
        }
        assert_eq!(seen, [0, 1, 2]);

        let mut a = run3(2);
        let mut seen = Vec::new();
        while let Some((i, _)) = a.zone() {
            seen.push(i);
            a.chose(i as u64);
        }
        assert_eq!(seen, [2, 0, 1]);
    }

    #[test]
    fn a_first_past_the_end_starts_at_the_first_zone() {
        assert_eq!(run3(7).zone(), Some((0, z(0))));
    }

    #[test]
    fn a_run_with_no_zones_is_done() {
        let a = Assist::new(Vec::new(), 0);
        assert!(a.done());
        assert_eq!(a.zone(), None);
        assert_eq!(a.progress(), (0, 0));
    }

    #[test]
    fn candidates_leave_out_the_picked_windows_and_keep_the_order() {
        let mut a = run3(0);
        let windows = [50, 40, 30, 20, 10];
        assert_eq!(a.candidates(&windows), windows);
        a.chose(40);
        assert_eq!(a.candidates(&windows), [50, 30, 20, 10]);
        a.chose(10);
        assert_eq!(a.candidates(&windows), [50, 30, 20]);
        // A picked window that is gone from the list changes nothing.
        assert_eq!(a.candidates(&[50, 30]), [50, 30]);
        assert!(a.candidates(&[]).is_empty());
    }

    #[test]
    fn progress_counts_zones_from_one() {
        let mut a = run3(2);
        assert_eq!(a.progress(), (1, 3));
        a.chose(1);
        assert_eq!(a.progress(), (2, 3));
        a.chose(2);
        assert_eq!(a.progress(), (3, 3));
        a.chose(3);
        assert_eq!(a.progress(), (3, 3));
    }

    #[test]
    fn done_after_the_last_zone_and_chose_then_does_nothing() {
        let mut a = Assist::new(vec![z(0), z(1)], 0);
        assert!(!a.done());
        a.chose(7);
        assert!(!a.done());
        a.chose(8);
        assert!(a.done());
        assert_eq!(a.zone(), None);
        a.chose(9);
        assert!(a.done());
        assert_eq!(a.chosen(), [7, 8]);
        assert_eq!(a.progress(), (2, 2));
    }

    #[test]
    fn chose_resets_the_selection() {
        let mut a = run3(0);
        assert_eq!(a.selected(), 0);
        a.set_selected(4);
        assert_eq!(a.selected(), 4);
        a.chose(1);
        assert_eq!(a.selected(), 0);
    }

    #[test]
    fn chosen_keeps_the_pick_order() {
        let mut a = run3(1);
        a.chose(30);
        a.chose(10);
        assert_eq!(a.chosen(), [30, 10]);
    }

    // --- layout ---

    fn inside(r: Rect, outer: Rect) -> bool {
        r.x >= outer.x - EPS
            && r.y >= outer.y - EPS
            && r.x + r.w <= outer.x + outer.w + EPS
            && r.y + r.h <= outer.y + outer.h + EPS
    }

    fn overlap(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.w - EPS
            && b.x < a.x + a.w - EPS
            && a.y < b.y + b.h - EPS
            && b.y < a.y + a.h - EPS
    }

    /// Every rule that holds for any `layout`, whatever the zone and the count.
    fn check(w: f32, h: f32, count: usize) -> AssistLayout {
        let l = layout(w, h, count);
        let what = format!("{w}x{h} count {count}");
        let zone = Rect::new(0.0, 0.0, w, h);
        let area = Rect::new(
            PAD,
            PAD + HEADER_H,
            w - 2.0 * PAD,
            h - 2.0 * PAD - HEADER_H - FOOTER_H,
        );

        assert_eq!(
            l.header,
            Rect::new(PAD, PAD, w - 2.0 * PAD, HEADER_H),
            "{what}"
        );
        assert_eq!(
            l.footer,
            Rect::new(PAD, h - PAD - FOOTER_H, w - 2.0 * PAD, FOOTER_H),
            "{what}"
        );
        assert_eq!(l.cards.len() + l.hidden, count, "{what}");
        assert_eq!(l.cols == 0, l.cards.is_empty(), "{what}");

        for (i, c) in l.cards.iter().enumerate() {
            assert!(inside(c.rect, zone), "{what}: card {i} outside the zone");
            assert!(
                inside(c.rect, area),
                "{what}: card {i} outside the card area"
            );
            assert!(!overlap(c.rect, l.header), "{what}: card {i} on the header");
            assert!(!overlap(c.rect, l.footer), "{what}: card {i} on the footer");
            for (j, d) in l.cards.iter().enumerate().skip(i + 1) {
                assert!(
                    !overlap(c.rect, d.rect),
                    "{what}: cards {i} and {j} overlap"
                );
            }
            // The card's parts.
            assert!(
                (CARD_MIN_W..=CARD_MAX_W).contains(&c.rect.w),
                "{what}: width {}",
                c.rect.w
            );
            assert_eq!(c.rect.w, c.rect.w.floor(), "{what}: whole px");
            assert!(
                (c.rect.h - (TITLE_H + c.rect.w / THUMB_ASPECT)).abs() < EPS,
                "{what}"
            );
            assert_eq!((c.icon.w, c.icon.h), (ICON, ICON), "{what}");
            assert_eq!(c.title.h, TITLE_H, "{what}");
            assert!(c.title.w > 0.0, "{what}");
            assert!(
                c.thumb.w == c.rect.w && c.thumb.y == c.rect.y + TITLE_H,
                "{what}"
            );
            assert!((c.thumb.w / c.thumb.h - THUMB_ASPECT).abs() < EPS, "{what}");
            assert!(
                (c.thumb.y + c.thumb.h - (c.rect.y + c.rect.h)).abs() < EPS,
                "{what}"
            );
            for part in [c.icon, c.title, c.thumb] {
                assert!(inside(part, c.rect), "{what}: part outside card {i}");
            }
            assert!(!overlap(c.icon, c.title), "{what}");
            assert!(
                !overlap(c.icon, c.thumb) && !overlap(c.title, c.thumb),
                "{what}"
            );
            // Row by row.
            let (col, row) = (i % l.cols, i / l.cols);
            let first = &l.cards[0].rect;
            assert!(
                (c.rect.x - (first.x + col as f32 * (first.w + CARD_GAP))).abs() < EPS,
                "{what}"
            );
            assert!(
                (c.rect.y - (first.y + row as f32 * (first.h + CARD_GAP))).abs() < EPS,
                "{what}"
            );
        }
        l
    }

    /// The widest whole-px card width that fits `count` cards in the zone, by brute force.
    fn widest(w: f32, h: f32, count: usize) -> Option<f32> {
        let (aw, ah) = (w - 2.0 * PAD, h - 2.0 * PAD - HEADER_H - FOOTER_H);
        (CARD_MIN_W as u32..=CARD_MAX_W as u32)
            .rev()
            .map(|cw| cw as f32)
            .find(|&cw| {
                let cols = ((aw + CARD_GAP) / (cw + CARD_GAP)).floor() as usize;
                if cols == 0 {
                    return false;
                }
                let rows = count.div_ceil(cols);
                rows as f32 * (TITLE_H + cw / THUMB_ASPECT) + (rows - 1) as f32 * CARD_GAP <= ah
            })
    }

    #[test]
    fn layout_in_halves_and_thirds_of_real_monitors() {
        let monitors = [
            IRect::new(0, 0, 1920, 1032),
            IRect::new(0, 0, 3440, 1392),
            // 1920x1080 at 150%: the zones in logical px.
            IRect::new(0, 0, 1280, 688),
        ];
        for work in monitors {
            let mut sizes = zone_sizes("50/50", work);
            sizes.extend(zone_sizes("3 columns", work));
            sizes.extend(zone_sizes("2x2", work));
            for (w, h) in sizes {
                for n in [0, 1, 2, 3, 7, 12, 30, 64] {
                    check(w, h, n);
                }
            }
        }
    }

    #[test]
    fn layout_is_the_widest_grid_that_fits() {
        // Whenever everything is shown, no wider card would have fit them all.
        for w in [220.0, 300.0, 480.0, 629.0, 944.0, 1100.0, 1672.0, 1900.0] {
            for h in [260.0, 400.0, 560.0, 688.0, 1016.0, 1392.0] {
                for n in 1..=40 {
                    let l = check(w, h, n);
                    match widest(w, h, n) {
                        Some(cw) => {
                            assert_eq!(l.hidden, 0, "{w}x{h} n {n}");
                            assert_eq!(l.cards[0].rect.w, cw, "{w}x{h} n {n}");
                        }
                        None => assert!(l.hidden > 0 || l.cards.is_empty(), "{w}x{h} n {n}"),
                    }
                }
            }
        }
    }

    #[test]
    fn one_window_gets_the_widest_card_centred() {
        let (w, h) = (944.0, 1016.0);
        let l = check(w, h, 1);
        assert_eq!(l.hidden, 0);
        assert_eq!(l.cols, 1);
        let c = &l.cards[0].rect;
        assert_eq!(c.w, CARD_MAX_W);
        // Centred across the zone and between the header and the footer.
        assert!((c.x - (w - c.w) / 2.0).abs() <= 0.5);
        let (top, bottom) = (c.y - (PAD + HEADER_H), (h - PAD - FOOTER_H) - (c.y + c.h));
        assert!((top - bottom).abs() <= 1.0, "{top} {bottom}");
    }

    #[test]
    fn two_windows_in_a_half_sit_side_by_side() {
        let (w, h) = zone_sizes("50/50", IRect::new(0, 0, 1920, 1032))[0];
        let l = check(w, h, 2);
        assert_eq!((l.cols, l.hidden), (2, 0));
        assert_eq!(l.cards[0].rect.w, CARD_MAX_W);
        assert_eq!(l.cards[0].rect.y, l.cards[1].rect.y);
        assert!(l.cards[1].rect.x > l.cards[0].rect.x);
        assert!((l.cards[1].rect.x - l.cards[0].rect.x - (CARD_MAX_W + CARD_GAP)).abs() < EPS);
    }

    #[test]
    fn seven_windows_in_a_half_of_1080p() {
        let (w, h) = zone_sizes("50/50", IRect::new(0, 0, 1920, 1032))[0];
        let l = check(w, h, 7);
        assert_eq!(l.hidden, 0);
        assert!(l.cols >= 2);
        // The last row is partial and starts at the left edge of the grid.
        let rows = 7usize.div_ceil(l.cols);
        assert_eq!(l.cards[(rows - 1) * l.cols].rect.x, l.cards[0].rect.x);
    }

    #[test]
    fn thirty_windows_shrink_the_cards() {
        let (w, h) = zone_sizes("50/50", IRect::new(0, 0, 3440, 1392))[0];
        let few = check(w, h, 2);
        let many = check(w, h, 30);
        assert!(many.cards[0].rect.w < few.cards[0].rect.w);
        assert_eq!(many.hidden, 0);
        assert_eq!(many.cards.len(), 30);
    }

    #[test]
    fn a_third_of_1080p_hides_what_does_not_fit_at_the_minimum() {
        // 3 columns on 1920x1032: zones 629 wide, 1016 tall; at 120 px that is 4 columns by
        // 7 rows, so 28 cards, and they grow to use the space (133 px).
        let (w, h) = zone_sizes("3 columns", IRect::new(0, 0, 1920, 1032))[0];
        let l = check(w, h, 30);
        assert_eq!(l.cards.len(), 28);
        assert_eq!(l.hidden, 2);
        assert_eq!(l.cols, 4);
        assert_eq!(l.cards[0].rect.w, 133.0);
        // 28 windows all fit, one more does not.
        assert_eq!(check(w, h, 28).hidden, 0);
        let l = check(w, h, 29);
        assert_eq!((l.cards.len(), l.hidden), (28, 1));
        assert_eq!(l.cards[0].rect.w, 133.0);
    }

    #[test]
    fn hidden_windows_leave_a_grid_that_uses_the_space() {
        // 300 x 340: the card area is 252 x 232, room for one column of two cards at the
        // minimum width; the two that are shown grow to 128 px (the height allows it).
        let l = check(300.0, 340.0, 5);
        assert_eq!((l.cols, l.cards.len(), l.hidden), (1, 2, 3));
        assert_eq!(l.cards[0].rect.w, 128.0);
    }

    #[test]
    fn a_zone_too_small_for_one_card_shows_none() {
        // Narrower than PAD + 120 + PAD.
        let l = check(160.0, 600.0, 3);
        assert!(l.cards.is_empty());
        assert_eq!((l.cols, l.hidden), (0, 3));
        // Shorter than the header, a card and the footer.
        let l = check(600.0, 200.0, 3);
        assert!(l.cards.is_empty());
        assert_eq!(l.hidden, 3);
        // Degenerate sizes don't panic.
        for (w, h) in [(0.0, 0.0), (10.0, 10.0), (48.0, 76.0)] {
            let l = layout(w, h, 4);
            assert!(l.cards.is_empty());
            assert_eq!(l.hidden, 4);
        }
    }

    #[test]
    fn no_windows_still_has_a_header_and_a_footer() {
        let l = check(944.0, 1016.0, 0);
        assert!(l.cards.is_empty());
        assert_eq!((l.cols, l.hidden), (0, 0));
        assert_eq!(l.header.y, PAD);
    }

    #[test]
    fn the_grid_is_centred_between_the_header_and_the_footer() {
        for (w, h, n) in [(944.0, 1016.0, 3), (1672.0, 1392.0, 9), (629.0, 1016.0, 5)] {
            let l = check(w, h, n);
            let (mut left, mut right) = (f32::MAX, f32::MIN);
            let (mut top, mut bottom) = (f32::MAX, f32::MIN);
            for c in &l.cards {
                left = left.min(c.rect.x);
                right = right.max(c.rect.x + c.rect.w);
                top = top.min(c.rect.y);
                bottom = bottom.max(c.rect.y + c.rect.h);
            }
            assert!(
                ((left - PAD) - ((w - PAD) - right)).abs() <= 1.0,
                "{w}x{h} {n}"
            );
            let (above, below) = (top - (PAD + HEADER_H), (h - PAD - FOOTER_H) - bottom);
            assert!((above - below).abs() <= 1.0, "{w}x{h} {n}: {above} {below}");
        }
    }

    #[test]
    fn the_icon_and_the_title_share_the_title_row() {
        let l = layout(944.0, 1016.0, 2);
        let c = &l.cards[0];
        assert_eq!(c.icon.x, c.rect.x + INSET);
        assert_eq!(c.icon.y + ICON / 2.0, c.rect.y + TITLE_H / 2.0);
        assert!(c.title.x >= c.icon.x + ICON);
        assert_eq!(c.title.y, c.rect.y);
        assert!((c.title.x + c.title.w - (c.rect.x + c.rect.w - INSET)).abs() < EPS);
        assert_eq!(c.thumb.y, c.rect.y + TITLE_H);
    }

    // --- hit ---

    #[test]
    fn hit_finds_the_card_under_the_point() {
        let l = layout(944.0, 1016.0, 7);
        for (i, c) in l.cards.iter().enumerate() {
            let r = c.rect;
            assert_eq!(hit(&l, r.x + r.w / 2.0, r.y + r.h / 2.0), Some(i));
            // The whole card counts: the title row, the thumbnail, the corners.
            assert_eq!(hit(&l, r.x + 1.0, r.y + 1.0), Some(i));
            assert_eq!(hit(&l, r.x + r.w - 1.0, r.y + r.h - 1.0), Some(i));
            assert_eq!(hit(&l, c.thumb.x + 5.0, c.thumb.y + 5.0), Some(i));
            // The edges are half-open: the far edge belongs to the gap.
            assert_eq!(hit(&l, r.x, r.y), Some(i));
            assert_eq!(hit(&l, r.x + r.w + 1.0, r.y + 1.0), None);
        }
    }

    #[test]
    fn hit_misses_the_gaps_the_header_the_footer_and_the_outside() {
        let l = layout(944.0, 1016.0, 7);
        let (a, b) = (l.cards[0].rect, l.cards[1].rect);
        assert_eq!(hit(&l, (a.x + a.w + b.x) / 2.0, a.y + 10.0), None);
        let below = l.cards[l.cols.min(6)].rect;
        assert_eq!(hit(&l, a.x + 10.0, (a.y + a.h + below.y) / 2.0), None);
        assert_eq!(hit(&l, l.header.x + 5.0, l.header.y + 5.0), None);
        assert_eq!(hit(&l, l.footer.x + 5.0, l.footer.y + 5.0), None);
        assert_eq!(hit(&l, -5.0, -5.0), None);
        assert_eq!(hit(&l, 2000.0, 2000.0), None);
        assert_eq!(hit(&layout(944.0, 1016.0, 0), 400.0, 400.0), None);
    }

    // --- fit ---

    #[test]
    fn fit_a_wide_source_fills_the_width_and_centres_vertically() {
        let area = Rect::new(10.0, 20.0, 200.0, 125.0);
        let r = fit(area, (1920.0, 800.0));
        assert_eq!(r.x, 10.0);
        assert_eq!(r.w, 200.0);
        assert!((r.h - 200.0 * 800.0 / 1920.0).abs() < EPS);
        assert!((r.y - (20.0 + (125.0 - r.h) / 2.0)).abs() < EPS);
    }

    #[test]
    fn fit_a_tall_source_fills_the_height_and_centres_horizontally() {
        let area = Rect::new(10.0, 20.0, 200.0, 125.0);
        let r = fit(area, (400.0, 800.0));
        assert_eq!(r.y, 20.0);
        assert_eq!(r.h, 125.0);
        assert!((r.w - 62.5).abs() < EPS);
        assert!((r.x - (10.0 + (200.0 - 62.5) / 2.0)).abs() < EPS);
    }

    #[test]
    fn fit_the_same_aspect_is_the_area_and_a_small_source_scales_up() {
        let area = Rect::new(0.0, 0.0, 160.0, 100.0);
        let same = fit(area, (1600.0, 1000.0));
        assert!((same.w - 160.0).abs() < EPS && (same.h - 100.0).abs() < EPS);
        let small = fit(area, (80.0, 50.0));
        assert!((small.w - 160.0).abs() < EPS && (small.h - 100.0).abs() < EPS);
    }

    #[test]
    fn fit_stays_inside_the_area_and_keeps_the_aspect() {
        let area = Rect::new(3.0, 7.0, 233.0, 145.5);
        for src in [
            (1.0, 1.0),
            (1920.0, 1080.0),
            (333.0, 1000.0),
            (7.0, 3.0),
            (4000.0, 90.0),
        ] {
            let r = fit(area, src);
            assert!(inside(r, area), "{src:?}");
            assert!(
                (r.w / r.h - src.0 / src.1).abs() < 1e-3 * (src.0 / src.1),
                "{src:?}"
            );
            assert!(r.w == area.w || r.h == area.h, "{src:?}");
        }
    }

    #[test]
    fn fit_without_a_source_size_gives_the_area() {
        let area = Rect::new(1.0, 2.0, 30.0, 40.0);
        assert_eq!(fit(area, (0.0, 100.0)), area);
        assert_eq!(fit(area, (100.0, 0.0)), area);
        assert_eq!(fit(area, (-5.0, 10.0)), area);
        assert_eq!(
            fit(Rect::new(0.0, 0.0, 0.0, 10.0), (10.0, 10.0)),
            Rect::new(0.0, 0.0, 0.0, 10.0)
        );
    }

    // --- keys ---

    /// An assist with the selection at `at`.
    fn at(at: usize) -> Assist {
        let mut a = run3(0);
        a.set_selected(at);
        a
    }

    #[test]
    fn left_and_right_move_by_one_and_clamp() {
        let mut a = at(0);
        assert_eq!(a.key(AssistKey::Left, 3, 7), AssistCmd::None);
        assert_eq!(a.selected(), 0);
        assert_eq!(a.key(AssistKey::Right, 3, 7), AssistCmd::Redraw);
        assert_eq!(a.selected(), 1);
        let mut a = at(6);
        assert_eq!(a.key(AssistKey::Right, 3, 7), AssistCmd::None);
        assert_eq!(a.selected(), 6);
        assert_eq!(a.key(AssistKey::Left, 3, 7), AssistCmd::Redraw);
        assert_eq!(a.selected(), 5);
        // The end of a row continues at the start of the next one (a move by one).
        let mut a = at(2);
        a.key(AssistKey::Right, 3, 7);
        assert_eq!(a.selected(), 3);
    }

    #[test]
    fn up_and_down_move_by_a_row_and_clamp() {
        let mut a = at(4);
        assert_eq!(a.key(AssistKey::Up, 3, 7), AssistCmd::Redraw);
        assert_eq!(a.selected(), 1);
        // The first row: clamp to the first card, no wrap to the bottom.
        assert_eq!(a.key(AssistKey::Up, 3, 7), AssistCmd::Redraw);
        assert_eq!(a.selected(), 0);
        assert_eq!(a.key(AssistKey::Up, 3, 7), AssistCmd::None);
        assert_eq!(a.selected(), 0);
        let mut a = at(1);
        assert_eq!(a.key(AssistKey::Down, 3, 7), AssistCmd::Redraw);
        assert_eq!(a.selected(), 4);
        // Below the last row's start: clamp to the last card.
        assert_eq!(a.key(AssistKey::Down, 3, 7), AssistCmd::Redraw);
        assert_eq!(a.selected(), 6);
        assert_eq!(a.key(AssistKey::Down, 3, 7), AssistCmd::None);
        assert_eq!(a.selected(), 6);
    }

    #[test]
    fn home_and_end_go_to_the_ends() {
        let mut a = at(3);
        assert_eq!(a.key(AssistKey::End, 3, 7), AssistCmd::Redraw);
        assert_eq!(a.selected(), 6);
        assert_eq!(a.key(AssistKey::End, 3, 7), AssistCmd::None);
        assert_eq!(a.key(AssistKey::Home, 3, 7), AssistCmd::Redraw);
        assert_eq!(a.selected(), 0);
        assert_eq!(a.key(AssistKey::Home, 3, 7), AssistCmd::None);
    }

    #[test]
    fn enter_picks_the_selected_card_and_escape_cancels() {
        let mut a = at(0);
        assert_eq!(a.key(AssistKey::Enter, 3, 7), AssistCmd::Pick(0));
        a.set_selected(5);
        assert_eq!(a.key(AssistKey::Enter, 3, 7), AssistCmd::Pick(5));
        assert_eq!(a.selected(), 5);
        assert_eq!(a.key(AssistKey::Escape, 3, 7), AssistCmd::Cancel);
        assert_eq!(a.key(AssistKey::Escape, 0, 0), AssistCmd::Cancel);
    }

    #[test]
    fn no_cards_means_no_movement_and_no_pick() {
        let mut a = at(0);
        for key in [
            AssistKey::Left,
            AssistKey::Right,
            AssistKey::Up,
            AssistKey::Down,
            AssistKey::Home,
            AssistKey::End,
            AssistKey::Enter,
        ] {
            assert_eq!(a.key(key, 0, 0), AssistCmd::None, "{key:?}");
            assert_eq!(a.key(key, 3, 0), AssistCmd::None, "{key:?}");
        }
        assert_eq!(a.selected(), 0);
    }

    #[test]
    fn one_card_is_always_selected() {
        let mut a = at(0);
        for key in [
            AssistKey::Left,
            AssistKey::Right,
            AssistKey::Up,
            AssistKey::Down,
            AssistKey::Home,
            AssistKey::End,
        ] {
            assert_eq!(a.key(key, 1, 1), AssistCmd::None, "{key:?}");
        }
        assert_eq!(a.key(AssistKey::Enter, 1, 1), AssistCmd::Pick(0));
    }

    #[test]
    fn a_selection_past_a_shrunken_list_is_clamped() {
        let mut a = at(9);
        assert_eq!(a.key(AssistKey::Enter, 3, 4), AssistCmd::Pick(3));
        assert_eq!(a.key(AssistKey::Left, 3, 4), AssistCmd::Redraw);
        assert_eq!(a.selected(), 2);
        let mut a = at(9);
        assert_eq!(a.key(AssistKey::Down, 3, 4), AssistCmd::Redraw);
        assert_eq!(a.selected(), 3);
    }

    #[test]
    fn zero_columns_move_like_one() {
        let mut a = at(2);
        assert_eq!(a.key(AssistKey::Down, 0, 5), AssistCmd::Redraw);
        assert_eq!(a.selected(), 3);
        assert_eq!(a.key(AssistKey::Up, 0, 5), AssistCmd::Redraw);
        assert_eq!(a.selected(), 2);
    }
}
