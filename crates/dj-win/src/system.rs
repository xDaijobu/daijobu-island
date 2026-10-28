//! Process-level helpers: single instance, autostart, local time, the
//! always-on-top hook and the theme directory watcher.

use crate::window::WM_THEMES_CHANGED;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HMODULE, HWND, LPARAM, WPARAM,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindowVisible, PostMessageW, SetWindowPos, EVENT_SYSTEM_FOREGROUND, HWND_TOPMOST,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNORMAL, WINEVENT_OUTOFCONTEXT,
};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("DaijobuIsland");
#[cfg(test)]
const TEST_VALUE: PCWSTR = w!("DaijobuIslandUnitTest");

/// Returns false if another instance already holds the named mutex.
pub fn acquire_single_instance() -> bool {
    // SAFETY: the mutex handle is intentionally leaked for the process lifetime.
    unsafe {
        match CreateMutexW(None, true, w!("Local\\DaijobuIsland.SingleInstance")) {
            Ok(_) => GetLastError() != ERROR_ALREADY_EXISTS,
            Err(_) => false,
        }
    }
}

fn open_run_key(access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Option<HKEY> {
    let mut key = HKEY::default();
    // SAFETY: valid key name and out-pointer.
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, access, &mut key) };
    status.is_ok().then_some(key)
}

/// True if `HKCU\...\Run\DaijobuIsland` exists.
pub fn autostart_enabled() -> bool {
    autostart_enabled_for(RUN_VALUE)
}

fn autostart_enabled_for(value: PCWSTR) -> bool {
    let Some(key) = open_run_key(KEY_QUERY_VALUE) else {
        return false;
    };
    // SAFETY: querying only the existence of the value; the key is closed after.
    unsafe {
        let r = RegQueryValueExW(key, value, None, None, None, None);
        let _ = RegCloseKey(key);
        r.is_ok()
    }
}

/// Adds or removes the Run entry. `exe` is stored quoted.
pub fn set_autostart(enabled: bool, exe: &Path) -> bool {
    set_autostart_for(RUN_VALUE, enabled, exe)
}

fn set_autostart_for(name: PCWSTR, enabled: bool, exe: &Path) -> bool {
    let Some(key) = open_run_key(KEY_SET_VALUE) else {
        return false;
    };
    // SAFETY: the byte slice covers exactly the UTF-16 buffer, including the NUL.
    unsafe {
        let ok = if enabled {
            let value: Vec<u16> = format!("\"{}\"", exe.display())
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let bytes = std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2);
            RegSetValueExW(key, name, None, REG_SZ, Some(bytes)).is_ok()
        } else {
            // Deleting a value that is already gone is success.
            let _ = RegDeleteValueW(key, name);
            true
        };
        let _ = RegCloseKey(key);
        ok
    }
}

/// Local wall-clock `(hour, minute, second)`.
/// `~/.daijobu/`, the app's data folder.
pub fn data_dir() -> std::path::PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    home.join(".daijobu")
}

/// Appends one line to `~/.daijobu/daijobu.log`. Failures are ignored.
pub fn log(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir().join("daijobu.log"))
    {
        let _ = writeln!(f, "{msg}");
    }
}

/// Opens a web address in the default browser. Returns false if the shell refused.
pub fn open_url(url: &str) -> bool {
    let wide: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call; "open" is a static string.
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    r.0 as isize > 32
}

/// Local wall-clock date and time: `(year, month, day, hour, minute, second)`.
pub fn local_datetime() -> (u16, u8, u8, u8, u8, u8) {
    // SAFETY: no pointers involved.
    let t = unsafe { GetLocalTime() };
    (
        t.wYear,
        t.wMonth as u8,
        t.wDay as u8,
        t.wHour as u8,
        t.wMinute as u8,
        t.wSecond as u8,
    )
}

pub fn local_time() -> (u32, u32, u32) {
    // SAFETY: no pointers involved.
    let t = unsafe { GetLocalTime() };
    (
        u32::from(t.wHour),
        u32::from(t.wMinute),
        u32::from(t.wSecond),
    )
}

static TOPMOST_HWND: AtomicIsize = AtomicIsize::new(0);
/// A second window that stays on top with the island (the Snap preview stack); 0 if none.
static COMPANION_HWND: AtomicIsize = AtomicIsize::new(0);

/// The island belongs on top (the default). Off, the topmost hook keeps it on the desktop
/// layer instead (see [`crate::window::set_layer`]).
static ISLAND_ON_TOP: AtomicBool = AtomicBool::new(true);

/// Tells the topmost hook which layer to keep the island on.
pub fn set_island_on_top(on_top: bool) {
    ISLAND_ON_TOP.store(on_top, Ordering::Relaxed);
}

// SAFETY: only ever invoked by the system as a WINEVENTPROC; it ignores all arguments.
unsafe extern "system" fn foreground_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    for (i, raw) in [&TOPMOST_HWND, &COMPANION_HWND]
        .map(|h| h.load(Ordering::Relaxed))
        .into_iter()
        .enumerate()
    {
        if raw == 0 {
            continue;
        }
        let ours = HWND(raw as *mut _);
        // The island (index 0) stays behind when the app says so; the companion never does.
        if i == 0 && !ISLAND_ON_TOP.load(Ordering::Relaxed) {
            // SAFETY: our own window; only the z-order is touched.
            if unsafe { IsWindowVisible(ours).as_bool() } {
                crate::window::set_layer(ours, crate::window::Layer::Desktop);
            }
            continue;
        }
        // SAFETY: the handle is one of our own windows; only the z-order is touched.
        unsafe {
            if IsWindowVisible(ours).as_bool() {
                let _ = SetWindowPos(
                    ours,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }
}

/// Keeps `hwnd` topmost along with the island (see [`install_topmost_hook`]); `None`
/// before that window is destroyed.
pub fn set_topmost_companion(hwnd: Option<HWND>) {
    COMPANION_HWND.store(hwnd.map_or(0, |h| h.0 as isize), Ordering::Relaxed);
}

/// Re-asserts topmost whenever the foreground window changes.
pub fn install_topmost_hook(hwnd: HWND) {
    TOPMOST_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
    // SAFETY: the callback is a plain function that only touches our own window;
    // out-of-context hooks are delivered through this thread's message loop.
    unsafe {
        let _ = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            Some(HMODULE::default()),
            Some(foreground_changed),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
    }
}

/// Watches `dir` on a background thread and posts `WM_THEMES_CHANGED` to `hwnd`
/// on every change (the window debounces).
pub fn watch_dir(hwnd: HWND, dir: &Path) {
    let hwnd_raw = hwnd.0 as isize;
    let dir: Vec<u16> = dir
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    std::thread::spawn(move || {
        // SAFETY: the directory handle and buffer live for the whole thread; the
        // window handle is only used with PostMessageW, which is thread-safe.
        unsafe {
            let Ok(handle) = CreateFileW(
                PCWSTR(dir.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                Some(HANDLE::default()),
            ) else {
                return;
            };
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                let mut returned = 0u32;
                let r = ReadDirectoryChangesW(
                    handle,
                    buf.as_mut_ptr().cast(),
                    buf.len() as u32,
                    false,
                    FILE_NOTIFY_CHANGE_FILE_NAME
                        | FILE_NOTIFY_CHANGE_LAST_WRITE
                        | FILE_NOTIFY_CHANGE_SIZE
                        | FILE_NOTIFY_CHANGE_CREATION,
                    Some(&mut returned),
                    None,
                    None,
                );
                if r.is_err() {
                    break;
                }
                let target = HWND(hwnd_raw as *mut _);
                if PostMessageW(Some(target), WM_THEMES_CHANGED, WPARAM(0), LPARAM(0)).is_err() {
                    break;
                }
            }
            let _ = CloseHandle(handle);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_roundtrip_uses_a_scratch_value() {
        let exe = Path::new(r"C:\Program Files\Daijobu Island\daijobu.exe");
        assert!(set_autostart_for(TEST_VALUE, true, exe));
        assert!(autostart_enabled_for(TEST_VALUE));
        assert!(set_autostart_for(TEST_VALUE, false, exe));
        assert!(!autostart_enabled_for(TEST_VALUE));
        // Removing something that is already gone still succeeds.
        assert!(set_autostart_for(TEST_VALUE, false, exe));
    }

    #[test]
    fn local_time_is_sane() {
        let (h, m, s) = local_time();
        assert!(h < 24 && m < 60 && s < 61);
    }

    #[test]
    fn local_datetime_is_sane() {
        let (y, mo, d, h, mi, s) = local_datetime();
        assert!(y >= 2024 && (1..=12).contains(&mo) && (1..=31).contains(&d));
        assert!(h < 24 && mi < 60 && s < 61);
    }
}
