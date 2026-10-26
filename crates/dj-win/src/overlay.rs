//! The Snap overlay window: a topmost `WS_POPUP` over the whole virtual desktop, made
//! when a snap starts and destroyed after it. Unlike the island it takes activation,
//! because it needs the keyboard (Esc, the arrows). Its messages become
//! [`Event::Snap`] on the same queue as the island's, so the app handles them outside
//! any Win32 callback.

use crate::window::{push_event, Event};
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RIGHT,
    VK_SHIFT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: PCWSTR = w!("DaijobuSnapOverlay");

/// Input on the overlay. Positions are client px (physical); add the overlay's origin
/// for desktop coordinates.
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
    /// An arrow key: one step on each axis that is non-zero; `big` while Shift is held.
    Arrow {
        dx: i32,
        dy: i32,
        big: bool,
    },
    /// Esc or a right click.
    Cancel,
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
    // SAFETY: standard class registration and window creation; all pointers are valid
    // for the duration of the calls and the class name is a static string. Registering
    // the class again on a later snap fails harmlessly.
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
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

fn shift_down() -> bool {
    // SAFETY: no pointers involved.
    unsafe { GetKeyState(i32::from(VK_SHIFT.0)) < 0 }
}

extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_SETCURSOR => {
            // SAFETY: a shared system cursor; nothing to free.
            unsafe {
                if let Ok(c) = LoadCursorW(None, IDC_CROSS) {
                    SetCursor(Some(c));
                }
            }
            return LRESULT(1);
        }
        WM_LBUTTONDOWN => {
            // SAFETY: our own window; capture keeps the release coming.
            let _ = unsafe { SetCapture(hwnd) };
            let (x, y) = lparam_xy(lp);
            push_event(Event::Snap(SnapInput::Press { x, y }));
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
            push_event(Event::Snap(SnapInput::Cancel));
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            let vk = wp.0 as u16;
            let input = if vk == VK_ESCAPE.0 {
                Some(SnapInput::Cancel)
            } else {
                let (dx, dy) = match vk {
                    v if v == VK_LEFT.0 => (-1, 0),
                    v if v == VK_RIGHT.0 => (1, 0),
                    v if v == VK_UP.0 => (0, -1),
                    v if v == VK_DOWN.0 => (0, 1),
                    _ => (0, 0),
                };
                (dx, dy).ne(&(0, 0)).then(|| SnapInput::Arrow {
                    dx,
                    dy,
                    big: shift_down(),
                })
            };
            if let Some(input) = input {
                push_event(Event::Snap(input));
                return LRESULT(0);
            }
        }
        // The overlay spans monitors with different DPIs on purpose: never resize it.
        WM_DPICHANGED => return LRESULT(0),
        _ => {}
    }
    // SAFETY: forwards the unhandled message with the original arguments.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}
