//! The terminal panel's controller: its tabs and the shell session behind each, what the
//! keyboard and the mouse do to them, and what the panel shows. Sessions, the spawner and
//! the clipboard are traits, so the tests drive it with fakes; the app plugs in `dj_term`
//! and `dj_win::clipboard`.

use dj_core::config::TerminalConfig;
use dj_core::layout::{TERM_CLOSE, TERM_HEADER_H, TERM_TEXT};
use dj_core::line_edit::LineEdit;
use dj_core::term_agents::{self as agents, Agent};
use dj_core::term_tabs::{
    self as tabs, MenuAction, PinnedTab, Press, Release, TabBarView, TabId, TabKey, Tabs,
};
use dj_core::terminal::{self as term, KeyAction, Modes, Mods, Palette, Screen, WheelAccum};
use dj_core::terminal_shortcuts::Shortcut;
use dj_term::{SpawnOptions, TermEvent};
use std::collections::HashMap;
use std::path::PathBuf;

/// `Ctrl+Alt+``: open or close the panel.
pub const HOTKEY_TERMINAL: i32 = 19;

const VK_RETURN: u16 = 0x0D;

/// A running shell: `dj_term::Session` in the app.
pub trait Backend {
    fn write(&self, bytes: Vec<u8>);
    fn resize(&mut self, cols: u16, rows: u16, cell_w: u16, cell_h: u16);
    fn wheel(&mut self, lines: i32, col: usize, row: usize);
    fn scroll_to_bottom(&mut self);
    fn select_start(&mut self, col: usize, row: usize, right_half: bool, word: bool);
    fn select_update(&mut self, col: usize, row: usize, right_half: bool);
    fn select_clear(&mut self);
    fn has_selection(&self) -> bool;
    fn selection_text(&self) -> Option<String>;
    fn modes(&self) -> Modes;
    fn memory_counters(&self) -> Option<(usize, usize, usize)> {
        None
    }
    fn screen(&self, palette: &Palette, focused: bool) -> Screen;
    /// Whether it printed since the last call, and its title / bell / exit events.
    fn drain(&mut self) -> (bool, Vec<TermEvent>);
}

impl Backend for dj_term::Session {
    fn memory_counters(&self) -> Option<(usize, usize, usize)> {
        dj_term::Session::memory_counters(self)
    }
    fn write(&self, bytes: Vec<u8>) {
        dj_term::Session::write(self, bytes);
    }
    fn resize(&mut self, cols: u16, rows: u16, cell_w: u16, cell_h: u16) {
        dj_term::Session::resize(self, cols, rows, cell_w, cell_h);
    }
    fn wheel(&mut self, lines: i32, col: usize, row: usize) {
        dj_term::Session::wheel(self, lines, col, row);
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
    fn drain(&mut self) -> (bool, Vec<TermEvent>) {
        dj_term::Session::drain(self)
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
    /// The close button, or the last tab closed: collapse the panel and give the keyboard
    /// back.
    Close,
}

/// What the panel draws this frame.
pub struct View {
    /// The active tab's title (the header's text when there are no tabs to draw).
    pub title: String,
    pub screen: Screen,
    pub notice: Option<String>,
    pub close_hover: bool,
    pub tabs: TabBarView,
}

/// What a tab's shell is started with (kept to restart it, and to save a pinned tab).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Spec {
    shell: String,
    args: Vec<String>,
    /// As configured or saved: empty is the user's home.
    cwd: String,
    /// A coding agent started in yolo mode inside the (PowerShell) shell.
    agent: Option<Agent>,
}

/// One tab's shell.
struct Tab {
    spec: Spec,
    session: Option<Box<dyn Backend>>,
    /// The program actually run (its file stem names the tab until the shell sets a title).
    program: String,
    /// The shell's own title, or the saved name of a restored pinned tab.
    title: Option<String>,
    custom_title: Option<String>,
    /// The shell ended (with its exit code, if known), or never started.
    exited: Option<Option<i32>>,
    failed: Option<String>,
    /// The folder the shell last reported (PowerShell tabs, through the prompt hook).
    cwd: Option<String>,
}

impl Tab {
    fn name(&self) -> String {
        if let Some(t) = self.custom_title.as_ref() {
            return t.clone();
        }
        self.automatic_name()
    }

    fn automatic_name(&self) -> String {
        if let Some(t) = self.title.as_ref() {
            return t.clone();
        }
        if let Some(a) = self.spec.agent {
            return a.command().to_string();
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
}

/// A title that is only a path to an executable says nothing: Windows titles a console with
/// the exe that started in it (PowerShell's own path, or `cmd.exe` when Codex runs a command).
fn is_exe_path(title: &str) -> bool {
    let t = title.trim().trim_matches('"');
    let p = std::path::Path::new(t);
    p.is_absolute() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
}

struct Rename {
    id: TabId,
    edit: LineEdit,
    selected: bool,
}

pub struct Terminal {
    config: TerminalConfig,
    spawner: Box<dyn Spawner>,
    clipboard: Box<dyn Clipboard>,
    tabs: Tabs,
    sessions: HashMap<TabId, Tab>,
    /// Pinned tabs saved by an earlier run, started without opening the panel.
    restore: Vec<PinnedTab>,
    /// The pinned tabs changed: the app saves `pinned_toml` (see `take_save`).
    save: bool,
    /// One cell in device px, the scale it was measured at, and the grid that fits.
    cell: (u16, u16),
    scale: f32,
    grid: (u16, u16),
    /// The `WM_CHAR` that follows a key already sent (or used as a shortcut) is dropped.
    skip_char: bool,
    selecting: bool,
    /// The last pointer position over the panel (panel-local): a release has none of its
    /// own.
    pointer: (f32, f32),
    focused: bool,
    close_hover: bool,
    /// A touchpad's small wheel steps add up to whole lines.
    wheel: WheelAccum,
    rename: Option<Rename>,
}

impl Terminal {
    pub fn memory_counters(&self) -> dj_core::dev_memory::Row {
        let live = self
            .sessions
            .values()
            .filter_map(|t| t.session.as_ref())
            .count();
        let counters: Vec<_> = self
            .sessions
            .values()
            .filter_map(|t| t.session.as_ref()?.memory_counters())
            .collect();
        let history: usize = counters.iter().map(|(_, _, h)| h).sum();
        let cells: usize = counters.iter().map(|(c, r, h)| c * (r + h)).sum();
        dj_core::dev_memory::Row::resource("Terminal scrollback", format!("{history} lines · {cells} cells"), format!("Keeps shell output for scrolling. {} tabs / {live} backends; {}/{live} readable, busy skipped. Counts, not allocated RAM.", self.sessions.len(), counters.len()))
    }

    pub fn new(
        config: TerminalConfig,
        spawner: Box<dyn Spawner>,
        clipboard: Box<dyn Clipboard>,
        pinned: Vec<PinnedTab>,
    ) -> Self {
        Terminal {
            config,
            spawner,
            clipboard,
            tabs: Tabs::new(),
            sessions: HashMap::new(),
            restore: pinned,
            save: false,
            cell: (8, 16),
            scale: 1.0,
            grid: (80, 24),
            skip_char: false,
            selecting: false,
            pointer: (0.0, 0.0),
            focused: false,
            close_hover: false,
            wheel: WheelAccum::default(),
            rename: None,
        }
    }

    pub fn config(&self) -> &TerminalConfig {
        &self.config
    }

    /// Live terminal tabs in display order. The shell stays live after an agent returns
    /// to its prompt; failed starts and exited shells have no shortcut.
    pub fn shortcuts(&self) -> Vec<Shortcut> {
        self.tabs
            .ids()
            .into_iter()
            .filter_map(|id| {
                let tab = self.sessions.get(&id)?;
                (tab.session.is_some() && tab.exited.is_none()).then_some(Shortcut {
                    id,
                    agent: tab.spec.agent,
                    active: self.tabs.active() == Some(id),
                    activity: self.tabs.has_activity(id),
                })
            })
            .collect()
    }

    /// Select an existing live session. In particular, this never restores, restarts or
    /// creates a shell in response to a stale shortcut.
    pub fn activate_shortcut(&mut self, id: TabId) -> bool {
        if !self.tabs.ids().contains(&id)
            || !self
                .sessions
                .get(&id)
                .is_some_and(|t| t.session.is_some() && t.exited.is_none())
        {
            return false;
        }
        self.on_hide();
        self.skip_char = false;
        self.tabs.activate(id);
        true
    }

    /// The exact currently selected live tab, suitable for a later Clipboard target.
    pub fn active_tab_id(&self) -> Option<TabId> {
        let id = self.tabs.active()?;
        self.sessions
            .get(&id)
            .filter(|t| t.session.is_some() && t.exited.is_none())
            .map(|_| id)
    }

    /// Selects and pastes into this existing live session only. A stale target never
    /// falls back to another tab or starts a shell, and no Enter is appended.
    pub fn paste_to_tab(&mut self, id: TabId, text: &str) -> Result<(), String> {
        if !self.activate_shortcut(id) {
            return Err("Terminal target is no longer live".into());
        }
        let session = self
            .sessions
            .get_mut(&id)
            .and_then(|t| t.session.as_mut())
            .ok_or_else(|| "Terminal target is no longer live".to_string())?;
        let bytes = term::paste_bytes(text, session.modes());
        Self::send(session.as_mut(), bytes);
        Ok(())
    }

    /// New `[terminal]` settings (from the Settings window). Tabs opened from now on use
    /// the new shell; running ones keep theirs. The app remeasures the cells after a font
    /// change (`measure`).
    pub fn set_config(&mut self, config: TerminalConfig) {
        self.config = config;
    }

    /// The panel opens with cells of `cell` device px at `scale`. Any pending pinned
    /// tabs come back first; with no tab at all it opens one.
    pub fn open(&mut self, cell: (u16, u16), scale: f32) {
        self.restore_pinned(cell, scale);
        if self.tabs.is_empty() {
            self.new_tab();
        }
    }

    /// Opens the panel on a new tab in `dir` (Spotlight's "Open in Terminal", "Claude
    /// here"), running `agent` in yolo mode if given. Pinned tabs come back first, and no
    /// default tab is opened besides this one.
    pub fn open_in(&mut self, cell: (u16, u16), scale: f32, dir: String, agent: Option<Agent>) {
        self.restore_pinned(cell, scale);
        let mut spec = match agent {
            Some(a) => self.agent_spec(a),
            None => self.config_spec(),
        };
        spec.cwd = dir;
        if !self.add_tab(spec, None, false) && self.tabs.is_empty() {
            self.new_tab();
        }
    }

    /// Restore saved sessions without showing the panel or creating a default shell.
    /// Safe to call again when the panel opens or its cell metrics change.
    pub fn restore_pinned(&mut self, cell: (u16, u16), scale: f32) {
        self.measure(cell, scale);
        for p in std::mem::take(&mut self.restore) {
            let spec = Spec {
                shell: p.shell,
                args: p.args,
                cwd: p.cwd,
                agent: Agent::from_key(&p.agent),
            };
            let title = Some(p.title).filter(|t| !t.trim().is_empty());
            if self.add_tab(spec, title, true) {
                let id = self.tabs.active().expect("added tab is active");
                if let Some(tab) = self.sessions.get_mut(&id) {
                    tab.custom_title = Some(p.custom_title).filter(|t| !t.trim().is_empty());
                    self.tabs.set_title(id, tab.name());
                }
            }
        }
    }

    /// New cell metrics (a DPI or font change): every shell's grid follows.
    pub fn measure(&mut self, cell: (u16, u16), scale: f32) {
        self.cell = cell;
        self.scale = scale;
        let grid = term::grid_fit(cell.0, cell.1, scale);
        if grid != self.grid {
            self.grid = grid;
            for t in self.sessions.values_mut() {
                if let Some(s) = t.session.as_mut() {
                    s.resize(grid.0, grid.1, cell.0, cell.1);
                }
            }
        }
    }

    /// The panel has the keyboard (a block cursor) or not (a hollow one).
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// The panel is going away: a press or a drag in progress, and the menu, end.
    pub fn on_hide(&mut self) {
        self.rename = None;
        self.tabs.cancel();
        self.tabs.close_menu();
        self.tabs.clear_hover();
        self.selecting = false;
        self.close_hover = false;
    }

    fn config_spec(&self) -> Spec {
        Spec {
            shell: self.config.shell.clone(),
            args: self.config.args.clone(),
            cwd: self.config.cwd.clone(),
            agent: None,
        }
    }

    /// A new regular tab with the configured shell, made active. False at the limit.
    fn new_tab(&mut self) -> bool {
        self.add_tab(self.config_spec(), None, false)
    }

    /// A new regular tab running `agent` in yolo mode, in the active tab's folder (the
    /// configured one when that is unknown or gone). The agents are npm PowerShell shims, so
    /// the tab is the configured shell if it is PowerShell, else Windows PowerShell.
    fn new_agent_tab(&mut self, agent: Agent) -> bool {
        let mut spec = self.agent_spec(agent);
        // The folder the active shell last reported, else the one it started in (an agent
        // tab that has not reached a prompt yet has reported nothing).
        if let Some(dir) = self.active().and_then(|t| {
            [t.cwd.clone(), Some(t.spec.cwd.trim().to_string())]
                .into_iter()
                .flatten()
                .find(|d| !d.is_empty() && std::path::Path::new(d).is_dir())
        }) {
            spec.cwd = dir;
        }
        self.add_tab(spec, None, false)
    }

    /// A tab running `agent` (the configured folder): the agents are npm PowerShell shims,
    /// so the shell is the configured one if it is PowerShell, else Windows PowerShell.
    fn agent_spec(&self, agent: Agent) -> Spec {
        let mut spec = self.config_spec();
        let (program, _) = dj_term::resolve_shell(&spec.shell, &spec.args);
        if !dj_term::is_powershell(&program) {
            spec.shell = "powershell".to_string();
            spec.args = Vec::new();
        }
        spec.agent = Some(agent);
        spec
    }

    fn add_tab(&mut self, spec: Spec, title: Option<String>, pinned: bool) -> bool {
        let Some(id) = self.tabs.add(String::new(), pinned) else {
            return false;
        };
        let mut tab = Tab {
            spec,
            session: None,
            program: String::new(),
            title,
            custom_title: None,
            exited: None,
            failed: None,
            cwd: None,
        };
        self.start(&mut tab);
        self.tabs.set_title(id, tab.name());
        self.tabs.set_exited(id, tab.exited.is_some());
        self.sessions.insert(id, tab);
        true
    }

    /// Starts (or restarts) `tab`'s shell.
    fn start(&self, tab: &mut Tab) {
        let cwd = if tab.spec.cwd.trim().is_empty() {
            std::env::var_os("USERPROFILE").map(PathBuf::from)
        } else {
            Some(PathBuf::from(tab.spec.cwd.trim()))
        };
        let (program, mut args) = dj_term::resolve_shell(&tab.spec.shell, &tab.spec.args);
        // PowerShell reports its folder at every prompt, and runs the tab's agent if it has one.
        if dj_term::is_powershell(&program) {
            args.extend(agents::powershell_extra_args(
                &tab.spec.args,
                tab.spec.agent,
            ));
        }
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
        tab.program = program;
        tab.exited = None;
        tab.failed = None;
        match self.spawner.spawn(&opts) {
            Ok(s) => tab.session = Some(s),
            Err(e) => {
                tab.session = None;
                tab.exited = Some(None);
                tab.failed = Some(e);
            }
        }
    }

    fn active_id(&self) -> Option<TabId> {
        self.tabs.active()
    }

    fn active(&self) -> Option<&Tab> {
        self.active_id().and_then(|id| self.sessions.get(&id))
    }

    fn active_mut(&mut self) -> Option<&mut Tab> {
        let id = self.active_id()?;
        self.sessions.get_mut(&id)
    }

    fn active_session(&mut self) -> Option<&mut Box<dyn Backend>> {
        self.active_mut()?.session.as_mut()
    }

    /// Closes tabs (their shells end). `Close` when none is left.
    fn close_tabs(&mut self, ids: &[TabId]) -> Outcome {
        for id in ids {
            if let Some(t) = self.sessions.remove(id) {
                if self.tabs.is_pinned(*id) {
                    self.save = true;
                }
                drop(t);
            }
        }
        self.selecting = false;
        if self.tabs.is_empty() {
            Outcome::Close
        } else {
            Outcome::Redraw
        }
    }

    fn close_tab(&mut self, id: TabId) -> Outcome {
        let was_pinned = self.tabs.is_pinned(id);
        if !self.tabs.close(id) {
            return Outcome::None;
        }
        self.save |= was_pinned;
        self.sessions.remove(&id);
        self.selecting = false;
        if self.tabs.is_empty() {
            Outcome::Close
        } else {
            Outcome::Redraw
        }
    }

    /// The pinned tabs as saved, in their order; the app writes it when `take_save` says
    /// they changed.
    pub fn pinned_toml(&self) -> String {
        let pinned: Vec<PinnedTab> = self
            .tabs
            .ids()
            .into_iter()
            .filter(|id| self.tabs.is_pinned(*id))
            .filter_map(|id| self.sessions.get(&id))
            .map(|t| PinnedTab {
                title: t.automatic_name(),
                shell: t.spec.shell.clone(),
                args: t.spec.args.clone(),
                cwd: t.cwd.clone().unwrap_or_else(|| t.spec.cwd.clone()),
                agent: t
                    .spec
                    .agent
                    .map(|a| a.key().to_string())
                    .unwrap_or_default(),
                custom_title: t.custom_title.clone().unwrap_or_default(),
            })
            .collect();
        tabs::pinned_to_toml(&pinned)
    }

    /// The file's new content if the pinned tabs changed since the last call.
    pub fn take_save(&mut self) -> Option<String> {
        std::mem::take(&mut self.save).then(|| self.pinned_toml())
    }

    fn pinned_order(&self) -> Vec<TabId> {
        self.tabs
            .ids()
            .into_iter()
            .filter(|id| self.tabs.is_pinned(*id))
            .collect()
    }

    fn tab_key(&mut self, key: TabKey) -> Outcome {
        match key {
            TabKey::New => {
                if self.new_tab() {
                    Outcome::Redraw
                } else {
                    Outcome::None
                }
            }
            TabKey::Close => match self.active_id() {
                Some(id) => self.close_tab(id),
                None => Outcome::None,
            },
            TabKey::Next => {
                self.tabs.next();
                Outcome::Redraw
            }
            TabKey::Prev => {
                self.tabs.prev();
                Outcome::Redraw
            }
            TabKey::Jump(i) => {
                self.tabs.jump(i);
                Outcome::Redraw
            }
        }
    }

    /// A key went down while the panel has the keyboard.
    pub fn key(&mut self, vk: u16, mods: Mods) -> Outcome {
        // A char only ever follows its own key, so a new key ends any skip.
        self.skip_char = false;
        if self.rename.is_some() {
            return self.rename_key(vk, mods);
        }
        if let Some(k) = tabs::shortcut(vk, mods.ctrl, mods.shift, mods.alt) {
            // Ctrl+Shift+T and its kin make a control char too: it must not follow.
            self.skip_char = true;
            return self.tab_key(k);
        }
        let ended = self.active().is_some_and(|t| t.exited.is_some());
        if ended {
            if vk == VK_RETURN {
                self.skip_char = true;
                let spawner_tab = self.active_id();
                if let Some(id) = spawner_tab {
                    if let Some(mut tab) = self.sessions.remove(&id) {
                        tab.session = None;
                        tab.title = None;
                        self.start(&mut tab);
                        self.tabs.set_title(id, tab.name());
                        self.tabs.set_exited(id, tab.exited.is_some());
                        self.sessions.insert(id, tab);
                    }
                }
                return Outcome::Redraw;
            }
            return Outcome::None;
        }
        let Some(s) = self.active_session() else {
            return Outcome::None;
        };
        let action = term::on_key(vk, mods, s.modes(), s.has_selection());
        let skip = action.skips_char(vk);
        let outcome = match action {
            KeyAction::Send(bytes) | KeyAction::SendSkipChar(bytes) => {
                Self::send(s.as_mut(), bytes);
                Outcome::Redraw
            }
            KeyAction::Copy => self.copy(),
            KeyAction::Paste => self.paste(),
            KeyAction::PassToChar => Outcome::None,
        };
        self.skip_char = skip;
        outcome
    }

    /// A character typed while the panel has the keyboard.
    pub fn char(&mut self, ch: char, mods: Mods) -> Outcome {
        if std::mem::take(&mut self.skip_char) {
            return Outcome::None;
        }
        if let Some(rename) = self.rename.as_mut() {
            if !mods.ctrl && !mods.alt && !ch.is_control() {
                if rename.selected {
                    rename.edit.clear();
                    rename.selected = false;
                }
                rename.edit.insert_char(ch);
            }
            return Outcome::Redraw;
        }
        if self.active().is_none_or(|t| t.exited.is_some()) {
            return Outcome::None;
        }
        let Some(s) = self.active_session() else {
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

    fn finish_rename(&mut self, apply: bool) -> Outcome {
        if let Some(rename) = self.rename.take().filter(|_| apply) {
            if let Some(tab) = self.sessions.get_mut(&rename.id) {
                let name = rename.edit.text().trim();
                let name = (!name.is_empty()).then(|| name.to_string());
                if name != tab.custom_title {
                    tab.custom_title = name;
                    self.tabs.set_title(rename.id, tab.name());
                    self.save |= self.tabs.is_pinned(rename.id);
                }
            }
        }
        Outcome::Redraw
    }

    fn rename_key(&mut self, vk: u16, mods: Mods) -> Outcome {
        // Editing is modal: no terminal/tab shortcut can escape into the shell.
        self.skip_char = vk == VK_RETURN || vk == 0x1b || vk == 0x08 || mods.ctrl;
        match vk {
            VK_RETURN => return self.finish_rename(true),
            0x1b => return self.finish_rename(false),
            _ => {}
        }
        let rename = self.rename.as_mut().expect("rename is open");
        if mods.ctrl && vk == 0x41 {
            rename.selected = true;
        } else if mods.ctrl && vk == 0x56 {
            if let Some(text) = self.clipboard.get() {
                if rename.selected {
                    rename.edit.clear();
                    rename.selected = false;
                }
                rename.edit.insert_str(&text);
            }
        } else if mods.ctrl && matches!(vk, 0x43 | 0x58) {
            if rename.selected {
                self.clipboard.set(rename.edit.text());
                if vk == 0x58 {
                    rename.edit.clear();
                    rename.selected = false;
                }
            }
        } else if !mods.alt {
            if rename.selected && matches!(vk, 0x08 | 0x2e) {
                rename.edit.clear();
                rename.selected = false;
            } else if matches!(vk, 0x23..=0x27) {
                if rename.selected && vk == 0x25 {
                    rename.edit.home();
                } else if rename.selected && vk == 0x27 {
                    rename.edit.end();
                } else {
                    rename.edit.key(vk, mods.ctrl, mods.shift);
                }
                rename.selected = false;
            } else {
                rename.edit.key(vk, mods.ctrl, mods.shift);
            }
        }
        Outcome::Redraw
    }

    fn copy(&mut self) -> Outcome {
        let Some(s) = self.active_session() else {
            return Outcome::None;
        };
        let text = s.selection_text().filter(|t| !t.is_empty());
        s.select_clear();
        if let Some(text) = text {
            self.clipboard.set(&text);
        }
        Outcome::Redraw
    }

    fn paste(&mut self) -> Outcome {
        let Some(text) = self.clipboard.get().filter(|t| !t.is_empty()) else {
            return Outcome::None;
        };
        let Some(s) = self.active_session() else {
            return Outcome::None;
        };
        let bytes = term::paste_bytes(&text, s.modes());
        Self::send(s.as_mut(), bytes);
        Outcome::Redraw
    }

    /// The cell under a panel-local point, if the active tab has a session.
    fn cell_at(&self, x: f32, y: f32) -> Option<(usize, usize, bool)> {
        self.active().and_then(|t| t.session.as_ref())?;
        let (cw, ch) = self.cell;
        term::cell_at(x, y, cw, ch, self.scale, self.grid.0, self.grid.1)
    }

    fn in_header(y: f32) -> bool {
        y < TERM_HEADER_H
    }

    /// The left button went down at a panel-local point: the panel's close button, the
    /// tab bar (or the open tab menu), or the start of a selection.
    pub fn press(&mut self, x: f32, y: f32) -> Outcome {
        if self.rename.is_some() {
            if tabs::RENAME_SAVE.contains(x, y) {
                return self.finish_rename(true);
            }
            if tabs::RENAME_CANCEL.contains(x, y) || !tabs::RENAME_DIALOG.contains(x, y) {
                return self.finish_rename(false);
            }
            return Outcome::Redraw;
        }
        self.pointer = (x, y);
        if self.tabs.menu_open() || Self::in_header(y) {
            if !self.tabs.menu_open() && TERM_CLOSE.contains(x, y) {
                return Outcome::Close;
            }
            return match self.tabs.press(x, y) {
                Press::None => Outcome::None,
                Press::Activated(_) | Press::MenuDismissed => Outcome::Redraw,
                Press::PlusMenu => Outcome::Redraw,
                Press::Menu(action, id) => self.menu_action(action, id),
            };
        }
        let Some((col, row, right)) = self.cell_at(x, y) else {
            return Outcome::None;
        };
        if let Some(s) = self.active_session() {
            s.select_clear();
            s.select_start(col, row, right, false);
            self.selecting = true;
        }
        Outcome::Redraw
    }

    fn menu_action(&mut self, action: MenuAction, id: TabId) -> Outcome {
        match action {
            MenuAction::Rename => {
                if let Some(tab) = self.sessions.get(&id) {
                    let mut edit = LineEdit::new();
                    edit.set_text(&tab.name());
                    self.tabs.cancel();
                    self.selecting = false;
                    self.rename = Some(Rename {
                        id,
                        edit,
                        selected: true,
                    });
                }
                Outcome::Redraw
            }
            MenuAction::Pin | MenuAction::Unpin => {
                self.tabs.set_pinned(id, action == MenuAction::Pin);
                self.save = true;
                Outcome::Redraw
            }
            MenuAction::Close => self.close_tab(id),
            MenuAction::CloseOthers => {
                let closed = self.tabs.close_others(id);
                self.close_tabs(&closed)
            }
            MenuAction::CloseRight => {
                let closed = self.tabs.close_right(id);
                self.close_tabs(&closed)
            }
            MenuAction::NewShell => {
                self.new_tab();
                Outcome::Redraw
            }
            MenuAction::NewClaude => {
                self.new_agent_tab(Agent::Claude);
                Outcome::Redraw
            }
            MenuAction::NewCodex => {
                self.new_agent_tab(Agent::Codex);
                Outcome::Redraw
            }
        }
    }

    /// A double click selects the word under the pointer.
    pub fn double_click(&mut self, x: f32, y: f32) -> Outcome {
        if self.rename.is_some() {
            return Outcome::None;
        }
        if Self::in_header(y) || self.tabs.menu_open() {
            return Outcome::None;
        }
        let Some((col, row, right)) = self.cell_at(x, y) else {
            return Outcome::None;
        };
        if let Some(s) = self.active_session() {
            s.select_clear();
            s.select_start(col, row, right, true);
            self.selecting = true;
        }
        Outcome::Redraw
    }

    pub fn drag(&mut self, x: f32, y: f32) -> Outcome {
        if self.rename.is_some() {
            return Outcome::None;
        }
        self.pointer = (x, y);
        if self.tabs.drag(x, y) {
            return Outcome::Redraw;
        }
        if !self.selecting {
            return Outcome::None;
        }
        // Dragging up into the header (or off the screen's top) keeps selecting the top row.
        let y = y.max(TERM_TEXT.y);
        let Some((col, row, right)) = self.cell_at(x, y) else {
            return Outcome::None;
        };
        if let Some(s) = self.active_session() {
            s.select_update(col, row, right);
        }
        Outcome::Redraw
    }

    /// The left button went up (where the pointer last was).
    pub fn release(&mut self) -> Outcome {
        if self.rename.is_some() {
            return Outcome::None;
        }
        self.selecting = false;
        let before = self.pinned_order();
        let (x, y) = self.pointer;
        match self.tabs.release(x, y) {
            Release::None => Outcome::None,
            Release::Close(id) => self.close_tab(id),
            Release::Reordered => {
                if self.pinned_order() != before {
                    self.save = true;
                }
                Outcome::Redraw
            }
        }
    }

    /// Right click: on a tab, its menu; elsewhere, copies a selection or pastes.
    pub fn right_click(&mut self, x: f32, y: f32) -> Outcome {
        if self.rename.is_some() {
            return Outcome::None;
        }
        if Self::in_header(y) || self.tabs.menu_open() {
            return if self.tabs.open_menu(x, y) {
                Outcome::Redraw
            } else {
                Outcome::None
            };
        }
        let selected = self
            .active()
            .and_then(|t| t.session.as_ref())
            .is_some_and(|s| s.has_selection());
        if selected {
            self.copy()
        } else {
            self.paste()
        }
    }

    /// Middle click: closes the regular tab under the pointer.
    pub fn middle_click(&mut self, x: f32, y: f32) -> Outcome {
        if self.rename.is_some() {
            return Outcome::None;
        }
        match self.tabs.middle_click(x, y) {
            Some(id) => self.close_tab(id),
            None => Outcome::None,
        }
    }

    pub fn wheel(&mut self, delta: i32, x: f32, y: f32) -> Outcome {
        if self.rename.is_some() || self.tabs.menu_open() || Self::in_header(y) {
            return Outcome::None;
        }
        let Some((col, row, _)) = self.cell_at(x, y) else {
            return Outcome::None;
        };
        let lines = self.wheel.feed(delta);
        match self.active_session() {
            Some(s) if lines != 0 => {
                s.wheel(lines, col, row);
                Outcome::Redraw
            }
            _ => Outcome::None,
        }
    }

    /// The pointer moved over the panel (panel-local). True if something hovered changed
    /// (the panel's close button, a tab, its ✕, "+", a menu row).
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        self.pointer = (x, y);
        let over = !self.tabs.menu_open() && TERM_CLOSE.contains(x, y);
        let close = std::mem::replace(&mut self.close_hover, over) != over;
        let bar = self.tabs.hover(x, y);
        close | bar
    }

    pub fn clear_hover(&mut self) -> bool {
        std::mem::take(&mut self.close_hover) | self.tabs.clear_hover()
    }

    /// The shells' news since the last call (output, titles, exits). True if the panel
    /// needs a frame.
    pub fn drain(&mut self) -> bool {
        let active = self.active_id();
        let mut changed = false;
        for (id, tab) in self.sessions.iter_mut() {
            let saved_name = self.tabs.is_pinned(*id).then(|| tab.name());
            let Some(s) = tab.session.as_mut() else {
                continue;
            };
            let (woke, events) = s.drain();
            let news = woke || !events.is_empty();
            for ev in events {
                match ev {
                    // A bare exe path (how Windows titles a console) keeps the tab name.
                    TermEvent::Title(t) => {
                        tab.title = Some(t).filter(|t| !t.trim().is_empty() && !is_exe_path(t))
                    }
                    TermEvent::ResetTitle => tab.title = None,
                    TermEvent::Bell => {}
                    TermEvent::Cwd(dir) => {
                        if tab.cwd.as_ref() != Some(&dir) {
                            self.save |= self.tabs.is_pinned(*id);
                            tab.cwd = Some(dir);
                        }
                    }
                    TermEvent::Exited(code) => tab.exited = Some(code),
                }
            }
            if saved_name.is_some_and(|name| name != tab.name()) {
                self.save = true;
            }
            self.tabs.set_title(*id, tab.name());
            self.tabs.set_exited(*id, tab.exited.is_some());
            if woke && Some(*id) != active {
                self.tabs.set_activity(*id);
            }
            changed |= news;
        }
        changed
    }

    /// What the panel draws.
    pub fn view(&self, palette: &Palette) -> View {
        let tab = self.active();
        let screen = match tab.and_then(|t| t.session.as_ref()) {
            Some(s) => s.screen(palette, self.focused),
            None => Screen::blank(self.grid.0 as usize, self.grid.1 as usize, palette),
        };
        let mut tabs = self.tabs.view();
        tabs.rename = self.rename.as_ref().map(|r| tabs::RenameView {
            field: r
                .edit
                .view(tabs::RENAME_FIELD, self.focused, "Automatic title"),
            selected: r.selected,
        });
        View {
            title: tab.map_or_else(|| "terminal".to_string(), Tab::name),
            screen,
            notice: tab.and_then(Tab::notice),
            close_hover: self.close_hover,
            tabs,
        }
    }

    /// Ends every shell (the island is quitting).
    pub fn shutdown(&mut self) {
        self.sessions.clear();
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
        wheel_cell: Option<(usize, usize)>,
        at_bottom: u32,
        /// (col, row, word) of the last selection start, and updates since.
        selection: Option<(usize, usize, bool)>,
        updates: Vec<(usize, usize)>,
        selected_text: Option<String>,
        modes: Modes,
        events: Vec<TermEvent>,
        woke: bool,
    }

    struct FakeBackend(Rc<RefCell<Shell>>);

    impl Backend for FakeBackend {
        fn write(&self, bytes: Vec<u8>) {
            self.0.borrow_mut().writes.push(bytes);
        }
        fn resize(&mut self, cols: u16, rows: u16, _: u16, _: u16) {
            self.0.borrow_mut().resizes.push((cols, rows));
        }
        fn wheel(&mut self, lines: i32, col: usize, row: usize) {
            let mut shell = self.0.borrow_mut();
            shell.scrolled += lines;
            shell.wheel_cell = Some((col, row));
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
        fn drain(&mut self) -> (bool, Vec<TermEvent>) {
            let mut s = self.0.borrow_mut();
            (std::mem::take(&mut s.woke), std::mem::take(&mut s.events))
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
                Vec::new(),
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
    fn clipboard_target_requires_a_live_session_and_never_starts_one() {
        let mut r = Rig::new();
        assert_eq!(r.term.active_tab_id(), None);
        assert!(r.term.paste_to_tab(1, "no target").is_err());
        assert!(r.spawns.borrow().opts.is_empty());
        r.spawns.borrow_mut().fail = true;
        r.open();
        let failed = r.term.active_id().unwrap();
        assert_eq!(r.term.active_tab_id(), None);
        assert!(r.term.paste_to_tab(failed, "do not retry").is_err());
        assert_eq!(r.spawns.borrow().opts.len(), 1);
    }

    #[test]
    fn clipboard_paste_selects_the_exact_background_tab_and_uses_its_bracketed_mode() {
        let mut r = Rig::new();
        r.open();
        let target = r.term.active_tab_id().unwrap();
        let first = r.shell();
        first.borrow_mut().modes.bracketed_paste = true;
        first.borrow_mut().selection = Some((0, 0, false));
        r.term.new_tab();
        let other = r.shell();
        *r.clip.borrow_mut() = Some("unrelated clipboard".into());
        r.term.paste_to_tab(target, "café\n日\u{1b}[31m").unwrap();
        assert_eq!(r.term.active_tab_id(), Some(target));
        assert_eq!(
            first.borrow().writes,
            ["\u{1b}[200~café\r日[31m\u{1b}[201~".as_bytes().to_vec()]
        );
        assert!(first.borrow().selection.is_none());
        assert_eq!(first.borrow().at_bottom, 1);
        assert!(other.borrow().writes.is_empty());
        assert_eq!(r.spawns.borrow().opts.len(), 2);
        assert_eq!(r.clip.borrow().as_deref(), Some("unrelated clipboard"));
    }

    #[test]
    fn clipboard_paste_rejects_exited_removed_and_unknown_tabs_without_touching_another() {
        let mut r = Rig::new();
        r.open();
        let target = r.term.active_tab_id().unwrap();
        let first = r.shell();
        r.term.new_tab();
        let other_id = r.term.active_tab_id();
        let other = r.shell();
        first.borrow_mut().events.push(TermEvent::Exited(Some(0)));
        r.term.drain();
        assert!(r.term.paste_to_tab(target, "exited").is_err());
        assert_eq!(r.term.active_tab_id(), other_id);
        r.term.close_tab(target);
        for id in [target, u64::MAX] {
            assert!(r.term.paste_to_tab(id, "removed").is_err());
            assert_eq!(r.term.active_tab_id(), other_id);
        }
        assert!(first.borrow().writes.is_empty());
        assert!(other.borrow().writes.is_empty());
        assert_eq!(other.borrow().at_bottom, 0);
        assert_eq!(r.spawns.borrow().opts.len(), 2);
    }

    #[test]
    fn clipboard_plain_paste_has_no_extra_enter_and_empty_paste_does_not_send() {
        let mut r = Rig::new();
        r.open();
        let target = r.term.active_tab_id().unwrap();
        r.term.paste_to_tab(target, "one\r\ntwo\tthree").unwrap();
        assert_eq!(r.writes(), [b"one\rtwo\tthree".to_vec()]);
        r.term.paste_to_tab(target, "").unwrap();
        r.term.paste_to_tab(target, "\0\u{1b}").unwrap();
        assert_eq!(r.writes(), [b"one\rtwo\tthree".to_vec()]);
        assert_eq!(r.shell().borrow().at_bottom, 1);
    }

    #[test]
    fn shortcuts_track_live_tabs_in_display_order_and_select_without_spawning() {
        let mut r = Rig::new();
        assert!(r.term.shortcuts().is_empty());
        r.open();
        let first = r.term.active_id().unwrap();
        let first_shell = r.shell();
        r.term.new_agent_tab(Agent::Claude);
        let claude = r.term.active_id().unwrap();
        r.term.new_agent_tab(Agent::Codex);
        let codex = r.term.active_id().unwrap();
        r.term.tabs.set_pinned(codex, true);
        first_shell.borrow_mut().woke = true;
        r.term.drain();
        assert_eq!(
            r.term.shortcuts(),
            vec![
                Shortcut {
                    id: codex,
                    agent: Some(Agent::Codex),
                    active: true,
                    activity: false
                },
                Shortcut {
                    id: first,
                    agent: None,
                    active: false,
                    activity: true
                },
                Shortcut {
                    id: claude,
                    agent: Some(Agent::Claude),
                    active: false,
                    activity: false
                },
            ]
        );
        r.term.on_hide();
        assert!(r.term.activate_shortcut(first));
        assert_eq!(r.term.active_id(), Some(first));
        assert!(!r.term.shortcuts()[1].activity);
        assert_eq!(r.spawns.borrow().opts.len(), 3);
        assert!(!r.term.activate_shortcut(u64::MAX));
        assert_eq!(r.term.active_id(), Some(first));
        r.term.close_tab(claude);
        assert!(!r.term.activate_shortcut(claude));
        assert_eq!(r.term.shortcuts().len(), 2);
        assert_eq!(r.spawns.borrow().opts.len(), 3);
    }

    #[test]
    fn failed_and_exited_shells_have_no_shortcuts_and_retries_restore_them() {
        let mut r = Rig::new();
        r.spawns.borrow_mut().fail = true;
        r.open();
        let id = r.term.active_id().unwrap();
        assert!(r.term.shortcuts().is_empty());
        assert!(!r.term.activate_shortcut(id));
        assert_eq!(r.spawns.borrow().opts.len(), 1);
        r.spawns.borrow_mut().fail = false;
        r.term.key(VK_RETURN, NONE);
        assert_eq!(r.term.shortcuts()[0].id, id);
        r.shell()
            .borrow_mut()
            .events
            .push(TermEvent::Exited(Some(0)));
        assert!(r.term.drain());
        assert!(r.term.shortcuts().is_empty());
        assert!(!r.term.activate_shortcut(id));
        r.term.key(VK_RETURN, NONE);
        assert_eq!(r.term.shortcuts()[0].id, id);
        r.term.close_tab(id);
        assert!(r.term.shortcuts().is_empty());
    }

    #[test]
    fn pinned_agents_get_shortcuts_only_after_their_shells_start() {
        let mut r = Rig::new();
        r.term.restore = vec![PinnedTab {
            title: "my agent".into(),
            shell: "powershell".into(),
            args: vec![],
            cwd: String::new(),
            agent: "claude".into(),
            custom_title: String::new(),
        }];
        assert!(r.term.shortcuts().is_empty());
        r.open();
        let shortcuts = r.term.shortcuts();
        assert_eq!(shortcuts.len(), 1);
        assert_eq!(shortcuts[0].agent, Some(Agent::Claude));
        assert!(r.term.activate_shortcut(shortcuts[0].id));
        assert_eq!(r.spawns.borrow().opts.len(), 1);
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
    fn a_selection_dragged_into_the_header_keeps_the_top_row() {
        let mut r = Rig::new();
        r.open();
        let t = dj_core::layout::TERM_TEXT;
        r.term.press(t.x + 30.0, t.y + 60.0);
        assert_eq!(r.term.drag(t.x + 10.0, 5.0), Outcome::Redraw);
        assert_eq!(r.term.drag(t.x + 10.0, -40.0), Outcome::Redraw);
        let updates = r.shell().borrow().updates.clone();
        assert_eq!(updates.len(), 2);
        assert!(updates.iter().all(|&(_, row)| row == 0), "{updates:?}");
    }

    #[test]
    fn right_click_copies_a_selection_or_pastes() {
        let mut r = Rig::new();
        r.open();
        let t = dj_core::layout::TERM_TEXT;
        r.select("ls");
        r.term.right_click(t.x + 4.0, t.y + 4.0);
        assert_eq!(r.clip.borrow().as_deref(), Some("ls"));
        assert!(r.writes().is_empty());
        r.term.right_click(t.x + 4.0, t.y + 4.0);
        assert_eq!(r.writes(), vec![b"ls".to_vec()]);
    }

    #[test]
    fn the_wheel_scrolls_in_whole_lines() {
        let mut r = Rig::new();
        r.open();
        let (x, y) = (TERM_TEXT.x + 17.0, TERM_TEXT.y + 35.0);
        assert_eq!(r.term.wheel(120, x, y), Outcome::Redraw);
        assert_eq!(r.shell().borrow().scrolled, 3);
        assert_eq!(r.shell().borrow().wheel_cell, Some((2, 2)));
        // A touchpad's small steps add up.
        assert_eq!(r.term.wheel(20, x, y), Outcome::None);
        r.term.wheel(20, x, y);
        assert_eq!(r.shell().borrow().scrolled, 4);
        r.term.wheel(-240, x, y);
        assert_eq!(r.shell().borrow().scrolled, -2);
    }

    #[test]
    fn wheel_over_header_or_menu_does_not_reach_the_program() {
        let mut r = Rig::new();
        r.open();
        assert_eq!(r.term.wheel(120, TERM_TEXT.x, 1.0), Outcome::None);
        let p = bar(&r).plus.unwrap();
        r.term.press(p.x + 1.0, p.y + 1.0);
        assert_eq!(
            r.term.wheel(120, TERM_TEXT.x, TERM_TEXT.y + 5.0),
            Outcome::None
        );
        assert!(r.shell().borrow().wheel_cell.is_none());
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

    // ---- tabs ----

    const CTRL_SHIFT: Mods = Mods {
        ctrl: true,
        shift: true,
        alt: false,
    };
    const VK_T: u16 = 0x54;
    const VK_TAB: u16 = 0x09;

    fn bar(r: &Rig) -> TabBarView {
        r.term.view(&palette()).tabs
    }

    /// The centre of tab `i` on the bar.
    fn tab_at(r: &Rig, i: usize) -> (f32, f32) {
        let t = &bar(r).tabs[i].rect;
        (t.x + t.w / 2.0, t.y + t.h / 2.0)
    }

    /// Right-clicks tab `i` and picks `action` from its menu.
    fn menu(r: &mut Rig, i: usize, action: MenuAction) -> Outcome {
        let (x, y) = tab_at(r, i);
        assert_eq!(r.term.right_click(x, y), Outcome::Redraw);
        let m = bar(r).menu.expect("the tab menu");
        let row = m.items.iter().find(|it| it.action == action).expect("row");
        let (mx, my) = (row.rect.x + 4.0, row.rect.y + row.rect.h / 2.0);
        r.term.press(mx, my)
    }

    #[test]
    fn a_new_tab_gets_its_own_shell_and_keys_go_to_the_active_one() {
        let mut r = Rig::new();
        r.open();
        let first = r.shell();
        r.type_key(VK_T, Some('\u{14}'), CTRL_SHIFT);
        assert_eq!(bar(&r).tabs.len(), 2);
        assert_eq!(r.spawns.borrow().opts.len(), 2);
        let second = r.shell();
        r.type_key(0x41, Some('a'), NONE);
        assert_eq!(second.borrow().writes, vec![b"a".to_vec()]);
        assert!(
            first.borrow().writes.is_empty(),
            "nor the Ctrl+Shift+T char"
        );
        // Ctrl+Tab back to the first.
        r.type_key(VK_TAB, None, CTRL);
        r.type_key(0x42, Some('b'), NONE);
        assert_eq!(first.borrow().writes, vec![b"b".to_vec()]);
    }

    #[test]
    fn closing_the_last_tab_closes_the_panel() {
        let mut r = Rig::new();
        r.open();
        assert_eq!(
            r.term.key(0x57, CTRL_SHIFT), // Ctrl+Shift+W
            Outcome::Close
        );
        assert!(bar(&r).tabs.is_empty());
        // The next open starts a fresh tab.
        r.open();
        assert_eq!(bar(&r).tabs.len(), 1);
        assert_eq!(r.spawns.borrow().opts.len(), 2);
    }

    #[test]
    fn a_middle_click_closes_a_regular_tab() {
        let mut r = Rig::new();
        r.open();
        r.term.key(VK_T, CTRL_SHIFT);
        let (x, y) = tab_at(&r, 0);
        assert_eq!(r.term.middle_click(x, y), Outcome::Redraw);
        assert_eq!(bar(&r).tabs.len(), 1);
    }

    #[test]
    fn output_in_a_background_tab_marks_it() {
        let mut r = Rig::new();
        r.open();
        let first = r.shell();
        r.term.key(VK_T, CTRL_SHIFT);
        first.borrow_mut().woke = true;
        assert!(r.term.drain());
        let tabs = bar(&r).tabs;
        assert!(tabs[0].activity, "the background tab printed");
        assert!(!tabs[1].activity);
    }

    #[test]
    fn a_pinned_tab_is_saved_and_comes_back_after_a_restart() {
        let mut r = Rig::new();
        r.open();
        r.shell()
            .borrow_mut()
            .events
            .push(TermEvent::Title("api".into()));
        r.term.drain();
        assert!(r.term.take_save().is_none(), "nothing pinned yet");
        assert_eq!(menu(&mut r, 0, MenuAction::Pin), Outcome::Redraw);
        let toml = r.term.take_save().expect("pinning saves");
        let (pinned, errors) = tabs::parse_pinned(&toml);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].title, "api");

        // A new island run: the pinned tab is live before opening the panel.
        let spawns = Rc::new(RefCell::new(Spawns::default()));
        let mut term = Terminal::new(
            TerminalConfig::default(),
            Box::new(FakeSpawner(Rc::clone(&spawns))),
            Box::new(FakeClipboard(Rc::new(RefCell::new(None)))),
            pinned,
        );
        term.restore_pinned((8, 17), 1.0);
        assert_eq!(term.shortcuts().len(), 1);
        let view = term.view(&palette()).tabs;
        assert_eq!(view.tabs.len(), 1);
        assert!(view.tabs[0].pinned);
        assert_eq!(view.tabs[0].title, "api");
        assert_eq!(spawns.borrow().opts.len(), 1);
        term.restore_pinned((8, 17), 1.0);
        term.open((8, 17), 1.0);
        assert_eq!(spawns.borrow().opts.len(), 1, "no duplicate on first open");
    }

    fn rename_to(r: &mut Rig, index: usize, title: &str) {
        assert_eq!(menu(r, index, MenuAction::Rename), Outcome::Redraw);
        r.type_key(0x08, Some('\u{8}'), NONE);
        for ch in title.chars() {
            r.term.char(ch, NONE);
        }
        r.type_key(VK_RETURN, Some('\r'), NONE);
    }

    #[test]
    fn rename_targets_the_background_tab_without_sending_input_or_spawning() {
        let mut r = Rig::new();
        r.open();
        let first = r.shell();
        r.term.key(VK_T, CTRL_SHIFT);
        let active = r.term.active_id();
        rename_to(&mut r, 0, "API server");
        assert_eq!(bar(&r).tabs[0].title, "API server");
        assert_eq!(r.term.active_id(), active);
        assert!(first.borrow().writes.is_empty());
        assert!(
            r.writes().is_empty(),
            "Enter's WM_CHAR never reaches the active shell"
        );
        assert_eq!(r.spawns.borrow().opts.len(), 2);
        assert!(
            r.term.take_save().is_none(),
            "unpinned rename isn't persisted"
        );
    }

    #[test]
    fn custom_title_survives_shell_titles_and_empty_name_restores_automatic_title() {
        let mut r = Rig::new();
        r.open();
        rename_to(&mut r, 0, "Backend");
        r.shell().borrow_mut().events.extend([
            TermEvent::Title("running tests".into()),
            TermEvent::ResetTitle,
            TermEvent::Title("automatic title".into()),
        ]);
        r.term.drain();
        assert_eq!(bar(&r).tabs[0].title, "Backend");
        rename_to(&mut r, 0, "   ");
        assert_eq!(bar(&r).tabs[0].title, "automatic title");
        assert!(r.writes().is_empty());
    }

    #[test]
    fn pinned_custom_title_round_trips_and_remains_after_a_shell_restart() {
        let mut r = Rig::new();
        r.open();
        menu(&mut r, 0, MenuAction::Pin);
        r.term.take_save();
        rename_to(&mut r, 0, "My project");
        let (pins, errors) = tabs::parse_pinned(&r.term.take_save().expect("rename saved"));
        assert!(errors.is_empty());
        assert_eq!(pins[0].custom_title, "My project");
        r.term.shutdown();
        r.term.tabs = Tabs::new();
        r.term.restore = pins;
        r.term.restore_pinned((8, 17), 1.0);
        r.shell()
            .borrow_mut()
            .events
            .push(TermEvent::Title("app replaces title".into()));
        r.term.drain();
        assert_eq!(bar(&r).tabs[0].title, "My project");
        r.shell()
            .borrow_mut()
            .events
            .push(TermEvent::Exited(Some(0)));
        r.term.drain();
        r.type_key(VK_RETURN, Some('\r'), NONE);
        assert_eq!(bar(&r).tabs[0].title, "My project");
        assert_eq!(r.spawns.borrow().opts.len(), 3);
    }

    #[test]
    fn rename_cancel_and_modal_mouse_input_leave_the_shell_untouched() {
        let mut r = Rig::new();
        r.open();
        let original = bar(&r).tabs[0].title.clone();
        for cancel in [0, 1, 2, 3] {
            menu(&mut r, 0, MenuAction::Rename);
            r.term.char('x', NONE);
            r.term.key(VK_T, CTRL_SHIFT);
            r.term.wheel(120, TERM_TEXT.x, TERM_TEXT.y + 5.0);
            r.term.right_click(TERM_TEXT.x, TERM_TEXT.y + 5.0);
            r.term.double_click(TERM_TEXT.x, TERM_TEXT.y + 5.0);
            match cancel {
                0 => {
                    r.type_key(0x1b, Some('\u{1b}'), NONE);
                }
                1 => {
                    r.term
                        .press(tabs::RENAME_CANCEL.x + 1.0, tabs::RENAME_CANCEL.y + 1.0);
                }
                2 => {
                    r.term.press(TERM_TEXT.x, TERM_TEXT.y + 1.0);
                }
                _ => r.term.on_hide(),
            }
            assert_eq!(bar(&r).tabs[0].title, original);
            assert!(r.term.view(&palette()).tabs.rename.is_none());
            assert!(r.writes().is_empty());
            assert_eq!(r.shell().borrow().scrolled, 0);
            assert_eq!(r.spawns.borrow().opts.len(), 1);
        }
    }

    #[test]
    fn rename_edits_unicode_and_pastes_plain_text_without_pasting_to_the_shell() {
        let mut r = Rig::new();
        r.open();
        menu(&mut r, 0, MenuAction::Rename);
        *r.clip.borrow_mut() = Some("cafe\n日".into());
        r.type_key(0x56, Some('\u{16}'), CTRL);
        r.term.key(0x25, NONE);
        r.term.char('é', NONE);
        r.term.key(0x23, NONE);
        r.term.char('!', NONE);
        r.term
            .press(tabs::RENAME_SAVE.x + 1.0, tabs::RENAME_SAVE.y + 1.0);
        assert_eq!(bar(&r).tabs[0].title, "cafe é日!");
        assert!(r.writes().is_empty());
    }

    #[test]
    fn renaming_an_exited_tab_does_not_restart_its_process() {
        let mut r = Rig::new();
        r.open();
        r.shell()
            .borrow_mut()
            .events
            .push(TermEvent::Exited(Some(0)));
        r.term.drain();
        rename_to(&mut r, 0, "finished");
        assert_eq!(bar(&r).tabs[0].title, "finished");
        assert!(bar(&r).tabs[0].exited);
        assert_eq!(r.spawns.borrow().opts.len(), 1);
        assert!(r.writes().is_empty());
    }

    #[test]
    fn startup_without_pins_does_not_spawn_a_default_shell() {
        let mut r = Rig::new();
        r.term.restore_pinned((8, 17), 1.0);
        assert!(r.spawns.borrow().opts.is_empty());
        assert!(r.term.shortcuts().is_empty());
        r.open();
        assert_eq!(r.spawns.borrow().opts.len(), 1);
    }

    #[test]
    fn pinned_metadata_changes_save_the_latest_folder_and_name() {
        let mut r = Rig::new();
        r.open();
        menu(&mut r, 0, MenuAction::Pin);
        r.term.take_save();
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        r.shell().borrow_mut().events.extend([
            TermEvent::Cwd(dir.clone()),
            TermEvent::Title("project".into()),
        ]);
        r.term.drain();
        let (pinned, notes) = tabs::parse_pinned(&r.term.take_save().expect("metadata saved"));
        assert!(notes.is_empty());
        assert_eq!(pinned[0].cwd, dir);
        assert_eq!(pinned[0].title, "project");
        r.shell()
            .borrow_mut()
            .events
            .push(TermEvent::Cwd(dir.clone()));
        r.term.drain();
        assert!(
            r.term.take_save().is_none(),
            "unchanged prompts don't write"
        );
        r.term.restore = pinned;
        r.term.shutdown();
        r.term.tabs = Tabs::new();
        r.term.restore_pinned((8, 17), 1.0);
        assert_eq!(r.spawns.borrow().opts.last().unwrap().cwd, Some(dir.into()));
    }

    #[test]
    fn regular_tab_metadata_does_not_trigger_pinned_file_writes() {
        let mut r = Rig::new();
        r.open();
        r.shell().borrow_mut().events.extend([
            TermEvent::Cwd("C:\\project".into()),
            TermEvent::Title("project".into()),
        ]);
        r.term.drain();
        assert!(r.term.take_save().is_none());
    }

    #[test]
    fn startup_restores_multiple_pins_in_saved_order_including_failed_tabs() {
        let mut r = Rig::new();
        r.term.restore = ["first", "second", "third"]
            .map(|title| PinnedTab {
                title: title.into(),
                shell: String::new(),
                args: vec![],
                cwd: String::new(),
                agent: String::new(),
                custom_title: String::new(),
            })
            .to_vec();
        r.spawns.borrow_mut().fail = true;
        r.term.restore_pinned((8, 17), 1.0);
        assert_eq!(
            bar(&r)
                .tabs
                .iter()
                .map(|t| t.title.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
        assert!(bar(&r).tabs.iter().all(|t| t.pinned && t.exited));
        assert!(r.term.shortcuts().is_empty());
        r.spawns.borrow_mut().fail = false;
        r.term.key(VK_RETURN, Mods::default());
        assert_eq!(r.term.shortcuts().len(), 1, "failed pin remains retryable");
        r.open();
        assert_eq!(
            r.spawns.borrow().opts.len(),
            4,
            "no duplicate restored tabs"
        );
    }

    /// Presses "+" and picks `action` from the new-tab menu.
    fn plus(r: &mut Rig, action: MenuAction) -> Outcome {
        let p = bar(r).plus.expect("the + button");
        assert_eq!(
            r.term.press(p.x + p.w / 2.0, p.y + p.h / 2.0),
            Outcome::Redraw
        );
        assert_eq!(r.spawns.borrow().opts.len(), 1, "+ alone starts nothing");
        let m = bar(r).menu.expect("the new-tab menu");
        let row = m.items.iter().find(|it| it.action == action).expect("row");
        r.term
            .press(row.rect.x + 4.0, row.rect.y + row.rect.h / 2.0)
    }

    fn last_spawn(r: &Rig) -> SpawnOptions {
        r.spawns.borrow().opts.last().cloned().expect("a spawn")
    }

    #[test]
    fn a_powershell_tab_installs_the_folder_hook() {
        let mut r = Rig::new();
        r.open();
        let o = last_spawn(&r);
        assert!(dj_term::is_powershell(&o.program), "{}", o.program);
        let n = o.args.len();
        assert_eq!(o.args[n - 3..], ["-NoExit", "-Command", agents::CWD_HOOK]);
        assert_eq!(plus(&mut r, MenuAction::NewShell), Outcome::Redraw);
        assert_eq!(
            last_spawn(&r).args,
            o.args,
            "the Terminal row is a plain tab"
        );
    }

    #[test]
    fn claude_from_the_plus_menu_runs_in_yolo_mode_in_the_active_tabs_folder() {
        let mut r = Rig::new();
        r.open();
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        r.shell()
            .borrow_mut()
            .events
            .push(TermEvent::Cwd(dir.clone()));
        r.term.drain();
        assert_eq!(plus(&mut r, MenuAction::NewClaude), Outcome::Redraw);
        let o = last_spawn(&r);
        assert!(dj_term::is_powershell(&o.program));
        assert_eq!(o.cwd, Some(PathBuf::from(&dir)));
        let cmd = o.args.last().unwrap();
        assert!(cmd.starts_with(agents::CWD_HOOK));
        assert!(
            cmd.ends_with("; claude --dangerously-skip-permissions"),
            "{cmd}"
        );
        let tabs = bar(&r).tabs;
        assert_eq!(tabs.len(), 2);
        assert!(tabs[1].active);
        assert_eq!(tabs[1].title, "claude");
    }

    #[test]
    fn codex_falls_back_to_the_configured_folder_when_the_reported_one_is_gone() {
        let mut r = Rig::new();
        r.open();
        r.shell()
            .borrow_mut()
            .events
            .push(TermEvent::Cwd(r"C:\no\such\folder\dj".into()));
        r.term.drain();
        plus(&mut r, MenuAction::NewCodex);
        let o = last_spawn(&r);
        assert_eq!(o.cwd, std::env::var_os("USERPROFILE").map(PathBuf::from));
        assert!(o
            .args
            .last()
            .unwrap()
            .ends_with("; codex --dangerously-bypass-approvals-and-sandbox"));
    }

    #[test]
    fn an_agent_from_an_agent_tab_without_a_prompt_yet_uses_its_start_folder() {
        let mut r = Rig::new();
        r.open();
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        r.shell()
            .borrow_mut()
            .events
            .push(TermEvent::Cwd(dir.clone()));
        r.term.drain();
        plus(&mut r, MenuAction::NewClaude);
        // The Claude tab is active and has reported nothing (Claude runs, no prompt yet).
        let p = bar(&r).plus.unwrap();
        r.term.press(p.x + p.w / 2.0, p.y + p.h / 2.0);
        let m = bar(&r).menu.unwrap();
        let row = m
            .items
            .iter()
            .find(|it| it.action == MenuAction::NewCodex)
            .unwrap();
        r.term
            .press(row.rect.x + 4.0, row.rect.y + row.rect.h / 2.0);
        assert_eq!(last_spawn(&r).cwd, Some(PathBuf::from(&dir)));
    }

    #[test]
    fn a_title_that_is_only_the_shells_path_keeps_the_tab_name() {
        let mut r = Rig::new();
        r.open();
        plus(&mut r, MenuAction::NewCodex);
        for path in [
            r"C:\WINDOWS\System32\WindowsPowerShell\v1.0\powershell.exe",
            r"C:\WINDOWS\system32\cmd.exe",
        ] {
            r.shell()
                .borrow_mut()
                .events
                .push(TermEvent::Title(path.into()));
            r.term.drain();
            assert_eq!(bar(&r).tabs[1].title, "codex", "{path}");
        }
        r.shell()
            .borrow_mut()
            .events
            .push(TermEvent::Title("codex: refactor".into()));
        r.term.drain();
        assert_eq!(bar(&r).tabs[1].title, "codex: refactor");
    }

    #[test]
    fn open_in_starts_one_tab_in_the_folder() {
        let mut r = Rig::new();
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        r.term.open_in((8, 17), 1.0, dir.clone(), None);
        assert_eq!(bar(&r).tabs.len(), 1, "no default tab besides it");
        assert_eq!(last_spawn(&r).cwd, Some(PathBuf::from(&dir)));
        // Again, with an agent: a second tab, active, running Claude in yolo mode there.
        r.term
            .open_in((8, 17), 1.0, dir.clone(), Some(Agent::Claude));
        let tabs = bar(&r).tabs;
        assert_eq!(tabs.len(), 2);
        assert!(tabs[1].active);
        let o = last_spawn(&r);
        assert_eq!(o.cwd, Some(PathBuf::from(&dir)));
        assert!(o
            .args
            .last()
            .unwrap()
            .ends_with("; claude --dangerously-skip-permissions"));
    }

    #[test]
    fn a_pinned_agent_tab_comes_back_as_the_same_agent() {
        let mut r = Rig::new();
        r.open();
        plus(&mut r, MenuAction::NewClaude);
        let args = last_spawn(&r).args;
        menu(&mut r, 1, MenuAction::Pin);
        let (pinned, errors) = tabs::parse_pinned(&r.term.take_save().expect("saved"));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].agent, "claude");

        let spawns = Rc::new(RefCell::new(Spawns::default()));
        let mut term = Terminal::new(
            TerminalConfig::default(),
            Box::new(FakeSpawner(Rc::clone(&spawns))),
            Box::new(FakeClipboard(Rc::new(RefCell::new(None)))),
            pinned,
        );
        term.open((8, 17), 1.0);
        assert_eq!(spawns.borrow().opts.len(), 1);
        assert_eq!(
            spawns.borrow().opts[0].args,
            args,
            "the hook is not doubled"
        );
    }

    #[test]
    fn close_others_keeps_pinned_tabs_and_their_shells() {
        let mut r = Rig::new();
        r.open();
        menu(&mut r, 0, MenuAction::Pin);
        r.term.key(VK_T, CTRL_SHIFT);
        r.term.key(VK_T, CTRL_SHIFT);
        assert_eq!(bar(&r).tabs.len(), 3);
        menu(&mut r, 2, MenuAction::CloseOthers);
        let tabs = bar(&r).tabs;
        assert_eq!(tabs.len(), 2);
        assert!(tabs[0].pinned);
        assert!(r.term.take_save().is_some(), "the pin was saved");
    }

    #[test]
    fn enter_restarts_only_the_active_tab_that_exited() {
        let mut r = Rig::new();
        r.open();
        let first = r.shell();
        r.term.key(VK_T, CTRL_SHIFT);
        first.borrow_mut().events.push(TermEvent::Exited(Some(0)));
        r.term.drain();
        assert!(bar(&r).tabs[0].exited);
        assert!(
            r.term.view(&palette()).notice.is_none(),
            "the active tab is fine"
        );
        r.type_key(VK_RETURN, Some('\r'), NONE);
        assert_eq!(
            r.spawns.borrow().opts.len(),
            2,
            "no restart from another tab"
        );
        r.type_key(VK_TAB, None, CTRL);
        assert!(r.term.view(&palette()).notice.is_some());
        r.type_key(VK_RETURN, Some('\r'), NONE);
        assert_eq!(r.spawns.borrow().opts.len(), 3, "restarted");
        assert!(!bar(&r).tabs[0].exited);
    }
}
