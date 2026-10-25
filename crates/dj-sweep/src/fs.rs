//! File system access: the measuring walker (time budget, cancel flag, never follows
//! reparse points), project facts, the final-path re-verify, the `git ls-files` check
//! and the permanent delete.

use crate::rules::{self, CandidateFacts, ProjectFacts};
use std::fs::{self, Metadata};
use std::io;
use std::os::windows::fs::MetadataExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime};

const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x4_0000;
const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Symlinks, junctions, mount points, cloud placeholders: anything we must not follow.
pub fn is_reparse(md: &Metadata) -> bool {
    md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Online-only files (OneDrive and friends). Their size is not on this disk.
pub fn is_cloud_only(md: &Metadata) -> bool {
    md.file_attributes()
        & (FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_RECALL_ON_OPEN
            | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
        != 0
}

/// A deadline plus the caller's cancel flag.
pub struct Budget<'a> {
    deadline: Instant,
    cancel: &'a AtomicBool,
}

impl<'a> Budget<'a> {
    pub fn new(budget: std::time::Duration, cancel: &'a AtomicBool) -> Self {
        Self {
            deadline: Instant::now() + budget,
            cancel,
        }
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// True once the deadline passed or the caller cancelled.
    pub fn over(&self) -> bool {
        self.cancelled() || Instant::now() >= self.deadline
    }
}

/// What a walk found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub bytes: u64,
    /// Newest modification time of any file or folder seen (including the root).
    pub newest: Option<SystemTime>,
    /// A `.git` folder or file somewhere below the root.
    pub has_git: bool,
    /// False when the budget ran out, so the numbers are partial.
    pub complete: bool,
}

impl Tally {
    fn see(&mut self, md: &Metadata) {
        if let Ok(t) = md.modified() {
            self.newest = Some(self.newest.map_or(t, |n| n.max(t)));
        }
    }
}

/// Sums the files under `root` (or `root` itself if it is a file). Reparse points are
/// neither followed nor counted. Unreadable folders are skipped.
pub fn measure(root: &Path, budget: &Budget) -> Tally {
    let mut t = Tally {
        complete: true,
        ..Tally::default()
    };
    let Ok(md) = fs::symlink_metadata(root) else {
        return t;
    };
    if is_reparse(&md) {
        return t;
    }
    t.see(&md);
    if !md.is_dir() {
        t.bytes = md.len();
        return t;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if budget.over() {
            t.complete = false;
            break;
        }
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            // On Windows this comes from the directory listing and does not traverse.
            let Ok(md) = e.metadata() else { continue };
            if e.file_name().eq_ignore_ascii_case(".git") {
                t.has_git = true;
            }
            if is_reparse(&md) {
                continue;
            }
            t.see(&md);
            if md.is_dir() {
                stack.push(e.path());
            } else {
                t.bytes = t.bytes.saturating_add(md.len());
            }
        }
    }
    t
}

/// The real, final path of `p` (8.3 names expanded, links resolved), without the
/// verbatim prefix.
pub fn final_path(p: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(p).map(|c| rules::plain_path(&c))
}

/// `final_path`, or `p` unchanged when it does not exist.
pub fn final_or_same(p: PathBuf) -> PathBuf {
    final_path(&p).unwrap_or(p)
}

/// The last check before anything is removed: `p` must exist, must not be a reparse
/// point itself, and its final path must be `p` (so no folder on the way has been
/// swapped for a link). Returns the metadata on success.
pub fn verify_final(p: &Path) -> Result<Metadata, String> {
    let md = fs::symlink_metadata(p).map_err(|_| "gone".to_string())?;
    if is_reparse(&md) {
        return Err("is a link or reparse point".into());
    }
    let fin = final_path(p).map_err(|e| format!("cannot resolve: {e}"))?;
    if !rules::same_path(&fin, p) {
        return Err(format!("path resolves elsewhere: {}", fin.display()));
    }
    Ok(md)
}

/// Reads the project folder's markers.
pub fn project_facts(dir: &Path) -> ProjectFacts {
    let mut f = ProjectFacts::default();
    let Ok(rd) = fs::read_dir(dir) else { return f };
    for e in rd.flatten() {
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() || is_reparse(&md) {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_lowercase();
        match name.as_str() {
            "cargo.toml" => f.cargo_toml = true,
            "pom.xml" => f.pom_xml = true,
            "cmakelists.txt" => f.cmake = true,
            "build.gradle" | "build.gradle.kts" => f.gradle = true,
            "package.json" => {
                f.package_json = true;
                f.package_build_script =
                    read_small(&e.path(), &md).is_some_and(|s| rules::package_json_has_build(&s));
            }
            "pyproject.toml" => {
                f.pyproject_build_system = read_small(&e.path(), &md)
                    .is_some_and(|s| rules::pyproject_has_build_system(&s));
            }
            _ => {}
        }
        if name.ends_with(".csproj") {
            f.csproj = true;
        }
        if name.ends_with(".py") {
            f.py_files = true;
        }
    }
    f
}

fn read_small(p: &Path, md: &Metadata) -> Option<String> {
    (md.len() <= 1 << 20)
        .then(|| fs::read_to_string(p).ok())
        .flatten()
}

pub fn candidate_facts(dir: &Path) -> CandidateFacts {
    CandidateFacts {
        pyvenv_cfg: fs::symlink_metadata(dir.join("pyvenv.cfg")).is_ok_and(|m| m.is_file()),
    }
}

/// The newest change to the project's own files: files up to three folders deep,
/// skipping build-output folders and `.git` internals (except its index and HEAD,
/// which move on commit and checkout). Folder times are ignored, because creating
/// `node_modules` alone should not count as work on the project.
pub fn project_activity(dir: &Path, budget: &Budget) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let mut bump = |md: &Metadata| {
        if let Ok(t) = md.modified() {
            newest = Some(newest.map_or(t, |n| n.max(t)));
        }
    };
    for f in [".git\\index", ".git\\HEAD"] {
        if let Ok(md) = fs::symlink_metadata(dir.join(f)) {
            bump(&md);
        }
    }
    let mut seen = 0usize;
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        if budget.over() || seen > 5000 {
            break;
        }
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            let Ok(md) = e.metadata() else { continue };
            if is_reparse(&md) {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if md.is_dir() {
                if depth < 3 && !rules::is_purge_name(&name) && !name.eq_ignore_ascii_case(".git") {
                    stack.push((e.path(), depth + 1));
                }
            } else {
                bump(&md);
            }
        }
    }
    newest
}

/// Does git track anything inside `project\name`? `Ok(false)` when the project is not
/// inside a work tree. Any git failure is an error, and the caller skips the candidate.
pub fn git_tracked(project: &Path, name: &str) -> Result<bool, String> {
    if !project.ancestors().any(|a| a.join(".git").exists()) {
        return Ok(false);
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["ls-files", "-z", "--"])
        .arg(name)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("git not available: {e}"))?;
    if !out.status.success() {
        return Err(format!("git ls-files failed ({})", out.status));
    }
    Ok(!out.stdout.is_empty())
}

/// Deletes `p` for good. Reparse points are removed as links, never followed. Keeps
/// going past files that cannot be removed (in use) and reports the first error at
/// the end. Returns the bytes of the files that were removed.
pub fn remove_tree(p: &Path) -> io::Result<u64> {
    let mut freed = 0u64;
    let mut first_err: Option<io::Error> = None;
    remove_rec(p, &mut freed, &mut first_err);
    match first_err {
        Some(e) => Err(e),
        None => Ok(freed),
    }
}

fn remove_rec(p: &Path, freed: &mut u64, err: &mut Option<io::Error>) {
    let note = |e: io::Error, err: &mut Option<io::Error>| {
        if err.is_none() {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            *err = Some(io::Error::new(e.kind(), format!("{name}: {e}")));
        }
    };
    let md = match fs::symlink_metadata(p) {
        Ok(md) => md,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => return note(e, err),
    };
    if is_reparse(&md) {
        // Remove the link itself. A directory link is removed with RemoveDirectory,
        // which does not touch the target.
        // `md.is_dir()` is false for links, so ask the attributes.
        let r = if md.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0 {
            fs::remove_dir(p)
        } else {
            fs::remove_file(p)
        };
        if let Err(e) = r {
            note(e, err);
        }
        return;
    }
    if md.is_dir() {
        match fs::read_dir(p) {
            Ok(rd) => {
                for e in rd.flatten() {
                    remove_rec(&e.path(), freed, err);
                }
            }
            Err(e) => return note(e, err),
        }
        if let Err(e) = fs::remove_dir(p) {
            note(e, err);
        }
    } else {
        if md.file_attributes() & FILE_ATTRIBUTE_READONLY != 0 {
            let mut perm = md.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            let _ = fs::set_permissions(p, perm);
        }
        match fs::remove_file(p) {
            Ok(()) => *freed = freed.saturating_add(md.len()),
            Err(e) => note(e, err),
        }
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::Path;
    use std::process::Command;

    /// Creates a directory junction with `mklink /J` (no admin needed). False if it
    /// could not be created, so the caller can skip.
    pub fn junction(link: &Path, target: &Path) -> bool {
        Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
            && link.exists()
    }

    pub fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;
    use std::time::Duration;

    fn tmp() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let root = final_path(t.path()).unwrap();
        (t, root)
    }

    fn write(p: &Path, n: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![7u8; n]).unwrap();
    }

    #[test]
    fn measure_sums_and_skips_junctions() {
        let (_t, root) = tmp();
        let dir = root.join("dir");
        write(&dir.join("a.bin"), 100);
        write(&dir.join("sub").join("b.bin"), 50);
        let outside = root.join("outside");
        write(&outside.join("huge.bin"), 10_000);
        let cancel = AtomicBool::new(false);
        let b = Budget::new(Duration::from_secs(30), &cancel);
        let t = measure(&dir, &b);
        assert_eq!(t.bytes, 150);
        assert!(t.complete && !t.has_git && t.newest.is_some());

        if !junction(&dir.join("link"), &outside) {
            eprintln!("skip: could not create a junction");
            return;
        }
        assert!(is_reparse(&fs::symlink_metadata(dir.join("link")).unwrap()));
        assert_eq!(measure(&dir, &b).bytes, 150, "junction not followed");
        assert_eq!(
            measure(&dir.join("link"), &b).bytes,
            0,
            "junction root not followed"
        );

        // Deleting the tree removes the link, not what it points to.
        assert_eq!(remove_tree(&dir).unwrap(), 150);
        assert!(!dir.exists());
        assert_eq!(
            fs::metadata(outside.join("huge.bin")).unwrap().len(),
            10_000
        );
    }

    #[test]
    fn measure_honours_cancel_and_finds_git() {
        let (_t, root) = tmp();
        write(&root.join("c").join(".git").join("HEAD"), 1);
        let cancel = AtomicBool::new(false);
        let b = Budget::new(Duration::from_secs(30), &cancel);
        assert!(measure(&root.join("c"), &b).has_git);
        cancel.store(true, Ordering::Relaxed);
        assert!(!measure(&root.join("c"), &b).complete);
        let zero = AtomicBool::new(false);
        let b0 = Budget::new(Duration::ZERO, &zero);
        assert!(!measure(&root.join("c"), &b0).complete);
        // A plain file is measured without walking.
        assert!(measure(&root.join("c").join(".git").join("HEAD"), &b0).complete);
    }

    #[test]
    fn verify_final_catches_a_swapped_parent() {
        let (_t, root) = tmp();
        let item = root.join("a").join("x");
        write(&item.join("f"), 10);
        assert!(verify_final(&item).is_ok());
        assert!(verify_final(&root.join("missing")).is_err());

        let other = root.join("other");
        write(&other.join("x").join("f"), 10);
        fs::remove_dir_all(root.join("a")).unwrap();
        if !junction(&root.join("a"), &other) {
            eprintln!("skip: could not create a junction");
            return;
        }
        let e = verify_final(&item).unwrap_err();
        assert!(e.contains("resolves elsewhere"), "{e}");
        assert!(verify_final(&root.join("a")).unwrap_err().contains("link"));
    }

    #[test]
    fn remove_tree_clears_readonly_and_counts() {
        let (_t, root) = tmp();
        let d = root.join("d");
        write(&d.join("ro.txt"), 30);
        write(&d.join("s").join("x"), 12);
        let mut perm = fs::metadata(d.join("ro.txt")).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(d.join("ro.txt"), perm).unwrap();
        assert_eq!(remove_tree(&d).unwrap(), 42);
        assert!(!d.exists());
        assert_eq!(remove_tree(&d).unwrap(), 0, "already gone is fine");
    }

    #[test]
    fn project_facts_read_markers() {
        let (_t, root) = tmp();
        write(&root.join("Cargo.toml"), 1);
        fs::write(root.join("package.json"), r#"{"scripts":{"build":"tsc"}}"#).unwrap();
        fs::write(root.join("pyproject.toml"), "[build-system]\n").unwrap();
        write(&root.join("App.csproj"), 1);
        write(&root.join("main.py"), 1);
        write(&root.join("CMakeLists.txt"), 1);
        write(&root.join("build.gradle.kts"), 1);
        let f = project_facts(&root);
        assert_eq!(
            f,
            ProjectFacts {
                cargo_toml: true,
                pom_xml: false,
                csproj: true,
                package_json: true,
                package_build_script: true,
                cmake: true,
                gradle: true,
                pyproject_build_system: true,
                py_files: true,
            }
        );
        assert_eq!(project_facts(&root.join("nope")), ProjectFacts::default());
        write(&root.join("venv").join("pyvenv.cfg"), 1);
        assert!(candidate_facts(&root.join("venv")).pyvenv_cfg);
        assert!(!candidate_facts(&root).pyvenv_cfg);
    }

    #[test]
    fn activity_ignores_build_output() {
        let (_t, root) = tmp();
        let cancel = AtomicBool::new(false);
        let b = Budget::new(Duration::from_secs(30), &cancel);
        assert_eq!(project_activity(&root, &b), None);
        write(&root.join("node_modules").join("x.js"), 1);
        assert_eq!(project_activity(&root, &b), None);
        write(&root.join("src").join("main.rs"), 1);
        assert!(project_activity(&root, &b).is_some());
    }

    #[test]
    fn git_tracked_check() {
        let (_t, root) = tmp();
        write(&root.join("target").join("x"), 1);
        assert_eq!(git_tracked(&root, "target"), Ok(false), "not a repo");
        if !git_available() {
            eprintln!("skip: git not installed");
            return;
        }
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        };
        assert!(git(&["init", "-q"]));
        assert_eq!(git_tracked(&root, "target"), Ok(false), "untracked");
        assert!(git(&["add", "-f", "target/x"]));
        assert_eq!(git_tracked(&root, "target"), Ok(true));
        // A broken repo is an error, never "untracked".
        fs::remove_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git"), "gitdir: nowhere").unwrap();
        assert!(git_tracked(&root, "target").is_err());
    }
}
