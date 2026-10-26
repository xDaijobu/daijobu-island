//! Snap (screenshots): selection geometry, loupe sampling, cropping across monitors,
//! the overlay and thumb labels, file names and the snap thumb's countdown.
//!
//! Everything is in physical desktop pixels. The virtual desktop's origin can be
//! negative (a monitor left of or above the primary one), so nothing here assumes
//! `x >= 0`. The frozen frames come from `dj-capture`; this module never touches the
//! screen.

use std::time::{Duration, Instant};

/// Movement (in px, either axis) after the press before it counts as a drag. Less than
/// this is a click, which picks the window under the cursor.
pub const DRAG_SLOP: i32 = 3;
/// A selection smaller than this on either side cancels the snap.
pub const MIN_SIZE: i32 = 2;
/// Arrow keys move the cursor this far; with Shift, `NUDGE_BIG`.
pub const NUDGE: i32 = 1;
pub const NUDGE_BIG: i32 = 10;
/// The loupe shows a `LOUPE_CELLS` x `LOUPE_CELLS` grid of pixels, each drawn
/// `LOUPE_ZOOM` times larger.
pub const LOUPE_CELLS: i32 = 15;
pub const LOUPE_ZOOM: i32 = 8;
/// How long the snap thumb stays up (hovering pauses it).
pub const THUMB_SECS: u64 = 6;

/// An integer rect: `x..x + w`, `y..y + h` (half-open).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl IRect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// The rect spanned by two corner points, in any order.
    pub fn from_corners(a: (i32, i32), b: (i32, i32)) -> Self {
        let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
        let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
        Self::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// The overlap of two rects, or None when they don't overlap.
    pub fn intersect(&self, o: &IRect) -> Option<IRect> {
        let (x0, y0) = (self.x.max(o.x), self.y.max(o.y));
        let (x1, y1) = (self.right().min(o.right()), self.bottom().min(o.bottom()));
        (x1 > x0 && y1 > y0).then(|| IRect::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// The smallest rect holding both. An empty rect adds nothing.
    pub fn union(&self, o: &IRect) -> IRect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let (x0, y0) = (self.x.min(o.x), self.y.min(o.y));
        let (x1, y1) = (self.right().max(o.right()), self.bottom().max(o.bottom()));
        IRect::new(x0, y0, x1 - x0, y1 - y0)
    }

    fn big_enough(&self) -> bool {
        self.w >= MIN_SIZE && self.h >= MIN_SIZE
    }
}

/// A BGRA bitmap, top-down rows, `width * 4` bytes per row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Bitmap {
    /// An opaque black bitmap.
    pub fn black(width: u32, height: u32) -> Self {
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        for px in pixels.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }
}

/// One monitor, frozen: its desktop rect, its DPI and the pixels (`bitmap` is exactly
/// `rect.w` x `rect.h`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorFrame {
    pub rect: IRect,
    pub dpi: u32,
    pub bitmap: Bitmap,
}

/// The bounding rect of all monitors.
pub fn desktop_of(frames: &[MonitorFrame]) -> IRect {
    frames
        .iter()
        .fold(IRect::default(), |acc, f| acc.union(&f.rect))
}

/// The BGRA pixel at desktop `(x, y)`, or None in a gap between monitors.
pub fn pixel_at(frames: &[MonitorFrame], x: i32, y: i32) -> Option<[u8; 4]> {
    let f = frames.iter().find(|f| f.rect.contains(x, y))?;
    let (lx, ly) = ((x - f.rect.x) as usize, (y - f.rect.y) as usize);
    let i = ly * f.bitmap.stride() + lx * 4;
    let px = f.bitmap.pixels.get(i..i + 4)?;
    Some([px[0], px[1], px[2], px[3]])
}

/// Copies `rect` out of the frozen frames, across monitors. Parts that no monitor covers
/// (gaps in an uneven layout) are opaque black. Output is in physical px, unscaled.
pub fn crop(frames: &[MonitorFrame], rect: IRect) -> Bitmap {
    let (w, h) = (rect.w.max(0) as u32, rect.h.max(0) as u32);
    let mut out = Bitmap::black(w, h);
    let out_stride = out.stride();
    for f in frames {
        let Some(part) = f.rect.intersect(&rect) else {
            continue;
        };
        let src_stride = f.bitmap.stride();
        let row_bytes = part.w as usize * 4;
        for row in 0..part.h {
            let sy = (part.y + row - f.rect.y) as usize;
            let sx = (part.x - f.rect.x) as usize;
            let dy = (part.y + row - rect.y) as usize;
            let dx = (part.x - rect.x) as usize;
            let s = sy * src_stride + sx * 4;
            let d = dy * out_stride + dx * 4;
            out.pixels[d..d + row_bytes].copy_from_slice(&f.bitmap.pixels[s..s + row_bytes]);
        }
    }
    out
}

/// The overlay's selection model: hover over windows, press, drag, nudge, release.
///
/// `targets` are the rects a click can pick, top of the z-order first (windows, then
/// the monitors as a last resort). Rects are clamped to the desktop when picked.
#[derive(Clone, Debug)]
pub struct Selector {
    desktop: IRect,
    targets: Vec<IRect>,
    cursor: (i32, i32),
    press: Option<(i32, i32)>,
    dragging: bool,
}

impl Selector {
    pub fn new(desktop: IRect, targets: Vec<IRect>, cursor: (i32, i32)) -> Self {
        let mut s = Self {
            desktop,
            targets,
            cursor: (0, 0),
            press: None,
            dragging: false,
        };
        s.cursor = s.clamp_point(cursor);
        s
    }

    pub fn desktop(&self) -> IRect {
        self.desktop
    }

    pub fn cursor(&self) -> (i32, i32) {
        self.cursor
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Keeps a point on the desktop's pixels.
    fn clamp_point(&self, (x, y): (i32, i32)) -> (i32, i32) {
        let d = self.desktop;
        (
            x.clamp(d.x, (d.right() - 1).max(d.x)),
            y.clamp(d.y, (d.bottom() - 1).max(d.y)),
        )
    }

    /// The pointer moved to desktop `(x, y)`. True if the cursor changed.
    pub fn move_to(&mut self, x: i32, y: i32) -> bool {
        let p = self.clamp_point((x, y));
        if p == self.cursor {
            return false;
        }
        self.cursor = p;
        if let Some(start) = self.press {
            let moved = (p.0 - start.0).abs().max((p.1 - start.1).abs());
            if moved >= DRAG_SLOP {
                self.dragging = true;
            }
        }
        true
    }

    /// The left button went down at desktop `(x, y)`.
    pub fn press(&mut self, x: i32, y: i32) {
        self.cursor = self.clamp_point((x, y));
        self.press = Some(self.cursor);
        self.dragging = false;
    }

    /// Moves the cursor (and with it the dragged corner) by one step per axis:
    /// `NUDGE` px, or `NUDGE_BIG` with `big`. Returns the new cursor.
    pub fn nudge(&mut self, dx: i32, dy: i32, big: bool) -> (i32, i32) {
        let step = if big { NUDGE_BIG } else { NUDGE };
        let (x, y) = self.cursor;
        self.move_to(x + dx.signum() * step, y + dy.signum() * step);
        self.cursor
    }

    /// The topmost target under the cursor, clamped to the desktop.
    fn target_at(&self, (x, y): (i32, i32)) -> Option<IRect> {
        self.targets
            .iter()
            .find(|r| r.contains(x, y))
            .and_then(|r| r.intersect(&self.desktop))
    }

    /// The live drag rect, clamped to the desktop. None before a drag starts.
    pub fn selection(&self) -> Option<IRect> {
        if !self.dragging {
            return None;
        }
        let start = self.press?;
        let r = IRect::from_corners(start, self.cursor);
        Some(
            r.intersect(&self.desktop)
                .unwrap_or(IRect::new(r.x, r.y, 0, 0)),
        )
    }

    /// What a click here would take: the window (or monitor) under the cursor. None
    /// while dragging.
    pub fn hover(&self) -> Option<IRect> {
        if self.dragging {
            None
        } else {
            self.target_at(self.cursor)
        }
    }

    /// The button went up. Returns the rect to capture, or None to cancel: a click
    /// takes the window under the cursor, a drag its rect, and anything smaller than
    /// `MIN_SIZE` on a side cancels.
    pub fn release(&mut self) -> Option<IRect> {
        self.press?;
        let picked = if self.dragging {
            self.selection()
        } else {
            self.target_at(self.cursor)
        };
        self.press = None;
        self.dragging = false;
        picked.filter(IRect::big_enough)
    }
}

/// Where the loupe's grid sits: its top-left desktop pixel and the cursor's cell in it.
/// The grid stays on the desktop, so at an edge the cursor cell moves off the centre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loupe {
    pub origin: (i32, i32),
    pub cursor_cell: (i32, i32),
}

impl Loupe {
    pub fn at(cursor: (i32, i32), desktop: IRect) -> Self {
        let half = LOUPE_CELLS / 2;
        let axis = |c: i32, lo: i32, len: i32| {
            let hi = lo + len - LOUPE_CELLS;
            if hi < lo {
                lo
            } else {
                (c - half).clamp(lo, hi)
            }
        };
        let origin = (
            axis(cursor.0, desktop.x, desktop.w),
            axis(cursor.1, desktop.y, desktop.h),
        );
        Self {
            origin,
            cursor_cell: (cursor.0 - origin.0, cursor.1 - origin.1),
        }
    }

    /// The desktop pixels the grid samples, row by row.
    pub fn samples(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        (0..LOUPE_CELLS).flat_map(move |row| {
            (0..LOUPE_CELLS).map(move |col| (self.origin.0 + col, self.origin.1 + row))
        })
    }
}

/// `#E0A458` for a BGRA pixel.
pub fn hex(bgra: [u8; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}", bgra[2], bgra[1], bgra[0])
}

/// The loupe caption: `#E0A458 · 812,344`. A gap between monitors shows `#------`.
pub fn loupe_label(bgra: Option<[u8; 4]>, (x, y): (i32, i32)) -> String {
    let color = bgra.map_or_else(|| "#------".to_string(), hex);
    format!("{color} · {x},{y}")
}

/// The overlay's size label: `612 × 388`.
pub fn size_label(w: i32, h: i32) -> String {
    format!("{w} × {h}")
}

/// The thumb's caption: `snap · 612×388`.
pub fn thumb_label(w: u32, h: u32) -> String {
    format!("snap · {w}×{h}")
}

/// Local wall-clock time for a file name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// `snap-YYYY-MM-DD_HHMMSS.png` for `n == 1`, and `…-n.png` for the n-th collision.
pub fn file_name(s: Stamp, n: u32) -> String {
    let base = format!(
        "snap-{:04}-{:02}-{:02}_{:02}{:02}{:02}",
        s.year, s.month, s.day, s.hour, s.minute, s.second
    );
    if n <= 1 {
        format!("{base}.png")
    } else {
        format!("{base}-{n}.png")
    }
}

/// The first of `file_name(s, 1)`, `file_name(s, 2)`, … that `taken` says is free.
pub fn unique_name(s: Stamp, taken: impl Fn(&str) -> bool) -> String {
    (1..)
        .map(|n| file_name(s, n))
        .find(|name| !taken(name))
        .expect("an unbounded range always finds a name")
}

/// What the thumb says under its caption.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbStatus {
    Copied,
    CopyFailed,
    Saved,
    SaveFailed,
}

impl ThumbStatus {
    pub fn label(self) -> &'static str {
        match self {
            ThumbStatus::Copied => "✓ copied",
            ThumbStatus::CopyFailed => "copy failed",
            ThumbStatus::Saved => "saved",
            ThumbStatus::SaveFailed => "save failed",
        }
    }

    pub fn is_error(self) -> bool {
        matches!(self, ThumbStatus::CopyFailed | ThumbStatus::SaveFailed)
    }
}

/// The thumb's countdown. Deadline-based like the timer: nothing ticks, the time left
/// comes from `now`. Paused while hovered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Countdown {
    left: Duration,
    /// Running since this instant; None while paused.
    since: Option<Instant>,
}

impl Countdown {
    pub fn start(total: Duration, now: Instant) -> Self {
        Self {
            left: total,
            since: Some(now),
        }
    }

    pub fn is_paused(&self) -> bool {
        self.since.is_none()
    }

    /// Time left at `now` (never negative).
    pub fn left(&self, now: Instant) -> Duration {
        match self.since {
            Some(t) => self.left.saturating_sub(now.saturating_duration_since(t)),
            None => self.left,
        }
    }

    pub fn expired(&self, now: Instant) -> bool {
        !self.is_paused() && self.left(now).is_zero()
    }

    pub fn pause(&mut self, now: Instant) {
        if self.since.is_some() {
            self.left = self.left(now);
            self.since = None;
        }
    }

    pub fn resume(&mut self, now: Instant) {
        if self.since.is_none() {
            self.since = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two monitors: a 100 x 50 one left of and above a 200 x 100 primary at the origin.
    fn frames() -> Vec<MonitorFrame> {
        let mon = |rect: IRect, seed: u8| {
            let mut bitmap = Bitmap::black(rect.w as u32, rect.h as u32);
            for y in 0..rect.h {
                for x in 0..rect.w {
                    let i = (y * rect.w + x) as usize * 4;
                    bitmap.pixels[i] = seed;
                    bitmap.pixels[i + 1] = x as u8;
                    bitmap.pixels[i + 2] = y as u8;
                }
            }
            MonitorFrame {
                rect,
                dpi: 96,
                bitmap,
            }
        };
        vec![
            mon(IRect::new(0, 0, 200, 100), 1),
            mon(IRect::new(-100, -50, 100, 50), 2),
        ]
    }

    const DESK: IRect = IRect::new(-100, -50, 300, 150);

    fn selector(targets: Vec<IRect>) -> Selector {
        Selector::new(DESK, targets, (10, 10))
    }

    #[test]
    fn rect_basics() {
        let r = IRect::from_corners((5, 9), (-3, 2));
        assert_eq!(r, IRect::new(-3, 2, 8, 7));
        assert!(r.contains(-3, 2) && !r.contains(5, 2) && !r.contains(0, 9));
        assert_eq!(
            r.intersect(&IRect::new(0, 0, 100, 100)),
            Some(IRect::new(0, 2, 5, 7))
        );
        assert_eq!(r.intersect(&IRect::new(5, 0, 10, 10)), None);
        assert_eq!(desktop_of(&frames()), DESK);
    }

    #[test]
    fn drag_in_every_direction_normalises() {
        for (from, to) in [
            ((10, 10), (60, 40)),
            ((60, 40), (10, 10)),
            ((60, 10), (10, 40)),
            ((10, 40), (60, 10)),
        ] {
            let mut s = selector(vec![]);
            s.press(from.0, from.1);
            s.move_to(to.0, to.1);
            assert!(s.is_dragging());
            assert_eq!(s.selection(), Some(IRect::new(10, 10, 50, 30)));
            assert_eq!(s.release(), Some(IRect::new(10, 10, 50, 30)), "{from:?}");
            assert!(!s.is_dragging());
        }
    }

    #[test]
    fn negative_coordinates_on_a_left_monitor() {
        let mut s = selector(vec![]);
        s.press(-90, -40);
        s.move_to(20, 30);
        // Crosses from the left monitor into the primary.
        assert_eq!(s.release(), Some(IRect::new(-90, -40, 110, 70)));
    }

    #[test]
    fn a_click_takes_the_topmost_window_under_the_cursor() {
        let back = IRect::new(0, 0, 150, 90);
        let front = IRect::new(20, 20, 40, 30);
        let monitor = IRect::new(0, 0, 200, 100);
        let mut s = selector(vec![front, back, monitor]);
        s.move_to(30, 30);
        assert_eq!(s.hover(), Some(front));
        s.press(30, 30);
        // Jitter under the slop is still a click.
        s.move_to(32, 31);
        assert!(!s.is_dragging());
        assert_eq!(s.release(), Some(front));
        // Outside the front window: the one behind it.
        s.press(100, 50);
        assert_eq!(s.release(), Some(back));
        // Nothing but the monitor: the whole monitor.
        s.press(190, 95);
        assert_eq!(s.release(), Some(monitor));
    }

    #[test]
    fn a_picked_window_is_clamped_to_the_desktop() {
        // A maximised window's frame hangs a few px off the screen.
        let win = IRect::new(-108, -58, 120, 70);
        let mut s = selector(vec![win]);
        s.press(-50, -20);
        assert_eq!(s.release(), Some(IRect::new(-100, -50, 112, 62)));
    }

    #[test]
    fn a_click_on_nothing_or_a_tiny_drag_cancels() {
        let mut s = selector(vec![]);
        s.press(10, 10);
        assert_eq!(s.release(), None);
        // A drag that ends 1 px wide.
        s.press(10, 10);
        s.move_to(40, 40);
        s.move_to(11, 40);
        assert!(s.is_dragging());
        assert_eq!(s.selection(), Some(IRect::new(10, 10, 1, 30)));
        assert_eq!(s.release(), None);
        // Exactly the minimum.
        s.press(10, 10);
        s.move_to(40, 40);
        s.move_to(12, 12);
        assert_eq!(s.release(), Some(IRect::new(10, 10, 2, 2)));
        // Release without a press does nothing.
        assert_eq!(s.release(), None);
    }

    #[test]
    fn the_drag_is_clamped_to_the_desktop() {
        let mut s = selector(vec![]);
        s.press(150, 80);
        s.move_to(5_000, 5_000);
        assert_eq!(s.cursor(), (199, 99));
        assert_eq!(s.release(), Some(IRect::new(150, 80, 49, 19)));
        s.press(-50, -20);
        s.move_to(-9_000, -9_000);
        assert_eq!(s.cursor(), (-100, -50));
        assert_eq!(s.release(), Some(IRect::new(-100, -50, 50, 30)));
    }

    #[test]
    fn arrows_nudge_the_cursor_and_the_dragged_corner() {
        let mut s = selector(vec![]);
        s.press(10, 10);
        s.move_to(30, 30);
        assert_eq!(s.nudge(1, 0, false), (31, 30));
        assert_eq!(s.nudge(0, -1, true), (31, 20));
        assert_eq!(
            s.nudge(-5, 0, false),
            (30, 20),
            "one step, whatever the size"
        );
        assert_eq!(s.selection(), Some(IRect::new(10, 10, 20, 10)));
        // Nudges past the edge stay on the desktop.
        let mut s = Selector::new(DESK, vec![], (199, 99));
        assert_eq!(s.nudge(1, 1, true), (199, 99));
        // A nudge before the press moves the hover point; past the slop it drags.
        let mut s = selector(vec![]);
        s.press(10, 10);
        s.nudge(1, 0, false);
        assert!(!s.is_dragging());
        s.nudge(1, 0, true);
        assert!(s.is_dragging());
        assert_eq!(s.release(), None, "11 x 0 is too small");
    }

    #[test]
    fn loupe_centres_on_the_cursor_and_clamps_at_the_edges() {
        let l = Loupe::at((50, 50), DESK);
        assert_eq!(l.origin, (43, 43));
        assert_eq!(l.cursor_cell, (7, 7));
        assert_eq!(l.samples().count(), 225);
        assert_eq!(l.samples().next(), Some((43, 43)));
        assert_eq!(l.samples().last(), Some((57, 57)));
        // Top-left corner of a negative desktop.
        let l = Loupe::at((-100, -50), DESK);
        assert_eq!(l.origin, (-100, -50));
        assert_eq!(l.cursor_cell, (0, 0));
        // Bottom-right corner.
        let l = Loupe::at((199, 99), DESK);
        assert_eq!(l.origin, (185, 85));
        assert_eq!(l.cursor_cell, (14, 14));
        // Every sample stays on the desktop.
        for c in [(-100, 99), (199, -50), (0, 0), (-93, -43)] {
            let l = Loupe::at(c, DESK);
            assert!(l.samples().all(|(x, y)| DESK.contains(x, y)), "{c:?}");
            let (cx, cy) = l.cursor_cell;
            assert_eq!((l.origin.0 + cx, l.origin.1 + cy), c);
        }
    }

    #[test]
    fn pixels_and_crops_across_monitors() {
        let f = frames();
        assert_eq!(pixel_at(&f, 3, 4), Some([1, 3, 4, 255]));
        assert_eq!(pixel_at(&f, -100, -50), Some([2, 0, 0, 255]));
        // The gap right of the left monitor, above the primary.
        assert_eq!(pixel_at(&f, 50, -10), None);
        let c = crop(&f, IRect::new(-2, -2, 4, 4));
        assert_eq!((c.width, c.height, c.pixels.len()), (4, 4, 64));
        let px = |x: usize, y: usize| {
            let i = (y * 4 + x) * 4;
            [
                c.pixels[i],
                c.pixels[i + 1],
                c.pixels[i + 2],
                c.pixels[i + 3],
            ]
        };
        assert_eq!(px(0, 0), [2, 98, 48, 255], "left monitor");
        assert_eq!(px(3, 0), [0, 0, 0, 255], "gap: opaque black");
        assert_eq!(px(0, 3), [0, 0, 0, 255], "gap below the left monitor");
        assert_eq!(px(2, 2), [1, 0, 0, 255], "primary origin");
        assert_eq!(px(3, 3), [1, 1, 1, 255]);
    }

    #[test]
    fn labels() {
        assert_eq!(hex([0x58, 0xA4, 0xE0, 255]), "#E0A458");
        assert_eq!(hex([0, 0, 0, 0]), "#000000");
        assert_eq!(
            loupe_label(Some([0x58, 0xA4, 0xE0, 255]), (812, 344)),
            "#E0A458 · 812,344"
        );
        assert_eq!(loupe_label(None, (-5, -12)), "#------ · -5,-12");
        assert_eq!(size_label(612, 388), "612 × 388");
        assert_eq!(thumb_label(612, 388), "snap · 612×388");
        assert_eq!(ThumbStatus::Copied.label(), "✓ copied");
        assert!(ThumbStatus::SaveFailed.is_error() && !ThumbStatus::Saved.is_error());
    }

    const STAMP: Stamp = Stamp {
        year: 2026,
        month: 3,
        day: 7,
        hour: 9,
        minute: 5,
        second: 2,
    };

    #[test]
    fn file_names_and_collisions() {
        assert_eq!(file_name(STAMP, 1), "snap-2026-03-07_090502.png");
        assert_eq!(file_name(STAMP, 2), "snap-2026-03-07_090502-2.png");
        assert_eq!(unique_name(STAMP, |_| false), "snap-2026-03-07_090502.png");
        let taken = ["snap-2026-03-07_090502.png", "snap-2026-03-07_090502-2.png"];
        assert_eq!(
            unique_name(STAMP, |n| taken.contains(&n)),
            "snap-2026-03-07_090502-3.png"
        );
    }

    #[test]
    fn countdown_pauses_and_never_goes_negative() {
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let mut c = Countdown::start(s(THUMB_SECS), t0);
        assert_eq!(c.left(t0 + s(2)), s(4));
        c.pause(t0 + s(2));
        assert!(c.is_paused());
        assert_eq!(c.left(t0 + s(60)), s(4));
        assert!(!c.expired(t0 + s(60)), "paused never expires");
        c.resume(t0 + s(60));
        assert_eq!(c.left(t0 + s(63)), s(1));
        assert!(!c.expired(t0 + s(63)));
        assert!(c.expired(t0 + s(64)));
        assert_eq!(c.left(t0 + s(99)), Duration::ZERO);
        // A clock that seems to go backwards adds no time.
        assert_eq!(c.left(t0), s(4));
    }
}
