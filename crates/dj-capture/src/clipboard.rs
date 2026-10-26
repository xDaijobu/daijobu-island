//! Puts a shot on the clipboard as `CF_DIB` and the registered `"PNG"` format, in one
//! `OpenClipboard` / `EmptyClipboard` session. Never called from tests: the app puts it
//! behind a trait.

use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_DIB;

/// Another app can hold the clipboard for a moment; try this many times.
const OPEN_TRIES: u32 = 10;
const OPEN_WAIT: std::time::Duration = std::time::Duration::from_millis(15);

/// Replaces the clipboard with `dib` (`CF_DIB`) and `png` (`"PNG"`). `owner` is the
/// window that owns the clipboard (the island). Errors name the step that failed.
pub fn put_image(owner: isize, png: &[u8], dib: &[u8]) -> Result<(), String> {
    let hwnd = HWND(owner as *mut _);
    // SAFETY: the format name is a static string.
    let png_format = unsafe { RegisterClipboardFormatW(w!("PNG")) };
    let mut opened = false;
    for _ in 0..OPEN_TRIES {
        // SAFETY: `hwnd` is our window (or null); a failure just means "busy".
        if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
            opened = true;
            break;
        }
        std::thread::sleep(OPEN_WAIT);
    }
    if !opened {
        return Err("the clipboard is busy".into());
    }
    // SAFETY: the clipboard is open on this thread until CloseClipboard below.
    let result = unsafe {
        match EmptyClipboard() {
            Err(e) => Err(format!("empty clipboard: {e}")),
            Ok(()) => {
                let dib_ok = set(CF_DIB.0 as u32, dib);
                let png_ok = if png_format == 0 {
                    Err("register PNG format".to_string())
                } else {
                    set(png_format, png)
                };
                dib_ok.and(png_ok)
            }
        }
    };
    // SAFETY: closes the clipboard this thread opened.
    unsafe {
        let _ = CloseClipboard();
    }
    result
}

/// Copies `bytes` into a movable global block and hands it to the clipboard.
///
/// # Safety
/// The clipboard must be open and emptied by this thread.
unsafe fn set(format: u32, bytes: &[u8]) -> Result<(), String> {
    // SAFETY: the block is `bytes.len()` long and locked while it is written; on success
    // the clipboard owns it, on failure it is freed here.
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))
            .map_err(|e| format!("alloc {format}: {e}"))?;
        let dst = GlobalLock(mem) as *mut u8;
        if dst.is_null() {
            let _ = GlobalFree(Some(mem));
            return Err(format!("lock {format}"));
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        let _ = GlobalUnlock(mem);
        match SetClipboardData(format, Some(HANDLE(mem.0))) {
            Ok(_) => Ok(()),
            Err(e) => {
                let _ = GlobalFree(Some(mem));
                Err(format!("set format {format}: {e}"))
            }
        }
    }
}
