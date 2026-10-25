//! The category scanners. Each category has an enumerator (where to look) and one
//! `check` per item that both the preview and `clean` use, so an item is removed only
//! if it would still be previewed right now.

use crate::fs::{self as sfs, Budget};
use crate::rules::{self, BrowserKind, Guard, TopN};
use crate::{Category, CategoryScan, Item, ScanEnv};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn guard(env: &ScanEnv) -> Guard {
    Guard::new(
        &env.profile,
        &env.local_appdata,
        &env.roaming_appdata,
        &env.temp,
        &env.system_dirs,
        &env.project_roots,
    )
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `Ok` when `env.temp` is the usual `%LOCALAPPDATA%\Temp` (see `rules::check_temp_dir`).
fn temp_ok(env: &ScanEnv) -> Result<(), String> {
    rules::check_temp_dir(
        &env.temp,
        &env.local_appdata,
        &env.profile,
        &env.project_roots,
    )
}

fn wer_dirs(env: &ScanEnv) -> [PathBuf; 2] {
    let wer = env.local_appdata.join("Microsoft\\Windows\\WER");
    [wer.join("ReportArchive"), wer.join("ReportQueue")]
}

fn crash_dumps(env: &ScanEnv) -> PathBuf {
    env.local_appdata.join("CrashDumps")
}

/// Checks one item against its category's rules right now. `Ok` carries its current
/// size and label; `Err` says why it must be left alone.
pub(crate) fn check(
    env: &ScanEnv,
    guard: &Guard,
    cat: Category,
    p: &Path,
    budget: &Budget,
) -> Result<Item, String> {
    if guard.is_protected(p) {
        return Err("protected path".into());
    }
    let md = fs::symlink_metadata(p).map_err(|_| "gone".to_string())?;
    if sfs::is_reparse(&md) {
        return Err("is a link or reparse point".into());
    }
    let parent = p.parent().ok_or("no parent")?;
    let name = name_of(p);
    let item = |bytes: u64, label: String| Item {
        path: p.to_path_buf(),
        bytes,
        label,
    };
    let measured = || {
        let t = sfs::measure(p, budget);
        if t.complete {
            Ok(t)
        } else {
            Err("time limit reached while measuring".to_string())
        }
    };

    match cat {
        Category::Temp => {
            let old = |t: &sfs::Tally| {
                t.newest
                    .is_some_and(|n| rules::is_older_than(n, env.now, rules::TEMP_MIN_AGE_DAYS))
            };
            if rules::same_path(parent, &env.temp) {
                temp_ok(env)?;
                let t = measured()?;
                if !old(&t) {
                    return Err("used in the last 7 days".into());
                }
                Ok(item(t.bytes, format!("temp · {name}")))
            } else if wer_dirs(env).iter().any(|w| rules::same_path(parent, w)) {
                let t = measured()?;
                Ok(item(t.bytes, format!("error report · {name}")))
            } else if rules::same_path(parent, &crash_dumps(env)) {
                let is_dmp = Path::new(&name)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("dmp"));
                if !md.is_file() || !is_dmp {
                    return Err("not a crash dump".into());
                }
                let t = measured()?;
                if !old(&t) {
                    return Err("newer than 7 days".into());
                }
                Ok(item(t.bytes, format!("crash dump · {name}")))
            } else {
                Err("not in a temp folder".into())
            }
        }

        Category::Browser => {
            if !md.is_dir() {
                return Err("not a folder".into());
            }
            let profile_dir = parent;
            let profile = name_of(profile_dir);
            let data = profile_dir.parent().ok_or("no parent")?;
            let b = rules::BROWSERS
                .iter()
                .find(|b| rules::same_path(data, &env.local_appdata.join(b.rel)))
                .ok_or("not a browser profile")?;
            let ok = match b.kind {
                BrowserKind::Chromium => {
                    rules::is_chromium_profile(&profile)
                        && rules::CHROMIUM_CACHES
                            .iter()
                            .any(|c| c.eq_ignore_ascii_case(&name))
                }
                BrowserKind::Firefox => name.eq_ignore_ascii_case(rules::FIREFOX_CACHE),
            };
            if !ok {
                return Err("not a browser cache folder".into());
            }
            if (env.running)(b.exe) {
                return Err(format!("{} is running", b.name));
            }
            let t = measured()?;
            Ok(item(t.bytes, format!("{} · {profile} · {name}", b.name)))
        }

        Category::AppCache => {
            if !md.is_dir() {
                return Err("not a folder".into());
            }
            let a = rules::APP_CACHES
                .iter()
                .find(|a| rules::same_path(p, &app_path(env, a)))
                .ok_or("not a known cache folder")?;
            if let Some(exe) = a.exe {
                if (env.running)(exe) {
                    return Err(format!("{exe} is running"));
                }
            }
            let t = measured()?;
            Ok(item(t.bytes, a.label.to_string()))
        }

        Category::DevPurge => {
            if !md.is_dir() {
                return Err("not a folder".into());
            }
            if !rules::is_purge_name(&name) {
                return Err("not a build-output name".into());
            }
            let in_root = env.project_roots.iter().any(|r| {
                rules::is_under(p, r) && {
                    let below = p
                        .components()
                        .count()
                        .saturating_sub(r.components().count());
                    (1..=rules::PURGE_MAX_DEPTH).contains(&below)
                }
            });
            if !in_root {
                return Err("not inside a project root".into());
            }
            if guard.is_protected(parent) {
                return Err("project folder is protected".into());
            }
            let facts = sfs::project_facts(parent);
            let cfacts = sfs::candidate_facts(p);
            if !rules::purge_context_ok(&name, &facts, &cfacts) {
                return Err("no project marker".into());
            }
            if let Some(t) = sfs::project_activity(parent, budget)? {
                if !rules::is_older_than(t, env.now, rules::PURGE_QUIET_DAYS) {
                    return Err("project touched in the last 14 days".into());
                }
            }
            let t = measured()?;
            if t.has_git {
                return Err("contains a .git folder".into());
            }
            match sfs::git_tracked(parent, &name) {
                Ok(false) => {}
                Ok(true) => return Err("has git-tracked files".into()),
                Err(e) => return Err(e),
            }
            Ok(item(t.bytes, format!("{} / {name}", name_of(parent))))
        }

        Category::BigFiles => {
            if !md.is_file() {
                return Err("not a file".into());
            }
            if sfs::is_cloud_only(&md) {
                return Err("online-only file".into());
            }
            if !rules::is_under(p, &env.profile) || rules::is_under(p, &appdata(env)) {
                return Err("not a user file".into());
            }
            Ok(item(md.len(), name))
        }
    }
}

fn app_path(env: &ScanEnv, a: &rules::AppCache) -> PathBuf {
    let base = match a.base {
        rules::Base::Local => &env.local_appdata,
        rules::Base::Roaming => &env.roaming_appdata,
        rules::Base::Profile => &env.profile,
    };
    base.join(a.rel)
}

/// `%USERPROFILE%\AppData`: app-managed files, left out of big files.
fn appdata(env: &ScanEnv) -> PathBuf {
    env.local_appdata
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| env.profile.join("AppData"))
}

fn children(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

fn is_plain_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !sfs::is_reparse(&m))
}

/// Paths to check for one category, plus notes about whole groups that were skipped.
fn candidates(
    env: &ScanEnv,
    guard: &Guard,
    cat: Category,
    budget: &Budget,
    notes: &mut Vec<String>,
) -> Vec<PathBuf> {
    match cat {
        Category::Temp => {
            let mut v = Vec::new();
            let temp = match temp_ok(env) {
                Ok(()) => Some(env.temp.clone()),
                Err(why) => {
                    notes.push(format!("temp files skipped: {why}"));
                    None
                }
            };
            for dir in temp
                .into_iter()
                .chain(wer_dirs(env))
                .chain([crash_dumps(env)])
            {
                if is_plain_dir(&dir) {
                    v.extend(children(&dir));
                }
            }
            v
        }
        Category::Browser => {
            let mut v = Vec::new();
            let mut running = Vec::new();
            for b in rules::BROWSERS {
                let data = env.local_appdata.join(b.rel);
                if !is_plain_dir(&data) {
                    continue;
                }
                if (env.running)(b.exe) {
                    running.push(b.name);
                    continue;
                }
                for profile in children(&data) {
                    if !is_plain_dir(&profile) {
                        continue;
                    }
                    match b.kind {
                        BrowserKind::Chromium => {
                            if rules::is_chromium_profile(&name_of(&profile)) {
                                v.extend(rules::CHROMIUM_CACHES.iter().map(|c| profile.join(c)));
                            }
                        }
                        BrowserKind::Firefox => v.push(profile.join(rules::FIREFOX_CACHE)),
                    }
                }
            }
            if !running.is_empty() {
                notes.push(format!("skipped while running: {}", running.join(", ")));
            }
            v.retain(|p| is_plain_dir(p));
            v
        }
        Category::AppCache => {
            let mut v = Vec::new();
            let mut running: Vec<&str> = Vec::new();
            for a in rules::APP_CACHES {
                let p = app_path(env, a);
                if !is_plain_dir(&p) {
                    continue;
                }
                if let Some(exe) = a.exe {
                    if (env.running)(exe) {
                        let app = a.label.split(" · ").next().unwrap_or(exe);
                        if !running.contains(&app) {
                            running.push(app);
                        }
                        continue;
                    }
                }
                v.push(p);
            }
            if !running.is_empty() {
                notes.push(format!("skipped while running: {}", running.join(", ")));
            }
            v
        }
        Category::DevPurge => purge_candidates(env, guard, budget),
        Category::BigFiles => big_file_candidates(env, guard, budget),
    }
}

/// Folders named like build output whose parent (the project) is at most
/// `PURGE_MAX_DEPTH - 1` below a root. Never descends into reparse points, hidden
/// folders or build-output folders.
fn purge_candidates(env: &ScanEnv, guard: &Guard, budget: &Budget) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in &env.project_roots {
        if !is_plain_dir(root) {
            continue;
        }
        let mut stack = vec![(root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if budget.over() {
                return out;
            }
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let Ok(md) = e.metadata() else { continue };
                if !md.is_dir() || sfs::is_reparse(&md) {
                    continue;
                }
                let name = e.file_name().to_string_lossy().into_owned();
                let path = e.path();
                if rules::is_purge_name(&name) {
                    out.push(path);
                } else if depth + 2 <= rules::PURGE_MAX_DEPTH
                    && !name.starts_with('.')
                    && !guard.is_protected(&path)
                {
                    stack.push((path, depth + 1));
                }
            }
        }
    }
    out
}

/// The largest files under the profile, skipping protected folders, AppData,
/// reparse points and online-only files.
fn big_file_candidates(env: &ScanEnv, guard: &Guard, budget: &Budget) -> Vec<PathBuf> {
    let mut top = TopN::new(rules::BIG_FILES);
    let appdata = appdata(env);
    if !is_plain_dir(&env.profile) {
        return Vec::new();
    }
    let mut stack = vec![env.profile.clone()];
    while let Some(dir) = stack.pop() {
        if budget.over() {
            break;
        }
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            if sfs::is_reparse(&md) {
                continue;
            }
            let path = e.path();
            if md.is_dir() {
                if !rules::same_path(&path, &appdata) && !guard.is_protected(&path) {
                    stack.push(path);
                }
            } else if !sfs::is_cloud_only(&md)
                && top.floor().is_none_or(|f| md.len() > f)
                && !guard.is_protected(&path)
            {
                top.push(md.len(), path);
            }
        }
    }
    top.into_sorted().into_iter().map(|(_, p)| p).collect()
}

pub(crate) fn run(env: &ScanEnv, cat: Category, budget: &Budget) -> CategoryScan {
    let guard = guard(env);
    let mut notes = Vec::new();
    let paths = candidates(env, &guard, cat, budget, &mut notes);
    let mut items = Vec::new();
    let mut skipped = 0usize;
    for p in paths {
        if budget.over() {
            break;
        }
        match check(env, &guard, cat, &p, budget) {
            Ok(it) if it.bytes > 0 => items.push(it),
            Ok(_) => {}
            Err(why) => {
                if cat == Category::DevPurge && why != "no project marker" {
                    skipped += 1;
                }
            }
        }
    }
    if skipped > 0 {
        notes.push(format!(
            "{skipped} skipped (recent, too big to check, or under git)"
        ));
    }
    if budget.cancelled() {
        notes.push("cancelled".into());
    } else if budget.over() {
        notes.push("time limit reached, the list may be incomplete".into());
    }
    rules::sort_items(&mut items);
    CategoryScan {
        category: cat,
        total: rules::total(&items),
        items,
        note: (!notes.is_empty()).then(|| notes.join("; ")),
    }
}
