//! Pure Clipboard panel interaction. Rows are authoritative store-query results:
//! never search truncated display metadata instead of the complete stored text.
use crate::clipboard::{
    bounded_text, valid_thumbnail, Filter, ItemId, Kind, Row, MAX_PINNED, MAX_PREVIEW_BYTES,
    MAX_UNPINNED,
};
use crate::layout::Rect;

pub const PANEL_W: f32 = 400.0;
pub const PANEL_H: f32 = 360.0;
pub const VISIBLE_ROWS: usize = 5;
pub const ROW_HEIGHT: f32 = 44.0;
pub const SEARCH: Rect = Rect::new(12.0, 36.0, 376.0, 28.0);
pub const LIST: Rect = Rect::new(12.0, 100.0, 376.0, 220.0);
pub const PREVIEW: Rect = LIST;
pub const ENABLE: Rect = Rect::new(110.0, 200.0, 180.0, 32.0);
pub const CLEAR_INCLUDE_PINNED: Rect = Rect::new(40.0, 188.0, 320.0, 28.0);
pub const CLEAR_CONFIRM: Rect = Rect::new(204.0, 244.0, 156.0, 28.0);
pub const CLEAR_CANCEL: Rect = Rect::new(40.0, 244.0, 156.0, 28.0);
pub const FILTERS: [(Filter, Rect); 5] = [
    (Filter::All, Rect::new(12.0, 70.0, 64.0, 24.0)),
    (Filter::Text, Rect::new(80.0, 70.0, 64.0, 24.0)),
    (Filter::Images, Rect::new(148.0, 70.0, 76.0, 24.0)),
    (Filter::Files, Rect::new(228.0, 70.0, 64.0, 24.0)),
    (Filter::Pinned, Rect::new(296.0, 70.0, 92.0, 24.0)),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Enable,
    Pause,
    Clear,
    Close,
    Pin,
    Delete,
    Copy,
    Paste,
    Preview,
    Drag,
    IncludePinned,
    ConfirmClear,
    CancelClear,
}
pub const HEADER_BUTTONS: [(Button, Rect); 3] = [
    (Button::Pause, Rect::new(172.0, 8.0, 72.0, 24.0)),
    (Button::Clear, Rect::new(248.0, 8.0, 104.0, 24.0)),
    (Button::Close, Rect::new(356.0, 8.0, 32.0, 24.0)),
];
pub const FOOTER_BUTTONS: [(Button, Rect); 6] = [
    (Button::Pin, Rect::new(12.0, 328.0, 52.0, 24.0)),
    (Button::Delete, Rect::new(68.0, 328.0, 60.0, 24.0)),
    (Button::Copy, Rect::new(132.0, 328.0, 52.0, 24.0)),
    (Button::Paste, Rect::new(188.0, 328.0, 56.0, 24.0)),
    (Button::Preview, Rect::new(248.0, 328.0, 76.0, 24.0)),
    (Button::Drag, Rect::new(328.0, 328.0, 60.0, 24.0)),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    Search,
    List,
    Filter(Filter),
    Button(Button),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    None,
    Search,
    Filter(Filter),
    Row(ItemId),
    Button(Button),
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Action {
    #[default]
    None,
    Redraw,
    SearchChanged,
    Enable,
    TogglePause,
    Close,
    Copy {
        plain: bool,
    },
    Paste {
        plain: bool,
    },
    TogglePin,
    Delete,
    ClearUnpinned,
    RequestClearAll,
    ConfirmClearAll,
    CancelClearAll,
    Preview,
    Drag,
    PasteSearch,
}

#[derive(Clone, Debug, Default)]
pub struct View {
    pub rows: Vec<Row>,
    pub query: String,
    pub filter: Filter,
    pub selected: Option<ItemId>,
    pub scroll: usize,
    pub enabled: bool,
    pub paused: bool,
    pub loading: bool,
    pub status: Option<String>,
    pub preview: bool,
    pub confirm_clear_all: bool,
    pub focus: Focus,
    /// Deliberate checkbox, reset whenever confirmation opens or closes.
    pub include_pinned: bool,
    caret: usize,
    anchor: Option<usize>,
    wheel_remainder: i32,
    selection_initialized: bool,
}

impl View {
    /// Rows may contain duplicate text, but IDs must be unique. Never retarget a
    /// missing ID to a different row after an asynchronous delete/prune/reload.
    pub fn set_rows(&mut self, rows: Vec<Row>) {
        let previous = self.selected;
        let mut ids = std::collections::HashSet::new();
        self.rows = rows
            .into_iter()
            .take(MAX_PINNED + MAX_UNPINNED)
            .filter(|row| ids.insert(row.id))
            .map(|mut row| {
                row.preview = bounded_text(&row.preview, MAX_PREVIEW_BYTES);
                row.source = row.source.map(|s| bounded_text(&s, MAX_PREVIEW_BYTES));
                if row.thumbnail.as_ref().is_some_and(|t| !valid_thumbnail(t)) {
                    row.thumbnail = None;
                }
                row
            })
            .collect();
        self.selected = match previous {
            Some(id) => self.rows.iter().any(|row| row.id == id).then_some(id),
            None if !self.selection_initialized => self.rows.first().map(|row| row.id),
            None => None,
        };
        self.selection_initialized |= self.selected.is_some();
        self.loading = false;
        self.scroll = self.scroll.min(self.max_scroll());
        if self.selected.is_none() {
            self.preview = false;
        }
    }

    pub fn selected_row(&self) -> Option<&Row> {
        let id = self.selected?;
        self.rows.iter().find(|row| row.id == id)
    }

    pub fn visible_rows(&self) -> &[Row] {
        let start = self.scroll.min(self.rows.len());
        &self.rows[start..(start + VISIBLE_ROWS).min(self.rows.len())]
    }

    pub fn caret(&self) -> usize {
        self.caret.min(self.query.chars().count())
    }

    /// Character indices, for renderer selection highlighting.
    pub fn search_selection(&self) -> Option<(usize, usize)> {
        let caret = self.caret();
        let anchor = self.anchor?.min(self.query.chars().count());
        (anchor != caret).then_some((anchor.min(caret), anchor.max(caret)))
    }

    pub fn set_filter(&mut self, filter: Filter) -> Action {
        if self.filter == filter {
            return Action::Redraw;
        }
        self.filter = filter;
        self.search_changed()
    }

    fn search_changed(&mut self) -> Action {
        self.rows.clear();
        self.selected = None;
        self.selection_initialized = false;
        self.scroll = 0;
        self.preview = false;
        self.loading = self.enabled;
        Action::SearchChanged
    }

    fn byte(&self, index: usize) -> usize {
        self.query
            .char_indices()
            .nth(index)
            .map_or(self.query.len(), |(byte, _)| byte)
    }

    fn delete_selection(&mut self) -> bool {
        if let Some((start, end)) = self.search_selection() {
            self.query
                .replace_range(self.byte(start)..self.byte(end), "");
            self.caret = start;
            self.anchor = None;
            true
        } else {
            false
        }
    }

    pub fn insert_search(&mut self, text: &str) -> Action {
        if self.confirm_clear_all {
            return Action::None;
        }
        let text: String = text
            .chars()
            .map(|c| {
                if matches!(c, '\r' | '\n' | '\t') {
                    ' '
                } else {
                    c
                }
            })
            .filter(|c| !c.is_control())
            .take(256)
            .collect();
        if text.is_empty() {
            return Action::None;
        }
        self.focus = Focus::Search;
        self.caret = self.caret();
        self.delete_selection();
        let capacity = 256usize.saturating_sub(self.query.chars().count());
        let text: String = text.chars().take(capacity).collect();
        if text.is_empty() {
            return Action::None;
        }
        self.query.insert_str(self.byte(self.caret), &text);
        self.caret += text.chars().count();
        self.search_changed()
    }

    pub fn character(&mut self, c: char) -> Action {
        if c.is_control()
            || self.confirm_clear_all
            || (c == ' ' && matches!(self.focus, Focus::Button(_) | Focus::Filter(_)))
        {
            return Action::None;
        }
        self.insert_search(c.encode_utf8(&mut [0; 4]))
    }

    pub fn key(&mut self, vk: u16, ctrl: bool, shift: bool, alt: bool) -> Action {
        if self.confirm_clear_all {
            return match vk {
                0x1B => self.cancel_clear(),
                0x09 => {
                    self.tab(shift);
                    Action::Redraw
                }
                0x0D | 0x20 => match self.focus {
                    Focus::Button(button) => self.activate(button),
                    _ => Action::None,
                },
                _ => Action::None,
            };
        }
        if vk == 0x1B {
            return Action::Close;
        }
        if alt {
            if !ctrl && (0x31..=0x39).contains(&vk) && !self.preview {
                let id = self.visible_rows().get((vk - 0x31) as usize).map(|r| r.id);
                if let Some(id) = id {
                    self.selected = Some(id);
                    self.focus = Focus::List;
                    return self.transfer(true, shift);
                }
            }
            return Action::None;
        }
        if ctrl {
            match vk {
                0x41 => {
                    self.focus = Focus::Search;
                    self.anchor = Some(0);
                    self.caret = self.query.chars().count();
                    return Action::Redraw;
                }
                0x43 => return self.transfer(false, shift),
                0x56 => {
                    self.focus = Focus::Search;
                    return Action::PasteSearch;
                }
                0x50 => return self.activate(Button::Pin),
                0x46 => {
                    self.focus = Focus::Search;
                    return Action::Redraw;
                }
                _ => {}
            }
        }
        match vk {
            0x09 => {
                self.tab(shift);
                Action::Redraw
            }
            0x0D => match self.focus {
                Focus::Button(button) => self.activate(button),
                Focus::Filter(filter) => self.set_filter(filter),
                _ => self.transfer(true, shift),
            },
            0x20 if matches!(self.focus, Focus::Button(_) | Focus::Filter(_)) => match self.focus {
                Focus::Button(button) => self.activate(button),
                Focus::Filter(filter) => self.set_filter(filter),
                _ => Action::None,
            },
            0x26 => self.navigate(-1),
            0x28 => self.navigate(1),
            0x21 => self.navigate(-(VISIBLE_ROWS as isize)),
            0x22 => self.navigate(VISIBLE_ROWS as isize),
            0x24 if self.focus != Focus::Search => self.navigate_to(0),
            0x23 if self.focus != Focus::Search => {
                self.navigate_to(self.rows.len().saturating_sub(1))
            }
            0x2E if self.focus != Focus::Search => self.activate(Button::Delete),
            0x08 | 0x2E | 0x25 | 0x27 | 0x24 | 0x23 if self.focus == Focus::Search => {
                self.edit_key(vk, ctrl, shift)
            }
            _ => Action::None,
        }
    }

    fn edit_key(&mut self, vk: u16, ctrl: bool, shift: bool) -> Action {
        self.caret = self.caret();
        let old = self.caret;
        if matches!(vk, 0x08 | 0x2E) {
            let mut changed = self.delete_selection();
            if !changed {
                let (start, end) = if vk == 0x08 {
                    (
                        if ctrl {
                            self.word_left(old)
                        } else {
                            old.saturating_sub(1)
                        },
                        old,
                    )
                } else {
                    (
                        old,
                        if ctrl {
                            self.word_right(old)
                        } else {
                            (old + 1).min(self.query.chars().count())
                        },
                    )
                };
                changed = start != end;
                self.query
                    .replace_range(self.byte(start)..self.byte(end), "");
                self.caret = start;
            }
            self.anchor = None;
            return if changed {
                self.search_changed()
            } else {
                Action::None
            };
        }
        if shift && self.anchor.is_none() {
            self.anchor = Some(old);
        }
        self.caret = match vk {
            0x24 => 0,
            0x23 => self.query.chars().count(),
            0x25 if !shift && self.search_selection().is_some() => {
                self.search_selection().unwrap().0
            }
            0x27 if !shift && self.search_selection().is_some() => {
                self.search_selection().unwrap().1
            }
            0x25 if ctrl => self.word_left(old),
            0x27 if ctrl => self.word_right(old),
            0x25 => old.saturating_sub(1),
            _ => (old + 1).min(self.query.chars().count()),
        };
        if !shift {
            self.anchor = None;
        }
        Action::Redraw
    }

    fn word_left(&self, from: usize) -> usize {
        let chars: Vec<char> = self.query.chars().collect();
        let mut at = from;
        while at > 0 && chars[at - 1].is_whitespace() {
            at -= 1;
        }
        while at > 0 && !chars[at - 1].is_whitespace() {
            at -= 1;
        }
        at
    }
    fn word_right(&self, from: usize) -> usize {
        let chars: Vec<char> = self.query.chars().collect();
        let mut at = from;
        while at < chars.len() && !chars[at].is_whitespace() {
            at += 1;
        }
        while at < chars.len() && chars[at].is_whitespace() {
            at += 1;
        }
        at
    }

    fn max_scroll(&self) -> usize {
        self.rows.len().saturating_sub(VISIBLE_ROWS)
    }
    fn navigate(&mut self, delta: isize) -> Action {
        let at = self
            .selected
            .and_then(|id| self.rows.iter().position(|r| r.id == id));
        let next = at.map_or(
            if delta < 0 {
                self.rows.len().saturating_sub(1)
            } else {
                0
            },
            |at| at.saturating_add_signed(delta),
        );
        self.navigate_to(next)
    }
    fn navigate_to(&mut self, index: usize) -> Action {
        let Some(row) = self.rows.get(index.min(self.rows.len().saturating_sub(1))) else {
            return Action::None;
        };
        self.selected = Some(row.id);
        self.focus = Focus::List;
        let index = index.min(self.rows.len() - 1);
        if index < self.scroll {
            self.scroll = index;
        }
        if index >= self.scroll.saturating_add(VISIBLE_ROWS) {
            self.scroll = index + 1 - VISIBLE_ROWS;
        }
        Action::Redraw
    }

    fn transfer(&self, paste: bool, plain: bool) -> Action {
        let Some(row) = self.selected_row().filter(|r| !r.corrupt) else {
            return Action::None;
        };
        if plain && row.kind == Kind::Image {
            return Action::None;
        }
        if paste {
            Action::Paste { plain }
        } else {
            Action::Copy { plain }
        }
    }

    pub fn click(&mut self, x: f32, y: f32, double: bool) -> Action {
        match hit_test(self, x, y) {
            Hit::None => Action::None,
            Hit::Search => {
                self.focus = Focus::Search;
                self.caret = self.query.chars().count();
                self.anchor = None;
                Action::Redraw
            }
            Hit::Filter(filter) => {
                self.focus = Focus::Filter(filter);
                self.set_filter(filter)
            }
            Hit::Row(id) => {
                self.selected = Some(id);
                self.focus = Focus::List;
                if double {
                    self.transfer(true, false)
                } else {
                    Action::Redraw
                }
            }
            Hit::Button(button) => {
                self.focus = Focus::Button(button);
                self.activate(button)
            }
        }
    }

    /// Windows wheel delta: 120 per notch, positive scrolls towards newer entries.
    pub fn wheel(&mut self, delta: i32) -> Action {
        if self.preview || self.confirm_clear_all {
            return Action::None;
        }
        self.wheel_remainder = self.wheel_remainder.saturating_add(delta);
        let steps = self.wheel_remainder / 120;
        self.wheel_remainder %= 120;
        let old = self.scroll;
        self.scroll = self
            .scroll
            .saturating_add_signed(-(steps as isize))
            .min(self.max_scroll());
        if old != self.scroll {
            Action::Redraw
        } else {
            Action::None
        }
    }

    fn activate(&mut self, button: Button) -> Action {
        if !button_enabled(self, button) {
            return Action::None;
        }
        match button {
            Button::Enable => Action::Enable,
            Button::Pause => Action::TogglePause,
            Button::Close => Action::Close,
            Button::Pin => Action::TogglePin,
            Button::Delete => Action::Delete,
            Button::Copy => self.transfer(false, false),
            Button::Paste => self.transfer(true, false),
            Button::Preview => {
                self.preview = !self.preview;
                Action::Preview
            }
            Button::Drag => Action::Drag,
            Button::Clear => {
                self.confirm_clear_all = true;
                self.include_pinned = false;
                self.preview = false;
                self.focus = Focus::Button(Button::CancelClear);
                Action::RequestClearAll
            }
            Button::IncludePinned => {
                self.include_pinned = !self.include_pinned;
                Action::Redraw
            }
            Button::CancelClear => self.cancel_clear(),
            Button::ConfirmClear => {
                let all = self.include_pinned;
                self.confirm_clear_all = false;
                self.include_pinned = false;
                self.focus = Focus::Search;
                if all {
                    Action::ConfirmClearAll
                } else {
                    Action::ClearUnpinned
                }
            }
        }
    }

    fn cancel_clear(&mut self) -> Action {
        self.confirm_clear_all = false;
        self.include_pinned = false;
        self.focus = Focus::Search;
        Action::CancelClearAll
    }

    fn tab(&mut self, reverse: bool) {
        let targets: Vec<Focus> = if self.confirm_clear_all {
            vec![
                Focus::Button(Button::CancelClear),
                Focus::Button(Button::IncludePinned),
                Focus::Button(Button::ConfirmClear),
            ]
        } else {
            let mut targets = vec![Focus::Search];
            targets.extend(FILTERS.iter().map(|(filter, _)| Focus::Filter(*filter)));
            if !self.rows.is_empty() {
                targets.push(Focus::List);
            }
            if !self.enabled {
                targets.push(Focus::Button(Button::Enable));
            }
            targets.extend(
                FOOTER_BUTTONS
                    .iter()
                    .chain(HEADER_BUTTONS.iter())
                    .filter(|(button, _)| button_enabled(self, *button))
                    .map(|(button, _)| Focus::Button(*button)),
            );
            targets
        };
        let at = targets.iter().position(|f| *f == self.focus);
        let next = match (at, reverse) {
            (Some(0), true) | (None, true) => targets.len() - 1,
            (Some(at), true) => at - 1,
            (Some(at), false) => (at + 1) % targets.len(),
            (None, false) => 0,
        };
        self.focus = targets[next];
    }
}

pub fn row_rect(slot: usize) -> Option<Rect> {
    (slot < VISIBLE_ROWS).then(|| {
        Rect::new(
            LIST.x,
            LIST.y + slot as f32 * ROW_HEIGHT,
            LIST.w,
            ROW_HEIGHT,
        )
    })
}

pub fn button_enabled(view: &View, button: Button) -> bool {
    if view.confirm_clear_all {
        return matches!(
            button,
            Button::IncludePinned | Button::ConfirmClear | Button::CancelClear
        );
    }
    match button {
        Button::Close => true,
        Button::Enable => !view.enabled,
        Button::Pause => view.enabled,
        Button::Clear => !view.rows.is_empty() || view.enabled,
        Button::Delete => view.selected_row().is_some(),
        Button::Pin | Button::Copy | Button::Paste | Button::Preview => {
            view.selected_row().is_some_and(|r| !r.corrupt)
        }
        Button::Drag => view
            .selected_row()
            .is_some_and(|r| !r.corrupt && matches!(r.kind, Kind::Image | Kind::Files)),
        Button::IncludePinned | Button::ConfirmClear | Button::CancelClear => false,
    }
}

pub fn button_label(view: &View, button: Button) -> &'static str {
    match button {
        Button::Enable => "Enable history",
        Button::Pause if view.paused => "Resume",
        Button::Pause => "Pause",
        Button::Clear => "Clear history",
        Button::Close => "Close",
        Button::Pin if view.selected_row().is_some_and(|r| r.pinned) => "Unpin",
        Button::Pin => "Pin",
        Button::Delete => "Delete",
        Button::Copy => "Copy",
        Button::Paste => "Paste",
        Button::Preview if view.preview => "Back",
        Button::Preview => "Preview",
        Button::Drag => "Drag",
        Button::IncludePinned => "Include pinned items",
        Button::ConfirmClear if view.include_pinned => "Clear all history",
        Button::ConfirmClear => "Clear unpinned",
        Button::CancelClear => "Cancel",
    }
}

pub fn button_tooltip(view: &View, button: Button) -> &'static str {
    match button {
        Button::Drag if view.selected_row().is_some_and(|r| r.kind == Kind::Image) => "Export PNG to Pictures/Daijobu/Clipboard and drag a copy. Export remains after cancel or deletion.",
        Button::Drag => "Drag original file paths as copies. Original files are never changed.",
        Button::Delete => "Delete from history only. Clipboard and original files stay intact.",
        Button::Pin => "Pin or unpin selected item (Ctrl+P).",
        Button::Copy => "Copy selected item (Ctrl+C).",
        Button::Paste => "Paste selected item (Enter). Shift+Enter pastes plain text.",
        Button::Preview => "Open or close a plain-text or image preview inside this panel.",
        Button::Clear => "Clear unpinned history, or explicitly include pinned items after confirmation.",
        _ => button_label(view, button),
    }
}

pub fn hit_test(view: &View, x: f32, y: f32) -> Hit {
    if view.confirm_clear_all {
        for (button, rect) in [
            (Button::IncludePinned, CLEAR_INCLUDE_PINNED),
            (Button::ConfirmClear, CLEAR_CONFIRM),
            (Button::CancelClear, CLEAR_CANCEL),
        ] {
            if rect.contains(x, y) {
                return Hit::Button(button);
            }
        }
        return Hit::None;
    }
    for (button, rect) in HEADER_BUTTONS.iter().chain(FOOTER_BUTTONS.iter()) {
        if rect.contains(x, y) {
            return Hit::Button(*button);
        }
    }
    if SEARCH.contains(x, y) {
        return Hit::Search;
    }
    for (filter, rect) in FILTERS {
        if rect.contains(x, y) {
            return Hit::Filter(filter);
        }
    }
    if !view.enabled && view.rows.is_empty() && ENABLE.contains(x, y) {
        return Hit::Button(Button::Enable);
    }
    if !view.preview && LIST.contains(x, y) {
        let slot = ((y - LIST.y) / ROW_HEIGHT) as usize;
        if let Some(row) = view.visible_rows().get(slot) {
            return Hit::Row(row.id);
        }
    }
    Hit::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: u64, kind: Kind) -> Row {
        Row {
            id: ItemId(id),
            created_ms: id,
            used_ms: id,
            pinned: false,
            kind,
            preview: "synthetic duplicate text".into(),
            source: None,
            thumbnail: None,
            bytes: 32,
            corrupt: false,
        }
    }
    fn populated(count: u64) -> View {
        let mut view = View {
            enabled: true,
            ..View::default()
        };
        view.set_rows((1..=count).map(|id| row(id, Kind::Text)).collect());
        view
    }
    fn click_button(view: &mut View, button: Button) -> Action {
        let rect = match button {
            Button::Enable => ENABLE,
            Button::IncludePinned => CLEAR_INCLUDE_PINNED,
            Button::ConfirmClear => CLEAR_CONFIRM,
            Button::CancelClear => CLEAR_CANCEL,
            _ => {
                HEADER_BUTTONS
                    .iter()
                    .chain(FOOTER_BUTTONS.iter())
                    .find(|(b, _)| *b == button)
                    .unwrap()
                    .1
            }
        };
        view.click(rect.x + 1.0, rect.y + 1.0, false)
    }

    #[test]
    fn disabled_enable_and_pause_wait_for_controller_acknowledgement() {
        let mut view = View::default();
        assert_eq!(click_button(&mut view, Button::Enable), Action::Enable);
        assert!(!view.enabled);
        view.enabled = true;
        assert_eq!(click_button(&mut view, Button::Pause), Action::TogglePause);
        assert!(!view.paused);
        view.paused = true;
        view.set_rows(vec![row(42, Kind::Text)]);
        assert_eq!(
            view.key(0x43, true, false, false),
            Action::Copy { plain: false }
        );
        assert_eq!(button_label(&view, Button::Pause), "Resume");
    }

    #[test]
    fn unicode_typing_selection_delete_backspace_and_search_paste() {
        let mut view = populated(2);
        assert_eq!(view.insert_search("a😀日本語"), Action::SearchChanged);
        assert!(view.rows.is_empty());
        assert_eq!(view.caret(), 5);
        view.key(0x25, false, true, false);
        view.key(0x25, false, true, false);
        assert_eq!(view.search_selection(), Some((3, 5)));
        assert_eq!(view.character('é'), Action::SearchChanged);
        assert_eq!(view.query, "a😀日é");
        view.key(0x24, false, false, false);
        view.key(0x2E, false, false, false);
        assert_eq!(view.query, "😀日é");
        view.key(0x23, false, false, false);
        view.key(0x08, false, false, false);
        assert_eq!(view.query, "😀日");
        assert_eq!(view.key(0x41, true, false, false), Action::Redraw);
        assert_eq!(view.search_selection(), Some((0, 2)));
        assert_eq!(view.key(0x56, true, false, false), Action::PasteSearch);
        assert_eq!(
            view.insert_search("replacement\r\ntext\t\0"),
            Action::SearchChanged
        );
        assert_eq!(view.query, "replacement  text ");
    }

    #[test]
    fn unicode_word_editing_and_selection_collapse() {
        let mut view = View::default();
        view.insert_search("éé 日本 😀");
        view.key(0x25, true, false, false);
        assert_eq!(view.caret(), 6);
        view.key(0x08, true, false, false);
        assert_eq!(view.query, "éé 😀");
        view.key(0x24, false, false, false);
        view.key(0x27, false, true, false);
        view.key(0x27, false, true, false);
        view.key(0x25, false, false, false);
        assert_eq!(view.caret(), 0);
        assert_eq!(view.search_selection(), None);
        view.key(0x2E, true, false, false);
        assert_eq!(view.query, "😀");
    }

    #[test]
    fn search_is_bounded_and_bare_digits_are_text_not_quick_slots() {
        let mut view = populated(3);
        assert_eq!(view.key(0x31, false, false, false), Action::None);
        assert_eq!(view.character('1'), Action::SearchChanged);
        assert_eq!(view.query, "1");
        view.key(0x41, true, false, false);
        view.insert_search(&"😀".repeat(300));
        assert_eq!(view.query.chars().count(), 256);
        assert_eq!(view.character('a'), Action::None);
        assert_eq!(view.character('\0'), Action::None);
        view.key(0x41, true, false, false);
        assert_eq!(view.key(0x08, false, false, false), Action::SearchChanged);
        assert!(view.query.is_empty());
    }

    #[test]
    fn filters_request_full_text_store_search_and_clear_stale_rows() {
        let mut view = populated(3);
        for filter in [
            Filter::Text,
            Filter::Images,
            Filter::Files,
            Filter::Pinned,
            Filter::All,
        ] {
            assert_eq!(view.set_filter(filter), Action::SearchChanged);
            assert_eq!(view.filter, filter);
            assert!(view.loading);
            assert_eq!(view.selected_row(), None);
            assert_eq!(view.key(0x0D, false, false, false), Action::None);
            view.set_rows(vec![row(100, Kind::Text)]);
        }
        let rect = FILTERS[1].1;
        assert_eq!(
            view.click(rect.x + 2.0, rect.y + 2.0, false),
            Action::SearchChanged
        );
        assert_eq!(view.filter, Filter::Text);
    }

    #[test]
    fn stable_ids_survive_reorders_repeats_and_do_not_retarget_pruning() {
        let mut view = populated(4);
        view.selected = Some(ItemId(3));
        view.set_rows(vec![
            row(4, Kind::Text),
            row(3, Kind::Text),
            row(3, Kind::Image),
            row(1, Kind::Text),
        ]);
        assert_eq!(view.rows.len(), 3);
        assert_eq!(view.selected_row().unwrap().id, ItemId(3));
        view.set_rows(vec![row(4, Kind::Text), row(1, Kind::Text)]);
        assert_eq!(view.selected, None);
        assert_eq!(view.key(0x43, true, false, false), Action::None);
        view.set_rows(vec![row(4, Kind::Text), row(1, Kind::Text)]);
        assert_eq!(view.selected, None);
        view.key(0x28, false, false, false);
        assert_eq!(view.selected, Some(ItemId(4)));
    }

    #[test]
    fn visible_alt_slots_and_wheel_use_exact_rows_only() {
        let mut view = populated(12);
        assert_eq!(view.wheel(-60), Action::None);
        assert_eq!(view.wheel(-60), Action::Redraw);
        assert_eq!(view.scroll, 1);
        assert_eq!(
            view.key(0x31, false, false, true),
            Action::Paste { plain: false }
        );
        assert_eq!(view.selected, Some(ItemId(2)));
        assert_eq!(
            view.key(0x35, false, false, true),
            Action::Paste { plain: false }
        );
        assert_eq!(view.selected, Some(ItemId(6)));
        assert_eq!(view.key(0x36, false, false, true), Action::None);
        assert_eq!(view.key(0x39, false, false, true), Action::None);
        view.wheel(i32::MIN);
        assert_eq!(view.scroll, 7);
        view.wheel(i32::MAX);
        assert_eq!(view.scroll, 0);
    }

    #[test]
    fn arrows_page_home_end_and_scroll_keep_selection_visible() {
        let mut view = populated(12);
        view.key(0x28, false, false, false);
        assert_eq!(view.selected, Some(ItemId(2)));
        view.key(0x22, false, false, false);
        assert_eq!(view.selected, Some(ItemId(7)));
        assert_eq!(view.scroll, 2);
        view.key(0x23, false, false, false);
        assert_eq!(view.selected, Some(ItemId(12)));
        assert_eq!(view.scroll, 7);
        view.key(0x21, false, false, false);
        assert_eq!(view.selected, Some(ItemId(7)));
        view.key(0x24, false, false, false);
        view.key(0x26, false, false, false);
        assert_eq!(view.selected, Some(ItemId(1)));
        assert_eq!(view.scroll, 0);
    }

    #[test]
    fn selection_double_click_and_keyboard_actions_do_not_mutate_history() {
        let mut view = populated(3);
        let rect = row_rect(1).unwrap();
        assert_eq!(
            view.click(rect.x + 1.0, rect.y + 1.0, false),
            Action::Redraw
        );
        assert_eq!(view.selected, Some(ItemId(2)));
        assert_eq!(
            view.click(rect.x + 1.0, rect.y + 1.0, true),
            Action::Paste { plain: false }
        );
        assert_eq!(
            view.key(0x0D, false, true, false),
            Action::Paste { plain: true }
        );
        assert_eq!(
            view.key(0x43, true, false, false),
            Action::Copy { plain: false }
        );
        assert_eq!(view.key(0x50, true, false, false), Action::TogglePin);
        assert!(!view.selected_row().unwrap().pinned);
        assert_eq!(view.key(0x2E, false, false, false), Action::Delete);
        assert_eq!(view.rows.len(), 3);
        view.status = Some("Synthetic write failure".into());
        assert_eq!(view.selected, Some(ItemId(2)));
        assert_eq!(view.rows.len(), 3);
    }

    #[test]
    fn corrupt_items_only_allow_history_deletion() {
        let mut view = populated(1);
        view.rows[0].corrupt = true;
        for button in [
            Button::Copy,
            Button::Paste,
            Button::Pin,
            Button::Preview,
            Button::Drag,
        ] {
            assert!(!button_enabled(&view, button));
            assert_eq!(click_button(&mut view, button), Action::None);
        }
        assert_eq!(click_button(&mut view, Button::Delete), Action::Delete);
    }

    #[test]
    fn image_plain_paste_and_text_drag_are_unavailable() {
        let mut view = populated(1);
        assert_eq!(click_button(&mut view, Button::Drag), Action::None);
        view.rows[0].kind = Kind::Image;
        view.focus = Focus::List;
        assert_eq!(view.key(0x0D, false, true, false), Action::None);
        assert_eq!(click_button(&mut view, Button::Drag), Action::Drag);
        assert!(button_tooltip(&view, Button::Drag).contains("Pictures/Daijobu/Clipboard"));
        view.rows[0].kind = Kind::Files;
        view.focus = Focus::List;
        assert_eq!(
            view.key(0x0D, false, true, false),
            Action::Paste { plain: true }
        );
    }

    #[test]
    fn preview_is_in_panel_and_blocks_row_hits() {
        let mut view = populated(3);
        assert_eq!(click_button(&mut view, Button::Preview), Action::Preview);
        assert!(view.preview);
        assert_eq!(hit_test(&view, 20.0, 110.0), Hit::None);
        assert_eq!(view.wheel(-120), Action::None);
        assert_eq!(view.key(0x1B, false, false, false), Action::Close);
        assert_eq!(click_button(&mut view, Button::Preview), Action::Preview);
        assert!(!view.preview);
        assert_eq!(view.key(0x1B, false, false, false), Action::Close);
    }

    #[test]
    fn clear_defaults_to_unpinned_and_all_requires_explicit_checkbox_and_confirm() {
        let mut view = populated(3);
        view.rows[1].pinned = true;
        assert_eq!(
            click_button(&mut view, Button::Clear),
            Action::RequestClearAll
        );
        assert!(view.confirm_clear_all);
        assert!(!view.include_pinned);
        assert_eq!(view.focus, Focus::Button(Button::CancelClear));
        assert_eq!(view.key(0x0D, false, false, false), Action::CancelClearAll);
        assert_eq!(view.rows.len(), 3);
        click_button(&mut view, Button::Clear);
        assert_eq!(
            click_button(&mut view, Button::ConfirmClear),
            Action::ClearUnpinned
        );
        assert_eq!(view.rows.len(), 3);
        click_button(&mut view, Button::Clear);
        assert_eq!(
            click_button(&mut view, Button::IncludePinned),
            Action::Redraw
        );
        assert!(view.include_pinned);
        assert_eq!(view.character('y'), Action::None);
        assert_eq!(view.key(0x43, true, false, false), Action::None);
        assert_eq!(
            click_button(&mut view, Button::ConfirmClear),
            Action::ConfirmClearAll
        );
        assert!(!view.confirm_clear_all);
        assert!(!view.include_pinned);
        assert_eq!(view.rows.len(), 3);
    }

    #[test]
    fn modal_tab_cycle_and_escape_cannot_leak_destructive_keys() {
        let mut view = populated(3);
        click_button(&mut view, Button::Clear);
        assert_eq!(view.key(0x2E, false, false, false), Action::None);
        view.key(0x09, false, false, false);
        assert_eq!(view.focus, Focus::Button(Button::IncludePinned));
        view.key(0x20, false, false, false);
        assert!(view.include_pinned);
        view.key(0x09, false, false, false);
        assert_eq!(view.focus, Focus::Button(Button::ConfirmClear));
        assert_eq!(view.key(0x1B, false, false, false), Action::CancelClearAll);
        click_button(&mut view, Button::Clear);
        assert!(!view.include_pinned);
        view.key(0x09, false, true, false);
        assert_eq!(view.focus, Focus::Button(Button::ConfirmClear));
        assert_eq!(view.key(0x0D, false, false, false), Action::ClearUnpinned);
    }

    #[test]
    fn tab_reaches_every_enabled_action_and_filter_without_mouse() {
        let mut view = populated(2);
        view.rows[0].kind = Kind::Files;
        let mut visited = Vec::new();
        for _ in 0..20 {
            view.key(0x09, false, false, false);
            visited.push(view.focus);
        }
        for (button, _) in HEADER_BUTTONS.into_iter().chain(FOOTER_BUTTONS) {
            assert!(visited.contains(&Focus::Button(button)));
        }
        for (filter, _) in FILTERS {
            assert!(visited.contains(&Focus::Filter(filter)));
        }
        view.focus = Focus::Button(Button::Pin);
        assert_eq!(view.key(0x20, false, false, false), Action::TogglePin);
        assert_eq!(view.character(' '), Action::None);
        view.focus = Focus::Filter(Filter::All);
        assert_eq!(view.key(0x20, false, false, false), Action::Redraw);
        assert_eq!(view.character(' '), Action::None);
    }

    #[test]
    fn presentation_metadata_is_bounded_and_bad_thumbnail_dropped() {
        let mut view = View::default();
        let mut large = row(1, Kind::Image);
        large.preview = "😀".repeat(5000);
        large.source = Some("x".repeat(10000));
        large.thumbnail = Some(crate::clipboard::Thumbnail {
            width: u32::MAX,
            height: 1,
            bgra: vec![],
        });
        view.set_rows(vec![large]);
        assert_eq!(view.rows[0].preview.len(), MAX_PREVIEW_BYTES);
        assert_eq!(
            view.rows[0].source.as_ref().unwrap().len(),
            MAX_PREVIEW_BYTES
        );
        assert!(view.rows[0].thumbnail.is_none());
    }

    #[test]
    fn hit_geometry_matches_rows_buttons_and_nonfinite_points() {
        let view = populated(8);
        for slot in 0..VISIBLE_ROWS {
            let rect = row_rect(slot).unwrap();
            assert_eq!(
                hit_test(&view, rect.x + 1.0, rect.y + 1.0),
                Hit::Row(ItemId(slot as u64 + 1))
            );
        }
        assert_eq!(row_rect(VISIBLE_ROWS), None);
        assert_eq!(hit_test(&view, f32::NAN, 100.0), Hit::None);
        assert_eq!(hit_test(&view, 400.0, 100.0), Hit::None);
        assert_eq!(hit_test(&view, 12.0, 320.0), Hit::None);
    }
}
