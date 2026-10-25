//! The real side effects: permanent delete, Recycle Bin via `IFileOperation`, DNS flush,
//! empty bin, reveal in Explorer, and the running-process probe. This is the only
//! module with `unsafe`. Tests never call it; they use a fake `Ops`.

use crate::{fs as sfs, rules, Ops, Recycled, WinOps};
use std::io;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use windows::core::{s, w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, RPC_E_CHANGED_MODE};
use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
    COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use windows::Win32::UI::Shell::{
    FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName, SHEmptyRecycleBinW,
    SHQueryRecycleBinW, FOFX_EARLYFAILURE, FOFX_RECYCLEONDELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION,
    FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI,
    SHERB_NOSOUND, SHQUERYRBINFO,
};

/// `GetDriveTypeW` for a hard disk (from WinBase.h).
const DRIVE_FIXED: u32 = 3;

fn win_err(e: windows::core::Error) -> io::Error {
    io::Error::other(e.message())
}

/// Initialises COM (STA) on this thread for the guard's lifetime.
struct ComGuard(bool);

impl ComGuard {
    fn new() -> io::Result<Self> {
        // SAFETY: plain COM initialisation on the current thread; balanced in Drop
        // when it succeeded (S_OK or S_FALSE).
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        if hr == RPC_E_CHANGED_MODE {
            // Already initialised as MTA by someone else: IFileOperation still works.
            return Ok(Self(false));
        }
        hr.ok().map_err(win_err)?;
        Ok(Self(true))
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx in `new` on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

impl Ops for WinOps {
    fn remove_permanent(&self, p: &Path) -> io::Result<u64> {
        sfs::remove_tree(p)
    }

    fn recycle(&self, p: &Path) -> io::Result<Recycled> {
        let plain = rules::plain_path(p);
        let root = volume_root(&plain)?;
        // Only fixed drives have a Recycle Bin. Elsewhere "recycle" would mean "delete".
        // SAFETY: `root` is a valid NUL-terminated wide string for the call.
        if unsafe { GetDriveTypeW(&root) } != DRIVE_FIXED {
            return Err(io::Error::other("this drive has no recycle bin"));
        }
        let before =
            bin_items(&root).ok_or_else(|| io::Error::other("this drive has no recycle bin"))?;
        let _com = ComGuard::new()?;
        let path = HSTRING::from(plain.as_os_str());
        // FOF_WANTNUKEWARNING stays: without it, FOF_NOCONFIRMATION lets Windows delete
        // an item that does not fit in the bin for good, silently. With it, Windows asks
        // first, and the bin count below tells whether the item really went to the bin.
        let flags = FOF_ALLOWUNDO
            | FOFX_RECYCLEONDELETE
            | FOF_NOCONFIRMATION
            | FOF_WANTNUKEWARNING
            | FOF_SILENT
            | FOF_NOERRORUI
            | FOFX_EARLYFAILURE;
        // SAFETY: COM is initialised on this thread (`_com`); every interface pointer
        // is owned by a windows-rs wrapper and released on drop; `path` outlives the call.
        unsafe {
            let op: IFileOperation =
                CoCreateInstance(&FileOperation, None, CLSCTX_ALL).map_err(win_err)?;
            op.SetOperationFlags(flags).map_err(win_err)?;
            let item: IShellItem = SHCreateItemFromParsingName(&path, None).map_err(win_err)?;
            op.DeleteItem(&item, None).map_err(win_err)?;
            op.PerformOperations().map_err(win_err)?;
            if op.GetAnyOperationsAborted().map_err(win_err)?.as_bool() {
                return Err(io::Error::other("recycle was cancelled"));
            }
        }
        // Recycling adds one entry to the bin, whatever the item's size. When the
        // count cannot be read afterwards, the earlier checks stand.
        Ok(match bin_items(&root) {
            Some(after) if after <= before => Recycled::DeletedForGood,
            _ => Recycled::InBin,
        })
    }

    fn flush_dns(&self) -> io::Result<()> {
        // DnsFlushResolverCache is exported by dnsapi.dll but not declared in the SDK
        // headers, so it is looked up at run time.
        // SAFETY: the library is loaded from System32 only; the symbol has the
        // signature `BOOL WINAPI DnsFlushResolverCache(void)` on every supported
        // Windows version; the module stays loaded for the process lifetime.
        unsafe {
            let lib = LoadLibraryExW(w!("dnsapi.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
                .map_err(win_err)?;
            let f = GetProcAddress(lib, s!("DnsFlushResolverCache"))
                .ok_or_else(|| io::Error::other("DnsFlushResolverCache not found"))?;
            let f: unsafe extern "system" fn() -> i32 = std::mem::transmute(f);
            if f() == 0 {
                return Err(io::Error::other("DnsFlushResolverCache failed"));
            }
        }
        Ok(())
    }

    fn empty_recycle_bin(&self) -> io::Result<()> {
        let mut info = SHQUERYRBINFO {
            cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `info` is a valid, sized out-parameter; a null root means all drives.
        let empty = unsafe { SHQueryRecycleBinW(PCWSTR::null(), &mut info) }.is_ok()
            && info.i64NumItems == 0;
        if empty {
            return Ok(());
        }
        // SAFETY: no owner window, null root = every drive's bin; flags suppress UI.
        unsafe {
            SHEmptyRecycleBinW(
                None,
                PCWSTR::null(),
                SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND,
            )
        }
        .map_err(win_err)
    }

    fn reveal(&self, p: &Path) -> io::Result<()> {
        let path = rules::plain_path(p);
        let s = path.to_string_lossy();
        if s.contains('"') {
            return Err(io::Error::other("bad path"));
        }
        Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{s}\""))
            .spawn()
            .map(|_| ())
    }
}

/// `C:\` for `C:\Users\ann\x.bin`: the root of the volume that holds `p`, as a
/// NUL-terminated wide string.
fn volume_root(p: &Path) -> io::Result<HSTRING> {
    let path = HSTRING::from(p.as_os_str());
    let mut buf = [0u16; 261];
    // SAFETY: `path` is NUL-terminated and outlives the call; the buffer length is
    // passed with the slice.
    unsafe { GetVolumePathNameW(&path, &mut buf) }.map_err(win_err)?;
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Ok(HSTRING::from_wide(&buf[..len]))
}

/// Entries in the Recycle Bin of the volume at `root`, or None if it has none.
fn bin_items(root: &HSTRING) -> Option<i64> {
    let mut info = SHQUERYRBINFO {
        cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `root` is NUL-terminated; `info` is a valid, sized out-parameter.
    unsafe { SHQueryRecycleBinW(root, &mut info) }.ok()?;
    Some(info.i64NumItems)
}

/// Lowercased exe names of the running processes, or None if they cannot be listed.
pub fn running_processes() -> Option<Vec<String>> {
    let mut out = Vec::new();
    // SAFETY: the snapshot handle is checked, iterated with a correctly sized
    // PROCESSENTRY32W and closed before returning.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let first = Process32FirstW(snap, &mut e).is_ok();
        let mut ok = first;
        while ok {
            let len = e
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(e.szExeFile.len());
            out.push(String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase());
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
        // An empty list would read as "nothing is running".
        if !first {
            return None;
        }
    }
    Some(out)
}
