//! Sweep: the scanners and the delete code behind the Vitals panel's cleaner.
//!
//! Three layers:
//! - `rules` (pure): category lists, the protected-path guard, the age filter, the
//!   dev-purge context rules, size aggregation and the operations-log line format.
//! - `fs`: the walker (time budget, cancel flag, never follows reparse points), the
//!   final-path re-verify and the `git ls-files` check.
//! - `ops`: the side effects (delete, recycle, DNS flush, empty bin, reveal) behind the
//!   [`Ops`] trait, so tests run against a fake.

pub use dj_core::sweep::{Category, Removal};

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// One previewed entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub path: PathBuf,
    pub bytes: u64,
    /// Short display text, e.g. "chrome · Default · Code Cache".
    pub label: String,
}

/// Answers "is a process with this exe name running?" (case-insensitive, e.g. "chrome.exe").
pub type ProcessProbe = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Where to look and what time it is. Built once per scan by the app.
#[derive(Clone)]
pub struct ScanEnv {
    pub profile: PathBuf,
    pub local_appdata: PathBuf,
    pub roaming_appdata: PathBuf,
    pub temp: PathBuf,
    pub project_roots: Vec<PathBuf>,
    pub now: SystemTime,
    /// Time budget per category scan.
    pub budget: Duration,
    /// Windows, Program Files (both), ProgramData. Always protected.
    pub system_dirs: Vec<PathBuf>,
    pub running: ProcessProbe,
}

impl std::fmt::Debug for ScanEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanEnv")
            .field("profile", &self.profile)
            .field("local_appdata", &self.local_appdata)
            .field("roaming_appdata", &self.roaming_appdata)
            .field("temp", &self.temp)
            .field("project_roots", &self.project_roots)
            .field("now", &self.now)
            .field("budget", &self.budget)
            .field("system_dirs", &self.system_dirs)
            .finish_non_exhaustive()
    }
}

impl ScanEnv {
    /// Reads the real locations from the environment. An empty `project_roots` means
    /// the defaults (see [`default_project_roots`]).
    pub fn from_system(project_roots: Vec<PathBuf>) -> Self {
        let _ = project_roots;
        todo!()
    }
}

/// `~/Documents`, `~/Projects`, `~/Code`, `~/source/repos`, `~/dev`.
pub fn default_project_roots(profile: &Path) -> Vec<PathBuf> {
    ["Documents", "Projects", "Code", "source\\repos", "dev"]
        .iter()
        .map(|p| profile.join(p))
        .collect()
}

/// One category's preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryScan {
    pub category: Category,
    /// Largest first.
    pub items: Vec<Item>,
    pub total: u64,
    /// Why something was left out, e.g. "chrome is running".
    pub note: Option<String>,
}

/// Previews one category. Never deletes anything.
pub fn scan(env: &ScanEnv, cat: Category, cancel: &AtomicBool) -> CategoryScan {
    let _ = (env, cancel);
    CategoryScan {
        category: cat,
        items: Vec::new(),
        total: 0,
        note: None,
    }
}

/// The side effects. `WinOps` is the real one; tests use a fake.
pub trait Ops: Send + Sync {
    /// Deletes a file or a folder tree for good, never following reparse points.
    /// Returns the bytes freed. An error means something was left behind.
    fn remove_permanent(&self, p: &Path) -> io::Result<u64>;
    /// Moves a file or folder to the Recycle Bin.
    fn recycle(&self, p: &Path) -> io::Result<()>;
    fn flush_dns(&self) -> io::Result<()>;
    fn empty_recycle_bin(&self) -> io::Result<()>;
    /// Opens Explorer with the file selected.
    fn reveal(&self, p: &Path) -> io::Result<()>;
}

/// The real Win32 operations.
#[derive(Clone, Copy, Debug, Default)]
pub struct WinOps;

impl Ops for WinOps {
    fn remove_permanent(&self, _p: &Path) -> io::Result<u64> {
        todo!()
    }
    fn recycle(&self, _p: &Path) -> io::Result<()> {
        todo!()
    }
    fn flush_dns(&self) -> io::Result<()> {
        todo!()
    }
    fn empty_recycle_bin(&self) -> io::Result<()> {
        todo!()
    }
    fn reveal(&self, _p: &Path) -> io::Result<()> {
        todo!()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Removed,
    Recycled,
    Failed,
}

/// Appends to `~/.daijobu/operations.log`.
#[derive(Clone, Debug)]
pub struct OpLog {
    path: PathBuf,
}

impl OpLog {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn record(&self, run: &str, action: Action, path: &Path, detail: &str) {
        let _ = (run, action, path, detail, &self.path);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CleanOutcome {
    pub freed: u64,
    pub failed: usize,
    /// Items that are completely gone.
    pub gone: Vec<PathBuf>,
}

/// Removes previewed items. Every item is re-checked against the guard, its category's
/// rules and its final path right before it goes. Recycle categories go to
/// `ops.recycle`, the rest to `ops.remove_permanent`. `progress(done, total)` is called
/// after each item.
pub fn clean(
    env: &ScanEnv,
    items: &[(Category, PathBuf)],
    ops: &dyn Ops,
    log: &OpLog,
    run_id: &str,
    progress: &mut dyn FnMut(usize, usize),
) -> CleanOutcome {
    let _ = (env, items, ops, log, run_id, progress);
    CleanOutcome::default()
}

/// A short id that ties one run's log lines together.
pub fn new_run_id() -> String {
    String::new()
}
