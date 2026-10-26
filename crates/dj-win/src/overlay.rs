//! The Snap overlay window: a topmost `WS_POPUP` over the whole virtual desktop, made
//! when a snap starts and destroyed after it. Unlike the island it takes activation,
//! because it needs the keyboard (Esc, the arrows, the editor's keys and typing). Its
//! messages become [`Event::Snap`] on the same queue as the island's, so the app
//! handles them outside any Win32 callback.

use crate::window::{push_event, Event};
use std::cell::Cell;
use std::sync::atomic::{AtomicU8, Ordering};
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: PCWSTR = w!("DaijobuSnapOverlay");

/// Input on the overlay. Positions are client px (physical); add the overlay's origin
/// for desktop coordinates. The app decides what each means in the selecting and the
/// editing phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapInput {
    Press {
        x: i32,
        y: i32,
    },
    Motion {
        x: i32,
        y: i32,
    },
    Release {
        x: i32,
        y: i32,
    },
    /// The second press of a double click (the class has `CS_DBLCLKS`, so this comes
    /// instead of a second `Press`).
    DoubleClick {
        x: i32,
        y: i32,
    },
    RightPress,
    /// A key went down. `ctrl` is false while Alt is held too, so AltGr (Ctrl+Alt)
    /// characters never read as Ctrl shortcuts.
    Key {
        vk: u16,
        ctrl: bool,
        shift: bool,
    },
    /// A typed character from `WM_CHAR` (surrogate pairs joined, control chars dropped).
    Char(char),
    /// Mouse wheel, in notches (positive is away from the user).
    Wheel(i32),
    /// The window was asked to close (Alt+F4).
    Close,
}

/// The overlay's mouse cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Cursor {
    Cross = 0,
    Arrow,
    IBeam,
    SizeNwse,
    SizeNesw,
    SizeWe,
    SizeNs,
    Move,
}

/// The cursor `WM_SETCURSOR` shows. A static, because the window procedure can't reach
/// the app.
static CURSOR: AtomicU8 = AtomicU8::new(Cursor::Cross as u8);

thread_local! {
    /// The high half of a surrogate pair, waiting for its low half.
    static HIGH_SURROGATE: Cell<Option<u16>> = const { Cell::new(None) };
}

fn cursor_id(c: u8) -> PCWSTR {
    match c {
        x if x == Cursor::Arrow as u8 => IDC_ARROW,
        x if x == Cursor::IBeam as u8 => IDC_IBEAM,
        x if x == Cursor::SizeNwse as u8 => IDC_SIZENWSE,
        x if x == Cursor::SizeNesw as u8 => IDC_SIZENESW,
        x if x == Cursor::SizeWe as u8 => IDC_SIZEWE,
        x if x == Cursor::SizeNs as u8 => IDC_SIZENS,
        x if x == Cursor::Move as u8 => IDC_SIZEALL,
        _ => IDC_CROSS,
    }
}

fn apply_cursor(c: u8) {
    // SAFETY: a shared system cursor; nothing to free.
    unsafe {
        if let Ok(h) = LoadCursorW(None, cursor_id(c)) {
            SetCursor(Some(h));
        }
    }
}

/// Sets the overlay's cursor (call on the UI thread). It changes at once, not only on
/// the next mouse move, so a tool picked by key shows its cursor straight away.
pub fn set_cursor(c: Cursor) {
    if CURSOR.swap(c as u8, Ordering::Relaxed) != c as u8 {
        apply_cursor(c as u8);
    }
}

/// Joins `WM_CHAR` UTF-16 units: a high surrogate waits in `pending` for its low half.
/// Returns the char to deliver (control chars are dropped) and the new pending unit.
pub fn join_utf16(pending: Option<u16>, unit: u16) -> (Option<char>, Option<u16>) {
    let ch = match unit {
        0xD800..=0xDBFF => return (None, Some(unit)),
        0xDC00..=0xDFFF => {
            let Some(high) = pending else {
                return (None, None);
            };
            let c = 0x10000 + ((u32::from(high) - 0xD800) << 10) + (u32::from(unit) - 0xDC00);
            char::from_u32(c)
        }
        _ => char::from_u32(u32::from(unit)),
    };
    (ch.filter(|c| !c.is_control()), None)
}

fn lparam_xy(lp: LPARAM) -> (i32, i32) {
    let v = lp.0 as u32;
    (
        i32::from(v as u16 as i16),
        i32::from((v >> 16) as u16 as i16),
    )
}

/// The virtual desktop in physical px: `(x, y, w, h)`. `x` and `y` can be negative.
pub fn virtual_screen() -> (i32, i32, i32, i32) {
    // SAFETY: plain metric queries.
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// The pointer in physical desktop px.
pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    // SAFETY: valid out-pointer.
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

/// Moves the pointer (after an arrow-key nudge, so the mouse and the selection agree).
pub fn set_cursor_pos(x: i32, y: i32) {
    // SAFETY: no pointers involved.
    unsafe {
        let _ = SetCursorPos(x, y);
    }
}

/// Creates the overlay, hidden, at `(x, y, w, h)` desktop px. Show it with [`show`]
/// once its first frame is drawn, so the live desktop never flashes through.
pub fn create(x: i32, y: i32, w: i32, h: i32) -> Result<HWND> {
    CURSOR.store(Cursor::Cross as u8, Ordering::Relaxed);
    HIGH_SURROGATE.with(|h| h.set(None));
    // SAFETY: standard class registration and window creation; all pointers are valid
    // for the duration of the calls and the class name is a static string. Registering
    // the class again on a later snap fails harmlessly.
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(overlay_proc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_CROSS)?,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CLASS_NAME,
            w!("Daijobu Island Snap"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinst.into()),
            None,
        )
    }
}

/// Shows the overlay on top of everything and gives it the keyboard.
pub fn show(hwnd: HWND) {
    // SAFETY: our own window, on the thread that owns it. A hotkey or a click on the
    // island just came in, so this process may take the foreground.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE,
        );
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
    }
}

pub fn destroy(hwnd: HWND) {
    // SAFETY: our own window, destroyed on the thread that created it.
    unsafe {
        let _ = ReleaseCapture();
        let _ = DestroyWindow(hwnd);
    }
}

fn key_down(vk: u16) -> bool {
    // SAFETY: no pointers involved.
    unsafe { GetKeyState(i32::from(vk)) < 0 }
}

extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_SETCURSOR if (lp.0 as u32 & 0xFFFF) == HTCLIENT => {
            apply_cursor(CURSOR.load(Ordering::Relaxed));
            return LRESULT(1);
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            // SAFETY: our own window; capture keeps the release coming.
            let _ = unsafe { SetCapture(hwnd) };
            let (x, y) = lparam_xy(lp);
            push_event(Event::Snap(if msg == WM_LBUTTONDOWN {
                SnapInput::Press { x, y }
            } else {
                SnapInput::DoubleClick { x, y }
            }));
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lp);
            push_event(Event::Snap(SnapInput::Motion { x, y }));
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            // SAFETY: releases only this thread's capture.
            let _ = unsafe { ReleaseCapture() };
            let (x, y) = lparam_xy(lp);
            push_event(Event::Snap(SnapInput::Release { x, y }));
            return LRESULT(0);
        }
        WM_RBUTTONDOWN => {
            push_event(Event::Snap(SnapInput::RightPress));
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            let delta = i32::from((wp.0 >> 16) as u16 as i16);
            let notches = delta / WHEEL_DELTA as i32;
            if notches != 0 {
                push_event(Event::Snap(SnapInput::Wheel(notches)));
            }
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            let ctrl = key_down(VK_CONTROL.0) && !key_down(VK_MENU.0);
            push_event(Event::Snap(SnapInput::Key {
                vk: wp.0 as u16,
                ctrl,
                shift: key_down(VK_SHIFT.0),
            }));
            return LRESULT(0);
        }
        WM_CHAR => {
            let pending = HIGH_SURROGATE.with(Cell::get);
            let (ch, pending) = join_utf16(pending, wp.0 as u16);
            HIGH_SURROGATE.with(|h| h.set(pending));
            if let Some(c) = ch {
                push_event(Event::Snap(SnapInput::Char(c)));
            }
            return LRESULT(0);
        }
        // Alt+F4: the app ends the snap and destroys the window itself.
        WM_CLOSE => {
            push_event(Event::Snap(SnapInput::Close));
            return LRESULT(0);
        }
        // The overlay spans monitors with different DPIs on purpose: never resize it.
        WM_DPICHANGED => return LRESULT(0),
        _ => {}
    }
    // SAFETY: forwards the unhandled message with the original arguments.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wm_char_units_join_into_chars() {
        assert_eq!(join_utf16(None, 'a' as u16), (Some('a'), None));
        assert_eq!(join_utf16(None, 0xE9), (Some('é'), None));
        // 😀 is D83D DE00.
        let (c, p) = join_utf16(None, 0xD83D);
        assert_eq!((c, p), (None, Some(0xD83D)));
        assert_eq!(join_utf16(p, 0xDE00), (Some('😀'), None));
        // A lone low half, or a high half followed by something else, gives nothing odd.
        assert_eq!(join_utf16(None, 0xDE00), (None, None));
        assert_eq!(join_utf16(Some(0xD83D), 'x' as u16), (Some('x'), None));
        // Control chars (Enter, Backspace, Ctrl+C, Esc, DEL) are dropped.
        for unit in [0x0D, 0x08, 0x03, 0x1B, 0x7F, 0x09] {
            assert_eq!(join_utf16(None, unit), (None, None), "{unit:#x}");
        }
        assert_eq!(join_utf16(None, ' ' as u16), (Some(' '), None));
    }
}
