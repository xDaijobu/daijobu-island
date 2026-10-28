//! The text clipboard (`CF_UNICODETEXT`), for the terminal's copy and paste.
//!
//! The clipboard belongs to the user, so nothing in here runs from a test: the tests only
//! cover the pure text conversion. Every call opens the clipboard for as short a time as
//! it can and closes it again on every path, including a panic ([`Open`] closes on drop).

use std::time::Duration;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};

/// Another app can hold the clipboard for a moment: after the first try, retry this many
/// times, waiting `OPEN_WAIT` before each.
const OPEN_RETRIES: u32 = 5;
const OPEN_WAIT: Duration = Duration::from_millis(10);

/// The clipboard, open on this thread. Closing it is the drop.
struct Open;

impl Open {
    /// Opens the clipboard for `owner` (the island's window; 0 for none). `None` when it
    /// stayed busy through every retry.
    fn acquire(owner: isize) -> Option<Open> {
        let hwnd = (owner != 0).then_some(HWND(owner as *mut _));
        for attempt in 0..=OPEN_RETRIES {
            if attempt > 0 {
                std::thread::sleep(OPEN_WAIT);
            }
            // SAFETY: `hwnd` is our window or none; a failure just means "busy".
            if unsafe { OpenClipboard(hwnd) }.is_ok() {
                return Some(Open);
            }
        }
        None
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: closes the clipboard this thread opened in `acquire`.
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// The clipboard's text, or `None` when it holds no text, only an empty one, or stayed
/// busy. Line breaks are left as the source wrote them (usually CRLF).
pub fn get_text(owner: isize) -> Option<String> {
    let _open = Open::acquire(owner)?;
    // SAFETY: the clipboard is open on this thread for the whole block (`_open`). The
    // handle belongs to the clipboard: it is locked and read, never freed. The text is
    // copied out before the unlock, and the block's real size bounds the read, so an
    // unterminated string can't run past it.
    let units = unsafe {
        IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).ok()?;
        let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
        let mem = HGLOBAL(handle.0);
        let ptr = GlobalLock(mem) as *const u8;
        if ptr.is_null() {
            return None;
        }
        let count = GlobalSize(mem) / size_of::<u16>();
        let mut buf = vec![0u16; count];
        // Bytes, so the block's alignment doesn't matter.
        std::ptr::copy_nonoverlapping(ptr, buf.as_mut_ptr() as *mut u8, count * size_of::<u16>());
        let _ = GlobalUnlock(mem);
        buf
    };
    let text = decode(&units);
    (!text.is_empty()).then_some(text)
}

/// Replaces the clipboard with `text`. `owner` is the island's window: a clipboard with
/// no owner refuses data. Lone `\n` becomes CRLF, the Windows form, so Notepad and the
/// like show the lines. Returns false when the clipboard stayed busy or refused.
pub fn set_text(owner: isize, text: &str) -> bool {
    let units = encode(text);
    let Some(_open) = Open::acquire(owner) else {
        return false;
    };
    // SAFETY: the clipboard is open on this thread for the whole block (`_open`).
    unsafe {
        if EmptyClipboard().is_err() {
            return false;
        }
        put_units(&units)
    }
}

/// Puts files on the clipboard (`CF_HDROP`), as Explorer's Copy does: they paste into a
/// folder, a chat or a mail. `owner` as for [`set_text`]. False when busy or refused.
pub fn set_files(owner: isize, paths: &[std::path::PathBuf]) -> bool {
    if paths.is_empty() {
        return false;
    }
    let bytes = drop_files(paths);
    let Some(_open) = Open::acquire(owner) else {
        return false;
    };
    // SAFETY: the clipboard is open on this thread for the whole block (`_open`); the
    // block is `bytes.len()` long and locked while written; on success the clipboard owns
    // it, on failure it is freed here.
    unsafe {
        if EmptyClipboard().is_err() {
            return false;
        }
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else {
            return false;
        };
        let dst = GlobalLock(mem) as *mut u8;
        if dst.is_null() {
            let _ = GlobalFree(Some(mem));
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        let _ = GlobalUnlock(mem);
        if SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(mem.0))).is_ok() {
            true
        } else {
            let _ = GlobalFree(Some(mem));
            false
        }
    }
}

/// A `DROPFILES` block: the 20-byte header (`pFiles` = 20, no point, not client area,
/// wide strings), then each path in UTF-16 with its NUL, then one more NUL.
fn drop_files(paths: &[std::path::PathBuf]) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    let mut out = Vec::new();
    for v in [20u32, 0, 0, 0, 1] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for p in paths {
        for u in p.as_os_str().encode_wide().filter(|&u| u != 0) {
            out.extend_from_slice(&u.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0]);
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// Copies `units` into a movable global block and hands it to the clipboard.
///
/// # Safety
/// The clipboard must be open, and emptied, by this thread.
unsafe fn put_units(units: &[u16]) -> bool {
    let bytes = std::mem::size_of_val(units);
    // SAFETY: the block is `bytes` long and locked while it is written. On success the
    // clipboard owns it; on every failure it is freed here.
    unsafe {
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes) else {
            return false;
        };
        let dst = GlobalLock(mem) as *mut u8;
        if dst.is_null() {
            let _ = GlobalFree(Some(mem));
            return false;
        }
        std::ptr::copy_nonoverlapping(units.as_ptr() as *const u8, dst, bytes);
        let _ = GlobalUnlock(mem);
        if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(mem.0))).is_ok() {
            true
        } else {
            let _ = GlobalFree(Some(mem));
            false
        }
    }
}

/// UTF-16 for the clipboard: lone `\n` becomes CRLF (an existing CRLF stays one), NULs
/// are dropped (they would end the string early), and one terminating NUL is added.
fn encode(text: &str) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::with_capacity(text.len() + 1);
    let mut after_cr = false;
    for ch in text.chars() {
        match ch {
            '\0' => continue,
            '\n' if !after_cr => out.extend([u16::from(b'\r'), u16::from(b'\n')]),
            _ => out.extend(ch.encode_utf16(&mut [0; 2]).iter()),
        }
        after_cr = ch == '\r';
    }
    out.push(0);
    out
}

/// Text from clipboard UTF-16: up to the first NUL (or the end of the block), with an
/// unpaired surrogate shown as U+FFFD.
fn decode(units: &[u16]) -> String {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(text: &str) -> Vec<u16> {
        encode(text)
    }

    #[test]
    fn text_is_nul_terminated_utf16() {
        assert_eq!(enc("abc"), vec![97, 98, 99, 0]);
        assert_eq!(enc(""), vec![0]);
        // Outside the BMP: a surrogate pair.
        assert_eq!(enc("\u{1F600}"), vec![0xD83D, 0xDE00, 0]);
    }

    #[test]
    fn lone_line_feeds_become_crlf() {
        assert_eq!(enc("a\nb"), vec![97, 13, 10, 98, 0]);
        // An existing CRLF is not doubled, and a lone CR is left alone.
        assert_eq!(enc("a\r\nb"), vec![97, 13, 10, 98, 0]);
        assert_eq!(enc("a\rb"), vec![97, 13, 98, 0]);
        assert_eq!(enc("\n\n"), vec![13, 10, 13, 10, 0]);
    }

    #[test]
    fn nuls_are_dropped() {
        assert_eq!(enc("a\0b"), vec![97, 98, 0]);
        // A NUL between CR and LF doesn't break the CRLF.
        assert_eq!(enc("\r\0\n"), vec![13, 10, 0]);
    }

    #[test]
    fn decoding_stops_at_the_nul() {
        assert_eq!(decode(&[104, 105, 0, 120, 121]), "hi");
        // A block without a terminator ends at its size.
        assert_eq!(decode(&[104, 105]), "hi");
        assert_eq!(decode(&[0]), "");
        assert_eq!(decode(&[]), "");
    }

    #[test]
    fn decoding_survives_bad_utf16() {
        assert_eq!(decode(&[97, 0xD800, 98, 0]), "a\u{FFFD}b");
        assert_eq!(decode(&[0xDE00, 0]), "\u{FFFD}");
        assert_eq!(decode(&[0xD83D, 0xDE00, 0]), "\u{1F600}");
    }

    #[test]
    fn encode_then_decode_round_trips_modulo_crlf() {
        for s in ["", "plain", "日本語 ✓", "🙂 mixed", "one\r\ntwo"] {
            assert_eq!(decode(&encode(s)), s);
        }
        assert_eq!(decode(&encode("one\ntwo")), "one\r\ntwo");
    }

    #[test]
    fn drop_files_layout() {
        let b = drop_files(&[r"C:\a".into(), r"D:\b c".into()]);
        // Header: pFiles = 20, pt = (0, 0), fNC = 0, fWide = 1.
        assert_eq!(
            &b[..20],
            &[20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]
        );
        let units: Vec<u16> = b[20..]
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let want: Vec<u16> = "C:\\a\0D:\\b c\0\0".encode_utf16().collect();
        assert_eq!(units, want);
    }
}
