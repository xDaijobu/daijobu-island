//! Spotlight's index and launching: the apps in `shell:AppsFolder` (Start Menu desktop
//! apps and Store apps), a name scan of the user's folders, launching, revealing and the
//! shell icons for both.
//!
//! COM: [`list_apps`], [`app_icon`] and [`file_icon`] initialise COM (apartment-threaded)
//! on the calling thread when it has not been initialised yet, and undo it on return, so
//! they may be called from any background thread. [`scan_files`] and [`user_folders`]
//! need no COM setup.

use std::collections::{HashSet, VecDeque};
use std::os::windows::fs::MetadataExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{SIZE, S_FALSE, S_OK};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetDIBits, GetObjectW,
    ReleaseDC, SelectObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
    HGDIOBJ,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SYSTEM,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_AppsFolder, FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads,
    FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Videos, IEnumShellItems, IShellItem,
    IShellItemImageFactory, SHCreateItemFromParsingName, SHGetFileInfoW, SHGetKnownFolderItem,
    SHGetKnownFolderPath, SHObjectProperties, ShellExecuteW, KF_FLAG_DEFAULT, SHFILEINFOW,
    SHGFI_ICON, SHGFI_LARGEICON, SHGFI_USEFILEATTRIBUTES, SHOP_FILEPATH, SIGDN_NORMALDISPLAY,
    SIGDN_PARENTRELATIVEPARSING, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, DrawIconEx, DI_NORMAL, SW_SHOWNORMAL};

/// One launchable app from `shell:AppsFolder`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppEntry {
    /// The display name ("Calculator").
    pub name: String,
    /// The relative parsing name: an AUMID for Store apps
    /// (`Microsoft.WindowsCalculator_8wekyb3d8bbwe!App`), a path or GUID-based path for
    /// desktop apps. Launch it with [`launch_app`].
    pub parsing_name: String,
}

/// A file or folder found by [`scan_files`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHit {
    pub path: PathBuf,
    pub is_dir: bool,
}

/// A shell icon: premultiplied BGRA, top-down, `w * h * 4` bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconBitmap {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
}

/// COM initialised on this thread for the guard's lifetime (a no-op when the thread
/// already has COM in another mode).
struct Com(bool);

impl Com {
    fn init() -> Com {
        // SAFETY: plain COM init; paired with `CoUninitialize` in `drop` only when this
        // call succeeded (S_OK or S_FALSE both count a reference).
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Com(hr == S_OK || hr == S_FALSE)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful `CoInitializeEx` in `init`.
            unsafe { CoUninitialize() };
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Reads and frees a shell-allocated string.
fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: `p` is a NUL-terminated string from the shell, allocated with the COM task
    // allocator; it is copied out and freed exactly once.
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Every app in `shell:AppsFolder`, deduped by parsing name and sorted by name
/// (case-insensitive). Empty when the shell can't be enumerated.
pub fn list_apps() -> Vec<AppEntry> {
    let _com = Com::init();
    let mut out = Vec::new();
    // SAFETY: COM calls on interfaces owned by this function; every string is freed by
    // `take_pwstr`; the enumerator's out-slice and count are valid for each call.
    unsafe {
        let Ok(folder) =
            SHGetKnownFolderItem::<IShellItem>(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)
        else {
            return out;
        };
        let Ok(en) = folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) else {
            return out;
        };
        loop {
            let mut item = [None::<IShellItem>];
            let mut got = 0u32;
            if en.Next(&mut item, Some(&mut got)).is_err() || got == 0 {
                break;
            }
            let Some(item) = item[0].take() else { break };
            let name = item
                .GetDisplayName(SIGDN_NORMALDISPLAY)
                .map(take_pwstr)
                .unwrap_or_default();
            let parsing = item
                .GetDisplayName(SIGDN_PARENTRELATIVEPARSING)
                .map(take_pwstr)
                .unwrap_or_default();
            if name.trim().is_empty() || parsing.is_empty() {
                continue;
            }
            out.push(AppEntry {
                name,
                parsing_name: parsing,
            });
        }
    }
    let mut seen = HashSet::new();
    out.retain(|a| seen.insert(a.parsing_name.clone()));
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// The existing folders among Desktop, Documents, Downloads, Pictures, Videos and Music.
pub fn user_folders() -> Vec<PathBuf> {
    let ids = [
        &FOLDERID_Desktop,
        &FOLDERID_Documents,
        &FOLDERID_Downloads,
        &FOLDERID_Pictures,
        &FOLDERID_Videos,
        &FOLDERID_Music,
    ];
    let mut out: Vec<PathBuf> = Vec::new();
    for id in ids {
        // SAFETY: the returned string is shell-allocated and freed by `take_pwstr`.
        let p = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) };
        if let Ok(p) = p {
            let path = PathBuf::from(take_pwstr(p));
            if path.is_dir() && !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

/// Directory names never entered by [`scan_files`] (compared case-insensitively).
const SKIP_DIRS: [&str; 6] = [
    "node_modules",
    ".git",
    "target",
    "$recycle.bin",
    "appdata",
    ".cache",
];

/// Walks `roots` breadth-first and lists files and folders (not the roots themselves) up
/// to `max_depth` levels below a root (1 = the root's direct children), at most
/// `max_entries` hits, stopping early when `cancel` is set. Hidden and system entries,
/// reparse points (junctions and symlinks are not followed) and [`SKIP_DIRS`] are
/// skipped; unreadable folders are skipped silently.
pub fn scan_files(
    roots: &[PathBuf],
    max_depth: usize,
    max_entries: usize,
    cancel: &AtomicBool,
) -> Vec<FileHit> {
    let mut hits = Vec::new();
    let mut queue: VecDeque<(PathBuf, usize)> = roots.iter().cloned().map(|r| (r, 0)).collect();
    while let Some((dir, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            if cancel.load(Ordering::Relaxed) || hits.len() >= max_entries {
                return hits;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let attrs = meta.file_attributes();
            if attrs
                & (FILE_ATTRIBUTE_HIDDEN.0
                    | FILE_ATTRIBUTE_SYSTEM.0
                    | FILE_ATTRIBUTE_REPARSE_POINT.0)
                != 0
            {
                continue;
            }
            let is_dir = attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
            let name = entry.file_name();
            if is_dir {
                let lower = name.to_string_lossy().to_lowercase();
                if SKIP_DIRS.contains(&lower.as_str()) {
                    continue;
                }
            }
            let path = entry.path();
            if is_dir {
                queue.push_back((path.clone(), depth + 1));
            }
            hits.push(FileHit { path, is_dir });
        }
    }
    hits
}

fn shell_open(target: &str) -> bool {
    shell_verb(w!("open"), target)
}

fn shell_verb(verb: PCWSTR, target: &str) -> bool {
    let t = wide(target);
    // SAFETY: `t` is NUL-terminated and outlives the call; `verb` is a static string.
    let r = unsafe {
        ShellExecuteW(
            None,
            verb,
            PCWSTR(t.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    r.0 as isize > 32
}

/// Runs an app (by its parsing name, `app`) or a program file elevated: Windows shows
/// the UAC prompt. False when refused or cancelled.
pub fn run_as_admin(target: &str, app: bool) -> bool {
    let t = if app {
        format!("shell:AppsFolder\\{target}")
    } else {
        target.to_string()
    };
    shell_verb(w!("runas"), &t)
}

/// Opens the file's or folder's Properties dialog (it runs on its own thread; this
/// returns at once).
pub fn properties(path: &Path) -> bool {
    let p = wide(&path.to_string_lossy());
    // SAFETY: `p` is NUL-terminated and outlives the call; no owner window.
    unsafe { SHObjectProperties(None, SHOP_FILEPATH, PCWSTR(p.as_ptr()), PCWSTR::null()) }.as_bool()
}

/// Windows' "Open with" chooser for a file, in its own process (it is modal, and must not
/// block the island).
pub fn open_with(path: &Path) {
    let _ = std::process::Command::new("rundll32.exe")
        .raw_arg(format!("shell32.dll,OpenAs_RunDLL {}", path.display()))
        .spawn();
}

/// Launches an app by its [`AppEntry::parsing_name`]. True when the shell accepted it.
pub fn launch_app(parsing_name: &str) -> bool {
    shell_open(&format!("shell:AppsFolder\\{parsing_name}"))
}

/// Opens a file or folder with its default handler. True when the shell accepted it.
pub fn open_path(path: &Path) -> bool {
    shell_open(&path.to_string_lossy())
}

/// Opens Explorer with `path` selected.
pub fn reveal(path: &Path) {
    let _ = std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn();
}

// ---- icons --------------------------------------------------------------------------

/// Copies an `HBITMAP` (not freed here) into top-down 32-bit BGRA. An all-zero alpha
/// channel (an icon without alpha) becomes opaque.
fn hbitmap_to_bgra(hbm: HBITMAP) -> Option<IconBitmap> {
    let mut bm = BITMAP::default();
    // SAFETY: `bm` is a valid out-buffer of the size passed; the DC from `GetDC(None)`
    // is released; the pixel buffer holds exactly `w * h * 4` bytes, as the header says.
    unsafe {
        if GetObjectW(
            HGDIOBJ(hbm.0),
            size_of::<BITMAP>() as i32,
            Some((&mut bm as *mut BITMAP).cast()),
        ) == 0
        {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
        if w <= 0 || h <= 0 {
            return None;
        }
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let lines = GetDIBits(
            dc,
            hbm,
            0,
            h as u32,
            Some(buf.as_mut_ptr().cast()),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, dc);
        if lines == 0 {
            return None;
        }
        make_opaque_if_no_alpha(&mut buf);
        Some(IconBitmap {
            w: w as u32,
            h: h as u32,
            bgra: buf,
        })
    }
}

fn make_opaque_if_no_alpha(buf: &mut [u8]) {
    if buf.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        for p in buf.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
    }
}

/// The shell's icon for the item at `parsing` (a path or `shell:` name) at `size` px.
fn item_icon(parsing: &str, size: u32) -> Option<IconBitmap> {
    let name = wide(parsing);
    // SAFETY: `name` is NUL-terminated and outlives the call; the bitmap from `GetImage`
    // is ours to delete once copied.
    unsafe {
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(PCWSTR(name.as_ptr()), None).ok()?;
        let hbm = factory
            .GetImage(
                SIZE {
                    cx: size as i32,
                    cy: size as i32,
                },
                SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK,
            )
            .ok()?;
        let icon = hbitmap_to_bgra(hbm);
        let _ = DeleteObject(HGDIOBJ(hbm.0));
        icon
    }
}

/// An app's icon at about `size` px (the shell may return a larger one).
pub fn app_icon(parsing_name: &str, size: u32) -> Option<IconBitmap> {
    let _com = Com::init();
    item_icon(&format!("shell:AppsFolder\\{parsing_name}"), size)
}

/// A file type's or file's icon. `path_or_ext` is a real path (its own icon), an
/// extension like `".pdf"`, or `"<folder>"` / `"<file>"` for the generic ones.
pub fn file_icon(path_or_ext: &str, size: u32) -> Option<IconBitmap> {
    let _com = Com::init();
    if Path::new(path_or_ext).exists() {
        if let Some(i) = item_icon(path_or_ext, size) {
            return Some(i);
        }
    }
    let (probe, attrs) = match path_or_ext {
        "<folder>" => ("folder".to_string(), FILE_ATTRIBUTE_DIRECTORY),
        "<file>" => ("file".to_string(), FILE_ATTRIBUTE_NORMAL),
        ext => (
            if ext.starts_with('.') {
                format!("file{ext}")
            } else {
                ext.to_string()
            },
            FILE_ATTRIBUTE_NORMAL,
        ),
    };
    let name = wide(&probe);
    let mut info = SHFILEINFOW::default();
    // SAFETY: `name` is NUL-terminated; `info` is a valid out-struct of the size passed.
    // With `USEFILEATTRIBUTES` the file need not exist. The icon, the DIB and the DCs
    // created below are all freed before returning.
    unsafe {
        let r = SHGetFileInfoW(
            PCWSTR(name.as_ptr()),
            attrs,
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON | SHGFI_USEFILEATTRIBUTES,
        );
        if r == 0 || info.hIcon.is_invalid() {
            return None;
        }
        let n = size as i32;
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: n,
                biHeight: -n,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = CreateCompatibleDC(None);
        let mut bits = std::ptr::null_mut();
        let dib = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
        let result = match dib {
            Ok(dib) if !bits.is_null() => {
                let old = SelectObject(dc, HGDIOBJ(dib.0));
                let _ = DrawIconEx(dc, 0, 0, info.hIcon, n, n, 0, None, DI_NORMAL);
                let mut buf = vec![0u8; (n * n * 4) as usize];
                // The section is `n * n * 4` bytes and is zeroed before the draw.
                std::ptr::copy_nonoverlapping(bits as *const u8, buf.as_mut_ptr(), buf.len());
                make_opaque_if_no_alpha(&mut buf);
                SelectObject(dc, old);
                let _ = DeleteObject(HGDIOBJ(dib.0));
                Some(IconBitmap {
                    w: size,
                    h: size,
                    bgra: buf,
                })
            }
            _ => None,
        };
        let _ = DeleteDC(dc);
        let _ = DestroyIcon(info.hIcon);
        result
    }
}

// ---- sizes --------------------------------------------------------------------------

/// The installed programs with an install size on record: `(display name, bytes)` from
/// the Uninstall keys (machine 64- and 32-bit, and the user), the numbers Windows'
/// "Installed apps" shows. Entries without `EstimatedSize` are left out.
pub fn installed_sizes() -> Vec<(String, u64)> {
    const UNINSTALL: PCWSTR = w!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall");
    const UNINSTALL_32: PCWSTR =
        w!(r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall");
    let mut out = Vec::new();
    for (root, path) in [
        (HKEY_LOCAL_MACHINE, UNINSTALL),
        (HKEY_LOCAL_MACHINE, UNINSTALL_32),
        (HKEY_CURRENT_USER, UNINSTALL),
    ] {
        read_uninstall(root, path, &mut out);
    }
    out
}

fn read_uninstall(root: HKEY, path: PCWSTR, out: &mut Vec<(String, u64)>) {
    let mut key = HKEY::default();
    // SAFETY: a read-only open of a well-known key with a valid out-pointer; the handle
    // is closed below.
    if unsafe { RegOpenKeyExW(root, path, None, KEY_READ, &mut key) }.is_err() {
        return;
    }
    let mut i = 0u32;
    loop {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        // SAFETY: `name` and `len` describe a valid buffer; the key handle is open.
        let r = unsafe {
            RegEnumKeyExW(
                key,
                i,
                Some(PWSTR(name.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if r.is_err() {
            break;
        }
        i += 1;
        let sub = PCWSTR(name.as_ptr());
        let (Some(display), Some(kb)) = (
            reg_string(key, sub, w!("DisplayName")),
            reg_dword(key, sub, w!("EstimatedSize")),
        ) else {
            continue;
        };
        if !display.trim().is_empty() && kb > 0 {
            out.push((display.trim().to_string(), kb as u64 * 1024));
        }
    }
    // SAFETY: the handle opened above.
    unsafe {
        let _ = RegCloseKey(key);
    }
}

fn reg_string(key: HKEY, sub: PCWSTR, value: PCWSTR) -> Option<String> {
    let mut buf = [0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: `buf`/`size` describe a valid buffer in bytes; the key handle is open and
    // `sub` / `value` are NUL-terminated.
    let r = unsafe {
        RegGetValueW(
            key,
            sub,
            value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if r.is_err() {
        return None;
    }
    let chars = (size as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..chars]))
}

fn reg_dword(key: HKEY, sub: PCWSTR, value: PCWSTR) -> Option<u32> {
    let mut v = 0u32;
    let mut size = 4u32;
    // SAFETY: `v` is a 4-byte out buffer; the key handle is open.
    let r = unsafe {
        RegGetValueW(
            key,
            sub,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut v as *mut u32).cast()),
            Some(&mut size),
        )
    };
    r.is_ok().then_some(v)
}

/// A folder's total size and file count, summed recursively. Stops early when
/// `max_files` files are counted, `deadline` passes or `cancel` is set, and then says
/// so (`capped`: both are lower bounds). Junctions and symlinks are not followed;
/// unreadable folders are skipped.
pub fn folder_size(
    root: &Path,
    max_files: u64,
    deadline: std::time::Instant,
    cancel: &AtomicBool,
) -> (u64, u64, bool) {
    const REPARSE: u32 = 0x400;
    let (mut bytes, mut files) = (0u64, 0u64);
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            if files >= max_files
                || cancel.load(Ordering::Relaxed)
                || std::time::Instant::now() >= deadline
            {
                return (bytes, files, true);
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.file_attributes() & REPARSE != 0 {
                continue;
            }
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                bytes += meta.len();
                files += 1;
            }
        }
    }
    (bytes, files, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_tree(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("dj-launcher-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn rel(root: &Path, hits: &[FileHit]) -> Vec<String> {
        let mut v: Vec<String> = hits
            .iter()
            .map(|h| {
                h.path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn scan_filters_depth_limits_and_cancel() {
        let root = temp_tree("scan");
        for d in [
            "a/b/c",
            "node_modules/x",
            ".git/y",
            "target/z",
            "AppData/q",
            "hid",
        ] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in [
            "top.txt",
            "a/one.txt",
            "a/b/two.txt",
            "a/b/c/deep.txt",
            "node_modules/x/n.txt",
            "hid/inside.txt",
            "secret.txt",
        ] {
            fs::write(root.join(f), b"x").unwrap();
        }
        let hide = |p: &str| {
            let status = std::process::Command::new("attrib")
                .arg("+h")
                .arg(root.join(p))
                .status()
                .unwrap();
            assert!(status.success());
        };
        hide("secret.txt");
        hide("hid");

        let no_cancel = AtomicBool::new(false);
        let all = rel(
            &root,
            &scan_files(std::slice::from_ref(&root), 10, 1000, &no_cancel),
        );
        assert_eq!(
            all,
            [
                "a",
                "a/b",
                "a/b/c",
                "a/b/c/deep.txt",
                "a/b/two.txt",
                "a/one.txt",
                "top.txt"
            ]
        );

        // Depth 1: direct children only; depth 2 adds one level.
        let d1 = rel(
            &root,
            &scan_files(std::slice::from_ref(&root), 1, 1000, &no_cancel),
        );
        assert_eq!(d1, ["a", "top.txt"]);
        let d2 = rel(
            &root,
            &scan_files(std::slice::from_ref(&root), 2, 1000, &no_cancel),
        );
        assert_eq!(d2, ["a", "a/b", "a/one.txt", "top.txt"]);

        // Entry limit.
        assert_eq!(
            scan_files(std::slice::from_ref(&root), 10, 3, &no_cancel).len(),
            3
        );
        // Cancel.
        let cancelled = AtomicBool::new(true);
        assert!(scan_files(std::slice::from_ref(&root), 10, 1000, &cancelled).is_empty());
        // is_dir.
        let hits = scan_files(std::slice::from_ref(&root), 1, 1000, &no_cancel);
        assert!(hits.iter().any(|h| h.is_dir && h.path.ends_with("a")));
        assert!(hits
            .iter()
            .any(|h| !h.is_dir && h.path.ends_with("top.txt")));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn opaque_fix_only_when_alpha_is_all_zero() {
        let mut a = [1, 2, 3, 0, 4, 5, 6, 0];
        make_opaque_if_no_alpha(&mut a);
        assert_eq!(a, [1, 2, 3, 255, 4, 5, 6, 255]);
        let mut b = [1, 2, 3, 0, 4, 5, 6, 128];
        make_opaque_if_no_alpha(&mut b);
        assert_eq!(b, [1, 2, 3, 0, 4, 5, 6, 128]);
    }

    #[test]
    fn user_folders_are_existing_dirs() {
        for p in user_folders() {
            assert!(p.is_dir(), "{}", p.display());
        }
    }

    /// Live-machine test: needs the real shell (runs on the dev machine).
    #[test]
    fn live_apps_and_icons() {
        let apps = list_apps();
        for a in apps.iter().take(5) {
            eprintln!("app sample: {:?} -> {:?}", a.name, a.parsing_name);
        }
        assert!(apps.len() > 10, "only {} apps", apps.len());
        assert!(
            apps.iter()
                .any(|a| a.name.contains("Explorer") || a.name.contains("Settings")),
            "no Explorer/Settings among {} apps",
            apps.len()
        );
        let first = app_icon(&apps[0].parsing_name, 32).expect("app icon");
        assert!(first.w >= 32 && first.h >= 32);
        assert_eq!(first.bgra.len(), (first.w * first.h * 4) as usize);
        assert!(first.bgra.iter().any(|&b| b != 0));

        let txt = file_icon(".txt", 32).expect("txt icon");
        assert_eq!((txt.w, txt.h), (32, 32));
        assert!(txt.bgra.iter().any(|&b| b != 0));
        assert!(file_icon("<folder>", 32).is_some());
    }

    #[test]
    fn folder_size_sums_files_recursively_and_caps() {
        let root = std::env::temp_dir().join(format!("dj-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("x.bin"), [0u8; 100]).unwrap();
        std::fs::write(root.join("a/y.bin"), [0u8; 20]).unwrap();
        std::fs::write(root.join("a/b/z.bin"), [0u8; 3]).unwrap();
        let far = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let cancel = AtomicBool::new(false);
        assert_eq!(folder_size(&root, 1000, far, &cancel), (123, 3, false));
        // The file cap: lower bounds, flagged.
        let (_, files, capped) = folder_size(&root, 2, far, &cancel);
        assert!(capped && files == 2);
        // A deadline already gone, and cancel.
        let past = std::time::Instant::now();
        assert!(folder_size(&root, 1000, past, &cancel).2);
        cancel.store(true, Ordering::Relaxed);
        assert!(folder_size(&root, 1000, far, &cancel).2);
        // A missing folder is empty, not an error.
        let none = AtomicBool::new(false);
        assert_eq!(
            folder_size(&root.join("missing"), 10, far, &none),
            (0, 0, false)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Live machine: some installed program records a size.
    #[test]
    fn live_installed_sizes() {
        let sizes = installed_sizes();
        assert!(!sizes.is_empty());
        assert!(sizes.iter().all(|(n, b)| !n.is_empty() && *b >= 1024));
    }
}
