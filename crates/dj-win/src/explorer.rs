//! Read-only Explorer automation. COM never leaves the request's dedicated STA.
//! Only visible active views are accepted; inactive Windows 11 tabs are not listed.
use dj_core::terminal_folders::{self as model, ExplorerSnapshot};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicIsize, Ordering},
    mpsc::{self, Receiver},
};
use windows::core::Interface;
use windows::Win32::{
    Foundation::HWND,
    System::{
        Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IServiceProvider,
            CLSCTX_ALL, COINIT_APARTMENTTHREADED,
        },
        SystemServices::{SFGAO_FILESYSTEM, SFGAO_FOLDER},
        Variant::VARIANT,
    },
    UI::{
        Shell::{
            IFolderView, IPersistFolder2, IShellBrowser, IShellItem, IShellWindows, IWebBrowserApp,
            SHCreateItemFromIDList, SID_STopLevelBrowser, ShellWindows, SIGDN_FILESYSPATH,
        },
        WindowsAndMessaging::{
            GetAncestor, GetClassNameW, GetForegroundWindow, IsWindow, IsWindowVisible, GA_ROOT,
        },
    },
};

static LAST_EXPLORER: AtomicIsize = AtomicIsize::new(0);

fn explorer_window(hwnd: HWND) -> bool {
    let mut class = [0u16; 64];
    // SAFETY: transient HWND is queried only; buffer is a live fixed-size slice.
    unsafe {
        IsWindow(Some(hwnd)).as_bool()
            && GetClassNameW(hwnd, &mut class) > 0
            && matches!(
                String::from_utf16_lossy(&class).trim_end_matches('\0'),
                "CabinetWClass" | "ExploreWClass"
            )
    }
}

pub(crate) fn note_foreground(hwnd: HWND) {
    if explorer_window(hwnd) {
        LAST_EXPLORER.store(hwnd.0 as isize, Ordering::SeqCst);
    }
}

pub fn request(owner: isize) -> Result<Receiver<Result<ExplorerSnapshot, String>>, String> {
    // SAFETY: read-only foreground query; does not activate or navigate anything.
    note_foreground(unsafe { GetForegroundWindow() });
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("terminal-explorer-sta".into())
        .spawn(move || {
            let result = snapshot();
            let _ = tx.send(result);
            crate::window::wake(owner);
        })
        .map_err(|e| e.to_string())?;
    Ok(rx)
}

fn snapshot() -> Result<ExplorerSnapshot, String> {
    // SAFETY: initialized/uninitialized on this dedicated thread. All COM pointers
    // are dropped by enumerate before uninitializing, including error paths.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        let result = enumerate();
        CoUninitialize();
        result.map_err(|e| format!("Explorer unavailable: {e}. Use Browse or Recent."))
    }
}

fn enumerate() -> windows::core::Result<ExplorerSnapshot> {
    // SAFETY: called only from the initialized STA; PIDLs and strings are freed
    // with the COM allocator, interfaces remain thread-local, operations read only.
    unsafe {
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL)?;
        let count = windows.Count()?;
        let last = LAST_EXPLORER.load(Ordering::SeqCst);
        let mut by_window: BTreeMap<isize, BTreeSet<String>> = BTreeMap::new();
        let mut skipped = 0;
        for index in 0..count.clamp(0, 128) {
            let result = (|| -> windows::core::Result<Option<(isize, String)>> {
                let dispatch = windows.Item(&VARIANT::from(index))?;
                let browser: IWebBrowserApp = dispatch.cast()?;
                let root = HWND(browser.HWND()?.0 as _);
                if !explorer_window(root) {
                    return Ok(None);
                }
                let provider: IServiceProvider = dispatch.cast()?;
                let shell: IShellBrowser = provider.QueryService(&SID_STopLevelBrowser)?;
                let view = shell.QueryActiveShellView()?;
                let view_hwnd = view.GetWindow()?;
                if !IsWindowVisible(view_hwnd).as_bool() || GetAncestor(view_hwnd, GA_ROOT) != root
                {
                    return Ok(None);
                }
                let folder: IFolderView = view.cast()?;
                let persist: IPersistFolder2 = folder.GetFolder()?;
                let pidl = persist.GetCurFolder()?;
                let item = SHCreateItemFromIDList::<IShellItem>(pidl);
                CoTaskMemFree(Some(pidl.cast()));
                let item = item?;
                let mask = SFGAO_FILESYSTEM | SFGAO_FOLDER;
                if item.GetAttributes(mask)? != mask {
                    return Ok(None);
                }
                let path = item.GetDisplayName(SIGDN_FILESYSPATH)?;
                let text = path.to_string();
                CoTaskMemFree(Some(path.0.cast()));
                let Ok(text) = text else {
                    return Ok(None);
                };
                if model::validate(&text).is_err() {
                    return Ok(None);
                }
                // Filesystem checks can block on a network share. A tab that
                // became hidden in that interval must not be labeled active.
                if !IsWindowVisible(view_hwnd).as_bool()
                    || shell.QueryActiveShellView()?.GetWindow()? != view_hwnd
                    || GetAncestor(view_hwnd, GA_ROOT) != root
                {
                    return Ok(None);
                }
                Ok(Some((root.0 as isize, text)))
            })();
            match result {
                Ok(Some((hwnd, path))) => {
                    by_window.entry(hwnd).or_default().insert(path);
                }
                _ => skipped += 1,
            }
        }
        // Conflicting visible registrations for one HWND are ambiguous: never
        // choose the first automation record and call it the active Windows 11 tab.
        let (rows, mut active, ambiguous) = resolve_views(by_window, last);
        if LAST_EXPLORER.load(Ordering::SeqCst) != last {
            active = None; // foreground changed during COM: do not prefill stale data
        }
        Ok(ExplorerSnapshot::bounded(
            rows,
            active,
            skipped > 0 || count > 128 || ambiguous,
        ))
    }
}

fn resolve_views(
    by_window: BTreeMap<isize, BTreeSet<String>>,
    last: isize,
) -> (Vec<String>, Option<String>, bool) {
    let mut rows = Vec::new();
    let mut active = None;
    let mut ambiguous = false;
    for (hwnd, paths) in by_window {
        if paths.len() != 1 {
            ambiguous = true;
            continue;
        }
        let path = paths.into_iter().next().unwrap();
        if hwnd == last {
            active = Some(path.clone());
        }
        rows.push(path);
    }
    (rows, active, ambiguous)
}

#[cfg(test)]
mod tests {
    #[test]
    fn latest_window_wins_and_conflicting_tab_registrations_are_rejected() {
        let views = [
            (10, ["old"].into_iter().map(String::from).collect()),
            (20, ["latest"].into_iter().map(String::from).collect()),
            (
                30,
                ["tab-a", "tab-b"].into_iter().map(String::from).collect(),
            ),
        ]
        .into_iter()
        .collect();
        let (rows, active, incomplete) = super::resolve_views(views, 20);
        assert_eq!(active.as_deref(), Some("latest"));
        assert_eq!(rows, ["old", "latest"]);
        assert!(incomplete);
    }
    #[test]
    fn read_only_explorer_smoke() {
        // May legitimately be empty (no Explorer windows/only virtual views).
        // No navigation, selection, focus change or provider launch occurs.
        let result = super::snapshot().expect("Explorer automation on dedicated test thread");
        eprintln!(
            "Explorer smoke: {} filesystem folders; active={}; incomplete={}",
            result.folders.len(),
            result.active.is_some(),
            result.incomplete
        );
        assert!(result.folders.len() <= dj_core::terminal_folders::EXPLORER_LIMIT);
        for path in result.folders {
            assert!(dj_core::terminal_folders::validate(&path).is_ok());
        }
    }
}
