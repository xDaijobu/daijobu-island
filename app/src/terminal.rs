//! The terminal panel's controller: the shell session behind it, what the keyboard and the
//! mouse do to it, and what the panel shows. The session, the spawner and the clipboard
//! are traits, so the tests drive it with fakes; the app plugs in `dj_term` and
//! `dj_win::clipboard`.

use dj_core::config::TerminalConfig;
use dj_core::layout::{Rect, TERM_CLOSE};
use dj_core::terminal::{self as term, KeyAction, Modes, Mods, Palette, Screen, WheelAccum};
use dj_term::{SpawnOptions, TermEvent};
use std::path::PathBuf;

/// `Ctrl+Alt+``: open or close the panel.
pub const HOTKEY_TERMINAL: i32 = 19;

const VK_RETURN: u16 = 0x0D;

/// A running shell: `dj_term::Session` in the app.
pub trait Backend {
    fn write(&self, bytes: Vec<u8>);
    fn resize(&mut self, cols: u16, rows: u16, cell_w: u16, cell_h: u16);
    fn scroll(&mut self, lines: i32);
    fn scroll_to_bottom(&mut self);
    fn select_start(&mut self, col: usize, row: usize, right_half: bool, word: bool);
    fn select_update(&mut self, col: usize, row: usize, right_half: bool);
    fn select_clear(&mut self);
    fn has_selection(&self) -> bool;
    fn selection_text(&self) -> Option<String>;
    fn modes(&self) -> Modes;
    fn screen(&self, palette: &Palette, focused: bool) -> Screen;
    fn take_events(&mut self) -> Vec<TermEvent>;
}

impl Backend for dj_term::Session {
    fn write(&self, bytes: Vec<u8>) {
        dj_term::Session::write(self, bytes);
    }
    fn resize(&mut self, cols: u16, rows: u16, cell_w: u16, cell_h: u16) {
        dj_term::Session::resize(self, cols, rows, cell_w, cell_h);
    }
    fn scroll(&mut self, lines: i32) {
        dj_term::Session::scroll(self, lines);
    }
    fn scroll_to_bottom(&mut self) {
        dj_term::Session::scroll_to_bottom(self);
    }
    fn select_start(&mut self, col: usize, row: usize, right_half: bool, word: bool) {
        dj_term::Session::select_start(self, col, row, right_half, word);
    }
    fn select_update(&mut self, col: usize, row: usize, right_half: bool) {
        dj_term::Session::select_update(self, col, row, right_half);
    }
    fn select_clear(&mut self) {
        dj_term::Session::select_clear(self);
    }
    fn has_selection(&self) -> bool {
        dj_term::Session::has_selection(self)
    }
    fn selection_text(&self) -> Option<String> {
        dj_term::Session::selection_text(self)
    }
    fn modes(&self) -> Modes {
        dj_term::Session::modes(self)
    }
    fn screen(&self, palette: &Palette, focused: bool) -> Screen {
        dj_term::Session::screen(self, palette, focused)
    }
    fn take_events(&mut self) -> Vec<TermEvent> {
        dj_term::Session::take_events(self)
    }
}

/// Starts shells: `dj_term::Session::spawn` with the app's wake in the app.
pub trait Spawner {
    fn spawn(&self, opts: &SpawnOptions) -> Result<Box<dyn Backend>, String>;
}

/// The text clipboard: `dj_win::clipboard` in the app.
pub trait Clipboard {
    fn get(&self) -> Option<String>;
    fn set(&self, text: &str) -> bool;
}

/// The real spawner: a ConPTY session whose output wakes the island's loop.
pub struct SessionSpawner {
    /// The island window, as a raw handle (the wake posts to it).
    pub hwnd: isize,
}

impl Spawner for SessionSpawner {
    fn spawn(&self, opts: &SpawnOptions) -> Result<Box<dyn Backend>, String> {
        let hwnd = self.hwnd;
        let wake = Box::new(move || dj_win::window::wake(hwnd));
        dj_term::Session::spawn(opts, wake).map(|s| Box::new(s) as Box<dyn Backend>)
    }
}

/// The system clipboard's text, owned by the island window.
pub struct TextClipboard {
    pub owner: isize,
}

impl Clipboard for TextClipboard {
    fn get(&self) -> Option<String> {
        dj_win::clipboard::get_text(self.owner)
    }
    fn set(&self, text: &str) -> bool {
        dj_win::clipboard::set_text(self.owner, text)
    }
}

/// What the app does after an input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    /// The panel shows something new.
    Redraw,
    /// The close button: collapse the panel and give the keyboard back.
    Close,
}

/// What the panel draws this frame.
pub struct View {
    pub title: String,
    pub screen: Screen,
    pub notice: Option<String>,
    pub close_hover: bool,
}

pub struct Terminal {
    config: TerminalConfig,
    spawner: Box<dyn Spawner>,
    clipboard: Box<dyn Clipboard>,
    session: Option<Box<dyn Backend>>,
    /// The program the session runs (its file stem titles the header until the shell
    /// sets a title).
    program: String,
    /// One cell in device px, the scale it was measured at, and the grid that fits.
    cell: (u16, u16),
    scale: f32,
    grid: (u16, u16),
    title: Option<String>,
    /// The shell ended (with its exit code, if known), or never started.
    exited: Option<Option<i32>>,
    failed: Option<String>,
    /// The `WM_CHAR` that follows a key already sent (or used as a shortcut) is dropped.
    skip_char: bool,
    selecting: bool,
    focused: bool,
    close_hover: bool,
    /// A touchpad's small wheel steps add up to whole lines.
    wheel: WheelAccum,
}

impl Terminal {
    pub fn new(
        config: TerminalConfig,
        spawner: Box<dyn Spawner>,
        clipboard: Box<dyn Clipboard>,
    ) -> Self {
        Terminal {
            config,
            spawner,
            clipboard,
            session: None,
            program: String::new(),
            cell: (8, 16),
            scale: 1.0,
            grid: (80, 24),
            title: None,
            exited: None,
            failed: None,
            skip_char: false,
            selecting: false,
            focused: false,
            close_hover: false,
            wheel: WheelAccum::default(),
        }
    }

    pub fn config(&self) -> &TerminalConfig {
        &self.config
    }

    /// The panel opens with cells of `cell` device px at `scale`: the shell starts on the
    /// first open (or after it failed to), and a running one is resized if the grid
    /// changed.
    pub fn open(&mut self, cell: (u16, u16), scale: f32) {
        self.measure(cell, scale);
        if self.session.is_none() && self.exited.is_none() {
            self.start();
        }
    }

    /// New cell metrics (a DPI or font change): resizes the grid if it changed.
    pub fn measure(&mut self, cell: (u16, u16), scale: f32) {
        self.cell = cell;
        self.scale = scale;
        let grid = term::grid_fit(cell.0, cell.1, scale);
        if grid != self.grid {
            self.grid = grid;
            if let Some(s) = self.session.as_mut() {
                s.resize(grid.0, grid.1, cell.0, cell.1);
            }
        }
    }

    /// The panel has the keyboard (a block cursor) or not (a hollow one).
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn start(&mut self) {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        let cwd = if self.config.cwd.trim().is_empty() {
            home
        } else {
            Some(PathBuf::from(self.config.cwd.trim()))
        };
        let (program, args) = dj_term::resolve_shell(&self.config.shell, &self.config.args);
        let opts = SpawnOptions {
            program: program.clone(),
            args,
            cwd,
            cols: self.grid.0,
            rows: self.grid.1,
            cell_w: self.cell.0,
            cell_h: self.cell.1,
            scrollback: self.config.scrollback as usize,
        };
        self.program = program;
        self.title = None;
        self.exited = None;
        self.failed = None;
        match self.spawner.spawn(&opts) {
            Ok(s) => self.session = Some(s),
            Err(e) => {
                self.session = None;
                self.exited = Some(None);
                self.failed = Some(e);
            }
        }
    }

    /// The shell is gone (exited, or failed to start): Enter starts a new one.
    fn ended(&self) -> bool {
        self.exited.is_some()
    }

    /// A key went down while the panel has the keyboard.
    pub fn key(&mut self, vk: u16, mods: Mods) -> Outcome {
        // A char only ever follows its own key, so a new key ends any skip.
        self.skip_char = false;
        if self.ended() {
            if vk == VK_RETURN {
                self.skip_char = true;
                self.session = None;
                self.start();
                return Outcome::Redraw;
            }
            return Outcome::None;
        }
        let Some(s) = self.session.as_mut() else {
            return Outcome::None;
        };
        let action = term::on_key(vk, mods, s.modes(), s.has_selection());
        // Ctrl+C / Ctrl+V and the keys sent here also make a char that must not follow.
        self.skip_char = action.skips_char(vk);
        match action {
            KeyAction::Send(bytes) | KeyAction::SendSkipChar(bytes) => {
                Self::send(s.as_mut(), bytes);
                Outcome::Redraw
            }
            KeyAction::Copy => self.copy(),
            KeyAction::Paste => self.paste(),
            KeyAction::PassToChar => Outcome::None,
        }
    }

    /// A character typed while the panel has the keyboard.
    pub fn char(&mut self, ch: char, mods: Mods) -> Outcome {
        if std::mem::take(&mut self.skip_char) || self.ended() {
            return Outcome::None;
        }
        let Some(s) = self.session.as_mut() else {
            return Outcome::None;
        };
        Self::send(s.as_mut(), term::on_char(ch, mods));
        Outcome::Redraw
    }

    /// Typing follows the output again and drops the selection, as in Windows Terminal.
    fn send(s: &mut dyn Backend, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        s.select_clear();
        s.scroll_to_bottom();
        s.write(bytes);
    }

    fn copy(&mut self) -> Outcome {
        let Some(s) = self.session.as_mut() else {
            return Outcome::None;
        };
        if let Some(text) = s.selection_text().filter(|t| !t.is_empty()) {
            self.clipboard.set(&text);
        }
        s.select_clear();
        Outcome::Redraw
    }

    fn paste(&mut self) -> Outcome {
        let Some(text) = self.clipboard.get().filter(|t| !t.is_empty()) else {
            return Outcome::None;
        };
        let Some(s) = self.session.as_mut() else {
            return Outcome::None;
        };
        let bytes = term::paste_bytes(&text, s.modes());
        Self::send(s.as_mut(), bytes);
        Outcome::Redraw
    }

    /// The cell under a panel-local point, if the session is up.
    fn cell_at(&self, x: f32, y: f32) -> Option<(usize, usize, bool)> {
        self.session.as_ref()?;
        let (cw, ch) = self.cell;
        term::cell_at(x, y, cw, ch, self.scale, self.grid.0, self.grid.1)
    }

    /// The left button went down at a panel-local point: the close button, or the start
    /// of a selection.
    pub fn press(&mut self, x: f32, y: f32) -> Outcome {
        if Self::close_rect().contains(x, y) {
            return Outcome::Close;
        }
        let Some((col, row, right)) = self.cell_at(x, y) else {
            return Outcome::None;
        };
        if let Some(s) = self.session.as_mut() {
            s.select_clear();
            s.select_start(col, row, right, false);
            self.selecting = true;
        }
        Outcome::Redraw
    }

    /// A double click selects the word under the pointer.
    pub fn double_click(&mut self, x: f32, y: f32) -> Outcome {
        let Some((col, row, right)) = self.cell_at(x, y) else {
            return Outcome::None;
        };
        if let Some(s) = self.session.as_mut() {
            s.select_clear();
            s.select_start(col, row, right, true);
            self.selecting = true;
        }
        Outcome::Redraw
    }

    pub fn drag(&mut self, x: f32, y: f32) -> Outcome {
        if !self.selecting {
            return Outcome::None;
        }
        let Some((col, row, right)) = self.cell_at(x, y) else {
            return Outcome::None;
        };
        if let Some(s) = self.session.as_mut() {
            s.select_update(col, row, right);
        }
        Outcome::Redraw
    }

    pub fn release(&mut self) {
        self.selecting = false;
    }

    /// Right click: copies a selection, or pastes when there is none.
    pub fn right_click(&mut self) -> Outcome {
        let selected = self.session.as_ref().is_some_and(|s| s.has_selection());
        if selected {
            self.copy()
        } else {
            self.paste()
        }
    }

    pub fn wheel(&mut self, delta: i32) -> Outcome {
        let lines = self.wheel.feed(delta);
        match self.session.as_mut() {
            Some(s) if lines != 0 => {
                s.scroll(lines);
                Outcome::Redraw
            }
            _ => Outcome::None,
        }
    }

    /// The pointer moved over the panel (panel-local). True if the close button's hover
    /// changed.
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let over = Self::close_rect().contains(x, y);
        std::mem::replace(&mut self.close_hover, over) != over
    }

    pub fn clear_hover(&mut self) -> bool {
        std::mem::take(&mut self.close_hover)
    }

    fn close_rect() -> Rect {
        TERM_CLOSE
    }

    /// The session's news since the last call (title, exit). True if the panel needs a
    /// frame: there was a wake, so the grid may have changed too.
    pub fn drain(&mut self) -> bool {
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        for ev in s.take_events() {
            match ev {
                TermEvent::Title(t) => self.title = Some(t).filter(|t| !t.trim().is_empty()),
                TermEvent::ResetTitle => self.title = None,
                TermEvent::Bell => {}
                TermEvent::Exited(code) => {
                    self.exited = Some(code);
                    self.selecting = false;
                }
            }
        }
        true
    }

    /// What the panel draws.
    pub fn view(&self, palette: &Palette) -> View {
        let screen = match self.session.as_ref() {
            Some(s) => s.screen(palette, self.focused),
            None => Screen::blank(self.grid.0 as usize, self.grid.1 as usize, palette),
        };
        View {
            title: self.header_title(),
            screen,
            notice: self.notice(),
            close_hover: self.close_hover,
        }
    }

    fn header_title(&self) -> String {
        if let Some(t) = self.title.as_ref() {
            return t.clone();
        }
        std::path::Path::new(&self.program)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "terminal".to_string())
    }

    fn notice(&self) -> Option<String> {
        if let Some(e) = self.failed.as_ref() {
            return Some(format!(
                "could not start {}: {e} · Enter to retry",
                self.program
            ));
        }
        match self.exited {
            Some(Some(code)) => Some(format!(
                "process exited with code {code} · Enter to restart"
            )),
            Some(None) => Some("process exited · Enter to restart".to_string()),
            None => None,
        }
    }

    /// Ends the shell (the island is quitting).
    pub fn shutdown(&mut self) {
        self.session = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::terminal::vk;
    use dj_core::theme::{ThemeRegistry, DEFAULT_THEME};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// What the fake shell saw, shared with the test.
    #[derive(Default)]
    struct Shell {
        writes: Vec<Vec<u8>>,
        resizes: Vec<(u16, u16)>,
        scrolled: i32,
        at_bottom: u32,
        /// (col, row, word) of the last selection start, and updates since.
        selection: Option<(usize, usize, bool)>,
        updates: Vec<(usize, usize)>,
        selected_text: Option<String>,
        modes: Modes,
        events: Vec<TermEvent>,
    }

    struct FakeBackend(Rc<RefCell<Shell>>);

    impl Backend for FakeBackend {
        fn write(&self, bytes: Vec<u8>) {
            self.0.borrow_mut().writes.push(bytes);
        }
        fn resize(&mut self, cols: u16, rows: u16, _: u16, _: u16) {
            self.0.borrow_mut().resizes.push((cols, rows));
        }
        fn scroll(&mut self, lines: i32) {
            self.0.borrow_mut().scrolled += lines;
        }
        fn scroll_to_bottom(&mut self) {
            self.0.borrow_mut().at_bottom += 1;
        }
        fn select_start(&mut self, col: usize, row: usize, _: bool, word: bool) {
            let mut s = self.0.borrow_mut();
            s.selection = Some((col, row, word));
            s.updates.clear();
        }
        fn select_update(&mut self, col: usize, row: usize, _: bool) {
            self.0.borrow_mut().updates.push((col, row));
        }
        fn select_clear(&mut self) {
            self.0.borrow_mut().selection = None;
        }
        fn has_selection(&self) -> bool {
            self.0.borrow().selection.is_some()
        }
        fn selection_text(&self) -> Option<String> {
            let s = self.0.borrow();
            s.selection.and(s.selected_text.clone())
        }
        fn modes(&self) -> Modes {
            self.0.borrow().modes
        }
        fn screen(&self, palette: &Palette, _: bool) -> Screen {
            Screen::blank(4, 2, palette)
        }
        fn take_events(&mut self) -> Vec<TermEvent> {
            std::mem::take(&mut self.0.borrow_mut().events)
        }
    }

    /// Hands out fake shells (the latest is `current`), or fails while `fail` is set.
    #[derive(Default)]
    struct Spawns {
        opts: Vec<SpawnOptions>,
        current: Rc<RefCell<Shell>>,
        fail: bool,
    }

    struct FakeSpawner(Rc<RefCell<Spawns>>);

    impl Spawner for FakeSpawner {
        fn spawn(&self, opts: &SpawnOptions) -> Result<Box<dyn Backend>, String> {
            let mut s = self.0.borrow_mut();
            s.opts.push(opts.clone());
            if s.fail {
                return Err("shell not found: nope.exe".to_string());
            }
            s.current = Rc::new(RefCell::new(Shell::default()));
            Ok(Box::new(FakeBackend(Rc::clone(&s.current))))
        }
    }

    struct FakeClipboard(Rc<RefCell<Option<String>>>);

    impl Clipboard for FakeClipboard {
        fn get(&self) -> Option<String> {
            self.0.borrow().clone()
        }
        fn set(&self, text: &str) -> bool {
            *self.0.borrow_mut() = Some(text.to_string());
            true
        }
    }

    struct Rig {
        term: Terminal,
        spawns: Rc<RefCell<Spawns>>,
        clip: Rc<RefCell<Option<String>>>,
    }

    impl Rig {
        fn new() -> Self {
            let spawns = Rc::new(RefCell::new(Spawns::default()));
            let clip = Rc::new(RefCell::new(None));
            let term = Terminal::new(
                TerminalConfig::default(),
                Box::new(FakeSpawner(Rc::clone(&spawns))),
                Box::new(FakeClipboard(Rc::clone(&clip))),
            );
            Rig { term, spawns, clip }
        }

        fn open(&mut self) {
            self.term.open((8, 17), 1.0);
            self.term.set_focused(true);
        }

        fn shell(&self) -> Rc<RefCell<Shell>> {
            Rc::clone(&self.spawns.borrow().current)
        }

        fn writes(&self) -> Vec<Vec<u8>> {
            self.shell().borrow().writes.clone()
        }

        /// A key and the char Windows makes for it, as the window procedure sends them.
        fn type_key(&mut self, vk: u16, ch: Option<char>, mods: Mods) {
            self.term.key(vk, mods);
            if let Some(c) = ch {
                self.term.char(c, mods);
            }
        }

        fn select(&mut self, text: &str) {
            let shell = self.shell();
            let mut s = shell.borrow_mut();
            s.selection = Some((0, 0, false));
            s.selected_text = Some(text.to_string());
        }
    }

    const NONE: Mods = Mods {
        ctrl: false,
        shift: false,
        alt: false,
    };
    const CTRL: Mods = Mods {
        ctrl: true,
        shift: false,
        alt: false,
    };
    const SHIFT: Mods = Mods {
        ctrl: false,
        shift: true,
        alt: false,
    };

    fn palette() -> Palette {
        let reg = ThemeRegistry::with_builtins();
        term::palette(reg.get(DEFAULT_THEME).unwrap())
    }

    #[test]
    fn the_first_open_starts_one_shell_sized_to_the_panel() {
        let mut r = Rig::new();
        r.open();
        r.open();
        let spawns = r.spawns.borrow();
        assert_eq!(spawns.opts.len(), 1, "a second open keeps the session");
        let o = &spawns.opts[0];
        assert_eq!((o.cols, o.rows), term::grid_fit(8, 17, 1.0));
        assert_eq!((o.cell_w, o.cell_h), (8, 17));
        assert_eq!(o.scrollback, TerminalConfig::default().scrollback as usize);
        assert!(!o.program.is_empty());
    }

    #[test]
    fn a_new_cell_size_resizes_the_running_shell_once() {
        let mut r = Rig::new();
        r.open();
        r.term.measure((8, 17), 1.0);
        assert!(r.shell().borrow().resizes.is_empty(), "same grid");
        r.term.measure((12, 25), 1.5);
        let want = term::grid_fit(12, 25, 1.5);
        assert_eq!(r.shell().borrow().resizes, vec![want]);
    }

    #[test]
    fn typing_sends_and_follows_the_output() {
        let mut r = Rig::new();
        r.open();
        r.select("old");
        r.type_key(0x41, Some('a'), NONE);
        r.type_key(vk::LEFT, None, NONE);
        assert_eq!(r.writes(), vec![b"a".to_vec(), b"\x1b[D".to_vec()]);
        let shell = r.shell();
        let s = shell.borrow();
        assert_eq!(s.at_bottom, 2);
        assert!(s.selection.is_none(), "typing drops the selection");
    }

    #[test]
    fn ctrl_c_copies_a_selection_and_its_char_never_reaches_the_shell() {
        let mut r = Rig::new();
        r.open();
        r.select("cargo build");
        r.type_key(0x11, None, CTRL); // Ctrl itself
        r.type_key(vk::C, Some('\u{3}'), CTRL);
        assert_eq!(r.clip.borrow().as_deref(), Some("cargo build"));
        assert!(r.writes().is_empty(), "{:?}", r.writes());
        assert!(r.shell().borrow().selection.is_none());
        // Without a selection Ctrl+C interrupts.
        r.type_key(vk::C, Some('\u{3}'), CTRL);
        assert_eq!(r.writes(), vec![vec![3]]);
    }

    #[test]
    fn ctrl_v_pastes_bracketed_and_shift_insert_eats_no_char() {
        let mut r = Rig::new();
        r.open();
        r.shell().borrow_mut().modes.bracketed_paste = true;
        *r.clip.borrow_mut() = Some("echo 1\r\necho 2".to_string());
        r.type_key(vk::V, Some('\u{16}'), CTRL);
        assert_eq!(
            r.writes(),
            vec![b"\x1b[200~echo 1\recho 2\x1b[201~".to_vec()]
        );
        // Shift+Insert makes no char: the next letter must still arrive.
        r.type_key(vk::INSERT, None, SHIFT);
        r.type_key(0x58, Some('x'), NONE);
        let w = r.writes();
        assert_eq!(w.len(), 3, "{w:?}");
        assert_eq!(w[2], b"x".to_vec());
    }

    #[test]
    fn backspace_sends_del_once() {
        let mut r = Rig::new();
        r.open();
        r.type_key(vk::BACK, Some('\u{8}'), NONE);
        assert_eq!(r.writes(), vec![vec![0x7f]]);
    }

    #[test]
    fn an_exited_shell_shows_a_notice_and_enter_restarts_it() {
        let mut r = Rig::new();
        r.open();
        r.shell()
            .borrow_mut()
            .events
            .push(TermEvent::Exited(Some(1)));
        assert!(r.term.drain());
        let v = r.term.view(&palette());
        assert!(
            v.notice.as_deref().unwrap().contains("code 1"),
            "{:?}",
            v.notice
        );
        // Typing goes nowhere until Enter.
        let old = r.shell();
        r.type_key(0x41, Some('a'), NONE);
        assert!(old.borrow().writes.is_empty());
        r.type_key(VK_RETURN, Some('\r'), NONE);
        assert_eq!(r.spawns.borrow().opts.len(), 2, "a new shell");
        assert!(
            r.writes().is_empty(),
            "the Enter that restarted it isn't sent"
        );
        assert!(r.term.view(&palette()).notice.is_none());
    }

    #[test]
    fn a_shell_that_cannot_start_says_so_and_enter_retries() {
        let mut r = Rig::new();
        r.spawns.borrow_mut().fail = true;
        r.open();
        let notice = r.term.view(&palette()).notice.unwrap();
        assert!(notice.contains("could not start"), "{notice}");
        assert!(notice.contains("nope.exe"), "{notice}");
        r.spawns.borrow_mut().fail = false;
        r.type_key(VK_RETURN, Some('\r'), NONE);
        assert_eq!(r.spawns.borrow().opts.len(), 2);
        assert!(r.term.view(&palette()).notice.is_none());
        r.type_key(0x41, Some('a'), NONE);
        assert_eq!(r.writes(), vec![b"a".to_vec()]);
    }

    #[test]
    fn the_close_button_closes_and_the_text_area_selects() {
        let mut r = Rig::new();
        r.open();
        let c = TERM_CLOSE;
        assert_eq!(
            r.term.press(c.x + c.w / 2.0, c.y + c.h / 2.0),
            Outcome::Close
        );
        let t = dj_core::layout::TERM_TEXT;
        assert_eq!(r.term.press(t.x + 2.0, t.y + 2.0), Outcome::Redraw);
        assert_eq!(r.shell().borrow().selection, Some((0, 0, false)));
        assert_eq!(r.term.drag(t.x + 40.0, t.y + 2.0), Outcome::Redraw);
        assert_eq!(r.shell().borrow().updates.len(), 1);
        r.term.release();
        assert_eq!(r.term.drag(t.x + 80.0, t.y + 2.0), Outcome::None);
        // A double click starts a word selection.
        r.term.double_click(t.x + 2.0, t.y + 2.0);
        assert_eq!(r.shell().borrow().selection, Some((0, 0, true)));
    }

    #[test]
    fn right_click_copies_a_selection_or_pastes() {
        let mut r = Rig::new();
        r.open();
        r.select("ls");
        r.term.right_click();
        assert_eq!(r.clip.borrow().as_deref(), Some("ls"));
        assert!(r.writes().is_empty());
        r.term.right_click();
        assert_eq!(r.writes(), vec![b"ls".to_vec()]);
    }

    #[test]
    fn the_wheel_scrolls_in_whole_lines() {
        let mut r = Rig::new();
        r.open();
        assert_eq!(r.term.wheel(120), Outcome::Redraw);
        assert_eq!(r.shell().borrow().scrolled, 3);
        // A touchpad's small steps add up.
        assert_eq!(r.term.wheel(20), Outcome::None);
        r.term.wheel(20);
        assert_eq!(r.shell().borrow().scrolled, 4);
        r.term.wheel(-240);
        assert_eq!(r.shell().borrow().scrolled, -2);
    }

    #[test]
    fn the_header_shows_the_shell_title_or_its_name() {
        let mut r = Rig::new();
        r.open();
        let named = r.term.view(&palette()).title;
        assert!(!named.is_empty() && !named.contains('\\'), "{named}");
        r.shell()
            .borrow_mut()
            .events
            .push(TermEvent::Title("~/projects".into()));
        r.term.drain();
        assert_eq!(r.term.view(&palette()).title, "~/projects");
        r.shell().borrow_mut().events.push(TermEvent::ResetTitle);
        r.term.drain();
        assert_eq!(r.term.view(&palette()).title, named);
    }

    #[test]
    fn the_close_hover_reports_changes_only() {
        let mut r = Rig::new();
        let c = TERM_CLOSE;
        assert!(r.term.hover(c.x + 2.0, c.y + 2.0));
        assert!(!r.term.hover(c.x + 3.0, c.y + 3.0));
        assert!(r.term.view(&palette()).close_hover);
        assert!(r.term.hover(0.0, 200.0));
        assert!(!r.term.clear_hover());
    }
}
