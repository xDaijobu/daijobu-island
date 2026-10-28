//! A one-line text editor model (the Settings search box, the Spotlight query): the text
//! and a caret counted in chars, so multi-byte text is safe. Pure.

use crate::layout::Rect;
use crate::settings_ui::FieldView;

/// The longest text, in chars.
pub const MAX_LEN: usize = 256;

pub const VK_BACK: u16 = 0x08;
pub const VK_END: u16 = 0x23;
pub const VK_HOME: u16 = 0x24;
pub const VK_LEFT: u16 = 0x25;
pub const VK_RIGHT: u16 = 0x27;
pub const VK_DELETE: u16 = 0x2E;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineEdit {
    text: String,
    /// In chars, `0..=len`.
    caret: usize,
}

impl LineEdit {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// Byte offset of char index `i`.
    fn byte(&self, i: usize) -> usize {
        self.text
            .char_indices()
            .nth(i)
            .map_or(self.text.len(), |(b, _)| b)
    }

    /// Replaces the text (newlines become spaces, other control chars are dropped, cut to
    /// `MAX_LEN`); the caret goes to the end.
    pub fn set_text(&mut self, s: &str) {
        self.text = s
            .chars()
            .map(|c| {
                if matches!(c, '\n' | '\r' | '\t') {
                    ' '
                } else {
                    c
                }
            })
            .filter(|c| !c.is_control())
            .take(MAX_LEN)
            .collect();
        self.caret = self.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.caret = 0;
    }

    /// Inserts one char at the caret. Control chars and a full field are ignored.
    pub fn insert_char(&mut self, c: char) -> bool {
        if c.is_control() || self.len() >= MAX_LEN {
            return false;
        }
        let b = self.byte(self.caret);
        self.text.insert(b, c);
        self.caret += 1;
        true
    }

    /// Inserts text at the caret; newlines and tabs become spaces, other control chars
    /// are dropped, and it stops at `MAX_LEN`.
    pub fn insert_str(&mut self, s: &str) -> bool {
        let mut changed = false;
        for c in s.chars() {
            let c = if matches!(c, '\n' | '\r' | '\t') {
                ' '
            } else {
                c
            };
            changed |= self.insert_char(c);
        }
        changed
    }

    pub fn backspace(&mut self) -> bool {
        if self.caret == 0 {
            return false;
        }
        let (a, b) = (self.byte(self.caret - 1), self.byte(self.caret));
        self.text.replace_range(a..b, "");
        self.caret -= 1;
        true
    }

    pub fn delete(&mut self) -> bool {
        if self.caret >= self.len() {
            return false;
        }
        let (a, b) = (self.byte(self.caret), self.byte(self.caret + 1));
        self.text.replace_range(a..b, "");
        true
    }

    pub fn left(&mut self) -> bool {
        let moved = self.caret > 0;
        self.caret = self.caret.saturating_sub(1);
        moved
    }

    pub fn right(&mut self) -> bool {
        let moved = self.caret < self.len();
        if moved {
            self.caret += 1;
        }
        moved
    }

    pub fn home(&mut self) -> bool {
        let moved = self.caret != 0;
        self.caret = 0;
        moved
    }

    pub fn end(&mut self) -> bool {
        let n = self.len();
        let moved = self.caret != n;
        self.caret = n;
        moved
    }

    /// The caret index one word to the left: over spaces, then over the word.
    fn word_left_index(&self) -> usize {
        let cs: Vec<char> = self.text.chars().collect();
        let mut i = self.caret;
        while i > 0 && cs[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !cs[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    pub fn word_left(&mut self) -> bool {
        let i = self.word_left_index();
        let moved = i != self.caret;
        self.caret = i;
        moved
    }

    /// One word to the right: over the word, then over the spaces after it.
    pub fn word_right(&mut self) -> bool {
        let cs: Vec<char> = self.text.chars().collect();
        let mut i = self.caret;
        while i < cs.len() && !cs[i].is_whitespace() {
            i += 1;
        }
        while i < cs.len() && cs[i].is_whitespace() {
            i += 1;
        }
        let moved = i != self.caret;
        self.caret = i;
        moved
    }

    /// Ctrl+Backspace: deletes back to the start of the word.
    pub fn backspace_word(&mut self) -> bool {
        let i = self.word_left_index();
        if i == self.caret {
            return false;
        }
        let (a, b) = (self.byte(i), self.byte(self.caret));
        self.text.replace_range(a..b, "");
        self.caret = i;
        true
    }

    /// Handles an editing / caret key. Returns whether the text or the caret changed.
    pub fn key(&mut self, vk: u16, ctrl: bool, _shift: bool) -> bool {
        match vk {
            VK_BACK if ctrl => self.backspace_word(),
            VK_BACK => self.backspace(),
            VK_DELETE => self.delete(),
            VK_LEFT if ctrl => self.word_left(),
            VK_LEFT => self.left(),
            VK_RIGHT if ctrl => self.word_right(),
            VK_RIGHT => self.right(),
            VK_HOME => self.home(),
            VK_END => self.end(),
            _ => false,
        }
    }

    pub fn view(&self, rect: Rect, focused: bool, placeholder: &'static str) -> FieldView {
        FieldView {
            rect,
            text: self.text.clone(),
            caret: self.caret,
            focused,
            placeholder,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le(s: &str) -> LineEdit {
        let mut e = LineEdit::new();
        e.insert_str(s);
        e
    }

    #[test]
    fn typing_inserts_at_the_caret() {
        let mut e = le("ac");
        e.left();
        assert!(e.insert_char('b'));
        assert_eq!((e.text(), e.caret()), ("abc", 2));
    }

    #[test]
    fn control_chars_are_ignored_and_newlines_become_spaces() {
        let mut e = LineEdit::new();
        assert!(!e.insert_char('\u{8}'));
        assert!(!e.insert_char('\r'));
        assert!(!e.insert_char('\u{7f}'));
        assert!(e.is_empty());
        e.insert_str("a\r\nb\tc\u{1}");
        assert_eq!(e.text(), "a  b c");
    }

    #[test]
    fn multibyte_text_edits_by_char() {
        let mut e = le("aé日b");
        assert_eq!(e.caret(), 4);
        e.left();
        e.left();
        assert!(e.backspace());
        assert_eq!((e.text(), e.caret()), ("a日b", 1));
        assert!(e.delete());
        assert_eq!((e.text(), e.caret()), ("ab", 1));
        e.insert_char('😀');
        assert_eq!(e.text(), "a😀b");
        assert_eq!(e.caret(), 2);
        assert!(e.end());
        assert!(e.backspace());
        assert!(e.backspace());
        assert_eq!(e.text(), "a");
    }

    #[test]
    fn edges_do_nothing() {
        let mut e = le("ab");
        assert!(!e.delete());
        assert!(!e.right());
        e.home();
        assert!(!e.backspace());
        assert!(!e.left());
        assert!(!e.home());
        assert!(e.end());
        assert!(!e.end());
    }

    #[test]
    fn word_moves_and_word_backspace() {
        let mut e = le("foo bar  baz");
        assert!(e.word_left());
        assert_eq!(e.caret(), 9);
        assert!(e.word_left());
        assert_eq!(e.caret(), 4);
        assert!(e.word_right());
        assert_eq!(e.caret(), 9);
        assert!(e.word_right());
        assert_eq!(e.caret(), 12);
        assert!(!e.word_right());
        assert!(e.backspace_word());
        assert_eq!(e.text(), "foo bar  ");
        assert!(e.backspace_word());
        assert_eq!(e.text(), "foo ");
        assert!(e.backspace_word());
        assert_eq!(e.text(), "");
        assert!(!e.backspace_word());
    }

    #[test]
    fn keys_map_to_edits_and_report_changes() {
        let mut e = le("hello world");
        assert!(e.key(VK_LEFT, false, false));
        assert!(e.key(VK_LEFT, true, false));
        assert_eq!(e.caret(), 6);
        assert!(e.key(VK_HOME, false, false));
        assert!(!e.key(VK_HOME, false, false));
        assert!(e.key(VK_DELETE, false, false));
        assert_eq!(e.text(), "ello world");
        assert!(e.key(VK_RIGHT, true, false));
        assert_eq!(e.caret(), 5);
        assert!(e.key(VK_BACK, true, false));
        assert_eq!(e.text(), "world");
        assert!(e.key(VK_END, false, false));
        assert!(e.key(VK_BACK, false, false));
        assert_eq!(e.text(), "worl");
        assert!(!e.key(0x41, false, false));
    }

    #[test]
    fn the_length_is_capped() {
        let mut e = LineEdit::new();
        e.insert_str(&"x".repeat(MAX_LEN + 50));
        assert_eq!(e.text().chars().count(), MAX_LEN);
        assert!(!e.insert_char('y'));
        e.set_text(&"é".repeat(MAX_LEN + 5));
        assert_eq!(e.text().chars().count(), MAX_LEN);
        assert_eq!(e.caret(), MAX_LEN);
    }

    #[test]
    fn set_text_moves_the_caret_to_the_end_and_clear_resets() {
        let mut e = LineEdit::new();
        e.set_text("a\nb");
        assert_eq!((e.text(), e.caret()), ("a b", 3));
        e.clear();
        assert_eq!((e.text(), e.caret()), ("", 0));
    }

    #[test]
    fn the_view_carries_text_caret_and_focus() {
        let e = le("ab");
        let r = Rect::new(1.0, 2.0, 3.0, 4.0);
        let v = e.view(r, true, "Search");
        assert_eq!(v.rect, r);
        assert_eq!((v.text.as_str(), v.caret, v.focused), ("ab", 2, true));
        assert_eq!(v.placeholder, "Search");
    }
}
