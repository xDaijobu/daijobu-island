//! The windows a click in the overlay can snap to, top of the z-order first:
//! visible, not minimised, not cloaked (other virtual desktops, suspended UWP apps),
//! not tool windows, not click-through, not ours, and not the desktop itself (a click
//! there takes the monitor instead). Sizes come from `DWMWA_EXTENDED_FRAME_BOUNDS`, so
//! the invisible resize border isn't included.

use dj_core::snap::IRect;
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, GWL_EXSTYLE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

/// Desktop and shell background windows: never a snap target.
const SKIP_CLASSES: [&str; 2] = ["Progman", "WorkerW"];

struct Walk {
    own_pid: u32,
    rects: Vec<IRect>,
}

/// Window rects in physical desktop px, top to bottom.
pub fn window_rects() -> Vec<IRect> {
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `&mut Walk` passed below and outlives the enumeration;
        // `hwnd` is valid for the duration of the callback.
        unsafe {
            let walk = &mut *(data.0 as *mut Walk);
            if let Some(r) = target(hwnd, walk.own_pid) {
                walk.rects.push(r);
            }
        }
        true.into()
    }
    let mut walk = Walk {
        // SAFETY: no arguments.
        own_pid: unsafe { GetCurrentProcessId() },
        rects: Vec::new(),
    };
    // SAFETY: the callback only touches `walk`, which outlives the call.
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut walk as *mut Walk as isize));
    }
    walk.rects
}

/// The window's snap rect, or None if it is not a target.
///
/// # Safety
/// `hwnd` must be a window handle from `EnumWindows`.
unsafe fn target(hwnd: HWND, own_pid: u32) -> Option<IRect> {
    // SAFETY: plain queries on a live handle with correctly sized out-buffers.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex & (WS_EX_TOOLWINDOW.0 | WS_EX_TRANSPARENT.0) != 0 {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == own_pid {
            return None;
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
        {
            return None;
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        if SKIP_CLASSES.contains(&class.as_str()) {
            return None;
        }
        let mut rc = RECT::default();
        let framed = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rc as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        )
        .is_ok();
        if !framed && GetWindowRect(hwnd, &mut rc).is_err() {
            return None;
        }
        let r = IRect::new(rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top);
        (!r.is_empty()).then_some(r)
    }
}
