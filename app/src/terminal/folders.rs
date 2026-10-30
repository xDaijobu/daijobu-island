use super::*;
use dj_core::terminal_folders::{self as model, History};
use std::sync::mpsc::Receiver;

#[derive(Default)]
pub(super) struct Folders {
    pub enabled: bool,
    pub owner: isize,
    pub history: History,
    pub dirty: bool,
    pub save_error: Option<String>,
    pub chooser: Option<Chooser>,
    picker: Option<Receiver<Result<Option<String>, String>>>,
    validation: Option<Receiver<Result<(usize, String), String>>>,
}

pub(super) struct Chooser {
    profile: usize,
    edit: LineEdit,
    selected: bool,
    focus: usize,
    error: String,
}

impl Folders {
    pub(super) fn cancel_validation(&mut self) {
        self.validation = None;
    }
}

impl Terminal {
    pub fn enable_folders(&mut self, owner: isize, history: History) {
        self.folders.enabled = true;
        self.folders.owner = owner;
        self.folders.history = history;
    }

    pub fn folder_save_result(&mut self, result: Result<(), String>) -> bool {
        let error = result.err();
        let changed = self.folders.save_error != error;
        self.folders.save_error = error;
        changed
    }

    pub fn folder_picker_open(&self) -> bool {
        self.folders.picker.is_some()
    }

    pub fn take_folder_save(&mut self) -> Option<String> {
        std::mem::take(&mut self.folders.dirty).then(|| self.folders.history.to_toml())
    }

    pub(super) fn choose_folder(&mut self, profile: usize, path: Option<String>) -> bool {
        // Re-entry (for example, Spotlight) replaces the pending request as well as
        // the field. Its old completion must never launch behind the new chooser.
        self.folders.cancel_validation();
        let path = path.unwrap_or_else(|| {
            let last = &self.folders.history.last[profile];
            if !last.is_empty() {
                return last.clone();
            }
            if !self.config.cwd.is_empty() {
                return self.config.cwd.clone();
            }
            std::env::var("USERPROFILE").unwrap_or_default()
        });
        let mut edit = LineEdit::with_limit(model::MAX_PATH);
        edit.set_text(&path);
        self.folders.chooser = Some(Chooser {
            profile,
            edit,
            selected: true,
            focus: 3,
            error: String::new(),
        });
        self.tabs.close_menu();
        true
    }

    pub(super) fn folder_view(&self) -> Option<ChooserView> {
        let c = self.folders.chooser.as_ref()?;
        let mut rows: Vec<String> = ["Shell", "Claude", "Codex"]
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{} {s}", if c.profile == i { "●" } else { "○" }))
            .collect();
        let start = c.edit.caret().saturating_sub(65);
        let before: String = c
            .edit
            .text()
            .chars()
            .skip(start)
            .take(c.edit.caret() - start)
            .collect();
        let after: String = c.edit.text().chars().skip(c.edit.caret()).collect();
        rows.push(format!(
            "Folder: {}{before}|{after}{}",
            if start > 0 { "…" } else { "" },
            if c.selected { "  [selected]" } else { "" }
        ));
        rows.extend(
            [
                if self.folders.picker.is_some() {
                    "Browse (waiting...)"
                } else {
                    "Browse"
                },
                "Open",
                "Cancel",
                "Clear history",
            ]
            .map(str::to_string),
        );
        rows.extend(
            self.folders
                .history
                .recent
                .iter()
                .map(|p| format!("Recent: {p}")),
        );
        Some(ChooserView {
            rows,
            focus: c.focus,
            error: if c.error.is_empty() {
                self.folders.save_error.clone().unwrap_or_default()
            } else {
                c.error.clone()
            },
        })
    }

    fn folder_action(&mut self, row: usize) -> Outcome {
        if self.folders.picker.is_some() || self.folders.validation.is_some() {
            return Outcome::Redraw;
        }
        if row <= 2 {
            self.choose_folder(row, None);
            return Outcome::Redraw;
        }
        match row {
            3 => {
                self.folders.chooser.as_mut().unwrap().focus = 3;
            }
            4 => match dj_win::folder_picker::browse(self.folders.owner) {
                Ok(rx) => self.folders.picker = Some(rx),
                Err(e) => self.folders.chooser.as_mut().unwrap().error = e,
            },
            5 => {
                let c = self.folders.chooser.as_ref().unwrap();
                let profile = c.profile;
                let path = c.edit.text().to_string();
                let (tx, rx) = std::sync::mpsc::channel();
                let owner = self.folders.owner;
                match std::thread::Builder::new()
                    .name("terminal-folder-validation".into())
                    .spawn(move || {
                        let result = model::validate(&path).map(|_| (profile, path));
                        let _ = tx.send(result);
                        dj_win::window::wake(owner);
                    }) {
                    Ok(_) => {
                        self.folders.validation = Some(rx);
                        self.folders.chooser.as_mut().unwrap().error = "Checking folder...".into();
                    }
                    Err(e) => self.folders.chooser.as_mut().unwrap().error = e.to_string(),
                }
            }
            6 => {
                self.folders.chooser = None;
                if self.tabs.is_empty() {
                    return Outcome::Close;
                }
            }
            7 => {
                self.folders.history = History::default();
                self.folders.dirty = true;
            }
            n => {
                if let Some(path) = self.folders.history.recent.get(n - 8).cloned() {
                    let c = self.folders.chooser.as_mut().unwrap();
                    c.edit.set_text(&path);
                    c.selected = true;
                    c.focus = 3;
                }
            }
        }
        Outcome::Redraw
    }

    pub(super) fn folder_press(&mut self, x: f32, y: f32) -> Outcome {
        let count = 8 + self.folders.history.recent.len();
        for row in 0..count {
            if model::row(row).contains(x, y) {
                return self.folder_action(row);
            }
        }
        Outcome::Redraw
    }

    pub(super) fn folder_key(&mut self, vk: u16, mods: Mods) -> Outcome {
        self.skip_char = vk == VK_RETURN || vk == 0x1b || vk == 0x08 || vk == 0x09 || mods.ctrl;
        if self.folders.validation.is_some() || self.folders.picker.is_some() {
            return Outcome::Redraw;
        }
        if vk == 0x1b {
            return self.folder_action(6);
        }
        let c = self.folders.chooser.as_mut().unwrap();
        if vk == 0x09 {
            let count = 8 + self.folders.history.recent.len();
            c.focus = (c.focus + if mods.shift { count - 1 } else { 1 }) % count;
        } else if vk == VK_RETURN {
            let row = if c.focus == 3 { 5 } else { c.focus };
            return self.folder_action(row);
        } else if c.focus == 3 {
            if mods.ctrl && vk == 0x41 {
                c.selected = true;
            } else if mods.ctrl && vk == 0x56 {
                if let Some(text) = self.clipboard.get() {
                    if model::valid_text(&text) {
                        if c.selected {
                            c.edit.clear();
                            c.selected = false;
                        }
                        c.edit.insert_str(&text);
                    } else {
                        c.error =
                            "Clipboard path is empty, too long or contains control characters."
                                .into();
                    }
                }
            } else if mods.ctrl && vk == 0x43 {
                if c.selected {
                    self.clipboard.set(c.edit.text());
                }
            } else if !mods.alt {
                if c.selected && matches!(vk, 0x08 | 0x2e) {
                    c.edit.clear();
                    c.selected = false;
                } else {
                    c.edit.key(vk, mods.ctrl, mods.shift);
                    if matches!(vk, 0x23..=0x27) {
                        c.selected = false;
                    }
                }
            }
        }
        Outcome::Redraw
    }

    pub(super) fn folder_char(&mut self, ch: char, mods: Mods) -> Outcome {
        if self.folders.validation.is_some() || self.folders.picker.is_some() {
            return Outcome::Redraw;
        }
        let c = self.folders.chooser.as_mut().unwrap();
        if c.focus == 3 && !mods.ctrl && !mods.alt && !ch.is_control() {
            if c.selected {
                c.edit.clear();
                c.selected = false;
            }
            c.edit.insert_char(ch);
        }
        Outcome::Redraw
    }

    pub(super) fn drain_folder_validation(&mut self) -> bool {
        let Some(rx) = &self.folders.validation else {
            return false;
        };
        let result = match rx.try_recv() {
            Ok(value) => value,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(_) => Err("Folder validation stopped unexpectedly.".into()),
        };
        self.folders.validation = None;
        if self.folders.chooser.is_none() {
            return true;
        }
        match result {
            Err(e) => self.folders.chooser.as_mut().unwrap().error = e,
            Ok((profile, path)) => {
                let mut spec = match profile {
                    1 => self.agent_spec(Agent::Claude),
                    2 => self.agent_spec(Agent::Codex),
                    _ => self.config_spec(),
                };
                spec.cwd = path.clone();
                if !self.add_tab(spec, None, false) {
                    self.folders.chooser.as_mut().unwrap().error =
                        "Session limit reached. Close a tab first.".into();
                } else if let Some(error) = self.active().and_then(|t| t.failed.clone()) {
                    if let Some(id) = self.active_id() {
                        self.close_tab(id);
                    }
                    self.folders.chooser.as_mut().unwrap().error = error;
                } else {
                    self.folders.history.remember(profile, path);
                    self.folders.dirty = true;
                    self.folders.chooser = None;
                }
            }
        }
        true
    }

    #[cfg(test)]
    pub(super) fn wait_folder_validation(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while self.folders.validation.is_some() {
            assert!(std::time::Instant::now() < deadline);
            self.drain_folder_validation();
            std::thread::yield_now();
        }
    }

    pub(super) fn drain_picker(&mut self) -> bool {
        let Some(rx) = &self.folders.picker else {
            return false;
        };
        let result = match rx.try_recv() {
            Ok(value) => value,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(_) => Err("Folder picker closed unexpectedly.".into()),
        };
        self.folders.picker = None;
        if let Some(c) = self.folders.chooser.as_mut() {
            match result {
                Ok(Some(path)) => {
                    c.edit.set_text(&path);
                    c.selected = true;
                    c.focus = 3;
                }
                Ok(None) => {}
                Err(e) => c.error = e,
            }
        }
        true
    }
}
