//! Pure rules: category lists, the protected-path guard, the age filter, the dev-purge
//! context rules, size aggregation and the operations-log line. No file system access.
//!
//! Every list here is our own, written from general Windows knowledge.

use crate::{Action, Item};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Temp files and crash dumps must be at least this old.
pub const TEMP_MIN_AGE_DAYS: u64 = 7;
/// A project touched more recently than this is left alone.
pub const PURGE_QUIET_DAYS: u64 = 14;
/// Candidates sit at most this many folders below a project root.
pub const PURGE_MAX_DEPTH: usize = 4;
/// Big files list length.
pub const BIG_FILES: usize = 20;

// ---------------------------------------------------------------- paths

/// Drops the `\\?\` prefix that `canonicalize` adds (and turns `\\?\UNC\` into `\\`).
pub fn plain_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        p.to_path_buf()
    }
}

/// Lowercase, backslashes, no verbatim prefix, no trailing separator. For comparing
/// Windows paths, which are case-insensitive.
pub fn norm(p: &Path) -> String {
    let s = plain_path(p)
        .to_string_lossy()
        .replace('/', "\\")
        .to_lowercase();
    s.trim_end_matches('\\').to_string()
}

/// True if `p` is `root` or lies below it. Compares whole components.
pub fn is_under(p: &Path, root: &Path) -> bool {
    under_norm(&norm(p), &norm(root))
}

fn under_norm(p: &str, root: &str) -> bool {
    !root.is_empty()
        && (p == root || (p.starts_with(root) && p.as_bytes().get(root.len()) == Some(&b'\\')))
}

/// Same path, ignoring case, separators and the verbatim prefix.
pub fn same_path(a: &Path, b: &Path) -> bool {
    norm(a) == norm(b)
}

// ---------------------------------------------------------------- guard

/// Names that are never touched, wherever they appear: browser credentials and
/// session state, plus Firefox's equivalents.
const PROTECTED_NAMES: &[&str] = &[
    "cookies",
    "local storage",
    "session storage",
    "indexeddb",
    "sessions",
    "web data",
    "local state",
    "cookies.sqlite",
    "logins.json",
    "key4.db",
];

/// Under the profile.
const PROFILE_SECRETS: &[&str] = &[
    ".ssh", ".gnupg", ".aws", ".azure", ".kube", ".docker", ".daijobu",
];

/// Under `%LOCALAPPDATA%`.
const LOCAL_PROTECTED: &[&str] = &["Packages", "Microsoft\\Credentials", "Microsoft\\Vault"];

/// Under `%APPDATA%`.
const ROAMING_PROTECTED: &[&str] = &[
    "Microsoft\\Credentials",
    "Microsoft\\Vault",
    "Microsoft\\Crypto",
    "Microsoft\\Protect",
    "Microsoft\\SystemCertificates",
];

/// The protected-path guard. A path is protected when it is inside (or is an ancestor
/// of) a protected tree, when it is (or is an ancestor of) a key folder such as the
/// profile root, when any component is a protected name, or when it is too shallow.
#[derive(Clone, Debug)]
pub struct Guard {
    /// Whole trees: the path, everything below it and every ancestor.
    trees: Vec<String>,
    /// The folder itself and its ancestors, but not its contents.
    anchors: Vec<String>,
}

impl Guard {
    pub fn new(
        profile: &Path,
        local_appdata: &Path,
        roaming_appdata: &Path,
        temp: &Path,
        system_dirs: &[PathBuf],
        project_roots: &[PathBuf],
    ) -> Self {
        let mut trees: Vec<String> = system_dirs.iter().map(|p| norm(p)).collect();
        trees.extend(PROFILE_SECRETS.iter().map(|s| norm(&profile.join(s))));
        trees.extend(LOCAL_PROTECTED.iter().map(|s| norm(&local_appdata.join(s))));
        trees.extend(
            ROAMING_PROTECTED
                .iter()
                .map(|s| norm(&roaming_appdata.join(s))),
        );
        let mut anchors = vec![
            norm(profile),
            norm(local_appdata),
            norm(roaming_appdata),
            norm(temp),
        ];
        if let Some(appdata) = local_appdata.parent() {
            anchors.push(norm(appdata));
        }
        anchors.extend(project_roots.iter().map(|p| norm(p)));
        trees.retain(|s| !s.is_empty());
        anchors.retain(|s| !s.is_empty());
        Self { trees, anchors }
    }

    pub fn is_protected(&self, p: &Path) -> bool {
        let n = norm(p);
        if !p.is_absolute() || depth(&n) < 2 {
            return true;
        }
        if self
            .trees
            .iter()
            .any(|t| under_norm(&n, t) || under_norm(t, &n))
        {
            return true;
        }
        if self.anchors.iter().any(|a| under_norm(a, &n)) {
            return true;
        }
        n.split('\\').any(|c| {
            PROTECTED_NAMES.contains(&c)
                || c.starts_with("login data")
                || c.starts_with("ntuser.")
                || c == "usrclass.dat"
        })
    }
}

/// Folders below the drive or share root, e.g. `c:\a\b` is 2.
fn depth(n: &str) -> usize {
    let rest = if let Some(unc) = n.strip_prefix("\\\\") {
        // \\server\share\...
        unc.splitn(3, '\\').nth(2).unwrap_or("")
    } else {
        n.split_once('\\').map(|(_, r)| r).unwrap_or("")
    };
    rest.split('\\').filter(|c| !c.is_empty()).count()
}

// ---------------------------------------------------------------- age

/// True when `mtime` is at least `days` before `now`. A time in the future is never old.
pub fn is_older_than(mtime: SystemTime, now: SystemTime, days: u64) -> bool {
    now.duration_since(mtime)
        .map(|d| d >= Duration::from_secs(days * 86_400))
        .unwrap_or(false)
}

// ---------------------------------------------------------------- browsers and apps

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserKind {
    /// `%LOCALAPPDATA%\<rel>` is the "User Data" folder with one folder per profile.
    Chromium,
    /// `%LOCALAPPDATA%\<rel>` holds one folder per profile, each with `cache2`.
    Firefox,
}

#[derive(Clone, Copy, Debug)]
pub struct Browser {
    pub name: &'static str,
    pub exe: &'static str,
    pub kind: BrowserKind,
    pub rel: &'static str,
}

pub const BROWSERS: &[Browser] = &[
    Browser {
        name: "chrome",
        exe: "chrome.exe",
        kind: BrowserKind::Chromium,
        rel: "Google\\Chrome\\User Data",
    },
    Browser {
        name: "edge",
        exe: "msedge.exe",
        kind: BrowserKind::Chromium,
        rel: "Microsoft\\Edge\\User Data",
    },
    Browser {
        name: "brave",
        exe: "brave.exe",
        kind: BrowserKind::Chromium,
        rel: "BraveSoftware\\Brave-Browser\\User Data",
    },
    Browser {
        name: "vivaldi",
        exe: "vivaldi.exe",
        kind: BrowserKind::Chromium,
        rel: "Vivaldi\\User Data",
    },
    Browser {
        name: "firefox",
        exe: "firefox.exe",
        kind: BrowserKind::Firefox,
        rel: "Mozilla\\Firefox\\Profiles",
    },
];

/// Disk-cache folders inside a Chromium profile.
pub const CHROMIUM_CACHES: &[&str] = &["Cache", "Code Cache", "GPUCache"];
/// Disk-cache folder inside a Firefox profile (under `%LOCALAPPDATA%`).
pub const FIREFOX_CACHE: &str = "cache2";

/// A Chromium profile folder: "Default" or "Profile N".
pub fn is_chromium_profile(name: &str) -> bool {
    name == "Default"
        || name
            .strip_prefix("Profile ")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    Local,
    Roaming,
    Profile,
}

#[derive(Clone, Copy, Debug)]
pub struct AppCache {
    pub label: &'static str,
    pub base: Base,
    pub rel: &'static str,
    /// Skipped while this process runs (Electron apps hold their caches open).
    pub exe: Option<&'static str>,
}

const fn app(
    label: &'static str,
    base: Base,
    rel: &'static str,
    exe: Option<&'static str>,
) -> AppCache {
    AppCache {
        label,
        base,
        rel,
        exe,
    }
}

/// Path-based only: nothing here spawns a tool.
pub const APP_CACHES: &[AppCache] = &[
    app(
        "vs code · Cache",
        Base::Roaming,
        "Code\\Cache",
        Some("code.exe"),
    ),
    app(
        "vs code · CachedData",
        Base::Roaming,
        "Code\\CachedData",
        Some("code.exe"),
    ),
    app(
        "vs code · Code Cache",
        Base::Roaming,
        "Code\\Code Cache",
        Some("code.exe"),
    ),
    app(
        "vs code · GPUCache",
        Base::Roaming,
        "Code\\GPUCache",
        Some("code.exe"),
    ),
    app(
        "discord · Cache",
        Base::Roaming,
        "discord\\Cache",
        Some("discord.exe"),
    ),
    app(
        "discord · Code Cache",
        Base::Roaming,
        "discord\\Code Cache",
        Some("discord.exe"),
    ),
    app(
        "discord · GPUCache",
        Base::Roaming,
        "discord\\GPUCache",
        Some("discord.exe"),
    ),
    app(
        "slack · Cache",
        Base::Roaming,
        "Slack\\Cache",
        Some("slack.exe"),
    ),
    app(
        "slack · Code Cache",
        Base::Roaming,
        "Slack\\Code Cache",
        Some("slack.exe"),
    ),
    app(
        "slack · GPUCache",
        Base::Roaming,
        "Slack\\GPUCache",
        Some("slack.exe"),
    ),
    app("npm cache", Base::Local, "npm-cache\\_cacache", None),
    app("pip cache", Base::Local, "pip\\cache", None),
    app(
        "cargo registry cache",
        Base::Profile,
        ".cargo\\registry\\cache",
        None,
    ),
    app("yarn cache", Base::Local, "Yarn\\Cache", None),
    app("go build cache", Base::Local, "go-build", None),
];

// ---------------------------------------------------------------- dev purge

/// Folder names that may be build output. Each needs a context marker.
pub const PURGE_NAMES: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".venv",
    "venv",
    "__pycache__",
    "bin",
    "obj",
];

pub fn is_purge_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    PURGE_NAMES.contains(&n.as_str())
}

/// What the project folder (the candidate's parent) contains.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProjectFacts {
    pub cargo_toml: bool,
    pub pom_xml: bool,
    pub csproj: bool,
    pub package_json: bool,
    /// `package.json` has a non-empty `scripts.build`.
    pub package_build_script: bool,
    pub cmake: bool,
    pub gradle: bool,
    /// `pyproject.toml` has a `[build-system]` table.
    pub pyproject_build_system: bool,
    /// Any `*.py` file.
    pub py_files: bool,
}

/// What the candidate folder itself contains.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CandidateFacts {
    /// `pyvenv.cfg`, which every virtual environment has.
    pub pyvenv_cfg: bool,
}

/// The context rule: is `name` build output given what its project contains?
pub fn purge_context_ok(name: &str, p: &ProjectFacts, c: &CandidateFacts) -> bool {
    match name.to_ascii_lowercase().as_str() {
        "node_modules" | ".next" => p.package_json,
        "target" => p.cargo_toml || p.pom_xml,
        "bin" | "obj" => p.csproj,
        "build" | "dist" => {
            p.package_build_script || p.cmake || p.gradle || p.pyproject_build_system
        }
        ".venv" | "venv" => c.pyvenv_cfg,
        "__pycache__" => p.py_files,
        _ => false,
    }
}

/// True if a `package.json` text has a non-empty `scripts.build`.
pub fn package_json_has_build(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| {
            v.get("scripts")?
                .get("build")?
                .as_str()
                .map(|s| !s.trim().is_empty())
        })
        .unwrap_or(false)
}

/// True if a `pyproject.toml` text has a `[build-system]` table.
pub fn pyproject_has_build_system(text: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim();
        l == "[build-system]" || l.starts_with("[build-system]")
    })
}

// ---------------------------------------------------------------- sizes

pub fn total(items: &[Item]) -> u64 {
    items.iter().map(|i| i.bytes).fold(0, u64::saturating_add)
}

/// Largest first, ties by path.
pub fn sort_items(items: &mut [Item]) {
    items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
}

/// Keeps the `n` largest of a stream of files.
#[derive(Debug)]
pub struct TopN {
    n: usize,
    heap: BinaryHeap<Reverse<(u64, PathBuf)>>,
}

impl TopN {
    pub fn new(n: usize) -> Self {
        Self {
            n,
            heap: BinaryHeap::with_capacity(n + 1),
        }
    }

    /// The size a file must beat to get in, once the list is full.
    pub fn floor(&self) -> Option<u64> {
        (self.heap.len() >= self.n)
            .then(|| self.heap.peek().map(|r| r.0 .0))
            .flatten()
    }

    pub fn push(&mut self, bytes: u64, path: PathBuf) {
        if self.n == 0 {
            return;
        }
        if self.heap.len() < self.n {
            self.heap.push(Reverse((bytes, path)));
        } else if self.floor().is_some_and(|f| bytes > f) {
            self.heap.pop();
            self.heap.push(Reverse((bytes, path)));
        }
    }

    /// Largest first.
    pub fn into_sorted(self) -> Vec<(u64, PathBuf)> {
        let mut v: Vec<(u64, PathBuf)> = self.heap.into_iter().map(|r| r.0).collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        v
    }
}

// ---------------------------------------------------------------- log

impl Action {
    pub fn word(self) -> &'static str {
        match self {
            Action::Removed => "REMOVED",
            Action::Recycled => "RECYCLED",
            Action::Failed => "FAILED",
        }
    }
}

/// `2026-10-26T09:05:03Z` (UTC).
pub fn utc_stamp(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (H. Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn one_line(s: &str) -> String {
    s.replace(['\r', '\n'], " ")
}

/// `[time] [sweep run=<id>] REMOVED|RECYCLED|FAILED <path> (detail)`. The detail part
/// is left out when empty.
pub fn log_line(time: SystemTime, run: &str, action: Action, path: &Path, detail: &str) -> String {
    let path = one_line(&plain_path(path).to_string_lossy());
    let mut line = format!(
        "[{}] [sweep run={}] {} {}",
        utc_stamp(time),
        one_line(run),
        action.word(),
        path
    );
    if !detail.is_empty() {
        line.push_str(&format!(" ({})", one_line(detail)));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> Guard {
        let profile = Path::new(r"C:\Users\ann");
        Guard::new(
            profile,
            &profile.join(r"AppData\Local"),
            &profile.join(r"AppData\Roaming"),
            &profile.join(r"AppData\Local\Temp"),
            &[
                PathBuf::from(r"C:\Windows"),
                PathBuf::from(r"C:\Program Files"),
                PathBuf::from(r"C:\Program Files (x86)"),
                PathBuf::from(r"C:\ProgramData"),
            ],
            &[PathBuf::from(r"C:\Users\ann\Projects")],
        )
    }

    #[test]
    fn guard_protects_system_and_secrets() {
        let g = guard();
        for p in [
            r"C:\",
            r"C:\Users",
            r"C:\Users\ann",
            r"c:\users\ANN\",
            r"\\?\C:\Users\ann",
            r"C:\Windows\Temp\x.tmp",
            r"C:\Program Files\App\cache",
            r"C:\Program Files (x86)",
            r"C:\ProgramData\Microsoft",
            r"C:\Users\ann\.ssh\id_ed25519",
            r"C:\Users\ann\.aws",
            r"C:\Users\ann\.kube\config",
            r"C:\Users\ann\.docker\config.json",
            r"C:\Users\ann\.gnupg",
            r"C:\Users\ann\.azure",
            r"C:\Users\ann\.daijobu\operations.log",
            r"C:\Users\ann\AppData",
            r"C:\Users\ann\AppData\Local",
            r"C:\Users\ann\AppData\Local\Temp",
            r"C:\Users\ann\AppData\Local\Packages\Foo\LocalCache",
            r"C:\Users\ann\AppData\Roaming\Microsoft\Credentials\abc",
            r"C:\Users\ann\AppData\Local\Microsoft\Vault",
            r"C:\Users\ann\AppData\Roaming\Microsoft\Crypto\RSA",
            r"C:\Users\ann\AppData\Roaming\Microsoft",
            r"C:\Users\ann\AppData\Local\Google\Chrome\User Data\Default\Login Data",
            r"C:\Users\ann\AppData\Local\Google\Chrome\User Data\Default\Login Data For Account",
            r"C:\Users\ann\AppData\Local\Google\Chrome\User Data\Default\Network\Cookies",
            r"C:\Users\ann\AppData\Local\Google\Chrome\User Data\Default\Local Storage\leveldb",
            r"C:\Users\ann\AppData\Local\Google\Chrome\User Data\Default\IndexedDB",
            r"C:\Users\ann\AppData\Local\Google\Chrome\User Data\Default\Sessions",
            r"C:\Users\ann\Projects",
            r"C:\stuff",
            r"C:\Users\ann\NTUSER.DAT",
            r"C:\Users\ann\ntuser.dat.LOG1",
            r"relative\path\here",
            "",
        ] {
            assert!(g.is_protected(Path::new(p)), "{p} must be protected");
        }
    }

    #[test]
    fn guard_allows_caches_and_projects() {
        let g = guard();
        for p in [
            r"C:\Users\ann\AppData\Local\Temp\old.tmp",
            r"C:\Users\ann\AppData\Local\Google\Chrome\User Data\Default\Code Cache",
            r"C:\Users\ann\AppData\Roaming\Code\Cache",
            r"C:\Users\ann\Projects\app\node_modules",
            r"C:\Users\ann\Videos\big.mkv",
            r"C:\Users\ann\.sshkeys-not-really\x",
            r"D:\games\thing",
        ] {
            assert!(!g.is_protected(Path::new(p)), "{p} must be allowed");
        }
    }

    #[test]
    fn under_compares_whole_components() {
        assert!(is_under(Path::new(r"C:\a\b"), Path::new(r"c:\A")));
        assert!(is_under(Path::new(r"C:\a"), Path::new(r"C:\a\")));
        assert!(!is_under(Path::new(r"C:\ab"), Path::new(r"C:\a")));
        assert!(is_under(Path::new(r"\\?\C:\a\b"), Path::new(r"C:\a")));
        assert!(same_path(Path::new(r"\\?\C:\X\y"), Path::new(r"c:/x/Y/")));
        assert_eq!(
            plain_path(Path::new(r"\\?\UNC\srv\s\f")),
            PathBuf::from(r"\\srv\s\f")
        );
        assert_eq!(depth(r"\\srv\share\a\b"), 2);
        assert_eq!(depth(r"c:\a\b"), 2);
    }

    #[test]
    fn age_filter() {
        let now = UNIX_EPOCH + Duration::from_secs(100 * 86_400);
        let day = Duration::from_secs(86_400);
        assert!(is_older_than(now - day * 7, now, 7));
        assert!(is_older_than(now - day * 30, now, 7));
        assert!(!is_older_than(
            now - day * 7 + Duration::from_secs(1),
            now,
            7
        ));
        assert!(!is_older_than(now, now, 7));
        assert!(
            !is_older_than(now + day, now, 7),
            "future times are never old"
        );
        assert!(is_older_than(now, now, 0));
    }

    #[test]
    fn purge_context_each_marker() {
        let none = ProjectFacts::default();
        let c0 = CandidateFacts::default();
        for n in PURGE_NAMES {
            assert!(!purge_context_ok(n, &none, &c0), "{n} needs a marker");
        }
        let f = |set: fn(&mut ProjectFacts)| {
            let mut p = ProjectFacts::default();
            set(&mut p);
            p
        };
        let pkg = f(|p| p.package_json = true);
        assert!(purge_context_ok("node_modules", &pkg, &c0));
        assert!(purge_context_ok(".next", &pkg, &c0));
        assert!(
            !purge_context_ok("dist", &pkg, &c0),
            "package.json alone is no build signal"
        );
        assert!(!purge_context_ok("target", &pkg, &c0));

        assert!(purge_context_ok("target", &f(|p| p.cargo_toml = true), &c0));
        assert!(purge_context_ok("target", &f(|p| p.pom_xml = true), &c0));
        assert!(!purge_context_ok("target", &f(|p| p.csproj = true), &c0));

        let cs = f(|p| p.csproj = true);
        assert!(purge_context_ok("bin", &cs, &c0));
        assert!(purge_context_ok("obj", &cs, &c0));
        assert!(!purge_context_ok("bin", &f(|p| p.cargo_toml = true), &c0));

        for set in [
            (|p: &mut ProjectFacts| p.package_build_script = true) as fn(&mut ProjectFacts),
            |p| p.cmake = true,
            |p| p.gradle = true,
            |p| p.pyproject_build_system = true,
        ] {
            let p = f(set);
            assert!(purge_context_ok("build", &p, &c0));
            assert!(purge_context_ok("dist", &p, &c0));
        }

        let venv = CandidateFacts { pyvenv_cfg: true };
        assert!(purge_context_ok(".venv", &none, &venv));
        assert!(purge_context_ok("venv", &none, &venv));
        assert!(purge_context_ok(
            "__pycache__",
            &f(|p| p.py_files = true),
            &c0
        ));
        assert!(!purge_context_ok("src", &f(|p| p.cargo_toml = true), &c0));
        assert!(purge_context_ok("Target", &f(|p| p.cargo_toml = true), &c0));
    }

    #[test]
    fn marker_file_parsing() {
        assert!(package_json_has_build(
            r#"{"scripts":{"build":"vite build"}}"#
        ));
        assert!(!package_json_has_build(r#"{"scripts":{"test":"jest"}}"#));
        assert!(!package_json_has_build(r#"{"scripts":{"build":"  "}}"#));
        assert!(!package_json_has_build("not json"));
        assert!(pyproject_has_build_system(
            "[project]\nname='x'\n\n[build-system]\nrequires=[]"
        ));
        assert!(!pyproject_has_build_system(
            "[project]\nname='x'\n# [build-system]"
        ));
        assert!(is_chromium_profile("Default"));
        assert!(is_chromium_profile("Profile 12"));
        assert!(!is_chromium_profile("Profile x"));
        assert!(!is_chromium_profile("System Profile"));
        assert!(is_purge_name("node_modules") && is_purge_name("OBJ") && !is_purge_name("src"));
    }

    #[test]
    fn size_aggregation_and_top_n() {
        let it = |p: &str, b| Item {
            path: PathBuf::from(p),
            bytes: b,
            label: p.into(),
        };
        let mut v = vec![it("a", 5), it("b", 50), it("c", 5), it("d", u64::MAX)];
        assert_eq!(total(&v), u64::MAX, "saturates");
        sort_items(&mut v);
        let order: Vec<_> = v.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(order, ["d", "b", "a", "c"]);
        assert_eq!(total(&[]), 0);

        let mut t = TopN::new(3);
        for (i, b) in [10u64, 1, 30, 20, 5, 40, 30].iter().enumerate() {
            t.push(*b, PathBuf::from(format!("f{i}")));
        }
        assert_eq!(t.floor(), Some(30));
        let got: Vec<u64> = t.into_sorted().iter().map(|x| x.0).collect();
        assert_eq!(got, [40, 30, 30]);
        let mut z = TopN::new(0);
        z.push(1, PathBuf::from("x"));
        assert!(z.into_sorted().is_empty());
    }

    #[test]
    fn oplog_format() {
        let t = UNIX_EPOCH + Duration::from_secs(1_793_005_503); // 2026-10-26T09:05:03Z
        assert_eq!(utc_stamp(t), "2026-10-26T09:05:03Z");
        assert_eq!(utc_stamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            log_line(
                t,
                "ab12",
                Action::Removed,
                Path::new(r"\\?\C:\t\x.tmp"),
                "temp, 4.0 KB"
            ),
            r"[2026-10-26T09:05:03Z] [sweep run=ab12] REMOVED C:\t\x.tmp (temp, 4.0 KB)"
        );
        assert_eq!(
            log_line(
                t,
                "ab12",
                Action::Recycled,
                Path::new(r"C:\p\node_modules"),
                ""
            ),
            r"[2026-10-26T09:05:03Z] [sweep run=ab12] RECYCLED C:\p\node_modules"
        );
        assert_eq!(
            log_line(
                t,
                "r",
                Action::Failed,
                Path::new("C:\\a\nb"),
                "access\r\ndenied"
            ),
            "[2026-10-26T09:05:03Z] [sweep run=r] FAILED C:\\a b (access  denied)"
        );
    }
}
