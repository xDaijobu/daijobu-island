//! Sweep: the scanners and the delete code behind the Vitals panel's cleaner.
//!
//! Three layers:
//! - `rules` (pure): category lists, the protected-path guard, the age filter, the
//!   dev-purge context rules, size aggregation and the operations-log line format.
//! - `fs`: the walker (time budget, cancel flag, never follows reparse points), the
//!   final-path re-verify and the `git ls-files` check.
//! - `ops`: the side effects (delete, recycle, DNS flush, empty bin, reveal) behind the
//!   [`Ops`] trait, so tests run against a fake.

mod fs;
mod ops;
pub mod rules;
mod scan;

pub use dj_core::sweep::{format_bytes, Category, Removal};

use std::io::{self, Write};
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
    /// the defaults (see [`default_project_roots`]). Paths are resolved to their final
    /// long form, so short (8.3) names in `%TEMP%` do not trip the re-verify. Fails
    /// when `USERPROFILE` is missing or not an absolute path, because every rule is
    /// anchored to it.
    pub fn from_system(project_roots: Vec<PathBuf>) -> Result<Self, String> {
        Self::from_vars(
            |k| std::env::var_os(k).map(PathBuf::from),
            std::env::temp_dir(),
            project_roots,
        )
    }

    /// [`from_system`](Self::from_system) with the environment passed in, for tests.
    pub fn from_vars(
        var: impl Fn(&str) -> Option<PathBuf>,
        temp: PathBuf,
        project_roots: Vec<PathBuf>,
    ) -> Result<Self, String> {
        let profile = var("USERPROFILE")
            .filter(|p| p.is_absolute() && rules::path_depth(p) >= 2)
            .ok_or("USERPROFILE is not set to a user folder")?;
        let profile = fs::final_or_same(profile);
        let abs = |p: Option<PathBuf>| p.filter(|p| p.is_absolute());
        let local_appdata = fs::final_or_same(
            abs(var("LOCALAPPDATA")).unwrap_or_else(|| profile.join(r"AppData\Local")),
        );
        let roaming_appdata = fs::final_or_same(
            abs(var("APPDATA")).unwrap_or_else(|| profile.join(r"AppData\Roaming")),
        );
        // Checked against %LOCALAPPDATA%\Temp before the Temp category uses it.
        let temp = fs::final_or_same(temp);
        let mut system_dirs: Vec<PathBuf> = [
            "SystemRoot",
            "windir",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
            "ProgramData",
        ]
        .iter()
        .filter_map(|k| var(k))
        .map(fs::final_or_same)
        .collect();
        let drive = var("SystemDrive").unwrap_or_else(|| PathBuf::from("C:"));
        for d in [
            "Windows",
            "Program Files",
            "Program Files (x86)",
            "ProgramData",
        ] {
            system_dirs.push(PathBuf::from(format!(r"{}\{d}", drive.display())));
        }
        let roots = if project_roots.is_empty() {
            default_project_roots(&profile)
        } else {
            project_roots
        };
        Ok(Self {
            project_roots: roots
                .into_iter()
                .filter(|r| r.is_absolute())
                .map(fs::final_or_same)
                .collect(),
            profile,
            local_appdata,
            roaming_appdata,
            temp,
            now: SystemTime::now(),
            budget: Duration::from_secs(15),
            system_dirs,
            // Fails closed: when the process list cannot be read, every app counts
            // as running and its cache is left alone.
            running: Arc::new(|exe: &str| {
                ops::running_processes()
                    .is_none_or(|ps| ps.iter().any(|p| p.eq_ignore_ascii_case(exe)))
            }),
        })
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

/// Previews one category within `env.budget`. Never deletes anything.
pub fn scan(env: &ScanEnv, cat: Category, cancel: &AtomicBool) -> CategoryScan {
    let budget = fs::Budget::new(env.budget, cancel);
    scan::run(env, cat, &budget)
}

/// Where a recycled item ended up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recycled {
    /// In the Recycle Bin, so the user can restore it.
    InBin,
    /// Deleted for good instead (Windows asked first, for example because the item
    /// was too big for the bin). Logged as REMOVED, never as RECYCLED.
    DeletedForGood,
}

/// The side effects. `WinOps` is the real one; tests use a fake.
pub trait Ops: Send + Sync {
    /// Deletes a file or a folder tree for good, never following reparse points.
    /// Returns the bytes freed. An error means something was left behind.
    fn remove_permanent(&self, p: &Path) -> io::Result<u64>;
    /// Moves a file or folder to the Recycle Bin. Fails, touching nothing, when the
    /// drive has no Recycle Bin.
    fn recycle(&self, p: &Path) -> io::Result<Recycled>;
    fn flush_dns(&self) -> io::Result<()>;
    fn empty_recycle_bin(&self) -> io::Result<()>;
    /// Opens Explorer with the file selected.
    fn reveal(&self, p: &Path) -> io::Result<()>;
}

/// The real Win32 operations (see the `ops` module).
#[derive(Clone, Copy, Debug, Default)]
pub struct WinOps;

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

    /// Appends one line. A logging error never undoes or stops the operation it
    /// describes, but the caller learns about it.
    pub fn record(&self, run: &str, action: Action, path: &Path, detail: &str) -> io::Result<()> {
        let line = rules::log_line(SystemTime::now(), run, action, path, detail);
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?
            .write_all(format!("{line}\r\n").as_bytes())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CleanOutcome {
    /// Bytes gone from the disk for good.
    pub freed: u64,
    /// Bytes moved to the Recycle Bin (still on the disk until it is emptied).
    pub recycled: u64,
    pub failed: usize,
    /// Items that are completely gone.
    pub gone: Vec<PathBuf>,
    /// True when at least one line could not be written to the operations log.
    pub log_failed: bool,
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
    let guard = scan::guard(env);
    let mut out = CleanOutcome::default();
    let total = items.len();
    for (i, (cat, path)) in items.iter().enumerate() {
        let cancel = AtomicBool::new(false);
        let budget = fs::Budget::new(env.budget, &cancel);
        let record = |out: &mut CleanOutcome, action: Action, detail: &str| {
            let line = format!("{}, {detail}", cat.key());
            if log.record(run_id, action, path, &line).is_err() {
                out.log_failed = true;
            }
        };
        let fail = |out: &mut CleanOutcome, why: &str| {
            out.failed += 1;
            record(out, Action::Failed, why);
        };
        let item = match scan::check(env, &guard, *cat, path, &budget)
            .and_then(|it| fs::verify_final(path).map(|_| it))
        {
            Ok(it) => it,
            Err(why) => {
                fail(&mut out, &why);
                progress(i + 1, total);
                continue;
            }
        };
        let size = format_bytes(item.bytes);
        match cat.removal() {
            Removal::Permanent => match ops.remove_permanent(path) {
                Ok(n) if !exists(path) => {
                    out.freed = out.freed.saturating_add(n);
                    out.gone.push(path.clone());
                    record(&mut out, Action::Removed, &format_bytes(n));
                }
                res => {
                    // The check above may have used up the budget, so what is left
                    // is measured with a fresh one.
                    let again = AtomicBool::new(false);
                    let fresh = fs::Budget::new(env.budget, &again);
                    let left = fs::measure(path, &fresh);
                    let part = if left.complete {
                        item.bytes.saturating_sub(left.bytes)
                    } else {
                        0
                    };
                    out.freed = out.freed.saturating_add(part);
                    let why = match res {
                        Err(e) => e.to_string(),
                        Ok(_) => "still there".into(),
                    };
                    fail(&mut out, &format!("{} freed, {why}", format_bytes(part)));
                }
            },
            Removal::Recycle => match ops.recycle(path) {
                Ok(_) if exists(path) => fail(&mut out, "still there after recycling"),
                Ok(Recycled::InBin) => {
                    out.recycled = out.recycled.saturating_add(item.bytes);
                    out.gone.push(path.clone());
                    record(&mut out, Action::Recycled, &size);
                }
                Ok(Recycled::DeletedForGood) => {
                    out.freed = out.freed.saturating_add(item.bytes);
                    out.gone.push(path.clone());
                    let detail = format!("{size}, not recycled: deleted for good");
                    record(&mut out, Action::Removed, &detail);
                }
                Err(e) => fail(&mut out, &e.to_string()),
            },
        }
        progress(i + 1, total);
    }
    out
}

fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// A short id that ties one run's log lines together, e.g. "5f2a09c3".
pub fn new_run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mixed =
        (nanos as u64) ^ ((nanos >> 64) as u64) ^ u64::from(std::process::id()).rotate_left(32);
    format!("{:08x}", (mixed ^ (mixed >> 32)) as u32)
}

#[cfg(test)]
mod tests;
