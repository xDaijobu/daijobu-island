//! Reserves a strip along the top of the primary monitor as an AppBar
//! (`SHAppBarMessage`), so maximized and snapped windows start below the notch instead
//! of under it. Nothing is drawn in the strip; the island window stays as it is.
//!
//! Windows only reserves whole edges, so the strip is as wide as the monitor. The shell
//! tells the window about changes with [`WM_APPBAR`]; on `ABN_POSCHANGED` the app sets
//! the strip again. The strip must be removed before the app exits, or the work area
//! stays short until Explorer notices the window is gone.

use crate::window::{push_event, Event};
use std::cell::Cell;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::UI::Shell::{
    SHAppBarMessage, ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, ABN_POSCHANGED,
    APPBARDATA,
};
use windows::Win32::UI::WindowsAndMessaging::WM_APP;

/// The shell's notifications for the AppBar (`wparam` is the `ABN_*` code).
pub const WM_APPBAR: u32 = WM_APP + 10;

thread_local! {
    /// Registered with the shell (`ABM_NEW` done, no `ABM_REMOVE` since).
    static REGISTERED: Cell<bool> = const { Cell::new(false) };
}

fn data(hwnd: HWND) -> APPBARDATA {
    APPBARDATA {
        cbSize: size_of::<APPBARDATA>() as u32,
        hWnd: hwnd,
        uCallbackMessage: WM_APPBAR,
        uEdge: ABE_TOP,
        rc: RECT::default(),
        lParam: LPARAM(0),
    }
}

/// Reserves `(left, top, right, bottom)` (physical desktop px) along the top edge,
/// registering the window first if needed. Returns the strip the shell granted, which
/// can sit lower than asked (e.g. under a taskbar docked at the top), or `None` if the
/// shell refused the registration.
pub fn reserve(hwnd: HWND, (l, t, r, b): (i32, i32, i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let mut abd = data(hwnd);
    // SAFETY: `abd` is fully initialised and outlives each call; the shell only reads
    // and writes the struct during the call. `hwnd` is our live top-level window.
    unsafe {
        if !REGISTERED.with(Cell::get) {
            if SHAppBarMessage(ABM_NEW, &mut abd) == 0 {
                return None;
            }
            REGISTERED.with(|c| c.set(true));
        }
        abd.rc = RECT {
            left: l,
            top: t,
            right: r,
            bottom: b,
        };
        // The shell may move the top edge down past other top bars; keep our height.
        SHAppBarMessage(ABM_QUERYPOS, &mut abd);
        abd.rc.bottom = abd.rc.top + (b - t);
        SHAppBarMessage(ABM_SETPOS, &mut abd);
    }
    let rc = abd.rc;
    Some((rc.left, rc.top, rc.right, rc.bottom))
}

/// Gives the strip back. Harmless when nothing is reserved.
pub fn release(hwnd: HWND) {
    if !REGISTERED.with(Cell::get) {
        return;
    }
    let mut abd = data(hwnd);
    // SAFETY: as in `reserve`; removing an AppBar only touches the struct during the call.
    unsafe {
        SHAppBarMessage(ABM_REMOVE, &mut abd);
    }
    REGISTERED.with(|c| c.set(false));
}

/// Explorer restarted: its AppBar list is gone, so the next [`reserve`] must register
/// again (without an `ABM_REMOVE` to a shell that never knew us).
pub fn forget() {
    REGISTERED.with(|c| c.set(false));
}

/// Called by the window procedure for [`WM_APPBAR`].
pub(crate) fn notify(code: usize) {
    if code == ABN_POSCHANGED as usize {
        push_event(Event::AppBarMoved);
    }
}
