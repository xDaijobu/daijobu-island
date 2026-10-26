//! Snap annotations: the editor the overlay switches to when a selection is released.
//!
//! The 7a [`Selector`](crate::snap::Selector) still runs the selecting phase; its
//! release hands the rect to an [`Editor`]. The editor keeps a retained list of shapes
//! with snapshot undo (each step is a whole copy of the list, capped at 100), the tool,
//! colour and size, the selection (resizable by 8 handles, movable), the shape being
//! drawn, the selected shape, the text being typed and the HSV picker. It takes overlay
//! input in desktop px and answers with an [`Outcome`] and a cursor kind.
//!
//! Pure: no Win32, no Skia. Text is measured through [`TextMeasure`].

mod chrome;
mod model;

pub use chrome::*;
pub use model::*;

use crate::snap::IRect;

/// A key, from a Windows virtual-key code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCode {
    Esc,
    Enter,
    Delete,
    Backspace,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    /// `A`..`Z`.
    Letter(char),
    Other,
}

impl KeyCode {
    pub fn from_vk(vk: u16) -> Self {
        match vk {
            0x1B => KeyCode::Esc,
            0x0D => KeyCode::Enter,
            0x2E => KeyCode::Delete,
            0x08 => KeyCode::Backspace,
            0x25 => KeyCode::Left,
            0x26 => KeyCode::Up,
            0x27 => KeyCode::Right,
            0x28 => KeyCode::Down,
            0x24 => KeyCode::Home,
            0x23 => KeyCode::End,
            0x41..=0x5A => KeyCode::Letter(vk as u8 as char),
            _ => KeyCode::Other,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub code: KeyCode,
    pub ctrl: bool,
    pub shift: bool,
}

impl Key {
    pub fn plain(code: KeyCode) -> Self {
        Self {
            code,
            ctrl: false,
            shift: false,
        }
    }

    pub fn ctrl(code: KeyCode) -> Self {
        Self {
            code,
            ctrl: true,
            shift: false,
        }
    }
}

/// What a key means. Tool letters, Enter (copy) and Delete/Backspace (remove a shape)
/// are off while a text is being typed; there Enter is a new line and the editing keys
/// edit the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Escape,
    Copy,
    Save,
    Undo,
    Redo,
    DeleteShape,
    Tool(Tool),
    Newline,
    Backspace,
    DeleteForward,
    CaretLeft,
    CaretRight,
    LineStart,
    LineEnd,
    None,
}

pub fn key_action(k: Key, typing: bool) -> KeyAction {
    use KeyCode as C;
    match (k.code, k.ctrl) {
        (C::Esc, _) => KeyAction::Escape,
        (C::Letter('C'), true) => KeyAction::Copy,
        (C::Letter('S'), true) => KeyAction::Save,
        (C::Letter('Z'), true) if k.shift => KeyAction::Redo,
        (C::Letter('Z'), true) => KeyAction::Undo,
        (C::Letter('Y'), true) => KeyAction::Redo,
        (_, true) => KeyAction::None,
        (C::Enter, _) if typing => KeyAction::Newline,
        (C::Backspace, _) if typing => KeyAction::Backspace,
        (C::Delete, _) if typing => KeyAction::DeleteForward,
        (C::Left, _) if typing => KeyAction::CaretLeft,
        (C::Right, _) if typing => KeyAction::CaretRight,
        (C::Home, _) if typing => KeyAction::LineStart,
        (C::End, _) if typing => KeyAction::LineEnd,
        _ if typing => KeyAction::None,
        (C::Enter, _) => KeyAction::Copy,
        (C::Delete | C::Backspace, _) => KeyAction::DeleteShape,
        (C::Letter(c), _) => Tool::from_letter(c).map_or(KeyAction::None, KeyAction::Tool),
        _ => KeyAction::None,
    }
}

/// What the app does after an input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    Redraw,
    /// Render the shot with its markup and copy it.
    Copy,
    /// The same, plus a save.
    Save,
    Cancel,
}

/// Overlay input, in desktop px.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditInput {
    Press {
        x: i32,
        y: i32,
    },
    Motion {
        x: i32,
        y: i32,
    },
    Release {
        x: i32,
        y: i32,
    },
    /// The second press of a double click.
    DoubleClick {
        x: i32,
        y: i32,
    },
    Key(Key),
    Char(char),
    /// Wheel notches; positive (away from the user) is bigger.
    Wheel(i32),
}

/// The tool, colour and size, remembered across snaps (`[snap.annotate]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefs {
    pub tool: Tool,
    pub color: ColorChoice,
    pub custom: Rgba,
    pub size: Size,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            tool: Tool::Arrow,
            color: ColorChoice::Swatch(0),
            custom: DEFAULT_CUSTOM,
            size: Size::M,
        }
    }
}

/// Where the caret is: in shape `shape` (a text), after `index` chars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caret {
    pub shape: usize,
    pub index: usize,
}

/// The text being typed. Its shape lives in the list (so it draws like any other); the
/// list as it was when typing began is kept so the whole session is one undo step.
#[derive(Clone, Debug)]
struct TextEdit {
    index: usize,
    caret: usize,
    before: Vec<Shape>,
}

#[derive(Clone, Debug)]
enum Drag {
    None,
    /// Drawing the draft from the press point.
    Draw {
        start: (i32, i32),
    },
    /// Moving shape `index`; `before` is the list at the press.
    Shape {
        index: usize,
        last: (i32, i32),
        before: Vec<Shape>,
    },
    Selection {
        start: (i32, i32),
        orig: IRect,
    },
    Handle {
        handle: usize,
        start: (i32, i32),
        orig: IRect,
    },
    /// `before` is set when the drag restyles a selected shape (one step on release).
    Picker {
        part: PickerPart,
        before: Option<Vec<Shape>>,
    },
    /// The press was used up (a button, closing the picker, a step): wait for release.
    Held,
}

/// No text to measure (the editor starts with no shapes).
struct NoText;

impl TextMeasure for NoText {
    fn width(&self, _: &str, _: f32) -> f32 {
        0.0
    }

    fn line_height(&self, px: f32) -> f32 {
        px
    }
}

#[derive(Clone, Debug)]
pub struct Editor {
    desktop: IRect,
    displays: Vec<Display>,
    selection: IRect,
    shapes: Vec<Shape>,
    history: History,
    tool: Tool,
    color: ColorChoice,
    custom: Rgba,
    accent: Rgba,
    size: Size,
    drag: Drag,
    draft: Option<Shape>,
    selected: Option<usize>,
    text: Option<TextEdit>,
    /// The picker is open, at this colour.
    picker: Option<Hsv>,
    cursor: (i32, i32),
    hover: Option<Item>,
    cursor_kind: CursorKind,
}

impl Editor {
    /// Starts editing `selection` (already at least 2 x 2, on the desktop).
    pub fn new(
        selection: IRect,
        desktop: IRect,
        displays: Vec<Display>,
        prefs: Prefs,
        accent: Rgba,
        cursor: (i32, i32),
    ) -> Self {
        let mut e = Self {
            desktop,
            displays,
            selection,
            shapes: Vec::new(),
            history: History::default(),
            tool: prefs.tool,
            color: prefs.color,
            custom: prefs.custom,
            accent,
            size: prefs.size,
            drag: Drag::None,
            draft: None,
            selected: None,
            text: None,
            picker: None,
            cursor,
            hover: None,
            cursor_kind: CursorKind::Arrow,
        };
        e.cursor_kind = e.compute_cursor(&NoText);
        e
    }

    // ---- what the overlay and the output draw ----

    pub fn selection(&self) -> IRect {
        self.selection
    }

    /// The shapes, bottom first. The text being typed is among them.
    pub fn shapes(&self) -> &[Shape] {
        &self.shapes
    }

    /// The shape being drawn (on top of the rest).
    pub fn draft(&self) -> Option<&Shape> {
        self.draft.as_ref()
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn caret(&self) -> Option<Caret> {
        self.text.as_ref().map(|t| Caret {
            shape: t.index,
            index: t.caret,
        })
    }

    pub fn is_typing(&self) -> bool {
        self.text.is_some()
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn color_choice(&self) -> ColorChoice {
        self.color
    }

    /// The current colour, resolved.
    pub fn color(&self) -> Rgba {
        self.color.resolve(self.accent, self.custom)
    }

    pub fn custom(&self) -> Rgba {
        self.custom
    }

    pub fn accent(&self) -> Rgba {
        self.accent
    }

    pub fn prefs(&self) -> Prefs {
        Prefs {
            tool: self.tool,
            color: self.color,
            custom: self.custom,
            size: self.size,
        }
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// The monitor holding most of the selection; its scale sizes new shapes and the UI.
    pub fn display(&self) -> Display {
        main_display(self.selection, &self.displays, self.desktop)
    }

    pub fn scale(&self) -> f32 {
        self.display().scale
    }

    pub fn toolbar(&self) -> Toolbar {
        let d = self.display();
        toolbar(self.selection, d.rect, d.scale)
    }

    /// The picker, if open, with its colour.
    pub fn picker(&self) -> Option<(Picker, Hsv)> {
        self.picker.map(|hsv| (self.picker_layout(), hsv))
    }

    fn picker_layout(&self) -> Picker {
        picker(&self.toolbar(), self.display().rect)
    }

    /// The toolbar button under the pointer.
    pub fn hover(&self) -> Option<Item> {
        self.hover
    }

    pub fn cursor_kind(&self) -> CursorKind {
        self.cursor_kind
    }

    // ---- input ----

    pub fn input(&mut self, ev: EditInput, m: &dyn TextMeasure) -> Outcome {
        let mut out = match ev {
            EditInput::Press { x, y } => self.press(x, y, m),
            EditInput::Motion { x, y } => self.motion(x, y),
            EditInput::Release { x, y } => self.release(x, y),
            EditInput::DoubleClick { x, y } => self.double_click(x, y, m),
            EditInput::Key(k) => self.key(k),
            EditInput::Char(c) => self.char(c),
            EditInput::Wheel(d) => self.wheel(d),
        };
        if matches!(self.drag, Drag::None | Drag::Held) {
            let (x, y) = (self.cursor.0 as f32, self.cursor.1 as f32);
            let hover = self.toolbar().item_at(x, y);
            if hover != self.hover {
                self.hover = hover;
                if out == Outcome::Nothing {
                    out = Outcome::Redraw;
                }
            }
        }
        self.cursor_kind = self.compute_cursor(m);
        out
    }

    fn idle(&self) -> bool {
        matches!(self.drag, Drag::None | Drag::Held)
    }

    fn new_shape(&self, kind: ShapeKind) -> Shape {
        Shape {
            kind,
            color: self.color(),
            size: self.size,
            scale: self.scale(),
        }
    }

    fn press(&mut self, x: i32, y: i32, m: &dyn TextMeasure) -> Outcome {
        self.cursor = (x, y);
        if !matches!(self.drag, Drag::None) {
            return Outcome::Nothing;
        }
        let (fx, fy) = (x as f32, y as f32);

        // The picker takes its own presses; a press outside it (and off the toolbar)
        // only closes it.
        if self.picker.is_some() {
            let p = self.picker_layout();
            if let Some(part) = p.part_at(fx, fy) {
                let restyling = self.text.is_none() && self.selected.is_some();
                self.drag = Drag::Picker {
                    part,
                    before: restyling.then(|| self.shapes.clone()),
                };
                self.pick(part, fx, fy);
                return Outcome::Redraw;
            }
            if p.contains(fx, fy) {
                self.drag = Drag::Held;
                return Outcome::Nothing;
            }
            if !self.toolbar().contains(fx, fy) {
                self.picker = None;
                self.drag = Drag::Held;
                return Outcome::Redraw;
            }
        }

        let bar = self.toolbar();
        if bar.contains(fx, fy) {
            self.drag = Drag::Held;
            return match bar.item_at(fx, fy) {
                Some(item) => self.toolbar_item(item),
                None => Outcome::Nothing,
            };
        }

        // A click in the text being typed moves the caret.
        if let Some(t) = &self.text {
            let s = &self.shapes[t.index];
            if let (Some(r), ShapeKind::Text { at, text }) = (s.text_rect(m), &s.kind) {
                if live_text_rect(r, s, m).contains(fx, fy) {
                    let caret = caret_at(text, *at, s.font_px(), m, (fx, fy));
                    if let Some(t) = self.text.as_mut() {
                        t.caret = caret;
                    }
                    self.drag = Drag::Held;
                    return Outcome::Redraw;
                }
            }
        }

        // Anywhere else ends the text being typed, then the tool acts.
        let committed = self.commit_text();
        let inside = self.selection.contains(x, y);
        let out = match self.tool {
            Tool::Select => self.press_select(x, y, m),
            Tool::Text if inside => self.press_text(fx, fy, m),
            Tool::Step if inside => {
                let before = self.shapes.clone();
                let n = next_step(&self.shapes);
                let s = self.new_shape(ShapeKind::Step { at: (fx, fy), n });
                self.shapes.push(s);
                self.history.record(before);
                self.drag = Drag::Held;
                Outcome::Redraw
            }
            Tool::Pen | Tool::Marker | Tool::Line | Tool::Arrow | Tool::Rect | Tool::Pixelate
                if inside =>
            {
                let p = (fx, fy);
                let kind = match self.tool {
                    Tool::Pen => ShapeKind::Pen(vec![p]),
                    Tool::Marker => ShapeKind::Marker(vec![p]),
                    Tool::Line => ShapeKind::Line(p, p),
                    Tool::Arrow => ShapeKind::Arrow(p, p),
                    Tool::Rect => ShapeKind::Rect(FRect::new(fx, fy, 0.0, 0.0)),
                    _ => ShapeKind::Pixelate(FRect::new(fx, fy, 0.0, 0.0)),
                };
                self.draft = Some(self.new_shape(kind));
                self.drag = Drag::Draw { start: (x, y) };
                Outcome::Redraw
            }
            _ => {
                self.drag = Drag::Held;
                Outcome::Nothing
            }
        };
        if committed && out == Outcome::Nothing {
            Outcome::Redraw
        } else {
            out
        }
    }

    fn press_select(&mut self, x: i32, y: i32, m: &dyn TextMeasure) -> Outcome {
        let (fx, fy) = (x as f32, y as f32);
        let had = self.selected.take().is_some();
        if let Some(handle) = handle_at(self.selection, fx, fy, self.scale()) {
            self.drag = Drag::Handle {
                handle,
                start: (x, y),
                orig: self.selection,
            };
            return Outcome::Redraw;
        }
        let inside = self.selection.contains(x, y);
        if let Some(index) = inside.then(|| hit_test(&self.shapes, fx, fy, m)).flatten() {
            self.selected = Some(index);
            self.drag = Drag::Shape {
                index,
                last: (x, y),
                before: self.shapes.clone(),
            };
            return Outcome::Redraw;
        }
        self.drag = if inside {
            Drag::Selection {
                start: (x, y),
                orig: self.selection,
            }
        } else {
            Drag::Held
        };
        if had {
            Outcome::Redraw
        } else {
            Outcome::Nothing
        }
    }

    fn press_text(&mut self, fx: f32, fy: f32, m: &dyn TextMeasure) -> Outcome {
        self.drag = Drag::Held;
        self.selected = None;
        if let Some(i) = self.text_at(fx, fy, m) {
            self.begin_edit(i, Some((fx, fy)), m);
            return Outcome::Redraw;
        }
        let px = self.size.font() * self.scale();
        let lh = m.line_height(px);
        let before = self.shapes.clone();
        let s = self.new_shape(ShapeKind::Text {
            at: (fx, fy - lh / 2.0),
            text: String::new(),
        });
        self.shapes.push(s);
        self.text = Some(TextEdit {
            index: self.shapes.len() - 1,
            caret: 0,
            before,
        });
        Outcome::Redraw
    }

    /// The topmost text under a point, inside the selection.
    fn text_at(&self, fx: f32, fy: f32, m: &dyn TextMeasure) -> Option<usize> {
        if !self.selection.contains(fx as i32, fy as i32) {
            return None;
        }
        self.shapes
            .iter()
            .rposition(|s| s.is_text() && s.hit(fx, fy, m))
    }

    fn begin_edit(&mut self, index: usize, at: Option<Pt>, m: &dyn TextMeasure) {
        let s = &self.shapes[index];
        let ShapeKind::Text { at: origin, text } = &s.kind else {
            return;
        };
        let caret = match at {
            Some(p) => caret_at(text, *origin, s.font_px(), m, p),
            None => text.chars().count(),
        };
        self.selected = None;
        self.text = Some(TextEdit {
            index,
            caret,
            before: self.shapes.clone(),
        });
    }

    /// Ends the text being typed: an empty one is dropped, and the session becomes one
    /// undo step if it changed anything. True if there was one.
    fn commit_text(&mut self) -> bool {
        let Some(t) = self.text.take() else {
            return false;
        };
        let empty = matches!(
            self.shapes.get(t.index).map(|s| &s.kind),
            Some(ShapeKind::Text { text, .. }) if text.is_empty()
        );
        if empty {
            self.shapes.remove(t.index);
        }
        if self.shapes != t.before {
            self.history.record(t.before);
        }
        true
    }

    fn motion(&mut self, x: i32, y: i32) -> Outcome {
        self.cursor = (x, y);
        let (fx, fy) = (x as f32, y as f32);
        let desktop = self.desktop;
        match &mut self.drag {
            Drag::Draw { start } => {
                let start = (start.0 as f32, start.1 as f32);
                let Some(d) = self.draft.as_mut() else {
                    return Outcome::Nothing;
                };
                let changed = match &mut d.kind {
                    ShapeKind::Pen(pts) | ShapeKind::Marker(pts) => push_point(pts, (fx, fy)),
                    ShapeKind::Line(_, b) | ShapeKind::Arrow(_, b) => {
                        let moved = *b != (fx, fy);
                        *b = (fx, fy);
                        moved
                    }
                    ShapeKind::Rect(r) | ShapeKind::Pixelate(r) => {
                        let n = FRect::from_corners(start, (fx, fy));
                        let moved = *r != n;
                        *r = n;
                        moved
                    }
                    _ => false,
                };
                redraw_if(changed)
            }
            Drag::Shape { index, last, .. } => {
                let (dx, dy) = (x - last.0, y - last.1);
                if (dx, dy) == (0, 0) {
                    return Outcome::Nothing;
                }
                *last = (x, y);
                let i = *index;
                self.shapes[i].translate(dx as f32, dy as f32);
                Outcome::Redraw
            }
            Drag::Selection { start, orig } => {
                let r = move_rect(*orig, x - start.0, y - start.1, desktop);
                let changed = r != self.selection;
                self.selection = r;
                redraw_if(changed)
            }
            Drag::Handle {
                handle,
                start,
                orig,
            } => {
                let r = resize(*orig, *handle, x - start.0, y - start.1, desktop);
                let changed = r != self.selection;
                self.selection = r;
                redraw_if(changed)
            }
            Drag::Picker { part, .. } => {
                let part = *part;
                self.pick(part, fx, fy);
                Outcome::Redraw
            }
            Drag::None | Drag::Held => Outcome::Nothing,
        }
    }

    fn release(&mut self, x: i32, y: i32) -> Outcome {
        if matches!(self.drag, Drag::None) {
            self.cursor = (x, y);
            return Outcome::Nothing;
        }
        let mut out = self.motion(x, y);
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Draw { .. } => {
                if let Some(d) = self.draft.take() {
                    if keep_draft(&d) {
                        let before = self.shapes.clone();
                        self.shapes.push(d);
                        self.history.record(before);
                    }
                }
                out = Outcome::Redraw;
            }
            Drag::Shape { before, .. }
            | Drag::Picker {
                before: Some(before),
                ..
            } if self.shapes != before => self.history.record(before),
            _ => {}
        }
        out
    }

    fn double_click(&mut self, x: i32, y: i32, m: &dyn TextMeasure) -> Outcome {
        let (fx, fy) = (x as f32, y as f32);
        if self.tool == Tool::Select
            && self.text.is_none()
            && self.idle()
            && self.picker.is_none()
            && !self.toolbar().contains(fx, fy)
            && handle_at(self.selection, fx, fy, self.scale()).is_none()
        {
            let top = self
                .selection
                .contains(x, y)
                .then(|| hit_test(&self.shapes, fx, fy, m))
                .flatten();
            if let Some(i) = top.filter(|&i| self.shapes[i].is_text()) {
                self.cursor = (x, y);
                self.begin_edit(i, Some((fx, fy)), m);
                self.drag = Drag::Held;
                return Outcome::Redraw;
            }
        }
        self.drag = Drag::None;
        self.press(x, y, m)
    }

    /// Esc, step by step: abort a drag, commit the text, close the picker, drop the
    /// shape selection, then cancel the snap.
    fn escape(&mut self) -> Outcome {
        if self.commit_text() || self.picker.take().is_some() || self.selected.take().is_some() {
            Outcome::Redraw
        } else {
            Outcome::Cancel
        }
    }

    /// Esc during a drag puts things back as they were at the press.
    fn abort_drag(&mut self) {
        match std::mem::replace(&mut self.drag, Drag::Held) {
            Drag::Draw { .. } => self.draft = None,
            Drag::Shape { before, .. } => self.shapes = before,
            Drag::Selection { orig, .. } | Drag::Handle { orig, .. } => self.selection = orig,
            Drag::Picker {
                before: Some(before),
                ..
            } if self.shapes != before => self.history.record(before),
            _ => {}
        }
    }

    fn key(&mut self, k: Key) -> Outcome {
        let action = key_action(k, self.text.is_some());
        if !self.idle() {
            return if action == KeyAction::Escape {
                self.abort_drag();
                Outcome::Redraw
            } else {
                Outcome::Nothing
            };
        }
        match action {
            KeyAction::Escape => self.escape(),
            KeyAction::Copy => {
                self.commit_text();
                Outcome::Copy
            }
            KeyAction::Save => {
                self.commit_text();
                Outcome::Save
            }
            KeyAction::Undo => {
                let committed = self.commit_text();
                redraw_if(self.undo() || committed)
            }
            KeyAction::Redo => {
                let committed = self.commit_text();
                redraw_if(self.redo() || committed)
            }
            KeyAction::DeleteShape => match self.selected.take() {
                Some(i) => {
                    let before = self.shapes.clone();
                    self.shapes.remove(i);
                    self.history.record(before);
                    Outcome::Redraw
                }
                None => Outcome::Nothing,
            },
            KeyAction::Tool(t) => self.set_tool(t),
            KeyAction::Newline => self.edit_text(|text, caret| insert(text, caret, '\n')),
            KeyAction::Backspace => self.edit_text(|text, caret| {
                if *caret > 0 {
                    *caret -= 1;
                    remove(text, *caret);
                }
            }),
            KeyAction::DeleteForward => self.edit_text(|text, caret| remove(text, *caret)),
            KeyAction::CaretLeft => self.edit_text(|_, caret| *caret = caret.saturating_sub(1)),
            KeyAction::CaretRight => self.edit_text(|text, caret| {
                *caret = (*caret + 1).min(text.chars().count());
            }),
            KeyAction::LineStart => {
                self.edit_text(|text, caret| *caret = line_bounds(text, *caret).0)
            }
            KeyAction::LineEnd => {
                self.edit_text(|text, caret| *caret = line_bounds(text, *caret).1)
            }
            KeyAction::None => Outcome::Nothing,
        }
    }

    fn char(&mut self, c: char) -> Outcome {
        if c.is_control() {
            return Outcome::Nothing;
        }
        self.edit_text(|text, caret| insert(text, caret, c))
    }

    fn edit_text(&mut self, f: impl FnOnce(&mut String, &mut usize)) -> Outcome {
        let Some(t) = self.text.as_mut() else {
            return Outcome::Nothing;
        };
        let Some(Shape {
            kind: ShapeKind::Text { text, .. },
            ..
        }) = self.shapes.get_mut(t.index)
        else {
            return Outcome::Nothing;
        };
        f(text, &mut t.caret);
        Outcome::Redraw
    }

    fn wheel(&mut self, d: i32) -> Outcome {
        if d == 0 || !self.idle() {
            return Outcome::Nothing;
        }
        let before = self.size;
        self.apply_size(self.size.step(d > 0));
        redraw_if(self.size != before)
    }

    fn set_tool(&mut self, t: Tool) -> Outcome {
        self.commit_text();
        self.tool = t;
        self.selected = None;
        Outcome::Redraw
    }

    fn undo(&mut self) -> bool {
        self.selected = None;
        self.draft = None;
        self.history.undo(&mut self.shapes)
    }

    fn redo(&mut self) -> bool {
        self.selected = None;
        self.draft = None;
        self.history.redo(&mut self.shapes)
    }

    /// Applies `f` to the text being typed (part of its session) or to the selected
    /// shape (one undo step if it changed).
    fn restyle(&mut self, f: impl Fn(&mut Shape)) {
        if let Some(t) = &self.text {
            f(&mut self.shapes[t.index]);
        } else if let Some(i) = self.selected {
            let before = self.shapes.clone();
            f(&mut self.shapes[i]);
            if self.shapes != before {
                self.history.record(before);
            }
        }
    }

    fn apply_size(&mut self, size: Size) {
        self.size = size;
        self.restyle(|s| s.size = size);
    }

    fn apply_color(&mut self, choice: ColorChoice) {
        self.color = choice;
        let c = self.color();
        self.restyle(|s| recolor(s, c));
    }

    fn toolbar_item(&mut self, item: Item) -> Outcome {
        if item != Item::Custom {
            self.picker = None;
        }
        match item {
            Item::Tool(t) => {
                // A click on the active tool goes back to select.
                let t = if t == self.tool { Tool::Select } else { t };
                self.set_tool(t)
            }
            Item::Swatch(i) => {
                self.apply_color(ColorChoice::Swatch(i));
                Outcome::Redraw
            }
            Item::Custom => {
                if self.picker.take().is_none() {
                    self.picker = Some(Hsv::from_rgba(self.custom));
                }
                self.apply_color(ColorChoice::Custom);
                Outcome::Redraw
            }
            Item::Size(s) => {
                self.apply_size(s);
                Outcome::Redraw
            }
            Item::Undo => {
                self.commit_text();
                self.undo();
                Outcome::Redraw
            }
            Item::Redo => {
                self.commit_text();
                self.redo();
                Outcome::Redraw
            }
            Item::Copy => {
                self.commit_text();
                Outcome::Copy
            }
            Item::Save => {
                self.commit_text();
                Outcome::Save
            }
            Item::Close => Outcome::Cancel,
        }
    }

    /// A picker drag at a point: the custom colour follows, live on the text being typed
    /// or the selected shape.
    fn pick(&mut self, part: PickerPart, fx: f32, fy: f32) {
        let Some(mut hsv) = self.picker else {
            return;
        };
        let p = self.picker_layout();
        match part {
            PickerPart::Sv => (hsv.s, hsv.v) = p.sv_at(fx, fy),
            PickerPart::Hue => hsv.h = p.hue_at(fy),
        }
        self.picker = Some(hsv);
        self.custom = hsv.to_rgba();
        self.color = ColorChoice::Custom;
        let c = self.custom;
        if let Some(i) = self.text.as_ref().map(|t| t.index).or(self.selected) {
            recolor(&mut self.shapes[i], c);
        }
    }

    fn compute_cursor(&self, m: &dyn TextMeasure) -> CursorKind {
        match &self.drag {
            Drag::Handle { handle, .. } => return handle_cursor(*handle),
            Drag::Shape { .. } | Drag::Selection { .. } => return CursorKind::Move,
            Drag::Draw { .. } => return CursorKind::Cross,
            Drag::Picker { .. } => return CursorKind::Arrow,
            Drag::None | Drag::Held => {}
        }
        let (x, y) = self.cursor;
        let (fx, fy) = (x as f32, y as f32);
        if self.picker.is_some() && self.picker_layout().contains(fx, fy) {
            return CursorKind::Arrow;
        }
        if self.toolbar().contains(fx, fy) {
            return CursorKind::Arrow;
        }
        if let Some(t) = &self.text {
            let s = &self.shapes[t.index];
            if s.text_rect(m)
                .is_some_and(|r| live_text_rect(r, s, m).contains(fx, fy))
            {
                return CursorKind::IBeam;
            }
        }
        let inside = self.selection.contains(x, y);
        match self.tool {
            Tool::Select => match handle_at(self.selection, fx, fy, self.scale()) {
                Some(h) => handle_cursor(h),
                None if inside => CursorKind::Move,
                None => CursorKind::Arrow,
            },
            Tool::Text if inside => CursorKind::IBeam,
            _ if inside => CursorKind::Cross,
            _ => CursorKind::Arrow,
        }
    }
}

fn redraw_if(changed: bool) -> Outcome {
    if changed {
        Outcome::Redraw
    } else {
        Outcome::Nothing
    }
}

/// Pixelate has no colour; everything else takes `c`.
fn recolor(s: &mut Shape, c: Rgba) {
    if !matches!(s.kind, ShapeKind::Pixelate(_)) {
        s.color = c;
    }
}

/// The area that counts as "in" the text being typed: its box, at least two line
/// heights wide so an empty text can be clicked, plus a little slack.
pub fn live_text_rect(r: FRect, s: &Shape, m: &dyn TextMeasure) -> FRect {
    let lh = m.line_height(s.font_px());
    FRect::new(r.x, r.y, r.w.max(lh * 2.0), r.h).inflate(HIT_TOL * s.scale / 2.0)
}

/// Line and arrow drags under 3 px, and rect/pixelate boxes under 3 px both ways, are
/// dropped; so is a pixelate box with no area. Pen and marker always stay (one point
/// is a dot).
fn keep_draft(d: &Shape) -> bool {
    match &d.kind {
        ShapeKind::Line(a, b) | ShapeKind::Arrow(a, b) => {
            ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt() >= MIN_DRAG
        }
        ShapeKind::Rect(r) => r.w.max(r.h) >= MIN_DRAG,
        ShapeKind::Pixelate(r) => r.w.max(r.h) >= MIN_DRAG && r.w.min(r.h) >= 1.0,
        _ => true,
    }
}

fn byte_at(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(b, _)| b)
}

fn insert(text: &mut String, caret: &mut usize, c: char) {
    text.insert(byte_at(text, *caret), c);
    *caret += 1;
}

fn remove(text: &mut String, at: usize) {
    if at < text.chars().count() {
        text.remove(byte_at(text, at));
    }
}

#[cfg(test)]
mod tests {
    use super::model::tests::FakeMeasure;
    use super::*;

    const DESK: IRect = IRect::new(-1920, 0, 3840, 1080);
    const SEL: IRect = IRect::new(100, 100, 400, 300);
    const ACCENT: Rgba = Rgba::rgb(0xE0, 0xA4, 0x58);
    const M: FakeMeasure = FakeMeasure;

    fn displays() -> Vec<Display> {
        vec![
            Display {
                rect: IRect::new(0, 0, 1920, 1080),
                scale: 1.0,
            },
            Display {
                rect: IRect::new(-1920, 0, 1920, 1080),
                scale: 2.0,
            },
        ]
    }

    fn editor(tool: Tool) -> Editor {
        Editor::new(
            SEL,
            DESK,
            displays(),
            Prefs {
                tool,
                ..Prefs::default()
            },
            ACCENT,
            (0, 0),
        )
    }

    fn press(e: &mut Editor, x: i32, y: i32) -> Outcome {
        e.input(EditInput::Press { x, y }, &M)
    }

    fn drag(e: &mut Editor, from: (i32, i32), to: (i32, i32)) -> Outcome {
        press(e, from.0, from.1);
        e.input(
            EditInput::Motion {
                x: (from.0 + to.0) / 2,
                y: (from.1 + to.1) / 2,
            },
            &M,
        );
        e.input(EditInput::Motion { x: to.0, y: to.1 }, &M);
        e.input(EditInput::Release { x: to.0, y: to.1 }, &M)
    }

    fn click(e: &mut Editor, x: i32, y: i32) -> Outcome {
        let out = press(e, x, y);
        e.input(EditInput::Release { x, y }, &M);
        out
    }

    fn button(e: &mut Editor, item: Item) -> Outcome {
        let (x, y) = e.toolbar().button(item).unwrap().center();
        click(e, x as i32, y as i32)
    }

    fn key(e: &mut Editor, k: Key) -> Outcome {
        e.input(EditInput::Key(k), &M)
    }

    fn letter(c: char) -> KeyCode {
        KeyCode::Letter(c)
    }

    fn typed(e: &mut Editor, s: &str) {
        for c in s.chars() {
            e.input(EditInput::Char(c), &M);
        }
    }

    fn text_of(e: &Editor, i: usize) -> &str {
        match &e.shapes()[i].kind {
            ShapeKind::Text { text, .. } => text,
            k => panic!("not a text: {k:?}"),
        }
    }

    #[test]
    fn key_codes_and_actions() {
        assert_eq!(KeyCode::from_vk(0x1B), KeyCode::Esc);
        assert_eq!(KeyCode::from_vk(0x56), letter('V'));
        assert_eq!(KeyCode::from_vk(0x24), KeyCode::Home);
        assert_eq!(KeyCode::from_vk(0x70), KeyCode::Other);
        let k = |code| Key::plain(code);
        // Tool letters work, except while typing.
        assert_eq!(
            key_action(k(letter('P')), false),
            KeyAction::Tool(Tool::Pen)
        );
        assert_eq!(
            key_action(k(letter('H')), false),
            KeyAction::Tool(Tool::Marker)
        );
        assert_eq!(
            key_action(k(letter('N')), false),
            KeyAction::Tool(Tool::Step)
        );
        assert_eq!(key_action(k(letter('Q')), false), KeyAction::None);
        assert_eq!(key_action(k(letter('P')), true), KeyAction::None);
        // Enter copies; while typing it is a new line.
        assert_eq!(key_action(k(KeyCode::Enter), false), KeyAction::Copy);
        assert_eq!(key_action(k(KeyCode::Enter), true), KeyAction::Newline);
        assert_eq!(
            key_action(k(KeyCode::Backspace), false),
            KeyAction::DeleteShape
        );
        assert_eq!(
            key_action(k(KeyCode::Backspace), true),
            KeyAction::Backspace
        );
        assert_eq!(
            key_action(k(KeyCode::Delete), true),
            KeyAction::DeleteForward
        );
        assert_eq!(key_action(k(KeyCode::Left), false), KeyAction::None);
        // Ctrl shortcuts work either way.
        for typing in [false, true] {
            assert_eq!(key_action(Key::ctrl(letter('C')), typing), KeyAction::Copy);
            assert_eq!(key_action(Key::ctrl(letter('S')), typing), KeyAction::Save);
            assert_eq!(key_action(Key::ctrl(letter('Z')), typing), KeyAction::Undo);
            assert_eq!(key_action(Key::ctrl(letter('Y')), typing), KeyAction::Redo);
            let redo = Key {
                code: letter('Z'),
                ctrl: true,
                shift: true,
            };
            assert_eq!(key_action(redo, typing), KeyAction::Redo);
            assert_eq!(key_action(Key::ctrl(letter('P')), typing), KeyAction::None);
            assert_eq!(key_action(k(KeyCode::Esc), typing), KeyAction::Escape);
        }
    }

    #[test]
    fn the_editor_starts_from_the_prefs_and_reports_them() {
        let p = Prefs {
            tool: Tool::Marker,
            color: ColorChoice::Custom,
            custom: Rgba::rgb(1, 2, 3),
            size: Size::L,
        };
        let e = Editor::new(SEL, DESK, displays(), p, ACCENT, (0, 0));
        assert_eq!(e.prefs(), p);
        assert_eq!(e.color(), Rgba::rgb(1, 2, 3));
        assert_eq!(e.selection(), SEL);
        assert!(e.shapes().is_empty() && !e.can_undo());
        let mut e = editor(Tool::Pen);
        e.input(EditInput::Key(Key::plain(letter('R'))), &M);
        button(&mut e, Item::Swatch(ACCENT_SWATCH));
        e.input(EditInput::Wheel(-1), &M);
        assert_eq!(
            e.prefs(),
            Prefs {
                tool: Tool::Rect,
                color: ColorChoice::Swatch(7),
                custom: DEFAULT_CUSTOM,
                size: Size::S,
            }
        );
        assert_eq!(e.color(), ACCENT);
    }

    #[test]
    fn pen_and_marker_draw_strokes_and_a_click_leaves_a_dot() {
        let mut e = editor(Tool::Pen);
        assert_eq!(drag(&mut e, (150, 150), (250, 200)), Outcome::Redraw);
        assert_eq!(e.shapes().len(), 1);
        let ShapeKind::Pen(pts) = &e.shapes()[0].kind else {
            panic!()
        };
        assert_eq!(pts, &vec![(150.0, 150.0), (200.0, 175.0), (250.0, 200.0)]);
        assert_eq!(e.shapes()[0].width(), 4.0);
        // A click: one point, kept as a dot.
        click(&mut e, 300, 300);
        assert_eq!(e.shapes()[1].kind, ShapeKind::Pen(vec![(300.0, 300.0)]));
        // Jitter under 1.5 px doesn't add points (the dot stays a dot).
        press(&mut e, 310, 310);
        e.input(EditInput::Motion { x: 311, y: 311 }, &M);
        e.input(EditInput::Release { x: 311, y: 311 }, &M);
        assert_eq!(e.shapes()[2].kind, ShapeKind::Pen(vec![(310.0, 310.0)]));
        // Marker: the same, 4x the width.
        let mut e = editor(Tool::Marker);
        drag(&mut e, (150, 150), (250, 150));
        assert!(matches!(e.shapes()[0].kind, ShapeKind::Marker(_)));
        assert_eq!(e.shapes()[0].width(), 16.0);
    }

    #[test]
    fn line_arrow_rect_and_pixelate_drop_drags_under_three_px() {
        for tool in [Tool::Line, Tool::Arrow] {
            let mut e = editor(tool);
            drag(&mut e, (150, 150), (152, 151));
            assert!(e.shapes().is_empty(), "{tool:?}: 2.2 px is too short");
            assert!(!e.can_undo());
            drag(&mut e, (150, 150), (153, 150));
            assert_eq!(e.shapes().len(), 1);
            let want = if tool == Tool::Line {
                ShapeKind::Line((150.0, 150.0), (153.0, 150.0))
            } else {
                ShapeKind::Arrow((150.0, 150.0), (153.0, 150.0))
            };
            assert_eq!(e.shapes()[0].kind, want);
        }
        let mut e = editor(Tool::Rect);
        click(&mut e, 200, 200);
        assert!(e.shapes().is_empty());
        // Dragged up-left: normalised.
        drag(&mut e, (300, 300), (200, 250));
        assert_eq!(
            e.shapes()[0].kind,
            ShapeKind::Rect(FRect::new(200.0, 250.0, 100.0, 50.0))
        );
        let mut e = editor(Tool::Pixelate);
        drag(&mut e, (200, 200), (260, 200));
        assert!(e.shapes().is_empty(), "no height");
        drag(&mut e, (200, 200), (260, 230));
        assert_eq!(
            e.shapes()[0].kind,
            ShapeKind::Pixelate(FRect::new(200.0, 200.0, 60.0, 30.0))
        );
    }

    #[test]
    fn drawing_starts_only_inside_the_selection() {
        let mut e = editor(Tool::Pen);
        drag(&mut e, (50, 50), (200, 200));
        assert!(e.shapes().is_empty());
        // Starting inside and leaving it is fine (clipped when drawn).
        drag(&mut e, (450, 350), (700, 700));
        assert_eq!(e.shapes().len(), 1);
        let mut e = editor(Tool::Step);
        click(&mut e, 99, 150);
        click(&mut e, 500, 150);
        assert!(e.shapes().is_empty());
    }

    #[test]
    fn steps_count_up_and_an_undo_gives_the_number_back() {
        let mut e = editor(Tool::Step);
        click(&mut e, 150, 150);
        click(&mut e, 200, 150);
        click(&mut e, 250, 150);
        let n = |e: &Editor, i: usize| match e.shapes()[i].kind {
            ShapeKind::Step { n, .. } => n,
            _ => panic!(),
        };
        assert_eq!((n(&e, 0), n(&e, 1), n(&e, 2)), (1, 2, 3));
        key(&mut e, Key::ctrl(letter('Z')));
        assert_eq!(e.shapes().len(), 2);
        click(&mut e, 300, 150);
        assert_eq!(n(&e, 2), 3);
        // Deleting the middle one doesn't renumber; the next is still largest + 1.
        key(&mut e, Key::plain(letter('V')));
        click(&mut e, 200, 150);
        key(&mut e, Key::plain(KeyCode::Delete));
        key(&mut e, Key::plain(letter('N')));
        click(&mut e, 350, 150);
        assert_eq!(n(&e, 2), 4);
        assert_eq!(e.shapes()[0].step_diameter(), 28.0);
    }

    #[test]
    fn new_shapes_take_the_scale_of_the_selections_monitor() {
        let mut e = Editor::new(
            IRect::new(-600, 100, 400, 300),
            DESK,
            displays(),
            Prefs {
                tool: Tool::Pen,
                ..Prefs::default()
            },
            ACCENT,
            (0, 0),
        );
        assert_eq!(e.scale(), 2.0);
        drag(&mut e, (-500, 200), (-400, 200));
        assert_eq!(e.shapes()[0].width(), 8.0);
        assert_eq!(e.toolbar().scale, 2.0);
    }

    #[test]
    fn select_moves_a_shape_in_one_step_and_deletes_it() {
        let mut e = editor(Tool::Rect);
        drag(&mut e, (150, 150), (250, 250));
        key(&mut e, Key::plain(letter('V')));
        // Click on the outline: selected.
        assert_eq!(click(&mut e, 250, 200), Outcome::Redraw);
        assert_eq!(e.selected(), Some(0));
        // Drag it by (+20, +10) in several moves: one undo step.
        press(&mut e, 150, 200);
        for i in 1..=5 {
            e.input(
                EditInput::Motion {
                    x: 150 + 4 * i,
                    y: 200 + 2 * i,
                },
                &M,
            );
        }
        e.input(EditInput::Release { x: 170, y: 210 }, &M);
        assert_eq!(
            e.shapes()[0].kind,
            ShapeKind::Rect(FRect::new(170.0, 160.0, 100.0, 100.0))
        );
        key(&mut e, Key::ctrl(letter('Z')));
        assert_eq!(
            e.shapes()[0].kind,
            ShapeKind::Rect(FRect::new(150.0, 150.0, 100.0, 100.0))
        );
        key(&mut e, Key::ctrl(letter('Y')));
        assert_eq!(
            e.shapes()[0].kind,
            ShapeKind::Rect(FRect::new(170.0, 160.0, 100.0, 100.0))
        );
        // A click without movement adds no step.
        click(&mut e, 170, 200);
        let steps = e.history.undo_len();
        click(&mut e, 170, 200);
        assert_eq!(e.history.undo_len(), steps);
        // Delete (or Backspace) removes the selected shape, as one step.
        assert_eq!(key(&mut e, Key::plain(KeyCode::Backspace)), Outcome::Redraw);
        assert!(e.shapes().is_empty());
        assert_eq!(e.selected(), None);
        key(&mut e, Key::ctrl(letter('Z')));
        assert_eq!(e.shapes().len(), 1);
        // Nothing selected: Delete does nothing.
        assert_eq!(key(&mut e, Key::plain(KeyCode::Delete)), Outcome::Nothing);
    }

    #[test]
    fn select_picks_the_topmost_shape_and_clicks_inside_the_selection_only() {
        let mut e = editor(Tool::Pixelate);
        drag(&mut e, (150, 150), (300, 300));
        key(&mut e, Key::plain(letter('L')));
        drag(&mut e, (120, 200), (480, 200));
        key(&mut e, Key::plain(letter('V')));
        click(&mut e, 200, 203);
        assert_eq!(e.selected(), Some(1), "the line is on top");
        click(&mut e, 200, 250);
        assert_eq!(e.selected(), Some(0));
        click(&mut e, 400, 300);
        assert_eq!(e.selected(), None, "an empty spot deselects");
    }

    #[test]
    fn swatches_and_sizes_restyle_the_selected_shape_one_step_each() {
        let mut e = editor(Tool::Line);
        drag(&mut e, (150, 150), (300, 150));
        let steps = e.history.undo_len();
        key(&mut e, Key::plain(letter('V')));
        click(&mut e, 200, 150);
        button(&mut e, Item::Swatch(3));
        assert_eq!(e.shapes()[0].color, SWATCHES[3]);
        button(&mut e, Item::Size(Size::L));
        assert_eq!(e.shapes()[0].size, Size::L);
        assert_eq!(e.history.undo_len(), steps + 2);
        // The same swatch again changes nothing: no step.
        button(&mut e, Item::Swatch(3));
        assert_eq!(e.history.undo_len(), steps + 2);
        // Toolbar clicks keep the shape selected.
        assert_eq!(e.selected(), Some(0));
        // The wheel restyles too.
        e.input(EditInput::Wheel(-1), &M);
        assert_eq!((e.size(), e.shapes()[0].size), (Size::M, Size::M));
        e.input(EditInput::Wheel(-1), &M);
        e.input(EditInput::Wheel(-1), &M);
        assert_eq!(e.size(), Size::S);
        key(&mut e, Key::ctrl(letter('Z')));
        key(&mut e, Key::ctrl(letter('Z')));
        key(&mut e, Key::ctrl(letter('Z')));
        key(&mut e, Key::ctrl(letter('Z')));
        assert_eq!(e.shapes()[0].color, SWATCHES[0]);
        assert_eq!(e.shapes()[0].size, Size::M);
        // With nothing selected, a swatch only sets the current colour.
        let mut e = editor(Tool::Line);
        drag(&mut e, (150, 150), (300, 150));
        button(&mut e, Item::Swatch(2));
        assert_eq!(e.shapes()[0].color, SWATCHES[0]);
        assert_eq!(e.color(), SWATCHES[2]);
    }

    #[test]
    fn text_typing_editing_keys_and_commit() {
        let mut e = editor(Tool::Text);
        // The click puts the caret there: the text's first line is centred on it.
        assert_eq!(click(&mut e, 200, 200), Outcome::Redraw);
        assert!(e.is_typing());
        let ShapeKind::Text { at, .. } = e.shapes()[0].kind.clone() else {
            panic!()
        };
        assert_eq!(at, (200.0, 200.0 - 12.5));
        // Backspace at 0 does nothing.
        key(&mut e, Key::plain(KeyCode::Backspace));
        typed(&mut e, "helo");
        assert_eq!(text_of(&e, 0), "helo");
        key(&mut e, Key::plain(KeyCode::Left));
        typed(&mut e, "l");
        assert_eq!(text_of(&e, 0), "hello");
        assert_eq!(e.caret(), Some(Caret { shape: 0, index: 4 }));
        key(&mut e, Key::plain(KeyCode::End));
        // Enter is a new line, not a copy; tool letters are text.
        assert_eq!(key(&mut e, Key::plain(KeyCode::Enter)), Outcome::Redraw);
        key(&mut e, Key::plain(letter('P')));
        assert_eq!(e.tool(), Tool::Text);
        typed(&mut e, "pé");
        assert_eq!(text_of(&e, 0), "hello\npé");
        key(&mut e, Key::plain(KeyCode::Home));
        assert_eq!(e.caret().unwrap().index, 6);
        key(&mut e, Key::plain(KeyCode::Delete));
        assert_eq!(text_of(&e, 0), "hello\né");
        key(&mut e, Key::plain(KeyCode::Backspace));
        assert_eq!(text_of(&e, 0), "helloé");
        key(&mut e, Key::plain(KeyCode::Right));
        key(&mut e, Key::plain(KeyCode::Right));
        assert_eq!(e.caret().unwrap().index, 6, "stops at the end");
        // Control chars are ignored.
        e.input(EditInput::Char('\u{8}'), &M);
        assert_eq!(text_of(&e, 0), "helloé");
        // Esc commits and stays; nothing is cancelled.
        assert_eq!(key(&mut e, Key::plain(KeyCode::Esc)), Outcome::Redraw);
        assert!(!e.is_typing());
        assert_eq!(e.history.undo_len(), 1, "one step for the session");
        // A second Esc cancels.
        assert_eq!(key(&mut e, Key::plain(KeyCode::Esc)), Outcome::Cancel);
    }

    #[test]
    fn an_empty_text_is_dropped_and_a_click_elsewhere_commits() {
        let mut e = editor(Tool::Text);
        click(&mut e, 200, 200);
        click(&mut e, 300, 300);
        // The first (empty) one went; the second is being typed.
        assert_eq!(e.shapes().len(), 1);
        assert!(!e.can_undo());
        typed(&mut e, "a");
        // A tool change commits.
        key(&mut e, Key::plain(KeyCode::Esc));
        assert!(e.can_undo());
        click(&mut e, 400, 150);
        typed(&mut e, "b");
        button(&mut e, Item::Tool(Tool::Rect));
        assert!(!e.is_typing());
        assert_eq!(e.tool(), Tool::Rect);
        assert_eq!(e.shapes().len(), 2);
        assert_eq!(e.history.undo_len(), 2);
    }

    #[test]
    fn re_editing_a_text_is_one_step() {
        let mut e = editor(Tool::Text);
        click(&mut e, 200, 200);
        typed(&mut e, "abc");
        key(&mut e, Key::plain(KeyCode::Esc));
        assert_eq!(e.history.undo_len(), 1);
        // Select tool: a double click on it edits it again, caret where clicked.
        key(&mut e, Key::plain(letter('V')));
        click(&mut e, 211, 200);
        assert_eq!(e.selected(), Some(0));
        let out = e.input(EditInput::DoubleClick { x: 211, y: 200 }, &M);
        e.input(EditInput::Release { x: 211, y: 200 }, &M);
        assert_eq!(out, Outcome::Redraw);
        assert_eq!(e.caret(), Some(Caret { shape: 0, index: 1 }));
        assert_eq!(e.selected(), None);
        typed(&mut e, "XY");
        key(&mut e, Key::plain(KeyCode::Backspace));
        assert_eq!(text_of(&e, 0), "aXbc");
        // Clicking in the text moves the caret, still the same session.
        click(&mut e, 230, 200);
        assert_eq!(e.caret().unwrap().index, 3);
        click(&mut e, 450, 350);
        assert_eq!(e.history.undo_len(), 2, "the whole re-edit is one step");
        key(&mut e, Key::ctrl(letter('Z')));
        assert_eq!(text_of(&e, 0), "abc");
        // The text tool edits an existing text on a click too.
        key(&mut e, Key::plain(letter('T')));
        click(&mut e, 200, 200);
        assert_eq!(e.caret(), Some(Caret { shape: 0, index: 0 }));
        assert_eq!(e.shapes().len(), 1);
        // Deleting every char drops the text: one step.
        key(&mut e, Key::plain(KeyCode::End));
        for _ in 0..3 {
            key(&mut e, Key::plain(KeyCode::Backspace));
        }
        key(&mut e, Key::plain(KeyCode::Esc));
        assert!(e.shapes().is_empty());
        key(&mut e, Key::ctrl(letter('Z')));
        assert_eq!(text_of(&e, 0), "abc");
    }

    #[test]
    fn double_click_on_toolbar_does_not_edit_committed_text() {
        let mut e = editor(Tool::Text);
        e.selection = e.display().rect;
        let bar = e.toolbar();
        assert_eq!(bar.place, Place::Inside);
        for (point, color, out) in [
            (
                bar.button(Item::Swatch(3)).unwrap().center(),
                ColorChoice::Swatch(3),
                Outcome::Redraw,
            ),
            (
                (bar.rect.x + 1.0, bar.rect.y + 1.0),
                ColorChoice::Swatch(0),
                Outcome::Nothing,
            ),
        ] {
            let mut e = e.clone();
            let (x, y) = (point.0 as i32, point.1 as i32);
            click(&mut e, x, bar.rect.y as i32 - 10);
            typed(&mut e, "abc");
            key(&mut e, Key::plain(KeyCode::Enter));
            typed(&mut e, "def");
            key(&mut e, Key::plain(KeyCode::Esc));
            key(&mut e, Key::plain(letter('V')));
            assert_eq!(hit_test(e.shapes(), x as f32, y as f32, &M), Some(0));
            assert_eq!(e.input(EditInput::DoubleClick { x, y }, &M), out);
            e.input(EditInput::Release { x, y }, &M);
            assert!(!e.is_typing());
            assert_eq!(e.color_choice(), color);
            assert_eq!(text_of(&e, 0), "abc\ndef");
            assert_eq!(e.history.undo_len(), 1);
        }
    }

    #[test]
    fn double_click_on_picker_does_not_edit_committed_text() {
        let mut e = editor(Tool::Text);
        e.selection = e.display().rect;
        let p = e.picker_layout();
        for point in [
            p.sv.center(),
            p.hue.center(),
            (p.rect.x + 1.0, p.rect.y + 1.0),
        ] {
            let mut e = e.clone();
            let (x, y) = (point.0 as i32, point.1 as i32);
            click(&mut e, x, y);
            typed(&mut e, "abc");
            key(&mut e, Key::plain(KeyCode::Esc));
            key(&mut e, Key::plain(letter('V')));
            button(&mut e, Item::Custom);
            assert_eq!(hit_test(e.shapes(), x as f32, y as f32, &M), Some(0));
            e.input(EditInput::Motion { x, y }, &M);
            let out = e.input(EditInput::DoubleClick { x, y }, &M);
            assert!(!e.is_typing());
            assert!(e.picker().is_some());
            if let Some(part) = p.part_at(x as f32, y as f32) {
                assert!(matches!(e.drag, Drag::Picker { part: actual, .. } if actual == part));
                assert_ne!(e.custom(), DEFAULT_CUSTOM);
                assert_eq!(out, Outcome::Redraw);
            } else {
                assert!(matches!(e.drag, Drag::Held));
                assert_eq!(out, Outcome::Nothing);
            }
            e.input(EditInput::Release { x, y }, &M);
            assert_eq!(text_of(&e, 0), "abc");
            assert_eq!(e.history.undo_len(), 1);
        }
    }

    #[test]
    fn double_click_on_a_selection_handle_grabs_it_instead_of_editing_text() {
        let mut e = editor(Tool::Text);
        click(&mut e, 480, 385);
        typed(&mut e, "abc");
        key(&mut e, Key::plain(KeyCode::Esc));
        key(&mut e, Key::plain(letter('V')));
        // Inside the text and within reach of the bottom-right handle, as a press sees it.
        let (x, y) = (495, 395);
        assert!(handle_at(e.selection, x as f32, y as f32, e.scale()).is_some());
        assert_eq!(hit_test(e.shapes(), x as f32, y as f32, &M), Some(0));
        e.input(EditInput::DoubleClick { x, y }, &M);
        assert!(!e.is_typing());
        assert!(matches!(e.drag, Drag::Handle { .. }));
        e.input(EditInput::Release { x, y }, &M);
        assert_eq!(text_of(&e, 0), "abc");
    }

    #[test]
    fn double_click_outside_picker_closes_it_before_re_editing_text() {
        let mut e = editor(Tool::Text);
        click(&mut e, 200, 200);
        typed(&mut e, "abc");
        key(&mut e, Key::plain(KeyCode::Esc));
        key(&mut e, Key::plain(letter('V')));
        button(&mut e, Item::Custom);
        assert!(!e.picker().unwrap().0.contains(211.0, 200.0));
        let double_click = EditInput::DoubleClick { x: 211, y: 200 };
        let release = EditInput::Release { x: 211, y: 200 };
        assert_eq!(e.input(double_click, &M), Outcome::Redraw);
        e.input(release, &M);
        assert!(e.picker().is_none());
        assert!(!e.is_typing());
        assert_eq!(text_of(&e, 0), "abc");
        assert_eq!(e.history.undo_len(), 1);
        assert_eq!(e.input(double_click, &M), Outcome::Redraw);
        e.input(release, &M);
        assert_eq!(e.caret(), Some(Caret { shape: 0, index: 1 }));
        typed(&mut e, "X");
        key(&mut e, Key::plain(KeyCode::Esc));
        assert_eq!(text_of(&e, 0), "aXbc");
        assert_eq!(e.history.undo_len(), 2);
    }

    #[test]
    fn ctrl_z_while_typing_commits_then_undoes_and_ctrl_c_commits_then_copies() {
        let mut e = editor(Tool::Text);
        click(&mut e, 200, 200);
        typed(&mut e, "hi");
        assert_eq!(key(&mut e, Key::ctrl(letter('Z'))), Outcome::Redraw);
        assert!(!e.is_typing());
        assert!(e.shapes().is_empty(), "the session was the step undone");
        assert!(e.can_redo());
        key(&mut e, Key::ctrl(letter('Y')));
        assert_eq!(text_of(&e, 0), "hi");
        click(&mut e, 300, 300);
        typed(&mut e, "yo");
        assert_eq!(key(&mut e, Key::ctrl(letter('C'))), Outcome::Copy);
        assert!(!e.is_typing());
        assert_eq!(text_of(&e, 1), "yo");
        click(&mut e, 300, 150);
        typed(&mut e, "s");
        assert_eq!(key(&mut e, Key::ctrl(letter('S'))), Outcome::Save);
        assert_eq!(e.shapes().len(), 3);
    }

    #[test]
    fn the_toolbar_wins_over_the_text_being_typed_where_they_overlap() {
        // A full-monitor selection puts the toolbar inside its bottom edge.
        let mut e = Editor::new(
            IRect::new(0, 0, 1920, 1080),
            DESK,
            displays(),
            Prefs {
                tool: Tool::Text,
                ..Prefs::default()
            },
            ACCENT,
            (0, 0),
        );
        assert_eq!(e.toolbar().place, Place::Inside);
        click(&mut e, 100, 1020);
        typed(&mut e, "a");
        key(&mut e, Key::plain(KeyCode::Enter));
        typed(&mut e, "b");
        // The two-line text now reaches under the toolbar's rect button.
        let rect = e.toolbar().button(Item::Tool(Tool::Rect)).unwrap();
        let (x, y) = (rect.center().0 as i32, rect.y as i32 + 10);
        let s = &e.shapes()[0];
        assert!(live_text_rect(s.text_rect(&M).unwrap(), s, &M).contains(x as f32, y as f32));
        click(&mut e, x, y);
        assert_eq!(e.tool(), Tool::Rect);
        assert!(!e.is_typing());
        assert_eq!(text_of(&e, 0), "a\nb");
    }

    #[test]
    fn colour_and_size_while_typing_apply_to_the_text() {
        let mut e = editor(Tool::Text);
        click(&mut e, 200, 200);
        typed(&mut e, "x");
        button(&mut e, Item::Swatch(4));
        button(&mut e, Item::Size(Size::L));
        assert!(e.is_typing(), "a swatch or a size keeps typing");
        assert_eq!(e.shapes()[0].color, SWATCHES[4]);
        assert_eq!(e.shapes()[0].size, Size::L);
        key(&mut e, Key::plain(KeyCode::Esc));
        assert_eq!(e.history.undo_len(), 1);
    }

    #[test]
    fn the_select_tool_resizes_and_moves_the_selection_without_undo() {
        let mut e = editor(Tool::Select);
        // Bottom-right handle out by (50, 20).
        drag(&mut e, (500, 400), (550, 420));
        assert_eq!(e.selection(), IRect::new(100, 100, 450, 320));
        // Left handle past the right edge: flips.
        drag(&mut e, (100, 260), (700, 260));
        assert_eq!(e.selection(), IRect::new(550, 100, 150, 320));
        // An empty spot inside moves it.
        drag(&mut e, (600, 200), (500, 150));
        assert_eq!(e.selection(), IRect::new(450, 50, 150, 320));
        // Clamped to the desktop.
        drag(&mut e, (500, 200), (500, -5000));
        assert_eq!(e.selection().y, 0);
        assert!(!e.can_undo());
        // Esc while dragging puts it back.
        press(&mut e, 500, 200);
        e.input(EditInput::Motion { x: 520, y: 220 }, &M);
        key(&mut e, Key::plain(KeyCode::Esc));
        e.input(EditInput::Release { x: 520, y: 220 }, &M);
        assert_eq!(e.selection(), IRect::new(450, 0, 150, 320));
        // Shapes stay put on screen when the selection moves.
        let mut e = editor(Tool::Step);
        click(&mut e, 150, 150);
        key(&mut e, Key::plain(letter('V')));
        drag(&mut e, (300, 300), (350, 300));
        assert_eq!(
            e.shapes()[0].kind,
            ShapeKind::Step {
                at: (150.0, 150.0),
                n: 1
            }
        );
    }

    #[test]
    fn a_click_on_the_active_tool_goes_back_to_select() {
        let mut e = editor(Tool::Arrow);
        button(&mut e, Item::Tool(Tool::Arrow));
        assert_eq!(e.tool(), Tool::Select);
        button(&mut e, Item::Tool(Tool::Select));
        assert_eq!(e.tool(), Tool::Select);
        button(&mut e, Item::Tool(Tool::Pen));
        assert_eq!(e.tool(), Tool::Pen);
        // The keyboard just picks the tool.
        key(&mut e, Key::plain(letter('P')));
        assert_eq!(e.tool(), Tool::Pen);
    }

    #[test]
    fn the_picker_sets_the_colour_live_and_a_drag_on_a_shape_is_one_step() {
        let mut e = editor(Tool::Line);
        drag(&mut e, (150, 150), (300, 150));
        key(&mut e, Key::plain(letter('V')));
        click(&mut e, 200, 150);
        let steps = e.history.undo_len();
        button(&mut e, Item::Custom);
        assert_eq!(
            e.history.undo_len(),
            steps + 1,
            "custom applied to the shape"
        );
        let (p, hsv) = e.picker().unwrap();
        assert_eq!(hsv.to_rgba(), DEFAULT_CUSTOM);
        // Drag across the hue strip, then the square.
        let hx = p.hue.center().0 as i32;
        press(&mut e, hx, p.hue.y as i32);
        e.input(
            EditInput::Motion {
                x: hx,
                y: p.hue_y(120.0).round() as i32,
            },
            &M,
        );
        e.input(
            EditInput::Release {
                x: hx,
                y: p.hue_y(120.0).round() as i32,
            },
            &M,
        );
        let (x0, y0) = (p.sv.x as i32, p.sv.y as i32);
        press(&mut e, x0 + 30, y0 + 30);
        e.input(
            EditInput::Motion {
                x: x0 + 9999,
                y: y0,
            },
            &M,
        );
        // Live: the shape already has the colour mid-drag.
        let green = Hsv {
            h: p.hue_at(p.hue_y(120.0).round()),
            s: 1.0,
            v: 1.0,
        }
        .to_rgba();
        assert_eq!(e.shapes()[0].color, green);
        e.input(
            EditInput::Release {
                x: x0 + 9999,
                y: y0,
            },
            &M,
        );
        assert_eq!(e.color_choice(), ColorChoice::Custom);
        assert_eq!(e.custom(), green);
        assert_eq!(e.history.undo_len(), steps + 3, "one step per drag");
        // A click outside closes the picker and does nothing else.
        assert_eq!(click(&mut e, 300, 300), Outcome::Redraw);
        assert!(e.picker().is_none());
        assert_eq!(e.selected(), Some(0));
        // Esc closes it too, before dropping the shape selection.
        button(&mut e, Item::Custom);
        key(&mut e, Key::plain(KeyCode::Esc));
        assert!(e.picker().is_none());
        assert_eq!(e.selected(), Some(0));
        key(&mut e, Key::plain(KeyCode::Esc));
        assert_eq!(e.selected(), None);
        assert_eq!(key(&mut e, Key::plain(KeyCode::Esc)), Outcome::Cancel);
    }

    #[test]
    fn the_picker_with_nothing_selected_only_sets_the_colour() {
        let mut e = editor(Tool::Pen);
        button(&mut e, Item::Custom);
        let (p, _) = e.picker().unwrap();
        click(&mut e, p.sv.x as i32, p.sv.bottom() as i32 - 1);
        assert_eq!(e.custom(), Rgba::rgb(0, 0, 0));
        assert!(!e.can_undo());
        // The custom swatch again closes it.
        button(&mut e, Item::Custom);
        assert!(e.picker().is_none());
        assert_eq!(e.color(), Rgba::rgb(0, 0, 0));
    }

    #[test]
    fn copy_save_and_close_buttons_work_with_the_mouse_alone() {
        let mut e = editor(Tool::Arrow);
        assert_eq!(button(&mut e, Item::Copy), Outcome::Copy);
        assert_eq!(button(&mut e, Item::Save), Outcome::Save);
        assert_eq!(button(&mut e, Item::Close), Outcome::Cancel);
        assert_eq!(key(&mut e, Key::plain(KeyCode::Enter)), Outcome::Copy);
        // The undo/redo buttons.
        drag(&mut e, (150, 150), (300, 150));
        button(&mut e, Item::Undo);
        assert!(e.shapes().is_empty());
        button(&mut e, Item::Redo);
        assert_eq!(e.shapes().len(), 1);
        // A press on the toolbar's padding does nothing.
        let bar = e.toolbar();
        let (px, py) = (bar.rect.x as i32 + 1, bar.rect.y as i32 + 1);
        e.input(EditInput::Motion { x: px, y: py }, &M);
        assert_eq!(click(&mut e, px, py), Outcome::Nothing);
        assert_eq!(e.shapes().len(), 1);
    }

    #[test]
    fn a_new_step_clears_redo_and_the_history_is_capped() {
        let mut e = editor(Tool::Step);
        for i in 0..120 {
            click(&mut e, 110 + i * 3, 150);
        }
        assert_eq!(e.shapes().len(), 120);
        while e.can_undo() {
            key(&mut e, Key::ctrl(letter('Z')));
        }
        assert_eq!(e.shapes().len(), 20, "only 100 steps back");
        key(&mut e, Key::ctrl(letter('Y')));
        assert!(e.can_redo());
        click(&mut e, 450, 350);
        assert!(!e.can_redo());
    }

    #[test]
    fn esc_during_a_drag_aborts_it() {
        let mut e = editor(Tool::Rect);
        press(&mut e, 150, 150);
        e.input(EditInput::Motion { x: 250, y: 250 }, &M);
        assert_eq!(key(&mut e, Key::plain(KeyCode::Esc)), Outcome::Redraw);
        assert!(e.draft().is_none());
        e.input(EditInput::Release { x: 250, y: 250 }, &M);
        assert!(e.shapes().is_empty());
        // Other keys mid-drag are ignored.
        press(&mut e, 150, 150);
        e.input(EditInput::Motion { x: 250, y: 250 }, &M);
        assert_eq!(key(&mut e, Key::ctrl(letter('C'))), Outcome::Nothing);
        e.input(EditInput::Release { x: 250, y: 250 }, &M);
        assert_eq!(e.shapes().len(), 1);
        // A shape move reverts.
        key(&mut e, Key::plain(letter('V')));
        press(&mut e, 250, 200);
        e.input(EditInput::Motion { x: 280, y: 200 }, &M);
        key(&mut e, Key::plain(KeyCode::Esc));
        e.input(EditInput::Release { x: 280, y: 200 }, &M);
        assert_eq!(
            e.shapes()[0].kind,
            ShapeKind::Rect(FRect::new(150.0, 150.0, 100.0, 100.0))
        );
        assert_eq!(e.history.undo_len(), 1);
    }

    #[test]
    fn cursors_follow_the_tool_and_the_spot() {
        let mut e = editor(Tool::Pen);
        let at = |e: &mut Editor, x: i32, y: i32| {
            e.input(EditInput::Motion { x, y }, &M);
            e.cursor_kind()
        };
        assert_eq!(at(&mut e, 200, 200), CursorKind::Cross);
        assert_eq!(at(&mut e, 50, 50), CursorKind::Arrow);
        let (bx, by) = e.toolbar().buttons[3].1.center();
        assert_eq!(at(&mut e, bx as i32, by as i32), CursorKind::Arrow);
        key(&mut e, Key::plain(letter('T')));
        assert_eq!(at(&mut e, 200, 200), CursorKind::IBeam);
        key(&mut e, Key::plain(letter('V')));
        assert_eq!(at(&mut e, 200, 200), CursorKind::Move);
        assert_eq!(at(&mut e, 100, 100), CursorKind::SizeNwse);
        assert_eq!(at(&mut e, 500, 100), CursorKind::SizeNesw);
        assert_eq!(at(&mut e, 300, 400), CursorKind::SizeNs);
        assert_eq!(at(&mut e, 500, 250), CursorKind::SizeWe);
        // While drawing: the crosshair even outside.
        key(&mut e, Key::plain(letter('L')));
        press(&mut e, 200, 200);
        assert_eq!(at(&mut e, 50, 50), CursorKind::Cross);
        // Hovering a toolbar button redraws once (it lights up).
        e.input(EditInput::Release { x: 50, y: 50 }, &M);
        let out = e.input(
            EditInput::Motion {
                x: bx as i32,
                y: by as i32,
            },
            &M,
        );
        assert_eq!(out, Outcome::Redraw);
        assert_eq!(e.hover(), Some(Item::Tool(Tool::Arrow)));
        let out = e.input(
            EditInput::Motion {
                x: bx as i32 + 1,
                y: by as i32,
            },
            &M,
        );
        assert_eq!(out, Outcome::Nothing);
    }
}
