//! Tile (window zones): layouts and their zones, zone math, navigation between zones,
//! frame-border correction, the island's Tile panel geometry, the drag state machine and
//! the restore-size bookkeeping.
//!
//! Pure: nothing here reads the cursor, moves a window or asks Windows for a rect. The app
//! feeds samples in (`Drag::poll`, `Placed::after_move`) and acts on the answers.
//!
//! Units: `IRect` is physical desktop px (negative for a monitor left of or above the
//! primary one); `Rect` is logical px (panel-local for the Tile panel).
//!
//! Zone edges: the work area is inset by `gap` on every side and `gap` separates
//! neighbouring tracks. Every track but the last gets `round(ratio share)` px and the last
//! takes the remainder, so the far edge is exact (`work.right() - gap`).

use crate::layout::Rect;
use crate::snap::IRect;
use std::collections::{BTreeMap, HashMap};
use toml::Value;

/// The Tile panel's size in logical px (the island's `layout::TILE_PANEL_W` / `TILE_PANEL_H`).
pub const PANEL_W: f32 = crate::layout::TILE_PANEL_W;
pub const PANEL_H: f32 = crate::layout::TILE_PANEL_H;
/// Most zones in one layout.
pub const MAX_ZONES: usize = 9;
/// Custom layouts the picker shows (after the presets).
pub const MAX_CUSTOM: usize = 3;
/// The layout a monitor starts with.
pub const DEFAULT_LAYOUT: &str = "50/50";
/// Most column (or row) ratios in one layout's grid.
pub const MAX_TRACKS: usize = 6;

/// Thumbnails in the picker: the 9 presets and `MAX_CUSTOM` custom layouts.
pub const MAX_THUMBS: usize = 9 + MAX_CUSTOM;
/// A thumbnail's size, the gaps between thumbnails and the top of the first row.
pub const THUMB_W: f32 = 52.0;
pub const THUMB_H: f32 = 32.0;
pub const THUMB_GAP_X: f32 = 9.0;
pub const THUMB_GAP_Y: f32 = 10.0;
pub const THUMBS_TOP: f32 = 42.0;
/// Thumbnails per row.
pub const THUMBS_PER_ROW: usize = 6;
/// The gap between a thumbnail's cells and between its cells and its edge.
pub const CELL_GAP: f32 = 2.0;
/// The header line ("tile · drop on a zone", the monitor on the right) and the footer
/// line (status or notice), panel-local.
pub const PANEL_HEADER: Rect = Rect::new(20.0, 14.0, 360.0, 16.0);
pub const PANEL_FOOTER: Rect = Rect::new(20.0, 128.0, 360.0, 18.0);

// ---------------------------------------------------------------------------------------
// Layouts

/// One zone as a span of grid cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub col: usize,
    pub row: usize,
    pub cols: usize,
    pub rows: usize,
}

/// A grid of column and row ratios plus the zones laid over it. `zones` is in reading
/// order (top-left first, then left to right, then down): zone `i` is zone number `i + 1`.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub name: String,
    pub cols: Vec<f32>,
    pub rows: Vec<f32>,
    pub zones: Vec<Span>,
}

const fn span(col: usize, row: usize, cols: usize, rows: usize) -> Span {
    Span {
        col,
        row,
        cols,
        rows,
    }
}

/// Zones sorted into reading order: by row, then column.
fn reading_order(zones: &mut [Span]) {
    zones.sort_by_key(|s| (s.row, s.col));
}

/// A plain grid: one zone per cell.
fn grid(name: &str, cols: &[f32], rows: &[f32]) -> Layout {
    let zones = (0..rows.len())
        .flat_map(|row| (0..cols.len()).map(move |col| span(col, row, 1, 1)))
        .collect();
    Layout {
        name: name.to_string(),
        cols: cols.to_vec(),
        rows: rows.to_vec(),
        zones,
    }
}

/// One tall zone on the left (`left` : 1) and `n` zones stacked on the right.
fn tall_left(name: &str, left: f32, n: usize) -> Layout {
    let mut zones = vec![span(0, 0, 1, n)];
    zones.extend((0..n).map(|row| span(1, row, 1, 1)));
    reading_order(&mut zones);
    Layout {
        name: name.to_string(),
        cols: vec![left, 1.0],
        rows: vec![1.0; n],
        zones,
    }
}

/// The nine built-in layouts, in the order the picker shows them.
pub fn presets() -> Vec<Layout> {
    vec![
        grid("50/50", &[1.0, 1.0], &[1.0]),
        grid("60/40", &[3.0, 2.0], &[1.0]),
        grid("70/30", &[7.0, 3.0], &[1.0]),
        grid("top/bottom", &[1.0], &[1.0, 1.0]),
        grid("3 columns", &[1.0, 1.0, 1.0], &[1.0]),
        tall_left("1+2", 1.4, 2),
        grid("wide center", &[1.0, 2.0, 1.0], &[1.0]),
        grid("2x2", &[1.0, 1.0], &[1.0, 1.0]),
        tall_left("1+3", 1.6, 3),
    ]
}

/// The picker's layouts: the presets, then the first `MAX_CUSTOM` custom layouts.
pub fn picker_layouts(custom: &[Layout]) -> Vec<Layout> {
    let mut all = presets();
    all.extend(custom.iter().take(MAX_CUSTOM).cloned());
    all
}

/// The index of the layout called `name` (case and surrounding spaces ignored).
pub fn find(layouts: &[Layout], name: &str) -> Option<usize> {
    let name = name.trim();
    layouts
        .iter()
        .position(|l| l.name.trim().eq_ignore_ascii_case(name))
}

/// The index to use when a monitor has no (or an unknown) active layout: `50/50`, else
/// the first layout.
fn fallback_layout(layouts: &[Layout]) -> Option<usize> {
    find(layouts, DEFAULT_LAYOUT).or((!layouts.is_empty()).then_some(0))
}

/// Why a layout is not usable, or Ok. Presets pass it too.
fn validate(l: &Layout) -> Result<(), String> {
    for (axis, ratios) in [("cols", &l.cols), ("rows", &l.rows)] {
        if ratios.is_empty() || ratios.len() > MAX_TRACKS {
            return Err(format!(
                "{axis} needs 1 to {MAX_TRACKS} ratios, found {}",
                ratios.len()
            ));
        }
        if let Some(bad) = ratios.iter().find(|r| !r.is_finite() || **r <= 0.0) {
            return Err(format!("{axis} ratios must be positive, found {bad}"));
        }
    }
    if l.zones.is_empty() {
        return Err("needs at least one zone".to_string());
    }
    if l.zones.len() > MAX_ZONES {
        return Err(format!(
            "has {} zones, at most {MAX_ZONES} are allowed",
            l.zones.len()
        ));
    }
    let (nc, nr) = (l.cols.len(), l.rows.len());
    let mut owner: Vec<Option<usize>> = vec![None; nc * nr];
    for (k, s) in l.zones.iter().enumerate() {
        let inside = s.cols >= 1
            && s.rows >= 1
            && s.col.saturating_add(s.cols) <= nc
            && s.row.saturating_add(s.rows) <= nr;
        if !inside {
            return Err(format!("zone {} is outside the grid ({nc}x{nr})", k + 1));
        }
        for row in s.row..s.row + s.rows {
            for col in s.col..s.col + s.cols {
                if let Some(other) = owner[row * nc + col] {
                    return Err(format!("zones {} and {} overlap", other + 1, k + 1));
                }
                owner[row * nc + col] = Some(k);
            }
        }
    }
    if let Some(i) = owner.iter().position(Option::is_none) {
        return Err(format!(
            "column {}, row {} is not covered by a zone (a hole)",
            i % nc + 1,
            i / nc + 1
        ));
    }
    Ok(())
}

/// A TOML number as a ratio.
fn ratio_of(v: &Value) -> Option<f32> {
    match v {
        Value::Integer(i) => Some(*i as f32),
        Value::Float(f) => Some(*f as f32),
        _ => None,
    }
}

/// One `[[layout]]` entry (`n` is its 1-based position in the file). `taken` are the
/// layouts accepted so far. The error is the whole message, naming the layout.
fn read_layout(n: usize, entry: &Value, taken: &[Layout]) -> Result<Layout, String> {
    let Some(t) = entry.as_table() else {
        return Err(format!("layout {n}: not a table"));
    };
    let name = t
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let who = if name.is_empty() {
        format!("layout {n}")
    } else {
        format!("layout {n} \"{name}\"")
    };
    let bad = |why: String| format!("{who}: {why}");
    if name.is_empty() {
        return Err(bad("name is empty or missing".to_string()));
    }
    if find(&presets(), name).is_some() {
        return Err(bad("name is a preset name".to_string()));
    }
    if find(taken, name).is_some() {
        return Err(bad("name is already used by an earlier layout".to_string()));
    }
    let mut axes: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
    for (axis, key) in axes.iter_mut().zip(["cols", "rows"]) {
        let Some(items) = t.get(key).and_then(Value::as_array) else {
            return Err(bad(format!("{key} must be an array of numbers")));
        };
        let Some(ratios) = items.iter().map(ratio_of).collect::<Option<Vec<f32>>>() else {
            return Err(bad(format!("{key} must be an array of numbers")));
        };
        *axis = ratios;
    }
    let [cols, rows] = axes;
    let Some(items) = t.get("zones").and_then(Value::as_array) else {
        return Err(bad(
            "zones must be an array of [col, row, col_span, row_span]".to_string(),
        ));
    };
    let mut zones = Vec::with_capacity(items.len().min(MAX_ZONES + 1));
    for (k, item) in items.iter().enumerate() {
        let nums: Option<Vec<usize>> = item.as_array().and_then(|a| {
            a.iter()
                .map(|v| v.as_integer().and_then(|i| usize::try_from(i).ok()))
                .collect()
        });
        match nums.as_deref() {
            Some(&[col, row, cs, rs]) => zones.push(span(col, row, cs, rs)),
            _ => {
                return Err(bad(format!(
                    "zone {} must be [col, row, col_span, row_span] with whole numbers",
                    k + 1
                )))
            }
        }
    }
    let mut layout = Layout {
        name: name.to_string(),
        cols,
        rows,
        zones,
    };
    validate(&layout).map_err(bad)?;
    reading_order(&mut layout.zones);
    Ok(layout)
}

/// Reads `layouts.toml`: the valid layouts, and one message per rejected one. A syntax
/// error (or a `layout` that is not an array of tables) rejects the whole file with one
/// message. An invalid layout never takes the others down.
///
/// ```toml
/// [[layout]]
/// name = "code + docs"
/// cols = [2, 1]
/// rows = [1, 1]
/// zones = [[0, 0, 1, 2], [1, 0, 1, 1], [1, 1, 1, 1]]   # col, row, col_span, row_span
/// ```
pub fn parse_custom(text: &str) -> (Vec<Layout>, Vec<String>) {
    let table = match toml::from_str::<toml::Table>(text) {
        Ok(t) => t,
        Err(e) => {
            let at = e.span().map_or(String::new(), |s| {
                let end = s.start.min(text.len());
                let line = text.as_bytes()[..end]
                    .iter()
                    .filter(|&&b| b == b'\n')
                    .count()
                    + 1;
                format!(" at line {line}")
            });
            return (
                Vec::new(),
                vec![format!("layouts.toml: syntax error{at}: {}", e.message())],
            );
        }
    };
    let entries = match table.get("layout") {
        None => return (Vec::new(), Vec::new()),
        Some(Value::Array(a)) => a,
        Some(_) => {
            return (
                Vec::new(),
                vec!["layouts.toml: `layout` must be an array of tables ([[layout]])".to_string()],
            )
        }
    };
    let (mut ok, mut errors) = (Vec::new(), Vec::new());
    for (i, entry) in entries.iter().enumerate() {
        match read_layout(i + 1, entry, &ok) {
            Ok(l) => ok.push(l),
            Err(e) => errors.push(e),
        }
    }
    (ok, errors)
}

// ---------------------------------------------------------------------------------------
// Zone math

/// Positive finite ratios as weights; all-equal when none of them is usable.
fn weights(ratios: &[f32]) -> (Vec<f64>, f64) {
    let mut w: Vec<f64> = ratios
        .iter()
        .map(|&r| {
            if r.is_finite() && r > 0.0 {
                r as f64
            } else {
                0.0
            }
        })
        .collect();
    let mut sum: f64 = w.iter().sum();
    if sum <= 0.0 {
        w.iter_mut().for_each(|x| *x = 1.0);
        sum = w.len() as f64;
    }
    (w, sum)
}

/// `(start, end)` of every track along one axis: `gap` at both ends and between tracks.
fn tracks(origin: i32, len: i32, ratios: &[f32], gap: i32) -> Vec<(i32, i32)> {
    let n = ratios.len() as i32;
    let mut gap = gap.max(0);
    if len - gap * (n + 1) < n {
        gap = 0; // no room for the gaps
    }
    let content = (len - gap * (n + 1)).max(0);
    let (w, sum) = weights(ratios);
    let mut out = Vec::with_capacity(w.len());
    let (mut pos, mut used) = (origin + gap, 0);
    for (i, share) in w.iter().enumerate() {
        let size = if i + 1 == w.len() {
            content - used
        } else {
            ((content as f64 * share / sum).round() as i32).clamp(0, content - used)
        };
        out.push((pos, pos + size));
        pos += size + gap;
        used += size;
    }
    out
}

/// The same in logical px: no rounding, the last edge exact.
fn tracks_f(origin: f32, len: f32, ratios: &[f32], gap: f32) -> Vec<(f32, f32)> {
    let n = ratios.len() as f32;
    let mut gap = if gap.is_finite() { gap.max(0.0) } else { 0.0 };
    if len - gap * (n + 1.0) < 0.0 {
        gap = 0.0;
    }
    let content = (len - gap * (n + 1.0)).max(0.0) as f64;
    let (w, sum) = weights(ratios);
    let mut out = Vec::with_capacity(w.len());
    let mut cum = 0.0;
    for (i, share) in w.iter().enumerate() {
        let start = origin + gap + (content * cum / sum) as f32 + gap * i as f32;
        cum += share;
        let end = if i + 1 == w.len() {
            (origin + len - gap).max(start)
        } else {
            origin + gap + (content * cum / sum) as f32 + gap * i as f32
        };
        out.push((start, end));
    }
    out
}

/// From the start of track `start` to the end of the track `count - 1` after it.
fn extent<T: Copy>(t: &[(T, T)], start: usize, count: usize) -> Option<(T, T)> {
    let last = t.len().checked_sub(1)?;
    let a = t[start.min(last)];
    let b = t[(start + count.saturating_sub(1)).min(last)];
    Some((a.0, b.1))
}

/// Zone rects in physical px inside `work`, `gap` px between zones and at the edges, in
/// reading order. Rounding is described in the module docs.
pub fn zones(layout: &Layout, work: IRect, gap: i32) -> Vec<IRect> {
    let xs = tracks(work.x, work.w, &layout.cols, gap);
    let ys = tracks(work.y, work.h, &layout.rows, gap);
    layout
        .zones
        .iter()
        .map(
            |s| match (extent(&xs, s.col, s.cols), extent(&ys, s.row, s.rows)) {
                (Some((x0, x1)), Some((y0, y1))) => IRect::new(x0, y0, x1 - x0, y1 - y0),
                _ => IRect::new(work.x, work.y, 0, 0),
            },
        )
        .collect()
}

/// The same in logical px, for thumbnails and the overlay preview.
pub fn zones_f(layout: &Layout, area: Rect, gap: f32) -> Vec<Rect> {
    let xs = tracks_f(area.x, area.w, &layout.cols, gap);
    let ys = tracks_f(area.y, area.h, &layout.rows, gap);
    layout
        .zones
        .iter()
        .map(
            |s| match (extent(&xs, s.col, s.cols), extent(&ys, s.row, s.rows)) {
                (Some((x0, x1)), Some((y0, y1))) => Rect::new(x0, y0, x1 - x0, y1 - y0),
                _ => Rect::new(area.x, area.y, 0.0, 0.0),
            },
        )
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Twice the centre, so odd sizes stay exact.
fn centre2(r: IRect) -> (i64, i64) {
    (2 * r.x as i64 + r.w as i64, 2 * r.y as i64 + r.h as i64)
}

/// The zone this frame fills: every edge within `tolerance` px of the zone's. The closest
/// one if several qualify.
pub fn zone_of(zones: &[IRect], frame: IRect, tolerance: i32) -> Option<usize> {
    let tol = tolerance.max(0) as u32;
    zones
        .iter()
        .enumerate()
        .filter_map(|(i, z)| {
            let d = [
                frame.x.abs_diff(z.x),
                frame.y.abs_diff(z.y),
                frame.right().abs_diff(z.right()),
                frame.bottom().abs_diff(z.bottom()),
            ];
            d.iter()
                .all(|&v| v <= tol)
                .then(|| (d.iter().map(|&v| v as u64).sum::<u64>(), i))
        })
        .min()
        .map(|(_, i)| i)
}

/// The zone whose centre is closest to the frame's centre (the lower index on a tie).
pub fn nearest(zones: &[IRect], frame: IRect) -> Option<usize> {
    let c = centre2(frame);
    zones
        .iter()
        .enumerate()
        .map(|(i, z)| {
            let p = centre2(*z);
            let (dx, dy) = (p.0 - c.0, p.1 - c.1);
            (dx * dx + dy * dy, i)
        })
        .min()
        .map(|(_, i)| i)
}

/// The zone to go to from zone `from`: the nearest centre strictly on that side (the lower
/// index on a tie), or None at the edge of the layout.
pub fn neighbour(zones: &[IRect], from: usize, dir: Dir) -> Option<usize> {
    let c = centre2(*zones.get(from)?);
    zones
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != from)
        .filter_map(|(i, z)| {
            let p = centre2(*z);
            let (dx, dy) = (p.0 - c.0, p.1 - c.1);
            let ahead = match dir {
                Dir::Left => dx < 0,
                Dir::Right => dx > 0,
                Dir::Up => dy < 0,
                Dir::Down => dy > 0,
            };
            ahead.then_some((dx * dx + dy * dy, i))
        })
        .min()
        .map(|(_, i)| i)
}

/// The rect for `SetWindowPos` so that the visible frame (`DWMWA_EXTENDED_FRAME_BOUNDS`)
/// fills `zone`. `window` is the current `GetWindowRect`, `frame` the current visible
/// frame; the difference is the invisible resize border (typically 7 px left, right and
/// bottom on Windows 11). An empty `window` or `frame` gives `zone` unchanged.
pub fn window_rect_for(zone: IRect, window: IRect, frame: IRect) -> IRect {
    if window.is_empty() || frame.is_empty() {
        return zone;
    }
    let left = frame.x - window.x;
    let top = frame.y - window.y;
    let right = window.right() - frame.right();
    let bottom = window.bottom() - frame.bottom();
    IRect::new(
        zone.x - left,
        zone.y - top,
        zone.w + left + right,
        zone.h + top + bottom,
    )
}

/// The active layout per monitor, in the order of `monitors`: `names` maps a monitor id
/// to a layout name (`[tile.monitors]`); a missing or unknown name means `50/50`.
pub fn active_layouts(
    monitors: &[Monitor],
    names: &BTreeMap<String, String>,
    layouts: &[Layout],
) -> Vec<usize> {
    monitors
        .iter()
        .map(|m| {
            names
                .get(&m.id)
                .and_then(|n| find(layouts, n))
                .or_else(|| fallback_layout(layouts))
                .unwrap_or(0)
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// The island's Tile panel

/// One layout's thumbnail in the panel: its rect and its zones' cells, panel-local logical
/// px. `layout` indexes the slice given to `panel_thumbs`.
#[derive(Clone, Debug, PartialEq)]
pub struct Thumb {
    pub layout: usize,
    pub rect: Rect,
    pub cells: Vec<Rect>,
}

/// The picker's thumbnails: at most `MAX_THUMBS`, rows of `THUMBS_PER_ROW`, the grid
/// centred horizontally (a short last row stays left-aligned in it). The first row's left
/// edge is rounded down to a whole px.
pub fn panel_thumbs(layouts: &[Layout]) -> Vec<Thumb> {
    let n = layouts.len().min(MAX_THUMBS);
    let per_row = n.min(THUMBS_PER_ROW);
    if per_row == 0 {
        return Vec::new();
    }
    let grid_w = per_row as f32 * THUMB_W + (per_row - 1) as f32 * THUMB_GAP_X;
    let left = ((PANEL_W - grid_w) / 2.0).floor();
    layouts
        .iter()
        .take(n)
        .enumerate()
        .map(|(i, layout)| {
            let (col, row) = (i % THUMBS_PER_ROW, i / THUMBS_PER_ROW);
            let rect = Rect::new(
                left + col as f32 * (THUMB_W + THUMB_GAP_X),
                THUMBS_TOP + row as f32 * (THUMB_H + THUMB_GAP_Y),
                THUMB_W,
                THUMB_H,
            );
            Thumb {
                layout: i,
                rect,
                cells: zones_f(layout, rect, CELL_GAP),
            }
        })
        .collect()
}

/// The `(layout, zone)` cell under a panel-local point. The gaps between and inside
/// thumbnails and everything else (the header, the footer) are misses.
pub fn panel_hit(thumbs: &[Thumb], x: f32, y: f32) -> Option<(usize, usize)> {
    thumbs.iter().find_map(|t| {
        t.cells
            .iter()
            .position(|c| c.contains(x, y))
            .map(|zone| (t.layout, zone))
    })
}

/// The panel header's right-hand text: the monitor's number (`index` counts from 0, the
/// label from 1) and its size in px, e.g. `monitor 1 · 3440×1440`.
pub fn monitor_label(index: usize, rect: IRect) -> String {
    format!("monitor {} · {}×{}", index + 1, rect.w, rect.h)
}

// ---------------------------------------------------------------------------------------
// One window move

/// A monitor as the drag sees it. `id` is the device name (`[tile.monitors]` key), `rect`
/// its desktop rect, `work` its work area, `scale` its device px per logical px.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub id: String,
    pub rect: IRect,
    pub work: IRect,
    pub scale: f32,
}

/// Everything a poll needs besides the cursor. `layouts` is the picker's list (see
/// `picker_layouts`); `Target::layout` and the thumbnails index it.
#[derive(Clone, Copy, Debug)]
pub struct Geometry<'a> {
    pub monitors: &'a [Monitor],
    pub layouts: &'a [Layout],
    /// The active layout per monitor (same order as `monitors`).
    pub active: &'a [usize],
    pub island_monitor: usize,
    /// The summon area, desktop px.
    pub summon: IRect,
    /// The open panel's final (not animated) rect, desktop px.
    pub panel: IRect,
    /// The island's scale (device px per logical px).
    pub panel_scale: f32,
    /// Logical px; × each monitor's scale, rounded.
    pub gap: f32,
}

/// Where a release would put the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub monitor: usize,
    pub layout: usize,
    pub zone: usize,
    /// Picked in the island's panel (so the layout becomes that monitor's active one),
    /// not by Shift.
    pub from_panel: bool,
}

/// The zone overlay to show: that monitor, that layout's zones, one highlighted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayView {
    pub monitor: usize,
    pub layout: usize,
    pub highlight: Option<usize>,
}

/// What one poll says to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DragView {
    pub panel_open: bool,
    /// The hovered `(layout, zone)` cell of the panel.
    pub panel_hover: Option<(usize, usize)>,
    pub overlay: Option<OverlayView>,
    pub target: Option<Target>,
}

/// The monitor holding `p`, else the nearest one (the lower index on a tie).
fn monitor_at(monitors: &[Monitor], p: (i32, i32)) -> Option<usize> {
    if let Some(i) = monitors.iter().position(|m| m.rect.contains(p.0, p.1)) {
        return Some(i);
    }
    let dist = |r: IRect| {
        let dx = (r.x - p.0).max(p.0 - (r.right() - 1)).max(0) as i64;
        let dy = (r.y - p.1).max(p.1 - (r.bottom() - 1)).max(0) as i64;
        dx * dx + dy * dy
    };
    monitors
        .iter()
        .enumerate()
        .map(|(i, m)| (dist(m.rect), i))
        .min()
        .map(|(_, i)| i)
}

/// The Shift part of a poll: the overlay and the target on the monitor under the cursor.
fn shift_view(cursor: (i32, i32), geo: &Geometry) -> Option<(OverlayView, Option<Target>)> {
    let monitor = monitor_at(geo.monitors, cursor)?;
    let layout = geo
        .active
        .get(monitor)
        .copied()
        .filter(|&a| a < geo.layouts.len())
        .or_else(|| fallback_layout(geo.layouts))?;
    let mon = &geo.monitors[monitor];
    let gap = (geo.gap * mon.scale).round().max(0.0) as i32;
    let zone = zones(&geo.layouts[layout], mon.work, gap)
        .iter()
        .position(|z| z.contains(cursor.0, cursor.1));
    let target = zone.map(|zone| Target {
        monitor,
        layout,
        zone,
        from_panel: false,
    });
    Some((
        OverlayView {
            monitor,
            layout,
            highlight: zone,
        },
        target,
    ))
}

/// The panel cell under the cursor, if it is inside the panel.
fn panel_cell(cursor: (i32, i32), geo: &Geometry) -> Option<(usize, usize)> {
    if !geo.panel.contains(cursor.0, cursor.1) {
        return None;
    }
    let s = if geo.panel_scale > 0.0 {
        geo.panel_scale
    } else {
        1.0
    };
    let x = (cursor.0 - geo.panel.x) as f32 / s;
    let y = (cursor.1 - geo.panel.y) as f32 / s;
    panel_hit(&panel_thumbs(geo.layouts), x, y)
}

/// One window move, from `MOVESIZESTART` to `MOVESIZEEND`. The app samples the cursor into
/// `poll` (about 60 times a second, only during the move) and asks `end` at the release.
#[derive(Clone, Debug)]
pub struct Drag {
    window: u64,
    panel_open: bool,
    /// A resize was seen: the rest of this drag does nothing.
    resized: bool,
    target: Option<Target>,
}

impl Drag {
    pub fn new(window: u64) -> Self {
        Self {
            window,
            panel_open: false,
            resized: false,
            target: None,
        }
    }

    pub fn window(&self) -> u64 {
        self.window
    }

    /// One cursor sample (desktop px). `shift`: Shift is down. `resizing`: the window's
    /// size changed since the start, so it is an edge drag, not a move: once seen, the
    /// panel stays closed and there is no overlay and no target for the rest of the drag.
    ///
    /// The panel opens when the cursor is in `geo.summon` and stays open while the cursor
    /// is in the summon area or the panel; any other place closes it (a cursor in the
    /// panel alone does not open it). With the panel open and the cursor on a cell, the
    /// target is that cell on the island's monitor. Otherwise, with Shift down, it is the
    /// zone under the cursor on the monitor holding the cursor (the nearest monitor if none
    /// does), from that monitor's active layout. Otherwise there is nothing.
    pub fn poll(
        &mut self,
        cursor: (i32, i32),
        shift: bool,
        resizing: bool,
        geo: &Geometry,
    ) -> DragView {
        self.resized |= resizing;
        if self.resized {
            self.panel_open = false;
            self.target = None;
            return DragView::default();
        }
        let in_summon = geo.summon.contains(cursor.0, cursor.1);
        let in_panel = geo.panel.contains(cursor.0, cursor.1);
        self.panel_open = in_summon || (self.panel_open && in_panel);

        let mut view = DragView {
            panel_open: self.panel_open,
            ..DragView::default()
        };
        if self.panel_open && !geo.monitors.is_empty() {
            if let Some((layout, zone)) = panel_cell(cursor, geo) {
                let monitor = geo.island_monitor.min(geo.monitors.len() - 1);
                view.panel_hover = Some((layout, zone));
                view.overlay = Some(OverlayView {
                    monitor,
                    layout,
                    highlight: Some(zone),
                });
                view.target = Some(Target {
                    monitor,
                    layout,
                    zone,
                    from_panel: true,
                });
            }
        }
        if view.target.is_none() && shift {
            if let Some((overlay, target)) = shift_view(cursor, geo) {
                view.overlay = Some(overlay);
                view.target = target;
            }
        }
        self.target = view.target;
        view
    }

    /// The release: the target of the last poll. None for a resize, for nothing under the
    /// cursor, and when `poll` was never called.
    pub fn end(self) -> Option<Target> {
        self.target
    }
}

// ---------------------------------------------------------------------------------------
// Restore size

/// The size each window had before the app first put it in a zone, so a drag out of the
/// zone can give it back. Sizes and rects are `GetWindowRect` space. The app forgets a
/// window when it goes away.
#[derive(Clone, Debug, Default)]
pub struct Placed {
    sizes: HashMap<u64, (i32, i32)>,
}

impl Placed {
    /// The window was just put in a zone; `before` is its `(width, height)` before that.
    /// An already placed window keeps its first size (it is already in a zone).
    pub fn placed(&mut self, window: u64, before: (i32, i32)) {
        self.sizes.entry(window).or_insert(before);
    }

    pub fn is_placed(&self, window: u64) -> bool {
        self.sizes.contains_key(&window)
    }

    /// A move ended outside any zone; `start` and `end` are the window's rects at the start
    /// and at the end of it. For a placed window that really moved: the rect to restore
    /// (the remembered size at the drop position), and the window is forgotten. A changed
    /// size means the user resized it: forget it and return None, the size is theirs. A
    /// window that did not move keeps its memory and gets None.
    pub fn after_move(&mut self, window: u64, start: IRect, end: IRect) -> Option<IRect> {
        if !self.is_placed(window) {
            return None;
        }
        if (start.w, start.h) != (end.w, end.h) {
            self.sizes.remove(&window);
            return None;
        }
        if (start.x, start.y) == (end.x, end.y) {
            return None;
        }
        let (w, h) = self.sizes.remove(&window)?;
        Some(IRect::new(end.x, end.y, w, h))
    }

    pub fn forget(&mut self, window: u64) {
        self.sizes.remove(&window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(name: &str) -> Layout {
        presets().swap_remove(find(&presets(), name).expect(name))
    }

    /// `[[layout]]` text for one layout.
    fn one(name: &str, cols: &str, rows: &str, zones: &str) -> String {
        format!("[[layout]]\nname = {name}\ncols = {cols}\nrows = {rows}\nzones = {zones}\n")
    }

    // --- A1: layouts ---

    #[test]
    fn presets_are_the_nine_in_spec_order() {
        let p = presets();
        let names: Vec<&str> = p.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "50/50",
                "60/40",
                "70/30",
                "top/bottom",
                "3 columns",
                "1+2",
                "wide center",
                "2x2",
                "1+3"
            ]
        );
        let counts: Vec<usize> = p.iter().map(|l| l.zones.len()).collect();
        assert_eq!(counts, [2, 2, 2, 2, 3, 3, 3, 4, 4]);
        assert_eq!(p[0].name, DEFAULT_LAYOUT);
        for l in &p {
            assert_eq!(validate(l), Ok(()), "{}", l.name);
            assert!(l.zones.len() <= MAX_ZONES);
        }
    }

    #[test]
    fn preset_ratios_and_spans() {
        let p = presets();
        assert_eq!(
            (p[1].cols.clone(), p[1].rows.clone()),
            (vec![3.0, 2.0], vec![1.0])
        );
        assert_eq!(p[2].cols, [7.0, 3.0]);
        assert_eq!(
            (p[3].cols.clone(), p[3].rows.clone()),
            (vec![1.0], vec![1.0, 1.0])
        );
        assert_eq!(p[4].cols, [1.0, 1.0, 1.0]);
        assert_eq!(p[6].cols, [1.0, 2.0, 1.0]);
        let one_two = preset("1+2");
        assert_eq!(
            (one_two.cols.clone(), one_two.rows.clone()),
            (vec![1.4, 1.0], vec![1.0, 1.0])
        );
        assert_eq!(
            one_two.zones,
            [span(0, 0, 1, 2), span(1, 0, 1, 1), span(1, 1, 1, 1)]
        );
        let one_three = preset("1+3");
        assert_eq!(
            (one_three.cols.clone(), one_three.rows.clone()),
            (vec![1.6, 1.0], vec![1.0, 1.0, 1.0])
        );
        assert_eq!(
            one_three.zones,
            [
                span(0, 0, 1, 3),
                span(1, 0, 1, 1),
                span(1, 1, 1, 1),
                span(1, 2, 1, 1)
            ]
        );
        assert_eq!(
            preset("2x2").zones,
            [
                span(0, 0, 1, 1),
                span(1, 0, 1, 1),
                span(0, 1, 1, 1),
                span(1, 1, 1, 1)
            ]
        );
        assert_eq!(
            preset("top/bottom").zones,
            [span(0, 0, 1, 1), span(0, 1, 1, 1)]
        );
    }

    #[test]
    fn preset_zones_are_in_reading_order() {
        for l in presets() {
            let keys: Vec<(usize, usize)> = l.zones.iter().map(|s| (s.row, s.col)).collect();
            let mut sorted = keys.clone();
            sorted.sort();
            assert_eq!(keys, sorted, "{}", l.name);
        }
    }

    #[test]
    fn parse_custom_reads_the_spec_example_in_reading_order() {
        // Zones listed out of order come back in reading order.
        let text = "[[layout]]\nname = \"code + docs\"\ncols = [2, 1]\nrows = [1, 1]\nzones = [[1, 1, 1, 1], [0, 0, 1, 2], [1, 0, 1, 1]]\n";
        let (ok, errors) = parse_custom(text);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].name, "code + docs");
        assert_eq!(ok[0].cols, [2.0, 1.0]);
        assert_eq!(
            ok[0].zones,
            [span(0, 0, 1, 2), span(1, 0, 1, 1), span(1, 1, 1, 1)]
        );
        // Floats and surrounding spaces in the name are fine.
        let (ok, errors) = parse_custom(&one(
            "\"  wide  \"",
            "[1.5, 1]",
            "[1]",
            "[[0,0,1,1],[1,0,1,1]]",
        ));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            (ok[0].name.as_str(), ok[0].cols.clone()),
            ("wide", vec![1.5, 1.0])
        );
    }

    #[test]
    fn parse_custom_rejects_each_bad_layout_with_a_reason() {
        let cases: Vec<(String, &str)> = vec![
            (one("\"a\"", "[1, 1]", "[1]", "[[0,0,1,1],[0,0,1,1],[1,0,1,1]]"), "overlap"),
            (one("\"a\"", "[1, 1]", "[1]", "[[0,0,2,1],[1,0,1,1]]"), "overlap"),
            (one("\"a\"", "[1, 1]", "[1]", "[[0,0,1,1],[1,0,2,1]]"), "outside the grid"),
            (one("\"a\"", "[1, 1]", "[1]", "[[0,0,1,1],[1,1,1,1]]"), "outside the grid"),
            (one("\"a\"", "[1, 1]", "[1]", "[[0,0,1,2],[1,0,1,1]]"), "outside the grid"),
            (one("\"a\"", "[1, 1]", "[1]", "[[0,0,1,0],[1,0,1,1]]"), "outside the grid"),
            (one("\"a\"", "[1, 1, 1]", "[1]", "[[0,0,1,1],[2,0,1,1]]"), "hole"),
            (one("\"a\"", "[1, 1]", "[1, 1]", "[[0,0,1,1],[1,0,1,1],[0,1,1,1]]"), "hole"),
            (
                one(
                    "\"a\"",
                    "[1, 1, 1, 1, 1]",
                    "[1, 1]",
                    "[[0,0,1,1],[1,0,1,1],[2,0,1,1],[3,0,1,1],[4,0,1,1],[0,1,1,1],[1,1,1,1],[2,1,1,1],[3,1,1,1],[4,1,1,1]]",
                ),
                "10 zones",
            ),
            (one("\"\"", "[1]", "[1]", "[[0,0,1,1]]"), "name is empty"),
            (one("\"   \"", "[1]", "[1]", "[[0,0,1,1]]"), "name is empty"),
            ("[[layout]]\ncols = [1]\nrows = [1]\nzones = [[0,0,1,1]]\n".to_string(), "name is empty"),
            ("[[layout]]\nname = 5\ncols = [1]\nrows = [1]\nzones = [[0,0,1,1]]\n".to_string(), "name is empty"),
            (one("\"50/50\"", "[1]", "[1]", "[[0,0,1,1]]"), "preset"),
            (one("\" 2X2 \"", "[1]", "[1]", "[[0,0,1,1]]"), "preset"),
            (one("\"a\"", "[1, 0]", "[1]", "[[0,0,1,1],[1,0,1,1]]"), "positive"),
            (one("\"a\"", "[1, -2]", "[1]", "[[0,0,1,1],[1,0,1,1]]"), "positive"),
            (one("\"a\"", "[1]", "[0.0]", "[[0,0,1,1]]"), "positive"),
            (one("\"a\"", "[1]", "[inf]", "[[0,0,1,1]]"), "positive"),
            (one("\"a\"", "[1]", "[nan]", "[[0,0,1,1]]"), "positive"),
            (one("\"a\"", "[1, \"x\"]", "[1]", "[[0,0,1,1]]"), "array of numbers"),
            (one("\"a\"", "5", "[1]", "[[0,0,1,1]]"), "array of numbers"),
            (one("\"a\"", "[]", "[1]", "[[0,0,1,1]]"), "1 to 6"),
            (one("\"a\"", "[1,1,1,1,1,1,1]", "[1]", "[[0,0,1,1]]"), "1 to 6"),
            (one("\"a\"", "[1]", "[]", "[[0,0,1,1]]"), "rows needs"),
            (one("\"a\"", "[1]", "[1]", "[]"), "at least one zone"),
            (one("\"a\"", "[1]", "[1]", "[[0,0,1]]"), "zone 1 must be"),
            (one("\"a\"", "[1]", "[1]", "[[0,0,1,1.5]]"), "zone 1 must be"),
            (one("\"a\"", "[1, 1]", "[1]", "[[0,0,1,1],[-1,0,1,1]]"), "zone 2 must be"),
            (one("\"a\"", "[1]", "[1]", "[\"x\"]"), "zone 1 must be"),
            (one("\"a\"", "[1]", "[1]", "7"), "zones must be an array"),
            ("[[layout]]\nname = \"a\"\ncols = [1]\nrows = [1]\n".to_string(), "zones must be an array"),
        ];
        for (text, why) in &cases {
            let (ok, errors) = parse_custom(text);
            assert!(ok.is_empty(), "accepted: {text}");
            assert_eq!(errors.len(), 1, "{text}: {errors:?}");
            assert!(
                errors[0].contains(why),
                "{text}: wanted {why:?} in {:?}",
                errors[0]
            );
            assert!(errors[0].starts_with("layout 1"), "{:?}", errors[0]);
        }
    }

    #[test]
    fn parse_custom_rejects_a_duplicate_name() {
        let text = format!(
            "{}{}",
            one("\"mine\"", "[1]", "[1]", "[[0,0,1,1]]"),
            one("\"Mine\"", "[1, 1]", "[1]", "[[0,0,1,1],[1,0,1,1]]"),
        );
        let (ok, errors) = parse_custom(&text);
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].cols.len(), 1, "the first one wins");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("layout 2 \"Mine\""), "{errors:?}");
        assert!(errors[0].contains("already used"), "{errors:?}");
    }

    #[test]
    fn one_bad_layout_does_not_take_the_others_down() {
        let text = format!(
            "{}{}{}",
            one("\"first\"", "[1]", "[1]", "[[0,0,1,1]]"),
            one("\"broken\"", "[1, 1]", "[1]", "[[0,0,2,1],[1,0,1,1]]"),
            one("\"third\"", "[1]", "[1, 1]", "[[0,0,1,1],[0,1,1,1]]"),
        );
        let (ok, errors) = parse_custom(&text);
        let names: Vec<&str> = ok.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["first", "third"]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("layout 2 \"broken\""), "{errors:?}");
        // A non-table entry is one more rejection.
        let (ok, errors) = parse_custom("layout = [1, 2]\n");
        assert!(ok.is_empty());
        assert_eq!(errors.len(), 2, "{errors:?}");
    }

    #[test]
    fn a_syntax_error_gives_no_layouts_and_one_error() {
        for text in [
            "[[layout]\nname = \"a\"",
            "[[layout]]\nname = \"a\"\ncols = [1,\n",
            "this is not toml",
            "[[layout]]\nname = \"a\"\nname = \"b\"\n",
        ] {
            let (ok, errors) = parse_custom(text);
            assert!(ok.is_empty(), "{text}");
            assert_eq!(errors.len(), 1, "{text}: {errors:?}");
            assert!(errors[0].contains("syntax error"), "{errors:?}");
        }
        let (_, errors) = parse_custom("\n\nthis is not toml\n");
        assert!(errors[0].contains("line 3"), "{errors:?}");
        // The wrong shape for the whole file is one error too.
        let (ok, errors) = parse_custom("layout = 5\n");
        assert!(ok.is_empty());
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn an_empty_file_or_no_layouts_is_fine() {
        assert_eq!(parse_custom(""), (vec![], vec![]));
        assert_eq!(parse_custom("# nothing here\n"), (vec![], vec![]));
        assert_eq!(parse_custom("title = \"x\"\n"), (vec![], vec![]));
    }

    #[test]
    fn the_picker_shows_the_presets_and_three_custom_layouts() {
        assert_eq!(picker_layouts(&[]), presets());
        let custom: Vec<Layout> = (0..5)
            .map(|i| grid(&format!("c{i}"), &[1.0], &[1.0]))
            .collect();
        let all = picker_layouts(&custom);
        assert_eq!(all.len(), 9 + MAX_CUSTOM);
        assert_eq!(&all[..9], &presets()[..]);
        let names: Vec<&str> = all[9..].iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["c0", "c1", "c2"]);
        assert_eq!(all.len(), MAX_THUMBS);
    }

    #[test]
    fn find_ignores_case_and_surrounding_spaces() {
        let p = presets();
        assert_eq!(find(&p, "2x2"), Some(7));
        assert_eq!(find(&p, " 2X2 "), Some(7));
        assert_eq!(find(&p, "wide center"), Some(6));
        assert_eq!(find(&p, "50/50"), Some(0));
        assert_eq!(find(&p, "nope"), None);
        assert_eq!(find(&p, ""), None);
        assert_eq!(find(&[], "50/50"), None);
    }

    #[test]
    fn active_layouts_fall_back_to_the_default() {
        let layouts = presets();
        let mon = |id: &str| Monitor {
            id: id.into(),
            rect: IRect::new(0, 0, 100, 100),
            work: IRect::new(0, 0, 100, 100),
            scale: 1.0,
        };
        let monitors = [mon("A"), mon("B"), mon("C")];
        let names = BTreeMap::from([
            ("A".to_string(), "1+2".to_string()),
            ("B".to_string(), "deleted layout".to_string()),
        ]);
        assert_eq!(active_layouts(&monitors, &names, &layouts), [5, 0, 0]);
        assert_eq!(active_layouts(&[], &names, &layouts), Vec::<usize>::new());
        assert_eq!(active_layouts(&monitors, &names, &[]), [0, 0, 0]);
    }

    // --- A2: zone math ---

    fn inset(r: IRect, by: i32) -> IRect {
        IRect::new(r.x + by, r.y + by, r.w - 2 * by, r.h - 2 * by)
    }

    /// Zones sit inside `work` inset by `gap`, never overlap, keep at least `gap` between
    /// each other, and together touch all four edges of that inset.
    fn check_zones(l: &Layout, work: IRect, gap: i32) {
        let zs = zones(l, work, gap);
        assert_eq!(zs.len(), l.zones.len(), "{}", l.name);
        let area = inset(work, gap);
        for (i, a) in zs.iter().enumerate() {
            assert!(!a.is_empty(), "{} zone {i}", l.name);
            assert!(
                a.x >= area.x
                    && a.y >= area.y
                    && a.right() <= area.right()
                    && a.bottom() <= area.bottom(),
                "{} zone {i} {a:?} outside {area:?}",
                l.name
            );
            for (j, b) in zs.iter().enumerate().skip(i + 1) {
                assert!(
                    a.intersect(b).is_none(),
                    "{} zones {i} and {j} overlap",
                    l.name
                );
                // Side by side: at least `gap` apart on that axis.
                if a.y < b.bottom() && b.y < a.bottom() {
                    assert!(
                        (a.x - b.right()).max(b.x - a.right()) >= gap,
                        "{} {i}/{j}",
                        l.name
                    );
                }
                if a.x < b.right() && b.x < a.right() {
                    assert!(
                        (a.y - b.bottom()).max(b.y - a.bottom()) >= gap,
                        "{} {i}/{j}",
                        l.name
                    );
                }
            }
        }
        let all = zs.iter().fold(IRect::default(), |u, z| u.union(z));
        assert_eq!(
            all, area,
            "{}: the zones fill the inset area exactly",
            l.name
        );
    }

    #[test]
    fn all_presets_at_1920x1080_with_a_bottom_taskbar() {
        let work = IRect::new(0, 0, 1920, 1032);
        for l in presets() {
            for gap in [0, 8] {
                check_zones(&l, work, gap);
            }
        }
        // Exact numbers, gap 8.
        let z = zones(&preset("50/50"), work, 8);
        assert_eq!(
            z,
            [IRect::new(8, 8, 948, 1016), IRect::new(964, 8, 948, 1016)]
        );
        // 60/40: the first track rounds (1137.6 -> 1138), the last takes the remainder.
        let z = zones(&preset("60/40"), work, 8);
        assert_eq!(
            z,
            [IRect::new(8, 8, 1138, 1016), IRect::new(1154, 8, 758, 1016)]
        );
        assert_eq!(z[1].right(), 1912);
        // 3 columns: 1888 px of content, the last column gets the extra px.
        let z = zones(&preset("3 columns"), work, 8);
        let xs: Vec<(i32, i32)> = z.iter().map(|r| (r.x, r.w)).collect();
        assert_eq!(xs, [(8, 629), (645, 629), (1282, 630)]);
        // top/bottom.
        let z = zones(&preset("top/bottom"), work, 8);
        assert_eq!(
            z,
            [IRect::new(8, 8, 1904, 504), IRect::new(8, 520, 1904, 504)]
        );
        // 1+2: the tall zone spans both rows and the gap between them.
        let z = zones(&preset("1+2"), work, 8);
        assert_eq!(z[0].y, 8);
        assert_eq!(z[0].bottom(), 1024);
        assert_eq!(z[1].y, 8);
        assert_eq!(z[2].bottom(), 1024);
        assert_eq!(z[2].y - z[1].bottom(), 8);
        assert_eq!(z[1].x - z[0].right(), 8);
        assert_eq!(z[1].x, z[2].x);
    }

    #[test]
    fn all_presets_on_an_ultrawide_with_the_appbar_strip() {
        // 3440x1440, the 42 px notch strip on top, a taskbar at the bottom.
        let work = IRect::new(0, 42, 3440, 1350);
        for l in presets() {
            check_zones(&l, work, 8);
        }
        let z = zones(&preset("wide center"), work, 8);
        let xs: Vec<(i32, i32)> = z.iter().map(|r| (r.x, r.w)).collect();
        assert_eq!(xs, [(8, 852), (868, 1704), (2580, 852)]);
        assert!(z.iter().all(|r| r.y == 50 && r.h == 1334));
        let z = zones(&preset("70/30"), work, 8);
        assert_eq!(z[0], IRect::new(8, 50, 2391, 1334));
        assert_eq!(z[1].right(), 3432);
    }

    #[test]
    fn all_presets_on_a_monitor_left_of_the_primary_at_1_5x() {
        // Physical px: x runs -1920..0; the gap is 8 logical px * 1.5.
        let work = IRect::new(-1920, 0, 1920, 1032);
        for l in presets() {
            check_zones(&l, work, 12);
        }
        let z = zones(&preset("50/50"), work, 12);
        assert_eq!(
            z,
            [
                IRect::new(-1908, 12, 942, 1008),
                IRect::new(-954, 12, 942, 1008)
            ]
        );
        assert_eq!(z[1].right(), -12);
        // A monitor above the primary one: negative y.
        let above = IRect::new(0, -1080, 1920, 1080);
        for l in presets() {
            check_zones(&l, above, 8);
        }
        assert_eq!(zones(&preset("top/bottom"), above, 8)[0].y, -1072);
    }

    #[test]
    fn zones_in_logical_px_for_thumbnails() {
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        let z = zones_f(&preset("50/50"), area, 10.0);
        assert_eq!(
            z,
            [
                Rect::new(10.0, 10.0, 185.0, 280.0),
                Rect::new(205.0, 10.0, 185.0, 280.0)
            ]
        );
        for l in presets() {
            let area = Rect::new(-30.5, 12.0, 52.0, 32.0);
            let z = zones_f(&l, area, 2.0);
            assert_eq!(z.len(), l.zones.len());
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for (i, a) in z.iter().enumerate() {
                assert!(a.w > 0.0 && a.h > 0.0, "{} {i}", l.name);
                x0 = x0.min(a.x);
                y0 = y0.min(a.y);
                x1 = x1.max(a.x + a.w);
                y1 = y1.max(a.y + a.h);
                for b in z.iter().skip(i + 1) {
                    let apart = a.x + a.w <= b.x + 1e-3
                        || b.x + b.w <= a.x + 1e-3
                        || a.y + a.h <= b.y + 1e-3
                        || b.y + b.h <= a.y + 1e-3;
                    assert!(apart, "{} overlap {a:?} {b:?}", l.name);
                }
            }
            // The cells fill the area minus the 2 px edge gap.
            assert!((x0 - (area.x + 2.0)).abs() < 1e-3, "{}", l.name);
            assert!((y0 - (area.y + 2.0)).abs() < 1e-3, "{}", l.name);
            assert!((x1 - (area.x + area.w - 2.0)).abs() < 1e-3, "{}", l.name);
            assert!((y1 - (area.y + area.h - 2.0)).abs() < 1e-3, "{}", l.name);
        }
        // 2x2 thumbnail cells.
        let z = zones_f(&preset("2x2"), Rect::new(0.0, 0.0, 52.0, 32.0), 2.0);
        assert_eq!(z[0], Rect::new(2.0, 2.0, 23.0, 13.0));
        assert_eq!(z[3], Rect::new(27.0, 17.0, 23.0, 13.0));
    }

    #[test]
    fn zones_survive_degenerate_input() {
        // No room for the gaps: they collapse instead of going negative.
        let z = zones(&preset("2x2"), IRect::new(5, 5, 10, 10), 8);
        assert!(z.iter().all(|r| r.w >= 0 && r.h >= 0));
        assert_eq!(z.len(), 4);
        // An empty work area and a negative gap.
        let z = zones(&preset("3 columns"), IRect::new(0, 0, 0, 0), -4);
        assert!(z.iter().all(|r| r.w >= 0 && r.h >= 0));
        // A layout with no grid (hand-built) gives empty zones, not a panic.
        let broken = Layout {
            name: "x".into(),
            cols: vec![],
            rows: vec![],
            zones: vec![span(0, 0, 1, 1)],
        };
        assert_eq!(
            zones(&broken, IRect::new(1, 2, 100, 100), 8),
            [IRect::new(1, 2, 0, 0)]
        );
        assert_eq!(
            zones_f(&broken, Rect::new(1.0, 2.0, 100.0, 100.0), 8.0).len(),
            1
        );
        // Bad ratios split evenly.
        let odd = Layout {
            cols: vec![0.0, -1.0],
            ..grid("odd", &[1.0, 1.0], &[1.0])
        };
        let z = zones(&odd, IRect::new(0, 0, 100, 50), 0);
        assert_eq!((z[0].w, z[1].w), (50, 50));
    }

    #[test]
    fn zone_of_matches_within_the_tolerance() {
        let z = zones(&preset("50/50"), IRect::new(0, 0, 1920, 1032), 8);
        assert_eq!(zone_of(&z, z[1], 0), Some(1));
        assert_eq!(zone_of(&z, z[0], 0), Some(0));
        let near = IRect::new(z[0].x + 3, z[0].y - 2, z[0].w - 3, z[0].h + 4);
        assert_eq!(zone_of(&z, near, 4), Some(0));
        assert_eq!(zone_of(&z, near, 2), None);
        let off = IRect::new(z[0].x + 5, z[0].y, z[0].w, z[0].h);
        assert_eq!(zone_of(&z, off, 4), None);
        assert_eq!(zone_of(&z, off, 5), Some(0));
        // A window half in a zone does not fill it.
        assert_eq!(zone_of(&z, IRect::new(8, 8, 500, 500), 4), None);
        assert_eq!(zone_of(&[], z[0], 4), None);
        // A negative tolerance is the same as none.
        assert_eq!(zone_of(&z, z[0], -3), Some(0));
        // Two zones both within the tolerance: the closer wins.
        let close = [IRect::new(0, 0, 100, 100), IRect::new(3, 0, 100, 100)];
        assert_eq!(zone_of(&close, IRect::new(2, 0, 100, 100), 5), Some(1));
        assert_eq!(zone_of(&close, IRect::new(1, 0, 100, 100), 5), Some(0));
    }

    #[test]
    fn nearest_picks_the_closest_centre() {
        let z = zones(&preset("3 columns"), IRect::new(0, 0, 1920, 1080), 8);
        assert_eq!(nearest(&z, IRect::new(100, 100, 400, 300)), Some(0));
        assert_eq!(nearest(&z, IRect::new(800, 100, 300, 300)), Some(1));
        assert_eq!(nearest(&z, IRect::new(1700, 700, 100, 100)), Some(2));
        // Off screen: still the closest.
        assert_eq!(nearest(&z, IRect::new(-5000, 0, 100, 100)), Some(0));
        assert_eq!(nearest(&[], IRect::new(0, 0, 1, 1)), None);
        // A tie goes to the lower index.
        let two = [IRect::new(0, 0, 100, 100), IRect::new(200, 0, 100, 100)];
        assert_eq!(nearest(&two, IRect::new(100, 0, 100, 100)), Some(0));
    }

    #[test]
    fn neighbour_in_2x2_all_directions() {
        // 0 1
        // 2 3
        let z = zones(&preset("2x2"), IRect::new(0, 0, 1920, 1080), 8);
        let n = |from, dir| neighbour(&z, from, dir);
        assert_eq!(n(0, Dir::Right), Some(1));
        assert_eq!(n(0, Dir::Down), Some(2));
        assert_eq!(n(0, Dir::Left), None);
        assert_eq!(n(0, Dir::Up), None);
        assert_eq!(n(1, Dir::Left), Some(0));
        assert_eq!(n(1, Dir::Down), Some(3));
        assert_eq!(n(1, Dir::Right), None);
        assert_eq!(n(1, Dir::Up), None);
        assert_eq!(n(2, Dir::Up), Some(0));
        assert_eq!(n(2, Dir::Right), Some(3));
        assert_eq!(n(2, Dir::Left), None);
        assert_eq!(n(2, Dir::Down), None);
        assert_eq!(n(3, Dir::Up), Some(1));
        assert_eq!(n(3, Dir::Left), Some(2));
        assert_eq!(n(3, Dir::Right), None);
        assert_eq!(n(3, Dir::Down), None);
        assert_eq!(n(4, Dir::Left), None, "no such zone");
    }

    #[test]
    fn neighbour_in_1_plus_2_all_directions() {
        // 0 1
        // 0 2     (zone 0 is the tall one on the left)
        let z = zones(&preset("1+2"), IRect::new(0, 0, 1920, 1080), 8);
        let n = |from, dir| neighbour(&z, from, dir);
        // From the tall zone both right-hand zones are equally near: the upper one.
        assert_eq!(n(0, Dir::Right), Some(1));
        assert_eq!(n(0, Dir::Left), None);
        // Its centre is level with the gap, so up reaches the upper zone and down the lower.
        assert_eq!(n(0, Dir::Up), Some(1));
        assert_eq!(n(0, Dir::Down), Some(2));
        assert_eq!(n(1, Dir::Left), Some(0));
        assert_eq!(n(1, Dir::Down), Some(2));
        assert_eq!(n(1, Dir::Right), None);
        assert_eq!(n(1, Dir::Up), None);
        assert_eq!(n(2, Dir::Left), Some(0));
        assert_eq!(n(2, Dir::Up), Some(1));
        assert_eq!(n(2, Dir::Right), None);
        assert_eq!(n(2, Dir::Down), None);
        // A single zone has no neighbour.
        assert_eq!(neighbour(&[IRect::new(0, 0, 10, 10)], 0, Dir::Right), None);
        assert_eq!(neighbour(&[], 0, Dir::Right), None);
    }

    #[test]
    fn neighbour_walks_a_row_of_three() {
        let z = zones(&preset("wide center"), IRect::new(0, 0, 3440, 1400), 8);
        assert_eq!(neighbour(&z, 0, Dir::Right), Some(1));
        assert_eq!(neighbour(&z, 1, Dir::Right), Some(2));
        assert_eq!(neighbour(&z, 2, Dir::Left), Some(1));
        assert_eq!(neighbour(&z, 1, Dir::Up), None);
        assert_eq!(neighbour(&z, 1, Dir::Down), None);
    }

    #[test]
    fn window_rect_for_corrects_the_invisible_border() {
        // Windows 11: 7 px invisible border left, right and bottom, none on top.
        let window = IRect::new(100, 100, 816, 639);
        let frame = IRect::new(107, 100, 802, 632);
        let zone = IRect::new(8, 50, 948, 1016);
        let r = window_rect_for(zone, window, frame);
        assert_eq!(r, IRect::new(1, 50, 962, 1023));
        // The visible frame inside the new rect is exactly the zone.
        let (l, t) = (frame.x - window.x, frame.y - window.y);
        let (rr, b) = (
            window.right() - frame.right(),
            window.bottom() - frame.bottom(),
        );
        assert_eq!((r.x + l, r.y + t), (zone.x, zone.y));
        assert_eq!(
            (r.right() - rr, r.bottom() - b),
            (zone.right(), zone.bottom())
        );
        // No border (frame == window): the zone itself.
        assert_eq!(window_rect_for(zone, window, window), zone);
        // A monitor at negative coordinates.
        let zone = IRect::new(-1908, 12, 942, 1008);
        assert_eq!(
            window_rect_for(zone, window, frame),
            IRect::new(-1915, 12, 956, 1015)
        );
        // Unusable rects leave the zone alone.
        assert_eq!(window_rect_for(zone, IRect::default(), frame), zone);
        assert_eq!(window_rect_for(zone, window, IRect::new(0, 0, 0, 0)), zone);
    }

    #[test]
    fn monitor_label_counts_from_one() {
        assert_eq!(
            monitor_label(0, IRect::new(0, 0, 3440, 1440)),
            "monitor 1 · 3440×1440"
        );
        assert_eq!(
            monitor_label(1, IRect::new(-1920, 0, 1920, 1080)),
            "monitor 2 · 1920×1080"
        );
    }

    // --- A3: panel geometry ---

    fn inside_panel(r: Rect) -> bool {
        r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= PANEL_W && r.y + r.h <= PANEL_H
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    #[test]
    fn nine_layouts_lay_out_in_rows_of_six_and_three() {
        let layouts = presets();
        let thumbs = panel_thumbs(&layouts);
        assert_eq!(thumbs.len(), 9);
        for (i, t) in thumbs.iter().enumerate() {
            assert_eq!(t.layout, i);
            assert_eq!((t.rect.w, t.rect.h), (52.0, 32.0));
            assert_eq!(t.cells.len(), layouts[i].zones.len());
            assert!(inside_panel(t.rect), "{i}");
            assert!(
                !overlaps(t.rect, PANEL_HEADER) && !overlaps(t.rect, PANEL_FOOTER),
                "{i}"
            );
        }
        // Row one at y 42, row two at y 84; a column step of 52 + 9.
        assert!(thumbs[..6].iter().all(|t| t.rect.y == 42.0));
        assert!(thumbs[6..].iter().all(|t| t.rect.y == 84.0));
        for i in 1..6 {
            assert_eq!(thumbs[i].rect.x - thumbs[i - 1].rect.x, 61.0);
        }
        assert_eq!(
            thumbs[6].rect.x, thumbs[0].rect.x,
            "the short row is left-aligned in the grid"
        );
        // The grid of six is centred (whole px: left margin 21, right 22).
        let left = thumbs[0].rect.x;
        let right = PANEL_W - (thumbs[5].rect.x + thumbs[5].rect.w);
        assert!((left - right).abs() <= 1.0, "{left} vs {right}");
        assert_eq!(left, 21.0);
        // Thumbnails never overlap each other.
        for (i, a) in thumbs.iter().enumerate() {
            for b in &thumbs[i + 1..] {
                assert!(!overlaps(a.rect, b.rect));
            }
        }
    }

    #[test]
    fn twelve_layouts_make_two_full_rows_and_more_are_cut() {
        let custom: Vec<Layout> = (0..3)
            .map(|i| grid(&format!("c{i}"), &[1.0, 1.0], &[1.0]))
            .collect();
        let layouts = picker_layouts(&custom);
        let thumbs = panel_thumbs(&layouts);
        assert_eq!(thumbs.len(), 12);
        assert!(thumbs.iter().all(|t| inside_panel(t.rect)));
        assert!(thumbs[6..].iter().all(|t| t.rect.y == 84.0));
        assert_eq!(thumbs[11].rect.x, thumbs[5].rect.x);
        // Bottom of the second row is above the footer line.
        assert!(thumbs[11].rect.y + thumbs[11].rect.h < PANEL_FOOTER.y);
        // 13 layouts: still 12 thumbnails.
        let mut more = layouts.clone();
        more.push(grid("extra", &[1.0], &[1.0]));
        assert_eq!(panel_thumbs(&more).len(), 12);
        assert_eq!(panel_thumbs(&[]), vec![]);
    }

    #[test]
    fn a_short_list_is_centred() {
        let thumbs = panel_thumbs(&presets()[..3]);
        assert_eq!(thumbs.len(), 3);
        assert!(thumbs.iter().all(|t| t.rect.y == 42.0));
        let left = thumbs[0].rect.x;
        let right = PANEL_W - (thumbs[2].rect.x + thumbs[2].rect.w);
        assert!((left - right).abs() <= 1.0);
        let one = panel_thumbs(&presets()[..1]);
        assert_eq!(one[0].rect.x, 174.0);
    }

    #[test]
    fn thumbnail_cells_stay_inside_their_thumbnail() {
        for t in panel_thumbs(&presets()) {
            for c in &t.cells {
                assert!(c.x >= t.rect.x + 2.0 - 1e-3 && c.y >= t.rect.y + 2.0 - 1e-3);
                assert!(c.x + c.w <= t.rect.x + t.rect.w - 2.0 + 1e-3);
                assert!(c.y + c.h <= t.rect.y + t.rect.h - 2.0 + 1e-3);
            }
        }
        assert_eq!(PANEL_HEADER, Rect::new(20.0, 14.0, 360.0, 16.0));
        assert_eq!(PANEL_FOOTER, Rect::new(20.0, 128.0, 360.0, 18.0));
        const { assert!(PANEL_HEADER.y + PANEL_HEADER.h < THUMBS_TOP) };
        assert!(inside_panel(PANEL_HEADER) && inside_panel(PANEL_FOOTER));
    }

    #[test]
    fn panel_hits_land_on_cells_and_miss_the_gaps() {
        let layouts = presets();
        let thumbs = panel_thumbs(&layouts);
        // The middle of every cell hits it.
        for t in &thumbs {
            for (z, c) in t.cells.iter().enumerate() {
                assert_eq!(
                    panel_hit(&thumbs, c.x + c.w / 2.0, c.y + c.h / 2.0),
                    Some((t.layout, z))
                );
            }
        }
        // 2x2 is layout 7: bottom-right cell is zone 3, top-left zone 0.
        let c = thumbs[7].cells[3];
        assert_eq!(panel_hit(&thumbs, c.x + 1.0, c.y + 1.0), Some((7, 3)));
        // Between two thumbnails (the 9 px gap), above and below the first row.
        let t0 = thumbs[0].rect;
        assert_eq!(panel_hit(&thumbs, t0.x + t0.w + 4.0, t0.y + 16.0), None);
        assert_eq!(panel_hit(&thumbs, t0.x + 10.0, t0.y + t0.h + 5.0), None);
        assert_eq!(panel_hit(&thumbs, t0.x + 10.0, t0.y - 3.0), None);
        // The 2 px gap inside a thumbnail: between the two cells of 50/50 and at its edge.
        let (a, b) = (thumbs[0].cells[0], thumbs[0].cells[1]);
        assert_eq!(
            panel_hit(&thumbs, (a.x + a.w + b.x) / 2.0, t0.y + 16.0),
            None
        );
        assert_eq!(panel_hit(&thumbs, t0.x + 0.5, t0.y + 16.0), None);
        // The header, the footer, the corners and outside the panel.
        assert_eq!(panel_hit(&thumbs, 50.0, 20.0), None);
        assert_eq!(panel_hit(&thumbs, 50.0, 135.0), None);
        assert_eq!(panel_hit(&thumbs, 0.0, 0.0), None);
        assert_eq!(panel_hit(&thumbs, -5.0, 50.0), None);
        assert_eq!(panel_hit(&thumbs, 500.0, 50.0), None);
        assert_eq!(panel_hit(&[], 50.0, 50.0), None);
    }

    // --- A4: the drag ---

    fn mon(id: &str, rect: IRect, work: IRect, scale: f32) -> Monitor {
        Monitor {
            id: id.into(),
            rect,
            work,
            scale,
        }
    }

    struct Rig {
        monitors: Vec<Monitor>,
        layouts: Vec<Layout>,
        active: Vec<usize>,
        summon: IRect,
        panel: IRect,
        scale: f32,
    }

    impl Rig {
        /// Monitor 0: 1920x1080 at the origin with a 42 px strip on top (the island is here);
        /// monitor 1: 1920x1080 at (-1920, 0), 1.5x, a taskbar at the bottom.
        fn new() -> Self {
            Rig {
                monitors: vec![
                    mon(
                        "\\\\.\\DISPLAY1",
                        IRect::new(0, 0, 1920, 1080),
                        IRect::new(0, 42, 1920, 1038),
                        1.0,
                    ),
                    mon(
                        "\\\\.\\DISPLAY2",
                        IRect::new(-1920, 0, 1920, 1080),
                        IRect::new(-1920, 0, 1920, 1032),
                        1.5,
                    ),
                ],
                layouts: presets(),
                active: vec![0, 0],
                summon: IRect::new(680, 44, 560, 80),
                panel: IRect::new(760, 0, 400, 168),
                scale: 1.0,
            }
        }

        fn geo(&self) -> Geometry<'_> {
            Geometry {
                monitors: &self.monitors,
                layouts: &self.layouts,
                active: &self.active,
                island_monitor: 0,
                summon: self.summon,
                panel: self.panel,
                panel_scale: self.scale,
                gap: 8.0,
            }
        }

        /// The desktop px of a cell's middle.
        fn cell(&self, layout: usize, zone: usize) -> (i32, i32) {
            let c = panel_thumbs(&self.layouts)[layout].cells[zone];
            let s = self.scale;
            (
                self.panel.x + ((c.x + c.w / 2.0) * s) as i32,
                self.panel.y + ((c.y + c.h / 2.0) * s) as i32,
            )
        }
    }

    #[test]
    fn the_panel_opens_in_the_summon_area_and_closes_outside_it_and_the_panel() {
        let rig = Rig::new();
        let geo = rig.geo();
        let mut d = Drag::new(7);
        assert_eq!(d.window(), 7);
        // Far from the island: closed, nothing to show.
        assert_eq!(d.poll((300, 600), false, false, &geo), DragView::default());
        // Into the summon area: open.
        let v = d.poll((960, 100), false, false, &geo);
        assert!(v.panel_open);
        assert_eq!((v.panel_hover, v.overlay, v.target), (None, None, None));
        // Up into the panel's own rect (above the summon area): stays open.
        assert!(d.poll((960, 20), false, false, &geo).panel_open);
        // Leaving both: closed.
        let v = d.poll((960, 400), false, false, &geo);
        assert_eq!(v, DragView::default());
        assert_eq!(d.end(), None);
    }

    #[test]
    fn the_panel_rect_alone_does_not_open_it_but_keeps_it_open() {
        let rig = Rig::new();
        let geo = rig.geo();
        // y 20 is in the panel (0..168) but not in the summon area (44..124).
        let mut d = Drag::new(1);
        assert!(!d.poll((960, 20), false, false, &geo).panel_open);
        assert!(
            !d.poll((960, 150), false, false, &geo).panel_open,
            "in the panel, below the summon area"
        );
        // Open it through the summon area, then wander in the panel-only part: still open.
        assert!(d.poll((960, 60), false, false, &geo).panel_open);
        assert!(d.poll((960, 150), false, false, &geo).panel_open);
        // Out of both and back into the panel only: closed stays closed.
        assert!(!d.poll((960, 300), false, false, &geo).panel_open);
        assert!(!d.poll((960, 150), false, false, &geo).panel_open);
        // Re-entering the summon area opens it again.
        assert!(d.poll((700, 100), false, false, &geo).panel_open);
    }

    #[test]
    fn hovering_a_cell_gives_a_target_and_an_overlay_on_the_island_monitor() {
        let rig = Rig::new();
        let geo = rig.geo();
        let mut d = Drag::new(1);
        d.poll((960, 100), false, false, &geo);
        // 1+2 is layout 5, its zone 2 is the lower right one.
        let at = rig.cell(5, 2);
        let v = d.poll(at, false, false, &geo);
        assert!(v.panel_open);
        assert_eq!(v.panel_hover, Some((5, 2)));
        assert_eq!(
            v.overlay,
            Some(OverlayView {
                monitor: 0,
                layout: 5,
                highlight: Some(2)
            })
        );
        let want = Target {
            monitor: 0,
            layout: 5,
            zone: 2,
            from_panel: true,
        };
        assert_eq!(v.target, Some(want));
        assert_eq!(d.end(), Some(want));
    }

    #[test]
    fn the_panel_maps_the_cursor_through_its_scale() {
        let mut rig = Rig::new();
        rig.scale = 1.5;
        rig.panel = IRect::new(660, 0, 600, 252);
        rig.summon = IRect::new(560, 66, 800, 120);
        let geo = rig.geo();
        let mut d = Drag::new(1);
        d.poll((960, 100), false, false, &geo);
        for (layout, zone) in [(0, 1), (7, 3), (8, 0)] {
            let v = d.poll(rig.cell(layout, zone), false, false, &geo);
            assert_eq!(v.panel_hover, Some((layout, zone)), "{layout}/{zone}");
            assert_eq!(v.target.map(|t| (t.layout, t.zone)), Some((layout, zone)));
        }
    }

    #[test]
    fn releasing_over_a_gap_gives_nothing() {
        let rig = Rig::new();
        let geo = rig.geo();
        let mut d = Drag::new(1);
        d.poll((960, 100), false, false, &geo);
        d.poll(rig.cell(0, 0), false, false, &geo);
        // Move over the 9 px gap between the first two thumbnails.
        let t = panel_thumbs(&rig.layouts);
        let gap_x = rig.panel.x + (t[0].rect.x + t[0].rect.w + 4.0) as i32;
        let v = d.poll((gap_x, rig.panel.y + 58), false, false, &geo);
        assert!(v.panel_open);
        assert_eq!((v.panel_hover, v.overlay, v.target), (None, None, None));
        assert_eq!(d.end(), None, "the release over a gap leaves the window");
        // Release over the header: nothing either.
        let mut d = Drag::new(2);
        d.poll((960, 100), false, false, &geo);
        d.poll(rig.cell(0, 0), false, false, &geo);
        d.poll((rig.panel.x + 40, rig.panel.y + 20), false, false, &geo);
        assert_eq!(d.end(), None);
    }

    #[test]
    fn leaving_the_panel_drops_the_target() {
        let rig = Rig::new();
        let geo = rig.geo();
        let mut d = Drag::new(1);
        d.poll((960, 100), false, false, &geo);
        assert!(d.poll(rig.cell(3, 1), false, false, &geo).target.is_some());
        let v = d.poll((960, 500), false, false, &geo);
        assert_eq!(v, DragView::default());
        assert_eq!(d.end(), None);
        // A cell position outside the summon area and the panel never targets.
        let mut d = Drag::new(1);
        assert_eq!(
            d.poll((rig.panel.x + 30, rig.panel.y + 200), false, false, &geo)
                .target,
            None
        );
    }

    #[test]
    fn shift_on_the_second_monitor_shows_its_zones_with_the_right_one_lit() {
        let mut rig = Rig::new();
        // 1+2 is active on monitor 2 (index 1).
        rig.active = vec![0, 5];
        let geo = rig.geo();
        // Gap = round(8 * 1.5) = 12 px on that monitor.
        let want = zones(&presets()[5], IRect::new(-1920, 0, 1920, 1032), 12);
        let mut d = Drag::new(9);
        for (zone, z) in want.iter().enumerate() {
            let cursor = (z.x + z.w / 2, z.y + z.h / 2);
            assert!(cursor.0 < 0, "negative desktop x");
            let v = d.poll(cursor, true, false, &geo);
            assert!(!v.panel_open);
            assert_eq!(v.panel_hover, None);
            assert_eq!(
                v.overlay,
                Some(OverlayView {
                    monitor: 1,
                    layout: 5,
                    highlight: Some(zone)
                })
            );
            assert_eq!(
                v.target,
                Some(Target {
                    monitor: 1,
                    layout: 5,
                    zone,
                    from_panel: false
                })
            );
        }
        assert_eq!(d.end().map(|t| (t.monitor, t.zone)), Some((1, 2)));
    }

    #[test]
    fn shift_released_removes_the_overlay_and_the_target() {
        let rig = Rig::new();
        let geo = rig.geo();
        let mut d = Drag::new(1);
        let v = d.poll((500, 500), true, false, &geo);
        assert!(v.overlay.is_some() && v.target.is_some());
        let v = d.poll((500, 500), false, false, &geo);
        assert_eq!(v, DragView::default());
        assert_eq!(d.end(), None);
    }

    #[test]
    fn shift_over_a_gap_or_the_taskbar_shows_the_overlay_without_a_target() {
        let rig = Rig::new();
        let geo = rig.geo();
        let zs = zones(&presets()[0], rig.monitors[0].work, 8);
        // Between the two zones of 50/50.
        let gap_x = zs[0].right() + 4;
        let mut d = Drag::new(1);
        let v = d.poll((gap_x, 500), true, false, &geo);
        assert_eq!(
            v.overlay,
            Some(OverlayView {
                monitor: 0,
                layout: 0,
                highlight: None
            })
        );
        assert_eq!(v.target, None);
        // Above the work area (the notch strip) is on the monitor but in no zone.
        let v = d.poll((500, 10), true, false, &geo);
        assert_eq!(v.overlay.map(|o| (o.monitor, o.highlight)), Some((0, None)));
        assert_eq!(d.end(), None);
    }

    #[test]
    fn shift_picks_the_nearest_monitor_when_the_cursor_is_between_or_outside() {
        let mut rig = Rig::new();
        // A hole in the desktop between the monitors.
        rig.monitors[1].rect = IRect::new(-2020, 0, 1920, 1080);
        rig.monitors[1].work = IRect::new(-2020, 0, 1920, 1032);
        let geo = rig.geo();
        let mut d = Drag::new(1);
        // x = -40 is in the hole, 40 px from monitor 1 (index 1) and 40... from monitor 0 at x 0.
        assert_eq!(
            d.poll((-30, 500), true, false, &geo)
                .overlay
                .map(|o| o.monitor),
            Some(0)
        );
        assert_eq!(
            d.poll((-70, 500), true, false, &geo)
                .overlay
                .map(|o| o.monitor),
            Some(1)
        );
        // Far outside on either side.
        assert_eq!(
            d.poll((9000, 500), true, false, &geo)
                .overlay
                .map(|o| o.monitor),
            Some(0)
        );
        assert_eq!(
            d.poll((-9000, 500), true, false, &geo)
                .overlay
                .map(|o| o.monitor),
            Some(1)
        );
        assert_eq!(
            d.poll((500, -900), true, false, &geo)
                .overlay
                .map(|o| o.monitor),
            Some(0)
        );
    }

    #[test]
    fn a_cell_under_the_cursor_wins_over_shift() {
        let rig = Rig::new();
        let geo = rig.geo();
        let mut d = Drag::new(1);
        d.poll((960, 100), true, false, &geo);
        let v = d.poll(rig.cell(2, 1), true, false, &geo);
        assert_eq!(
            v.target,
            Some(Target {
                monitor: 0,
                layout: 2,
                zone: 1,
                from_panel: true
            })
        );
        // The panel open but on no cell, Shift down: the zone below (monitor 0, here the
        // strip, so none) is what Shift says.
        let t = panel_thumbs(&rig.layouts);
        let gap_x = rig.panel.x + (t[0].rect.x + t[0].rect.w + 4.0) as i32;
        let v = d.poll((gap_x, rig.panel.y + 58), true, false, &geo);
        assert!(v.panel_open);
        assert_eq!(v.panel_hover, None);
        assert_eq!(v.overlay.map(|o| o.monitor), Some(0));
    }

    #[test]
    fn an_edge_drag_never_opens_the_picker_and_never_snaps() {
        let rig = Rig::new();
        let geo = rig.geo();
        // Resizing from the first sample: in the summon area, on a cell, with Shift.
        let mut d = Drag::new(1);
        for cursor in [(960, 100), rig.cell(0, 0), (500, 500)] {
            let v = d.poll(cursor, true, true, &geo);
            assert_eq!(v, DragView::default(), "{cursor:?}");
        }
        assert_eq!(d.end(), None);
        // A drag that turns into a resize half way: the panel closes, the target goes, and
        // it stays that way even if the size looks unchanged again.
        let mut d = Drag::new(2);
        d.poll((960, 100), false, false, &geo);
        assert!(d.poll(rig.cell(5, 0), false, false, &geo).target.is_some());
        assert_eq!(
            d.poll(rig.cell(5, 0), false, true, &geo),
            DragView::default()
        );
        for shift in [false, true] {
            assert_eq!(
                d.poll(rig.cell(5, 0), shift, false, &geo),
                DragView::default()
            );
        }
        assert_eq!(d.end(), None, "no zone over a cell either");
    }

    #[test]
    fn an_unknown_active_layout_falls_back_to_the_default() {
        let mut rig = Rig::new();
        rig.active = vec![99];
        let geo = rig.geo();
        let mut d = Drag::new(1);
        let v = d.poll((500, 500), true, false, &geo);
        assert_eq!(v.overlay.map(|o| o.layout), Some(0));
        // Monitor 1 has no entry in `active` either.
        let v = d.poll((-500, 500), true, false, &geo);
        assert_eq!(v.overlay.map(|o| (o.monitor, o.layout)), Some((1, 0)));
    }

    #[test]
    fn a_drag_without_monitors_or_layouts_does_nothing() {
        let mut rig = Rig::new();
        rig.monitors.clear();
        let geo = rig.geo();
        let mut d = Drag::new(1);
        let v = d.poll((960, 100), true, false, &geo);
        assert!(v.panel_open);
        assert_eq!((v.overlay, v.target), (None, None));
        let mut rig = Rig::new();
        rig.layouts.clear();
        let geo = rig.geo();
        let v = d.poll((500, 500), true, false, &geo);
        assert_eq!((v.overlay, v.target), (None, None));
        assert_eq!(Drag::new(3).end(), None);
    }

    // --- A5: restore size ---

    #[test]
    fn placed_keeps_the_first_size() {
        let mut p = Placed::default();
        assert!(!p.is_placed(1));
        p.placed(1, (800, 600));
        assert!(p.is_placed(1));
        // Moved from zone to zone: the size from before the first placement stays.
        p.placed(1, (960, 1000));
        let start = IRect::new(10, 10, 960, 1000);
        let end = IRect::new(300, 200, 960, 1000);
        assert_eq!(
            p.after_move(1, start, end),
            Some(IRect::new(300, 200, 800, 600))
        );
        assert!(!p.is_placed(1), "restored windows are forgotten");
        // After a restore a later placement remembers the new size.
        p.placed(1, (800, 600));
        p.placed(1, (111, 222));
        assert_eq!(
            p.after_move(1, start, end),
            Some(IRect::new(300, 200, 800, 600))
        );
    }

    #[test]
    fn after_move_restores_at_the_drop_position() {
        let mut p = Placed::default();
        p.placed(5, (640, 480));
        // Dropped at negative coordinates, same size as at the start.
        let r = p.after_move(
            5,
            IRect::new(8, 8, 948, 1016),
            IRect::new(-700, -20, 948, 1016),
        );
        assert_eq!(r, Some(IRect::new(-700, -20, 640, 480)));
        // A window the app never placed has nothing to restore.
        assert_eq!(
            p.after_move(6, IRect::new(0, 0, 10, 10), IRect::new(5, 5, 10, 10)),
            None
        );
        assert!(!p.is_placed(6));
    }

    #[test]
    fn a_resize_forgets_the_window_and_restores_nothing() {
        let mut p = Placed::default();
        p.placed(5, (640, 480));
        let start = IRect::new(8, 8, 948, 1016);
        for end in [
            IRect::new(8, 8, 1000, 1016),
            IRect::new(8, 8, 948, 900),
            IRect::new(-50, 8, 1006, 1016),
        ] {
            p.placed(5, (640, 480));
            assert_eq!(p.after_move(5, start, end), None, "{end:?}");
            assert!(!p.is_placed(5), "the size is the user's now: {end:?}");
        }
    }

    #[test]
    fn a_window_that_did_not_move_keeps_its_memory() {
        let mut p = Placed::default();
        p.placed(5, (640, 480));
        let r = IRect::new(8, 8, 948, 1016);
        assert_eq!(p.after_move(5, r, r), None);
        assert!(p.is_placed(5));
        assert_eq!(
            p.after_move(5, r, IRect::new(40, 8, 948, 1016)),
            Some(IRect::new(40, 8, 640, 480))
        );
    }

    #[test]
    fn forget_drops_the_memory() {
        let mut p = Placed::default();
        p.placed(1, (1, 1));
        p.placed(2, (2, 2));
        p.forget(1);
        assert!(!p.is_placed(1) && p.is_placed(2));
        p.forget(1);
        p.forget(99);
        assert!(p.is_placed(2));
    }
}
