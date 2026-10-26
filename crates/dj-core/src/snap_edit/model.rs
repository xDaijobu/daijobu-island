//! The annotation model: tools, sizes, colours, shapes, their geometry (hit tests,
//! bounds, the arrow head, pen thinning), text layout through [`TextMeasure`], and the
//! snapshot history.
//!
//! Geometry is in physical desktop px (f32). A shape keeps the DPI scale it was drawn
//! at, so its widths don't change when the selection moves to another monitor.

use crate::color::Rgb;
use crate::snap::{pixel_at, Bitmap, IRect, MonitorFrame};

/// A point in desktop px.
pub type Pt = (f32, f32);

/// Pen points closer than this to the last kept one are dropped.
pub const THIN_PX: f32 = 1.5;
/// Strokes are hit this far (logical px) beyond their half width.
pub const HIT_TOL: f32 = 6.0;
/// Line and arrow drags shorter than this are dropped.
pub const MIN_DRAG: f32 = 3.0;
/// Pixelate blocks are never smaller than this (physical px).
pub const MIN_BLOCK: f32 = 12.0;
/// The history keeps at most this many undo steps.
pub const HISTORY_CAP: usize = 100;
/// The marker's alpha over its colour.
pub const MARKER_ALPHA: f32 = 0.4;
/// The marker is this many times the pen's width.
pub const MARKER_FACTOR: f32 = 4.0;

/// A float rect in desktop px: `x..x + w`, `y..y + h`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl FRect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// The rect spanned by two corners, in any order.
    pub fn from_corners(a: Pt, b: Pt) -> Self {
        let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
        let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
        Self::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn from_irect(r: IRect) -> Self {
        Self::new(r.x as f32, r.y as f32, r.w as f32, r.h as f32)
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center(&self) -> Pt {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Grown by `d` on every side.
    pub fn inflate(&self, d: f32) -> Self {
        Self::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }

    pub fn union(&self, o: &FRect) -> Self {
        let (x0, y0) = (self.x.min(o.x), self.y.min(o.y));
        let (x1, y1) = (self.right().max(o.right()), self.bottom().max(o.bottom()));
        Self::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// The nearest whole-pixel rect (the pixelate grid needs integer corners).
    pub fn to_irect(&self) -> IRect {
        let (x0, y0) = (self.x.round() as i32, self.y.round() as i32);
        let (x1, y1) = (self.right().round() as i32, self.bottom().round() as i32);
        IRect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

/// The editor's tools, in toolbar order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Select,
    Pen,
    Line,
    Arrow,
    Rect,
    Marker,
    Text,
    Pixelate,
    Step,
}

impl Tool {
    pub const ALL: [Tool; 9] = [
        Tool::Select,
        Tool::Pen,
        Tool::Line,
        Tool::Arrow,
        Tool::Rect,
        Tool::Marker,
        Tool::Text,
        Tool::Pixelate,
        Tool::Step,
    ];

    /// The `[snap.annotate] tool` name.
    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => "select",
            Tool::Pen => "pen",
            Tool::Line => "line",
            Tool::Arrow => "arrow",
            Tool::Rect => "rect",
            Tool::Marker => "marker",
            Tool::Text => "text",
            Tool::Pixelate => "pixelate",
            Tool::Step => "step",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        Self::ALL
            .into_iter()
            .find(|t| t.name().eq_ignore_ascii_case(s))
    }

    /// The keyboard shortcut.
    pub fn letter(self) -> char {
        match self {
            Tool::Select => 'V',
            Tool::Pen => 'P',
            Tool::Line => 'L',
            Tool::Arrow => 'A',
            Tool::Rect => 'R',
            Tool::Marker => 'H',
            Tool::Text => 'T',
            Tool::Pixelate => 'X',
            Tool::Step => 'N',
        }
    }

    pub fn from_letter(c: char) -> Option<Self> {
        let c = c.to_ascii_uppercase();
        Self::ALL.into_iter().find(|t| t.letter() == c)
    }
}

/// Stroke, font, step and block sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Size {
    S,
    M,
    L,
}

impl Size {
    pub const ALL: [Size; 3] = [Size::S, Size::M, Size::L];

    pub fn name(self) -> &'static str {
        match self {
            Size::S => "S",
            Size::M => "M",
            Size::L => "L",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        Self::ALL
            .into_iter()
            .find(|z| z.name().eq_ignore_ascii_case(s))
    }

    fn index(self) -> usize {
        self as usize
    }

    /// One size up (`up`) or down, stopping at S and L.
    pub fn step(self, up: bool) -> Self {
        let i = self.index();
        let i = if up {
            (i + 1).min(2)
        } else {
            i.saturating_sub(1)
        };
        Self::ALL[i]
    }

    fn pick(self, v: [f32; 3]) -> f32 {
        v[self.index()]
    }

    /// Pen, line, arrow and rect width (logical px).
    pub fn stroke(self) -> f32 {
        self.pick([2.0, 4.0, 8.0])
    }

    /// Text size (logical px).
    pub fn font(self) -> f32 {
        self.pick([14.0, 20.0, 28.0])
    }

    /// Step circle diameter (logical px).
    pub fn step_diameter(self) -> f32 {
        self.pick([20.0, 28.0, 38.0])
    }

    /// Pixelate block before the 12 px floor (logical px).
    pub fn block(self) -> f32 {
        self.pick([10.0, 14.0, 20.0])
    }
}

/// An 8-bit colour. Shapes are opaque; the marker applies its alpha when drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// `#RRGGBB` (the `#` is optional). Opaque.
    pub fn from_hex(s: &str) -> Option<Self> {
        let [r, g, b] = Rgb::from_hex(s).ok()?.to_u8();
        Some(Self::rgb(r, g, b))
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    pub fn from_rgb(c: Rgb) -> Self {
        let [r, g, b] = c.to_u8();
        Self::rgb(r, g, b)
    }

    pub fn to_rgb(self) -> Rgb {
        Rgb::new(
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
        )
    }
}

/// The fixed swatches; the 8th is the theme accent.
pub const SWATCHES: [Rgba; 7] = [
    Rgba::rgb(0xE5, 0x48, 0x4D),
    Rgba::rgb(0xF5, 0xC4, 0x51),
    Rgba::rgb(0x46, 0xA7, 0x58),
    Rgba::rgb(0x3E, 0x7B, 0xFA),
    Rgba::rgb(0x8E, 0x4E, 0xC6),
    Rgba::rgb(0xFF, 0xFF, 0xFF),
    Rgba::rgb(0x11, 0x11, 0x11),
];
pub const SWATCH_COUNT: usize = 8;
pub const ACCENT_SWATCH: usize = 7;
pub const SWATCH_NAMES: [&str; SWATCH_COUNT] = [
    "red", "yellow", "green", "blue", "purple", "white", "black", "accent",
];
/// The custom colour before the user picks one.
pub const DEFAULT_CUSTOM: Rgba = Rgba::rgb(0xFF, 0x7A, 0xC6);

/// Swatch `i` (0..8); 7 is the accent.
pub fn swatch_color(i: usize, accent: Rgba) -> Rgba {
    SWATCHES.get(i).copied().unwrap_or(accent)
}

/// The current colour: a swatch or the custom colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorChoice {
    Swatch(usize),
    Custom,
}

impl ColorChoice {
    /// The `[snap.annotate] color` name: a swatch name or `custom`.
    pub fn name(self) -> &'static str {
        match self {
            ColorChoice::Swatch(i) => SWATCH_NAMES.get(i).copied().unwrap_or("red"),
            ColorChoice::Custom => "custom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("custom") {
            return Some(ColorChoice::Custom);
        }
        SWATCH_NAMES
            .iter()
            .position(|n| n.eq_ignore_ascii_case(s))
            .map(ColorChoice::Swatch)
    }

    pub fn resolve(self, accent: Rgba, custom: Rgba) -> Rgba {
        match self {
            ColorChoice::Swatch(i) => swatch_color(i, accent),
            ColorChoice::Custom => custom,
        }
    }
}

/// Hue in degrees (0..360), saturation and value in 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsv {
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

impl Hsv {
    pub fn from_rgba(c: Rgba) -> Self {
        let (r, g, b) = (c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let h = if d <= 0.0 {
            0.0
        } else if max == r {
            60.0 * ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        let s = if max <= 0.0 { 0.0 } else { d / max };
        Self { h, s, v: max }
    }

    /// Opaque.
    pub fn to_rgba(self) -> Rgba {
        let h = self.h.rem_euclid(360.0) / 60.0;
        let (s, v) = (self.s.clamp(0.0, 1.0), self.v.clamp(0.0, 1.0));
        let c = v * s;
        let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = v - c;
        let q = |f: f32| ((f + m).clamp(0.0, 1.0) * 255.0).round() as u8;
        Rgba::rgb(q(r), q(g), q(b))
    }
}

/// Black or white, whichever contrasts more with `c` (the step number).
pub fn step_ink(c: Rgba) -> Rgba {
    let (white, black) = (Rgba::rgb(255, 255, 255), Rgba::rgb(0, 0, 0));
    let bg = c.to_rgb();
    if bg.contrast(white.to_rgb()) >= bg.contrast(black.to_rgb()) {
        white
    } else {
        black
    }
}

/// Text sizing. dj-render implements it with Skia's Geist Medium; tests use a fake.
pub trait TextMeasure {
    /// The advance width of one line of `text` at `px` device px.
    fn width(&self, text: &str, px: f32) -> f32;
    /// The distance between baselines at `px`.
    fn line_height(&self, px: f32) -> f32;
}

/// The lines of a text (an empty text has one empty line).
pub fn text_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n')
}

/// The (line, column) of a caret given in chars.
pub fn caret_line_col(text: &str, caret: usize) -> (usize, usize) {
    let mut line = 0;
    let mut col = 0;
    for (i, c) in text.chars().enumerate() {
        if i == caret {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// The char index of (line, column), clamped to the text.
fn index_of(text: &str, line: usize, col: usize) -> usize {
    let mut idx = 0;
    for (i, l) in text_lines(text).enumerate() {
        let n = l.chars().count();
        if i == line {
            return idx + col.min(n);
        }
        idx += n + 1;
    }
    text.chars().count()
}

/// The start and end (in chars) of the line the caret is on.
pub fn line_bounds(text: &str, caret: usize) -> (usize, usize) {
    let (line, _) = caret_line_col(text, caret);
    let start = index_of(text, line, 0);
    let len = text_lines(text).nth(line).map_or(0, |l| l.chars().count());
    (start, start + len)
}

/// The width (widest line) and height (lines x line height) of a text.
pub fn text_size(text: &str, px: f32, m: &dyn TextMeasure) -> (f32, f32) {
    let mut w: f32 = 0.0;
    let mut n = 0;
    for l in text_lines(text) {
        w = w.max(m.width(l, px));
        n += 1;
    }
    (w, n as f32 * m.line_height(px))
}

/// The caret index nearest desktop `(x, y)` in a text whose top-left is `at`.
pub fn caret_at(text: &str, at: Pt, px: f32, m: &dyn TextMeasure, (x, y): Pt) -> usize {
    let lh = m.line_height(px).max(1.0);
    let lines: Vec<&str> = text_lines(text).collect();
    let line = (((y - at.1) / lh).floor().max(0.0) as usize).min(lines.len() - 1);
    let chars: Vec<char> = lines[line].chars().collect();
    let dx = x - at.0;
    let mut best = (0, f32::INFINITY);
    for k in 0..=chars.len() {
        let w = m.width(&chars[..k].iter().collect::<String>(), px);
        let d = (w - dx).abs();
        if d < best.1 {
            best = (k, d);
        }
    }
    index_of(text, line, best.0)
}

/// What a shape is. Geometry in desktop px.
#[derive(Clone, Debug, PartialEq)]
pub enum ShapeKind {
    /// Freehand; one point is a dot.
    Pen(Vec<Pt>),
    /// Like pen, 4x wider, at 40% alpha.
    Marker(Vec<Pt>),
    Line(Pt, Pt),
    /// From the first point to the head at the second.
    Arrow(Pt, Pt),
    /// Outline only.
    Rect(FRect),
    /// `at` is the top-left of the first line.
    Text {
        at: Pt,
        text: String,
    },
    /// Whole-pixel corners; the grid starts at the top-left.
    Pixelate(FRect),
    /// A numbered circle centred on `at`.
    Step {
        at: Pt,
        n: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub kind: ShapeKind,
    pub color: Rgba,
    pub size: Size,
    /// The DPI scale it was drawn at.
    pub scale: f32,
}

/// Where the arrow's shaft ends and its filled head (tip, left, right).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrowGeom {
    pub shaft_end: Pt,
    pub head: [Pt; 3],
}

fn dist(a: Pt, b: Pt) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// The distance from `p` to the segment `a`-`b`.
pub fn dist_to_segment(p: Pt, a: Pt, b: Pt) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        return dist(p, a);
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
    dist(p, (a.0 + t * dx, a.1 + t * dy))
}

fn dist_to_polyline(p: Pt, pts: &[Pt]) -> f32 {
    match pts {
        [] => f32::INFINITY,
        [a] => dist(p, *a),
        _ => pts
            .windows(2)
            .map(|w| dist_to_segment(p, w[0], w[1]))
            .fold(f32::INFINITY, f32::min),
    }
}

/// The arrow head scales with the width: length 3w + 6 (logical), half as wide.
pub fn arrow_geom(a: Pt, b: Pt, width: f32, scale: f32) -> ArrowGeom {
    let len = dist(a, b).max(f32::EPSILON);
    let (ux, uy) = ((b.0 - a.0) / len, (b.1 - a.1) / len);
    let head_len = (3.0 * width + 6.0 * scale).min(len);
    let half = head_len * 0.6;
    let base = (b.0 - ux * head_len, b.1 - uy * head_len);
    // The shaft runs a little into the head so no seam shows.
    let into = head_len * 0.3;
    ArrowGeom {
        shaft_end: (base.0 + ux * into, base.1 + uy * into),
        head: [
            b,
            (base.0 - uy * half, base.1 + ux * half),
            (base.0 + uy * half, base.1 - ux * half),
        ],
    }
}

/// Adds `p` to a pen stroke unless it is within [`THIN_PX`] of the last point.
pub fn push_point(points: &mut Vec<Pt>, p: Pt) -> bool {
    if points.last().is_some_and(|l| dist(*l, p) < THIN_PX) {
        return false;
    }
    points.push(p);
    true
}

/// The next step number: the largest on the canvas plus one.
pub fn next_step(shapes: &[Shape]) -> u32 {
    shapes
        .iter()
        .filter_map(|s| match s.kind {
            ShapeKind::Step { n, .. } => Some(n),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        + 1
}

fn bbox(pts: &[Pt]) -> FRect {
    let mut r = FRect::new(pts[0].0, pts[0].1, 0.0, 0.0);
    for p in &pts[1..] {
        r = r.union(&FRect::new(p.0, p.1, 0.0, 0.0));
    }
    r
}

impl Shape {
    /// The stroke width in device px (the marker's is 4x the pen's).
    pub fn width(&self) -> f32 {
        let w = self.size.stroke() * self.scale;
        match self.kind {
            ShapeKind::Marker(_) => w * MARKER_FACTOR,
            _ => w,
        }
    }

    pub fn font_px(&self) -> f32 {
        self.size.font() * self.scale
    }

    pub fn step_diameter(&self) -> f32 {
        self.size.step_diameter() * self.scale
    }

    /// The pixelate block: max(12 px, 10/14/20 x scale), whole px.
    pub fn block(&self) -> i32 {
        (self.size.block() * self.scale).max(MIN_BLOCK).round() as i32
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        let mv = |p: &mut Pt| {
            p.0 += dx;
            p.1 += dy;
        };
        match &mut self.kind {
            ShapeKind::Pen(pts) | ShapeKind::Marker(pts) => pts.iter_mut().for_each(mv),
            ShapeKind::Line(a, b) | ShapeKind::Arrow(a, b) => {
                mv(a);
                mv(b);
            }
            ShapeKind::Rect(r) | ShapeKind::Pixelate(r) => {
                r.x += dx;
                r.y += dy;
            }
            ShapeKind::Text { at, .. } | ShapeKind::Step { at, .. } => mv(at),
        }
    }

    /// The text box of a text shape.
    pub fn text_rect(&self, m: &dyn TextMeasure) -> Option<FRect> {
        match &self.kind {
            ShapeKind::Text { at, text } => {
                let (w, h) = text_size(text, self.font_px(), m);
                Some(FRect::new(at.0, at.1, w, h))
            }
            _ => None,
        }
    }

    /// What the shape covers on screen (for the selected shape's dashed box).
    pub fn bounds(&self, m: &dyn TextMeasure) -> FRect {
        let half = self.width() / 2.0;
        match &self.kind {
            ShapeKind::Pen(pts) | ShapeKind::Marker(pts) if !pts.is_empty() => {
                bbox(pts).inflate(half)
            }
            ShapeKind::Pen(_) | ShapeKind::Marker(_) => FRect::default(),
            ShapeKind::Line(a, b) => FRect::from_corners(*a, *b).inflate(half),
            ShapeKind::Arrow(a, b) => {
                let g = arrow_geom(*a, *b, self.width(), self.scale);
                bbox(&[*a, g.head[0], g.head[1], g.head[2]]).inflate(half)
            }
            ShapeKind::Rect(r) => r.inflate(half),
            ShapeKind::Text { .. } => self.text_rect(m).unwrap_or_default(),
            ShapeKind::Pixelate(r) => *r,
            ShapeKind::Step { at, .. } => {
                let d = self.step_diameter();
                FRect::new(at.0 - d / 2.0, at.1 - d / 2.0, d, d)
            }
        }
    }

    /// True if desktop `(x, y)` hits the shape: strokes within half their width plus
    /// [`HIT_TOL`], text, steps and pixelate boxes anywhere inside.
    pub fn hit(&self, x: f32, y: f32, m: &dyn TextMeasure) -> bool {
        let p = (x, y);
        let reach = self.width() / 2.0 + HIT_TOL * self.scale;
        match &self.kind {
            ShapeKind::Pen(pts) | ShapeKind::Marker(pts) => dist_to_polyline(p, pts) <= reach,
            ShapeKind::Line(a, b) => dist_to_segment(p, *a, *b) <= reach,
            ShapeKind::Arrow(a, b) => {
                let g = arrow_geom(*a, *b, self.width(), self.scale);
                let head = dist(p, *b) <= dist(g.head[0], g.head[1]) + HIT_TOL * self.scale;
                head || dist_to_segment(p, *a, *b) <= reach
            }
            ShapeKind::Rect(r) => {
                let c = [
                    (r.x, r.y),
                    (r.right(), r.y),
                    (r.right(), r.bottom()),
                    (r.x, r.bottom()),
                    (r.x, r.y),
                ];
                dist_to_polyline(p, &c) <= reach
            }
            ShapeKind::Text { .. } => self
                .text_rect(m)
                .is_some_and(|r| r.inflate(HIT_TOL * self.scale / 2.0).contains(x, y)),
            ShapeKind::Pixelate(r) => r.contains(x, y),
            ShapeKind::Step { at, .. } => dist(p, *at) <= self.step_diameter() / 2.0,
        }
    }

    pub fn is_text(&self) -> bool {
        matches!(self.kind, ShapeKind::Text { .. })
    }
}

/// The topmost shape under desktop `(x, y)`.
pub fn hit_test(shapes: &[Shape], x: f32, y: f32, m: &dyn TextMeasure) -> Option<usize> {
    shapes.iter().rposition(|s| s.hit(x, y, m))
}

/// The pixelate box's blocks, one pixel per block: each takes the frozen screen's pixel
/// at the block's centre (clamped to the box). A gap between monitors is black. The
/// grid starts at the box's top-left, so the overlay and the output agree.
pub fn pixelate_cells(frames: &[MonitorFrame], rect: IRect, block: i32) -> Bitmap {
    let b = block.max(1);
    let (cw, ch) = ((rect.w.max(0) + b - 1) / b, (rect.h.max(0) + b - 1) / b);
    let mut out = Bitmap::black(cw as u32, ch as u32);
    for j in 0..ch {
        for i in 0..cw {
            let x = rect.x + (i * b + b / 2).min(rect.w - 1);
            let y = rect.y + (j * b + b / 2).min(rect.h - 1);
            let px = pixel_at(frames, x, y).unwrap_or([0, 0, 0, 255]);
            let k = ((j * cw + i) * 4) as usize;
            out.pixels[k..k + 4].copy_from_slice(&[px[0], px[1], px[2], 255]);
        }
    }
    out
}

/// Snapshot undo: each step is a whole copy of the shape list.
#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<Vec<Shape>>,
    redo: Vec<Vec<Shape>>,
}

impl History {
    /// Records the state from before a change. Clears redo; keeps the newest 100.
    pub fn record(&mut self, before: Vec<Shape>) {
        self.undo.push(before);
        if self.undo.len() > HISTORY_CAP {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, shapes: &mut Vec<Shape>) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(shapes, prev));
        true
    }

    pub fn redo(&mut self, shapes: &mut Vec<Shape>) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(shapes, next));
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Every char is 0.5 em wide; lines are 1.25 em apart.
    pub struct FakeMeasure;

    impl TextMeasure for FakeMeasure {
        fn width(&self, text: &str, px: f32) -> f32 {
            text.chars().count() as f32 * px * 0.5
        }

        fn line_height(&self, px: f32) -> f32 {
            px * 1.25
        }
    }

    pub fn shape(kind: ShapeKind) -> Shape {
        Shape {
            kind,
            color: SWATCHES[0],
            size: Size::M,
            scale: 1.0,
        }
    }

    #[test]
    fn tools_sizes_and_names() {
        for t in Tool::ALL {
            assert_eq!(Tool::parse(t.name()), Some(t));
            assert_eq!(Tool::from_letter(t.letter()), Some(t));
            assert_eq!(Tool::from_letter(t.letter().to_ascii_lowercase()), Some(t));
        }
        assert_eq!(Tool::parse(" Arrow "), Some(Tool::Arrow));
        assert_eq!(Tool::parse("brush"), None);
        assert_eq!(Tool::from_letter('Q'), None);
        assert_eq!(Size::parse("m"), Some(Size::M));
        assert_eq!(Size::parse("XL"), None);
        assert_eq!(Size::S.step(false), Size::S);
        assert_eq!(Size::S.step(true), Size::M);
        assert_eq!(Size::L.step(true), Size::L);
        assert_eq!(
            (Size::S.stroke(), Size::M.stroke(), Size::L.stroke()),
            (2.0, 4.0, 8.0)
        );
        assert_eq!(Size::L.font(), 28.0);
        assert_eq!(Size::S.step_diameter(), 20.0);
    }

    #[test]
    fn colours_and_choices() {
        assert_eq!(Rgba::from_hex("#E5484D"), Some(SWATCHES[0]));
        assert_eq!(Rgba::from_hex("ff7ac6"), Some(DEFAULT_CUSTOM));
        assert_eq!(Rgba::from_hex("#12"), None);
        assert_eq!(DEFAULT_CUSTOM.to_hex(), "#FF7AC6");
        let accent = Rgba::rgb(1, 2, 3);
        assert_eq!(swatch_color(ACCENT_SWATCH, accent), accent);
        assert_eq!(
            ColorChoice::Swatch(6).resolve(accent, DEFAULT_CUSTOM),
            Rgba::rgb(0x11, 0x11, 0x11)
        );
        assert_eq!(
            ColorChoice::Custom.resolve(accent, DEFAULT_CUSTOM),
            DEFAULT_CUSTOM
        );
        for i in 0..SWATCH_COUNT {
            let c = ColorChoice::Swatch(i);
            assert_eq!(ColorChoice::parse(c.name()), Some(c));
        }
        assert_eq!(ColorChoice::parse("Custom"), Some(ColorChoice::Custom));
        assert_eq!(ColorChoice::parse("pink"), None);
        assert_eq!(step_ink(Rgba::rgb(255, 255, 255)), Rgba::rgb(0, 0, 0));
        assert_eq!(
            step_ink(Rgba::rgb(0x11, 0x11, 0x11)),
            Rgba::rgb(255, 255, 255)
        );
        assert_eq!(step_ink(SWATCHES[1]), Rgba::rgb(0, 0, 0), "yellow");
        assert_eq!(step_ink(SWATCHES[4]), Rgba::rgb(255, 255, 255), "purple");
    }

    #[test]
    fn hsv_round_trips() {
        for c in SWATCHES.into_iter().chain([
            DEFAULT_CUSTOM,
            Rgba::rgb(0, 0, 0),
            Rgba::rgb(255, 0, 0),
            Rgba::rgb(0, 255, 0),
            Rgba::rgb(0, 0, 255),
            Rgba::rgb(12, 200, 99),
            Rgba::rgb(128, 128, 128),
        ]) {
            assert_eq!(Hsv::from_rgba(c).to_rgba(), c, "{c:?}");
        }
        let red = Hsv::from_rgba(Rgba::rgb(255, 0, 0));
        assert_eq!((red.h, red.s, red.v), (0.0, 1.0, 1.0));
        let blue = Hsv::from_rgba(Rgba::rgb(0, 0, 255));
        assert!((blue.h - 240.0).abs() < 1e-3);
        // Hue wraps; out-of-range s/v clamp.
        let h = Hsv {
            h: 480.0,
            s: 2.0,
            v: 1.0,
        };
        assert_eq!(h.to_rgba(), Rgba::rgb(0, 255, 0));
        assert_eq!(
            Hsv {
                h: 200.0,
                s: 0.0,
                v: 1.0
            }
            .to_rgba(),
            Rgba::rgb(255, 255, 255)
        );
    }

    #[test]
    fn text_layout_through_the_measurer() {
        let m = FakeMeasure;
        // 20 px: 10 px per char, 25 px per line.
        assert_eq!(text_size("abc\nde", 20.0, &m), (30.0, 50.0));
        assert_eq!(text_size("", 20.0, &m), (0.0, 25.0));
        assert_eq!(caret_line_col("abc\nde", 0), (0, 0));
        assert_eq!(caret_line_col("abc\nde", 3), (0, 3));
        assert_eq!(caret_line_col("abc\nde", 4), (1, 0));
        assert_eq!(caret_line_col("abc\nde", 6), (1, 2));
        assert_eq!(line_bounds("abc\nde", 5), (4, 6));
        assert_eq!(line_bounds("abc\nde", 1), (0, 3));
        assert_eq!(line_bounds("", 0), (0, 0));
        let at = (100.0, 50.0);
        assert_eq!(caret_at("abc\nde", at, 20.0, &m, (100.0, 55.0)), 0);
        assert_eq!(caret_at("abc\nde", at, 20.0, &m, (114.0, 55.0)), 1);
        assert_eq!(caret_at("abc\nde", at, 20.0, &m, (500.0, 55.0)), 3);
        assert_eq!(caret_at("abc\nde", at, 20.0, &m, (121.0, 80.0)), 6);
        // Below the last line: the last line.
        assert_eq!(caret_at("abc\nde", at, 20.0, &m, (100.0, 400.0)), 4);
        // Multi-byte chars count once.
        assert_eq!(caret_line_col("é😀\nx", 3), (1, 0));
    }

    #[test]
    fn hit_tests_strokes_with_tolerance_and_fills_inside() {
        let m = FakeMeasure;
        // M = 4 px wide: reach 2 + 6 = 8 px.
        let line = shape(ShapeKind::Line((0.0, 0.0), (100.0, 0.0)));
        assert!(line.hit(50.0, 7.9, &m));
        assert!(!line.hit(50.0, 8.5, &m));
        assert!(line.hit(-7.0, 0.0, &m), "past the end, within the reach");
        assert!(!line.hit(-9.0, 0.0, &m));
        // A pen dot.
        let dot = shape(ShapeKind::Pen(vec![(10.0, 10.0)]));
        assert!(dot.hit(16.0, 10.0, &m) && !dot.hit(19.0, 10.0, &m));
        // Rect: only near the outline.
        let rect = shape(ShapeKind::Rect(FRect::new(0.0, 0.0, 100.0, 60.0)));
        assert!(rect.hit(100.0, 30.0, &m));
        assert!(rect.hit(50.0, 66.0, &m));
        assert!(!rect.hit(50.0, 30.0, &m), "the inside is empty");
        // Marker: 16 px wide.
        let marker = shape(ShapeKind::Marker(vec![(0.0, 0.0), (50.0, 0.0)]));
        assert!(marker.hit(25.0, 13.0, &m) && !marker.hit(25.0, 15.0, &m));
        // Filled ones.
        let px = shape(ShapeKind::Pixelate(FRect::new(0.0, 0.0, 40.0, 40.0)));
        assert!(px.hit(20.0, 20.0, &m) && !px.hit(41.0, 20.0, &m));
        let step = shape(ShapeKind::Step {
            at: (50.0, 50.0),
            n: 1,
        });
        assert!(step.hit(63.0, 50.0, &m) && !step.hit(65.0, 50.0, &m));
        // Text bounds come from the measurer: "hey" at 20 px = 30 x 25.
        let text = shape(ShapeKind::Text {
            at: (10.0, 10.0),
            text: "hey".into(),
        });
        assert_eq!(text.text_rect(&m), Some(FRect::new(10.0, 10.0, 30.0, 25.0)));
        assert!(text.hit(39.0, 34.0, &m) && text.hit(41.0, 20.0, &m));
        assert!(!text.hit(45.0, 20.0, &m));
        // Scale widens strokes and the tolerance.
        let mut big = line.clone();
        big.scale = 2.0;
        assert!(big.hit(50.0, 15.9, &m) && !big.hit(50.0, 16.5, &m));
    }

    #[test]
    fn hit_test_takes_the_topmost() {
        let m = FakeMeasure;
        let shapes = vec![
            shape(ShapeKind::Pixelate(FRect::new(0.0, 0.0, 100.0, 100.0))),
            shape(ShapeKind::Line((0.0, 50.0), (100.0, 50.0))),
            shape(ShapeKind::Step {
                at: (50.0, 50.0),
                n: 1,
            }),
        ];
        assert_eq!(hit_test(&shapes, 50.0, 50.0, &m), Some(2));
        assert_eq!(hit_test(&shapes, 20.0, 52.0, &m), Some(1));
        assert_eq!(hit_test(&shapes, 20.0, 20.0, &m), Some(0));
        assert_eq!(hit_test(&shapes, 200.0, 20.0, &m), None);
    }

    #[test]
    fn arrow_head_scales_with_the_width_and_fits_short_arrows() {
        let g = arrow_geom((0.0, 0.0), (100.0, 0.0), 4.0, 1.0);
        // 3 * 4 + 6 = 18 long, 10.8 half-wide.
        assert_eq!(g.head[0], (100.0, 0.0));
        assert!((g.head[1].0 - 82.0).abs() < 1e-4 && (g.head[1].1 - 10.8).abs() < 1e-4);
        assert!((g.head[2].1 + 10.8).abs() < 1e-4);
        assert!(g.shaft_end.0 > 82.0 && g.shaft_end.0 < 100.0);
        let wide = arrow_geom((0.0, 0.0), (100.0, 0.0), 8.0, 1.0);
        assert!(wide.head[1].1 > g.head[1].1);
        // A head never runs past the tail.
        let short = arrow_geom((0.0, 0.0), (5.0, 0.0), 8.0, 1.0);
        assert!(short.head[1].0 >= -1e-4);
    }

    #[test]
    fn pen_points_are_thinned_and_steps_count_up() {
        let mut pts = vec![(0.0, 0.0)];
        assert!(!push_point(&mut pts, (1.0, 1.0)), "1.41 px is too close");
        assert!(push_point(&mut pts, (1.5, 0.0)));
        assert!(!push_point(&mut pts, (2.0, 0.5)));
        assert!(push_point(&mut pts, (3.0, 1.0)));
        assert_eq!(pts.len(), 3);
        let mut shapes = vec![];
        assert_eq!(next_step(&shapes), 1);
        shapes.push(shape(ShapeKind::Step {
            at: (0.0, 0.0),
            n: 4,
        }));
        shapes.push(shape(ShapeKind::Step {
            at: (0.0, 0.0),
            n: 2,
        }));
        assert_eq!(next_step(&shapes), 5);
    }

    #[test]
    fn bounds_and_translate() {
        let m = FakeMeasure;
        let mut s = shape(ShapeKind::Pen(vec![(10.0, 10.0), (30.0, 20.0)]));
        assert_eq!(s.bounds(&m), FRect::new(8.0, 8.0, 24.0, 14.0));
        s.translate(-5.0, 2.5);
        assert_eq!(s.kind, ShapeKind::Pen(vec![(5.0, 12.5), (25.0, 22.5)]));
        let mut r = shape(ShapeKind::Pixelate(FRect::new(1.0, 2.0, 3.0, 4.0)));
        r.translate(1.0, 1.0);
        assert_eq!(r.bounds(&m), FRect::new(2.0, 3.0, 3.0, 4.0));
        let st = shape(ShapeKind::Step {
            at: (50.0, 50.0),
            n: 1,
        });
        assert_eq!(st.bounds(&m), FRect::new(36.0, 36.0, 28.0, 28.0));
    }

    #[test]
    fn pixelate_blocks_have_a_floor_and_sample_block_centres() {
        let mut s = shape(ShapeKind::Pixelate(FRect::new(0.0, 0.0, 10.0, 10.0)));
        s.size = Size::S;
        assert_eq!(s.block(), 12, "10 px is under the floor");
        s.size = Size::L;
        assert_eq!(s.block(), 20);
        s.scale = 1.5;
        assert_eq!(s.block(), 30);
        // A 4 x 3 monitor where pixel (x, y) = [x, y, 7, 255].
        let mut bitmap = Bitmap::black(4, 3);
        for y in 0..3 {
            for x in 0..4 {
                let i = (y * 4 + x) * 4;
                bitmap.pixels[i] = x as u8;
                bitmap.pixels[i + 1] = y as u8;
                bitmap.pixels[i + 2] = 7;
            }
        }
        let frames = [MonitorFrame {
            rect: IRect::new(-2, 0, 4, 3),
            dpi: 96,
            bitmap,
        }];
        // Blocks of 2 over the whole monitor: 2 x 2 cells.
        let c = pixelate_cells(&frames, IRect::new(-2, 0, 4, 3), 2);
        assert_eq!((c.width, c.height), (2, 2));
        assert_eq!(&c.pixels[0..4], &[1, 1, 7, 255]);
        assert_eq!(&c.pixels[4..8], &[3, 1, 7, 255]);
        // The last row is one pixel tall: its centre clamps into the box.
        assert_eq!(&c.pixels[8..12], &[1, 2, 7, 255]);
        // Off the monitor: black.
        let c = pixelate_cells(&frames, IRect::new(2, 0, 2, 2), 2);
        assert_eq!(&c.pixels[0..4], &[0, 0, 0, 255]);
    }

    #[test]
    fn history_undo_redo_cap_and_redo_cleared() {
        let mut h = History::default();
        let mut shapes: Vec<Shape> = vec![];
        let a = shape(ShapeKind::Line((0.0, 0.0), (9.0, 9.0)));
        h.record(shapes.clone());
        shapes.push(a.clone());
        h.record(shapes.clone());
        shapes.push(a.clone());
        assert!(h.undo(&mut shapes));
        assert_eq!(shapes.len(), 1);
        assert!(h.undo(&mut shapes));
        assert!(shapes.is_empty());
        assert!(!h.undo(&mut shapes));
        assert!(h.redo(&mut shapes));
        assert_eq!(shapes.len(), 1);
        // A new step clears redo.
        h.record(shapes.clone());
        shapes.clear();
        assert!(!h.can_redo());
        assert!(!h.redo(&mut shapes));
        // The cap keeps the newest 100.
        let mut h = History::default();
        for i in 0..150 {
            h.record(vec![shape(ShapeKind::Step {
                at: (0.0, 0.0),
                n: i,
            })]);
        }
        assert_eq!(h.undo_len(), HISTORY_CAP);
        let mut cur = vec![];
        while h.undo(&mut cur) {}
        assert_eq!(
            cur[0].kind,
            ShapeKind::Step {
                at: (0.0, 0.0),
                n: 50
            },
            "the oldest 50 were dropped"
        );
    }
}
