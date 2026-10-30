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
    generation: u64,
    explorer: Option<(u64, Receiver<Result<model::ExplorerSnapshot, String>>)>,
    refresh_queued: bool,
    snapshot: model::ExplorerSnapshot,
    explorer_status: String,
    show_explorer: bool,
}

pub(super) struct Chooser {
    profile: usize,
    edit: LineEdit,
    selected: bool,
    focus: usize,
    error: String,
    auto_prefill: bool,
}

impl Folders {
    pub(super) fn cancel_validation(&mut self) {
        self.validation = None;
        self.invalidate_explorer();
        self.refresh_queued = false;
    }

    fn invalidate_explorer(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(c) = &mut self.chooser {
            c.auto_prefill = false;
        }
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
        let auto_prefill = path.is_none() && self.config.follow_explorer;
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        let path = model::starting_folder(
            path.as_deref(),
            None,
            &self.folders.history.last[profile],
            &self.config.cwd,
            &home,
        )
        .to_string();
        let mut edit = LineEdit::with_limit(model::MAX_PATH);
        edit.set_text(&path);
        self.folders.chooser = Some(Chooser {
            profile,
            edit,
            selected: true,
            focus: 3,
            error: String::new(),
            auto_prefill,
        });
        self.folders.snapshot = model::ExplorerSnapshot::default();
        self.folders.explorer_status.clear();
        self.folders.show_explorer = true;
        if self.folders.owner != 0 {
            self.refresh_explorer();
        }
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
        rows.push(format!(
            "{} (F6: switch source, F5: refresh Explorer){}",
            if self.folders.show_explorer {
                "Explorer folders"
            } else {
                "Recent folders"
            },
            if self.folders.explorer.is_some() {
                " · loading..."
            } else {
                ""
            }
        ));
        let (label, paths) = if self.folders.show_explorer {
            ("Explorer", &self.folders.snapshot.folders)
        } else {
            ("Recent", &self.folders.history.recent)
        };
        rows.extend(
            paths
                .iter()
                .take(model::LIMIT)
                .map(|p| format!("{label}: {p}")),
        );
        Some(ChooserView {
            rows,
            focus: c.focus,
            error: if c.error.is_empty() {
                self.folders
                    .save_error
                    .clone()
                    .unwrap_or_else(|| self.folders.explorer_status.clone())
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
            let c = self.folders.chooser.as_ref().unwrap();
            let path = (!c.auto_prefill).then(|| c.edit.text().to_string());
            self.choose_folder(row, path);
            return Outcome::Redraw;
        }
        match row {
            3 => {
                self.folders.chooser.as_mut().unwrap().focus = 3;
            }
            4 => {
                self.folders.invalidate_explorer();
                match dj_win::folder_picker::browse(self.folders.owner) {
                    Ok(rx) => self.folders.picker = Some(rx),
                    Err(e) => self.folders.chooser.as_mut().unwrap().error = e,
                }
            }
            5 => {
                self.folders.invalidate_explorer();
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
                self.folders.cancel_validation();
                self.folders.chooser = None;
                if self.tabs.is_empty() {
                    return Outcome::Close;
                }
            }
            7 => {
                self.folders.history = History::default();
                self.folders.dirty = true;
            }
            model::SOURCE_ROW => {
                self.folders.show_explorer = !self.folders.show_explorer;
                let count = self.folder_row_count();
                let c = self.folders.chooser.as_mut().unwrap();
                c.focus = c.focus.min(count - 1);
            }
            n => {
                let paths = if self.folders.show_explorer {
                    &self.folders.snapshot.folders
                } else {
                    &self.folders.history.recent
                };
                if let Some(path) = paths.get(n - model::FIRST_FOLDER_ROW).cloned() {
                    self.folders.invalidate_explorer();
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
        let count = self.folder_row_count();
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
        if vk == 0x74 {
            self.folders.generation = self.folders.generation.wrapping_add(1);
            self.refresh_explorer();
            return Outcome::Redraw;
        }
        if vk == 0x75 {
            return self.folder_action(model::SOURCE_ROW);
        }
        if vk != 0x09 && vk != VK_RETURN {
            self.folders.invalidate_explorer();
        }
        let count = self.folder_row_count();
        let c = self.folders.chooser.as_mut().unwrap();
        if vk == 0x09 {
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
        self.folders.invalidate_explorer();
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

    fn folder_row_count(&self) -> usize {
        model::FIRST_FOLDER_ROW
            + if self.folders.show_explorer {
                self.folders
                    .snapshot
                    .folders
                    .len()
                    .min(model::EXPLORER_LIMIT)
            } else {
                self.folders.history.recent.len().min(model::LIMIT)
            }
    }

    fn refresh_explorer(&mut self) {
        if self.folders.explorer.is_some() {
            self.folders.refresh_queued = true;
            return;
        }
        match dj_win::explorer::request(self.folders.owner) {
            Ok(rx) => self.folders.explorer = Some((self.folders.generation, rx)),
            Err(e) => self.folders.explorer_status = e,
        }
    }

    #[cfg(test)]
    pub(super) fn explorer_test_request(
        &mut self,
    ) -> std::sync::mpsc::Sender<Result<model::ExplorerSnapshot, String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.folders.explorer = Some((self.folders.generation, rx));
        tx
    }

    pub(super) fn drain_explorer(&mut self) -> bool {
        let Some((generation, rx)) = &self.folders.explorer else {
            return false;
        };
        let result = match rx.try_recv() {
            Ok(value) => value,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(_) => Err("Explorer stopped responding. Use Browse or Recent; F5 retries.".into()),
        };
        let current = *generation == self.folders.generation;
        self.folders.explorer = None;
        if let (true, Some(c)) = (current, self.folders.chooser.as_mut()) {
            if c.auto_prefill && self.config.follow_explorer {
                let home = std::env::var("USERPROFILE").unwrap_or_default();
                let active = result.as_ref().ok().and_then(|s| s.active.as_deref());
                let path = model::starting_folder(
                    None,
                    active,
                    &self.folders.history.last[c.profile],
                    &self.config.cwd,
                    &home,
                );
                c.edit.set_text(path);
                c.selected = true;
            }
            match result {
                Ok(snapshot) => {
                    self.folders.explorer_status = if snapshot.folders.is_empty() {
                        "No usable Explorer folders. Virtual/hidden tabs unavailable; use Browse or Recent.".into()
                    } else if snapshot.incomplete {
                        "Some Explorer views unavailable or list limited to six. F5 refreshes."
                            .into()
                    } else {
                        String::new()
                    };
                    self.folders.snapshot = snapshot;
                }
                Err(e) => {
                    self.folders.snapshot = model::ExplorerSnapshot::default();
                    self.folders.explorer_status = e;
                }
            }
            let count = model::FIRST_FOLDER_ROW
                + if self.folders.show_explorer {
                    self.folders
                        .snapshot
                        .folders
                        .len()
                        .min(model::EXPLORER_LIMIT)
                } else {
                    self.folders.history.recent.len().min(model::LIMIT)
                };
            c.focus = c.focus.min(count - 1);
        }
        if std::mem::take(&mut self.folders.refresh_queued) && self.folders.chooser.is_some() {
            self.refresh_explorer();
        }
        true
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
