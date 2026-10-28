//! Which program to start, and where.
//!
//! `[terminal] shell` is a word ("pwsh", "cmd", ...), a path, or empty. Starting a ConPTY with
//! a program that does not exist would leave a half-built pseudo console behind (the emulator
//! crate only cleans up after a successful start), so [`locate_program`] checks first.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// "" → `pwsh.exe` if it is on PATH, else Windows PowerShell (System32); "pwsh", "powershell",
/// "cmd" and "wsl" → that program; anything else is a path or program name used as given.
/// `args` pass through. PowerShell started by "", "pwsh" or "powershell" without any args gets
/// `-NoLogo`, so the panel opens on a prompt and not on a banner.
pub fn resolve_shell(shell: &str, args: &[String]) -> (String, Vec<String>) {
    let pwsh_on_path = find_on_path("pwsh.exe").is_some();
    resolve_with(shell, args, pwsh_on_path, &windows_powershell())
}

pub(crate) fn resolve_with(
    shell: &str,
    args: &[String],
    pwsh_on_path: bool,
    windows_powershell: &str,
) -> (String, Vec<String>) {
    let shell = shell.trim();
    let powershell = |program: &str| {
        let args = if args.is_empty() {
            vec!["-NoLogo".to_string()]
        } else {
            args.to_vec()
        };
        (program.to_string(), args)
    };
    let plain = |program: &str| (program.to_string(), args.to_vec());
    match shell.to_ascii_lowercase().as_str() {
        "" if pwsh_on_path => powershell("pwsh.exe"),
        "" | "powershell" => powershell(windows_powershell),
        "pwsh" => powershell("pwsh.exe"),
        "cmd" => plain("cmd.exe"),
        "wsl" => plain("wsl.exe"),
        _ => plain(shell),
    }
}

/// Whether `program` is Windows PowerShell or PowerShell 7: the file stem is `powershell` or
/// `pwsh`, in any case, with or without `.exe`, at any path.
pub fn is_powershell(program: &str) -> bool {
    let program = program.trim().trim_matches('"');
    let name = program.rsplit(['\\', '/']).next().unwrap_or(program);
    let stem = Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    stem == "powershell" || stem == "pwsh"
}

fn system_root() -> PathBuf {
    std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("windir"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

/// Windows PowerShell 5.1 by its full System32 path, or the bare name if that is missing.
fn windows_powershell() -> String {
    let full = system_root().join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    if is_file(&full) {
        full.to_string_lossy().into_owned()
    } else {
        "powershell.exe".to_string()
    }
}

/// Exists and is not a directory. `symlink_metadata` so a Store app alias (a reparse point
/// that cannot be followed) still counts.
fn is_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| !m.is_dir())
        .unwrap_or(false)
}

fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default()
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    path_dirs()
        .into_iter()
        .map(|d| d.join(name))
        .find(|p| is_file(p))
}

/// The file names `CreateProcess` would try for `name`: as given when it has an extension,
/// else with `.exe` appended.
fn candidates(name: &str) -> Vec<OsString> {
    if Path::new(name).extension().is_some() {
        vec![OsString::from(name)]
    } else {
        vec![OsString::from(format!("{name}.exe"))]
    }
}

/// Where `program` is, searched like `CreateProcess` does: a path as given, else the
/// island's directory, the current directory, System32, the Windows directory, then PATH.
/// `None` when it is nowhere.
pub(crate) fn locate_program(program: &str) -> Option<PathBuf> {
    let root = system_root();
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(PathBuf::from))
    {
        dirs.push(dir);
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd);
    }
    dirs.push(root.join("System32"));
    dirs.push(root);
    dirs.extend(path_dirs());
    locate_in(program, &dirs)
}

pub(crate) fn locate_in(program: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let program = program.trim().trim_matches('"');
    if program.is_empty() {
        return None;
    }
    if program.contains(['\\', '/', ':']) {
        return candidates(program)
            .into_iter()
            .map(PathBuf::from)
            .find(|p| is_file(p));
    }
    for dir in dirs {
        for name in candidates(program) {
            let p = dir.join(name);
            if is_file(&p) {
                return Some(p);
            }
        }
    }
    None
}

/// The program as the first word of a command line: quoted when it has a space.
pub(crate) fn quote_program(program: &str) -> String {
    if program.contains([' ', '\t']) && !program.starts_with('"') {
        format!("\"{program}\"")
    } else {
        program.to_string()
    }
}

/// The directory to start in: `cwd` when it exists, else the user's home, else none.
pub(crate) fn pick_cwd(cwd: Option<&Path>) -> Option<PathBuf> {
    let home = || std::env::var_os("USERPROFILE").map(PathBuf::from);
    cwd.map(Path::to_path_buf)
        .filter(|p| p.is_dir())
        .or_else(|| home().filter(|p| p.is_dir()))
}
