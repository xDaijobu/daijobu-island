//! End-to-end tests of `scan` and `clean`. Everything happens inside a fresh temp
//! directory that stands in for the user profile, with a fake `Ops`. Nothing here
//! touches the real profile, the real Recycle Bin, DNS or Explorer.

use super::*;
use crate::fs::testutil::{git_available, junction};
use std::collections::HashSet;
use std::process::Command;
use std::sync::Mutex;

const DAY: Duration = Duration::from_secs(86_400);

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    env: ScanEnv,
    running: Arc<Mutex<HashSet<String>>>,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::final_path(tmp.path()).unwrap();
        let home = root.join("Users").join("ann");
        let local = home.join("AppData").join("Local");
        let roaming = home.join("AppData").join("Roaming");
        let temp = local.join("Temp");
        for d in [
            &temp,
            &roaming,
            &home.join("Projects"),
            &root.join("Windows"),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        let running: Arc<Mutex<HashSet<String>>> = Arc::default();
        let probe = running.clone();
        let env = ScanEnv {
            profile: home.clone(),
            local_appdata: local,
            roaming_appdata: roaming,
            temp,
            project_roots: vec![home.join("Projects")],
            // A month ahead: everything written by the test counts as old.
            now: SystemTime::now() + 30 * DAY,
            budget: Duration::from_secs(60),
            system_dirs: vec![root.join("Windows")],
            running: Arc::new(move |exe: &str| probe.lock().unwrap().contains(exe)),
        };
        Self {
            _tmp: tmp,
            root,
            env,
            running,
        }
    }

    fn home(&self) -> PathBuf {
        self.env.profile.clone()
    }

    fn run(&self, exe: &str) {
        self.running.lock().unwrap().insert(exe.to_string());
    }

    fn scan(&self, cat: Category) -> CategoryScan {
        scan(&self.env, cat, &AtomicBool::new(false))
    }
}

fn write(p: &Path, n: usize) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, vec![1u8; n]).unwrap();
}

fn paths(s: &CategoryScan) -> Vec<PathBuf> {
    s.items.iter().map(|i| i.path.clone()).collect()
}

/// Records every call. Deletes inside the test's temp dir to look like the real thing,
/// unless the path is in `fail`.
#[derive(Default)]
struct FakeOps {
    calls: Mutex<Vec<(&'static str, PathBuf)>>,
    fail: Mutex<HashSet<PathBuf>>,
}

impl FakeOps {
    fn calls(&self) -> Vec<(&'static str, PathBuf)> {
        self.calls.lock().unwrap().clone()
    }
    fn act(&self, what: &'static str, p: &Path) -> io::Result<u64> {
        self.calls.lock().unwrap().push((what, p.to_path_buf()));
        if self.fail.lock().unwrap().contains(p) {
            return Err(io::Error::other("file in use"));
        }
        fs::remove_tree(p)
    }
}

impl Ops for FakeOps {
    fn remove_permanent(&self, p: &Path) -> io::Result<u64> {
        self.act("remove", p)
    }
    fn recycle(&self, p: &Path) -> io::Result<()> {
        self.act("recycle", p).map(|_| ())
    }
    fn flush_dns(&self) -> io::Result<()> {
        self.calls.lock().unwrap().push(("dns", PathBuf::new()));
        Ok(())
    }
    fn empty_recycle_bin(&self) -> io::Result<()> {
        self.calls.lock().unwrap().push(("bin", PathBuf::new()));
        Ok(())
    }
    fn reveal(&self, p: &Path) -> io::Result<()> {
        self.calls.lock().unwrap().push(("reveal", p.to_path_buf()));
        Ok(())
    }
}

fn log_lines(p: &Path) -> Vec<String> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn temp_and_reports_with_age_filter() {
    let mut fx = Fx::new();
    let temp = fx.env.temp.clone();
    let local = fx.env.local_appdata.clone();
    write(&temp.join("old.tmp"), 100);
    write(&temp.join("olddir").join("a").join("b.bin"), 50);
    let report = local.join("Microsoft\\Windows\\WER\\ReportArchive\\AppCrash_x");
    write(&report.join("Report.wer"), 10);
    write(&local.join("CrashDumps").join("app.exe.123.dmp"), 20);
    write(&local.join("CrashDumps").join("notes.txt"), 5);
    std::fs::create_dir_all(temp.join("empty")).unwrap();

    let s = fx.scan(Category::Temp);
    assert_eq!(
        paths(&s),
        vec![
            temp.join("old.tmp"),
            temp.join("olddir"),
            local.join("CrashDumps").join("app.exe.123.dmp"),
            report.clone(),
        ]
    );
    assert_eq!(s.total, 180);
    assert_eq!(s.items[0].label, "temp · old.tmp");
    assert_eq!(s.note, None);

    // Today: temp files and dumps are too new, reports have no age limit.
    fx.env.now = SystemTime::now();
    let s = fx.scan(Category::Temp);
    assert_eq!(paths(&s), vec![report]);
    assert_eq!(s.items[0].label, "error report · AppCrash_x");
}

#[test]
fn browser_caches_skip_running_browsers_and_secrets() {
    let fx = Fx::new();
    let local = fx.env.local_appdata.clone();
    let chrome = local.join("Google\\Chrome\\User Data");
    write(&chrome.join("Default\\Cache\\Cache_Data\\f"), 100);
    write(&chrome.join("Profile 2\\Code Cache\\js\\f"), 50);
    write(&chrome.join("Default\\Network\\Cookies"), 999);
    write(&chrome.join("Default\\Local Storage\\leveldb\\x"), 999);
    write(&chrome.join("System Profile\\Cache\\f"), 999);
    write(
        &local.join("Microsoft\\Edge\\User Data\\Default\\GPUCache\\f"),
        70,
    );
    write(
        &local.join("Mozilla\\Firefox\\Profiles\\ab.default-release\\cache2\\e"),
        30,
    );
    fx.run("msedge.exe");

    let s = fx.scan(Category::Browser);
    assert_eq!(
        paths(&s),
        vec![
            chrome.join("Default\\Cache"),
            chrome.join("Profile 2\\Code Cache"),
            local.join("Mozilla\\Firefox\\Profiles\\ab.default-release\\cache2"),
        ]
    );
    assert_eq!(s.items[0].label, "chrome · Default · Cache");
    assert_eq!(s.note.as_deref(), Some("skipped while running: edge"));
}

#[test]
fn app_and_dev_caches_are_path_based() {
    let fx = Fx::new();
    let home = fx.home();
    write(&fx.env.roaming_appdata.join("Code\\Cache\\x"), 10);
    write(&fx.env.roaming_appdata.join("discord\\Code Cache\\x"), 10);
    write(&fx.env.local_appdata.join("npm-cache\\_cacache\\x"), 40);
    write(&fx.env.local_appdata.join("npm-cache\\_logs\\x"), 40);
    write(&home.join(".cargo\\registry\\cache\\x"), 30);
    write(&home.join(".cargo\\registry\\src\\x"), 30);
    fx.run("discord.exe");
    let s = fx.scan(Category::AppCache);
    let labels: Vec<_> = s.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(
        labels,
        ["npm cache", "cargo registry cache", "vs code · Cache"]
    );
    assert_eq!(s.total, 80);
    assert_eq!(s.note.as_deref(), Some("skipped while running: discord"));
}

#[test]
fn dev_purge_context_depth_and_git() {
    let mut fx = Fx::new();
    let pr = fx.home().join("Projects");
    // rust: target with Cargo.toml
    write(&pr.join("rs\\Cargo.toml"), 1);
    write(&pr.join("rs\\target\\debug\\app.exe"), 100);
    // js: node_modules yes, dist without a build script no
    std::fs::create_dir_all(pr.join("js")).unwrap();
    std::fs::write(pr.join("js\\package.json"), r#"{"scripts":{"test":"x"}}"#).unwrap();
    write(&pr.join("js\\node_modules\\x\\i.js"), 10);
    write(&pr.join("js\\dist\\i.js"), 10);
    // dotnet
    write(&pr.join("cs\\App.csproj"), 1);
    write(&pr.join("cs\\bin\\Debug\\a.dll"), 7);
    write(&pr.join("cs\\obj\\x"), 3);
    // python
    write(&pr.join("py\\main.py"), 1);
    write(&pr.join("py\\__pycache__\\main.pyc"), 4);
    write(&pr.join("py\\.venv\\pyvenv.cfg"), 5);
    write(&pr.join("py\\venv\\Lib\\x"), 5); // no pyvenv.cfg: not a venv
                                            // build via CMake
    write(&pr.join("cpp\\CMakeLists.txt"), 1);
    write(&pr.join("cpp\\build\\a.o"), 6);
    // no marker
    write(&pr.join("nomarker\\target\\x"), 50);
    // depth: target at depth 4 is in, depth 5 is out
    write(&pr.join("a\\b\\ok\\Cargo.toml"), 1);
    write(&pr.join("a\\b\\ok\\target\\x"), 8);
    write(&pr.join("a\\b\\c\\deep\\Cargo.toml"), 1);
    write(&pr.join("a\\b\\c\\deep\\target\\x"), 9);
    // nested .git inside the candidate
    write(&pr.join("sub\\Cargo.toml"), 1);
    write(&pr.join("sub\\target\\vendored\\.git\\HEAD"), 2);
    // a junction to a project outside the roots is never followed
    let outside = fx.root.join("outside");
    write(&outside.join("Cargo.toml"), 1);
    write(&outside.join("target\\x"), 1000);
    let linked = junction(&pr.join("linked"), &outside);

    let s = fx.scan(Category::DevPurge);
    let mut got = paths(&s);
    got.sort();
    let mut want = vec![
        pr.join("rs\\target"),
        pr.join("js\\node_modules"),
        pr.join("cs\\bin"),
        pr.join("cs\\obj"),
        pr.join("py\\__pycache__"),
        pr.join("py\\.venv"),
        pr.join("cpp\\build"),
        pr.join("a\\b\\ok\\target"),
    ];
    want.sort();
    assert_eq!(got, want, "linked junction created: {linked}");
    assert_eq!(
        s.note.as_deref(),
        Some("1 skipped (recent, tracked by git or contains .git)")
    );
    let rs = s
        .items
        .iter()
        .find(|i| i.path == pr.join("rs\\target"))
        .unwrap();
    assert_eq!(rs.label, "rs / target");

    // Touched today: nothing is offered.
    fx.env.now = SystemTime::now();
    let s = fx.scan(Category::DevPurge);
    assert!(s.items.is_empty());
    assert!(s.note.unwrap().starts_with("9 skipped"));
}

#[test]
fn dev_purge_skips_git_tracked_output() {
    if !git_available() {
        eprintln!("skip: git not installed");
        return;
    }
    let fx = Fx::new();
    let pr = fx.home().join("Projects");
    let repo = pr.join("tracked");
    write(&repo.join("Cargo.toml"), 1);
    write(&repo.join("target\\committed.txt"), 10);
    write(&pr.join("free\\Cargo.toml"), 1);
    write(&pr.join("free\\target\\x"), 10);
    let git = |dir: &Path, args: &[&str]| {
        assert!(Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    };
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "-f", "target/committed.txt"]);
    git(&pr.join("free"), &["init", "-q"]);

    let s = fx.scan(Category::DevPurge);
    assert_eq!(paths(&s), vec![pr.join("free\\target")]);
}

#[test]
fn big_files_top_list_skips_protected_and_links() {
    let fx = Fx::new();
    let home = fx.home();
    write(&home.join("Videos\\a.mkv"), 5000);
    write(&home.join("b.bin"), 3000);
    write(&home.join("Documents\\small.txt"), 10);
    write(&home.join(".ssh\\id_big"), 9000);
    write(&home.join("NTUSER.DAT"), 7000);
    write(&home.join("AppData\\Local\\Docker\\disk.vhdx"), 8000);
    let outside = fx.root.join("elsewhere");
    write(&outside.join("huge.iso"), 20_000);
    let _ = junction(&home.join("link"), &outside);
    for i in 0..25 {
        write(&home.join(format!("many\\f{i:02}")), 100 + i);
    }
    let s = fx.scan(Category::BigFiles);
    assert_eq!(s.items.len(), rules::BIG_FILES);
    assert_eq!(s.items[0].path, home.join("Videos\\a.mkv"));
    assert_eq!(s.items[1].path, home.join("b.bin"));
    assert_eq!(s.items[0].label, "a.mkv");
    assert!(s.items[2..].iter().all(|i| i.bytes >= 100 + 7));
}

#[test]
fn scan_honours_cancel_and_budget() {
    let mut fx = Fx::new();
    write(&fx.env.temp.join("old.tmp"), 100);
    let s = scan(&fx.env, Category::Temp, &AtomicBool::new(true));
    assert!(s.items.is_empty());
    assert_eq!(s.note.as_deref(), Some("cancelled"));
    fx.env.budget = Duration::ZERO;
    let s = fx.scan(Category::Temp);
    assert_eq!(
        s.note.as_deref(),
        Some("time limit reached, the list may be incomplete")
    );
}

#[test]
fn clean_routes_logs_and_reports_progress() {
    let fx = Fx::new();
    let pr = fx.home().join("Projects");
    let tmp_file = fx.env.temp.join("old.tmp");
    write(&tmp_file, 100);
    write(&pr.join("rs\\Cargo.toml"), 1);
    let target = pr.join("rs\\target");
    write(&target.join("x"), 300);
    let big = fx.home().join("Videos\\a.mkv");
    write(&big, 5000);

    let ops = FakeOps::default();
    let log = OpLog::new(fx.home().join(".daijobu\\operations.log"));
    let mut seen = Vec::new();
    let out = clean(
        &fx.env,
        &[
            (Category::Temp, tmp_file.clone()),
            (Category::DevPurge, target.clone()),
            (Category::BigFiles, big.clone()),
        ],
        &ops,
        &log,
        "run1",
        &mut |d, t| seen.push((d, t)),
    );
    assert_eq!(
        ops.calls(),
        vec![
            ("remove", tmp_file.clone()),
            ("recycle", target.clone()),
            ("recycle", big.clone())
        ]
    );
    assert_eq!(out.freed, 5400);
    assert_eq!(out.failed, 0);
    assert_eq!(
        out.gone,
        vec![tmp_file.clone(), target.clone(), big.clone()]
    );
    assert_eq!(seen, vec![(1, 3), (2, 3), (3, 3)]);

    let lines = log_lines(&fx.home().join(".daijobu\\operations.log"));
    assert_eq!(lines.len(), 3);
    let tail = |l: &str| l.split_once("] ").unwrap().1.to_string();
    assert!(lines[0].starts_with('[') && lines[0].contains("Z] [sweep run=run1] "));
    assert_eq!(
        tail(&lines[0]),
        format!(
            "[sweep run=run1] REMOVED {} (temp, 100 B)",
            tmp_file.display()
        )
    );
    assert_eq!(
        tail(&lines[1]),
        format!(
            "[sweep run=run1] RECYCLED {} (dev_purge, 300 B)",
            target.display()
        )
    );
    assert_eq!(
        tail(&lines[2]),
        format!(
            "[sweep run=run1] RECYCLED {} (big_files, 4.9 KB)",
            big.display()
        )
    );
}

#[test]
fn clean_refuses_what_the_guard_or_rules_reject() {
    let fx = Fx::new();
    let home = fx.home();
    let key = home.join(".ssh\\id_ed25519");
    write(&key, 10);
    let doc = home.join("Documents\\thesis.docx");
    write(&doc, 10);
    let fresh = fx.env.temp.join("fresh.tmp");
    write(&fresh, 10);
    let chrome_cache = fx
        .env
        .local_appdata
        .join("Google\\Chrome\\User Data\\Default\\Cache");
    write(&chrome_cache.join("f"), 10);
    fx.run("chrome.exe");

    let mut env = fx.env.clone();
    env.now = SystemTime::now();
    let ops = FakeOps::default();
    let log = OpLog::new(home.join(".daijobu\\operations.log"));
    let out = clean(
        &env,
        &[
            (Category::Temp, key.clone()),
            (Category::Temp, doc.clone()),
            (Category::Temp, fresh.clone()),
            (Category::Browser, chrome_cache.clone()),
            (Category::Temp, home.clone()),
            (Category::DevPurge, home.join("Projects")),
            (Category::Temp, fx.env.temp.join("missing.tmp")),
        ],
        &ops,
        &log,
        "r",
        &mut |_, _| {},
    );
    assert!(ops.calls().is_empty(), "nothing reached the ops");
    assert_eq!(out.failed, 7);
    assert_eq!(out.freed, 0);
    assert!(out.gone.is_empty());
    for p in [&key, &doc, &fresh, &chrome_cache] {
        assert!(p.exists());
    }
    let lines = log_lines(&home.join(".daijobu\\operations.log"));
    assert_eq!(lines.len(), 7);
    assert!(lines.iter().all(|l| l.contains("] FAILED ")));
    assert!(lines[0].ends_with("(temp, protected path)"));
    assert!(lines[1].ends_with("(temp, not in a temp folder)"));
    assert!(lines[2].ends_with("(temp, used in the last 7 days)"));
    assert!(lines[3].ends_with("(browser, chrome is running)"));
    assert!(lines[4].ends_with("(temp, protected path)"));
    assert!(lines[5].ends_with("(dev_purge, protected path)"));
    assert!(lines[6].ends_with("(temp, gone)"));
}

#[test]
fn clean_logs_failures_from_ops() {
    let fx = Fx::new();
    let a = fx.env.temp.join("a.tmp");
    let b = fx.env.temp.join("b.tmp");
    write(&a, 10);
    write(&b, 20);
    let ops = FakeOps::default();
    ops.fail.lock().unwrap().insert(a.clone());
    let log = OpLog::new(fx.home().join(".daijobu\\operations.log"));
    let out = clean(
        &fx.env,
        &[(Category::Temp, a.clone()), (Category::Temp, b.clone())],
        &ops,
        &log,
        "r2",
        &mut |_, _| {},
    );
    assert_eq!(out.failed, 1);
    assert_eq!(out.freed, 20);
    assert_eq!(out.gone, vec![b]);
    let lines = log_lines(&fx.home().join(".daijobu\\operations.log"));
    assert!(lines[0].contains(&format!(
        "FAILED {} (temp, 0 B freed, file in use)",
        a.display()
    )));
    assert!(lines[1].contains(" REMOVED "));
}

#[test]
fn clean_reverifies_after_a_path_swap() {
    let fx = Fx::new();
    let pr = fx.home().join("Projects");
    let target = pr.join("rs\\target");
    write(&pr.join("rs\\Cargo.toml"), 1);
    write(&target.join("x"), 10);
    let s = fx.scan(Category::DevPurge);
    assert_eq!(paths(&s), vec![target.clone()]);

    // After the preview, the project folder is swapped for a junction to a look-alike
    // somewhere else. The item path is unchanged, but it now leads elsewhere.
    let decoy = fx.root.join("decoy");
    write(&decoy.join("Cargo.toml"), 1);
    write(&decoy.join("target\\precious"), 10);
    std::fs::remove_dir_all(pr.join("rs")).unwrap();
    if !junction(&pr.join("rs"), &decoy) {
        eprintln!("skip: could not create a junction");
        return;
    }
    let ops = FakeOps::default();
    let log = OpLog::new(fx.home().join(".daijobu\\operations.log"));
    let out = clean(
        &fx.env,
        &[(Category::DevPurge, target.clone())],
        &ops,
        &log,
        "r3",
        &mut |_, _| {},
    );
    assert!(ops.calls().is_empty());
    assert_eq!(out.failed, 1);
    assert!(decoy.join("target\\precious").exists());
    let lines = log_lines(&fx.home().join(".daijobu\\operations.log"));
    assert!(lines[0].contains("path resolves elsewhere"), "{}", lines[0]);

    // An item that itself became a link is refused too.
    let tmp_dir = fx.env.temp.join("olddir");
    std::fs::create_dir_all(fx.root.join("t2")).unwrap();
    write(&fx.root.join("t2\\keep"), 5);
    assert!(junction(&tmp_dir, &fx.root.join("t2")));
    let out = clean(
        &fx.env,
        &[(Category::Temp, tmp_dir)],
        &ops,
        &log,
        "r4",
        &mut |_, _| {},
    );
    assert_eq!(out.failed, 1);
    assert!(fx.root.join("t2\\keep").exists());
}

#[test]
fn run_ids_are_short_hex() {
    let id = new_run_id();
    assert_eq!(id.len(), 8);
    assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
}

#[test]
fn fake_quick_actions_are_wired_through_the_trait() {
    // The real ones (DNS flush, empty bin, Explorer) are never called in tests.
    let ops = FakeOps::default();
    let dyn_ops: &dyn Ops = &ops;
    dyn_ops.flush_dns().unwrap();
    dyn_ops.empty_recycle_bin().unwrap();
    dyn_ops.reveal(Path::new(r"C:\x\y.bin")).unwrap();
    let names: Vec<_> = ops.calls().iter().map(|c| c.0).collect();
    assert_eq!(names, ["dns", "bin", "reveal"]);
}
