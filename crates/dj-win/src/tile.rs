//! Win32 side of Tile (window zones): the move hook, window queries and placement,
//! monitors, the shift key and the zone overlay window.
//!
//! This module only wraps system calls. Every decision (which zone, which layout, when to
//! restore a size) lives in `dj_core::tile` and the app's controller. The pure parts, the
//! event translation, the eligibility rules, the error mapping and the overlay's message
//! answers, are plain functions with tests; nothing here is exercised against a real window
//! in tests.
//!
//! Rectangles in this module's API are `(x, y, w, h)` in physical desktop px (`x` and `y`
//! can be negative on a monitor left of or above the primary one), the same order as
//! `dj_core::snap::IRect::new`. They are NOT `(left, top, right, bottom)`.

use crate::window::{push_event, Event};
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::core::{w, Result, BOOL, PCWSTR};
use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_WINDOW_HANDLE, HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// A window handle as a plain value (`HWND.0 as isize`), so the app and `dj-core` never
/// need the `windows` types.
pub type Raw = isize;

fn hwnd_of(w: Raw) -> HWND {
    HWND(w as *mut _)
}

// ---- the move hook ------------------------------------------------------------------

/// A window entered or left its move/size modal loop. A resize from an edge or a corner
/// raises the same pair as a move, so the app tells them apart by comparing the window's
/// size at `Start` and at `End`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveEvent {
    Start(Raw),
    End(Raw),
}

/// The installed hook, or 0. Only the UI thread installs and removes it.
static MOVE_HOOK: AtomicIsize = AtomicIsize::new(0);

/// Maps a WinEvent to a [`MoveEvent`]. A null window or any other event gives nothing.
fn translate_move(event: u32, hwnd: Raw) -> Option<MoveEvent> {
    if hwnd == 0 {
        return None;
    }
    match event {
        EVENT_SYSTEM_MOVESIZESTART => Some(MoveEvent::Start(hwnd)),
        EVENT_SYSTEM_MOVESIZEEND => Some(MoveEvent::End(hwnd)),
        _ => None,
    }
}

// SAFETY: only ever invoked by the system as a WINEVENTPROC, on the thread that installed
// the hook (out-of-context hooks are delivered through that thread's message loop). It
// touches only that thread's event queue and posts a thread message.
unsafe extern "system" fn move_changed(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    let Some(ev) = translate_move(event, hwnd.0 as Raw) else {
        return;
    };
    push_event(Event::TileMove(ev));
    // The system runs this callback from inside `GetMessageW`/`PeekMessageW`, which may
    // keep waiting afterwards. A thread message with no window makes the call return, so
    // the loop drains the queue now instead of at the next unrelated message. Dispatching
    // a window-less `WM_NULL` does nothing.
    // SAFETY: no pointers involved; the target is this very thread.
    unsafe {
        let _ = PostThreadMessageW(GetCurrentThreadId(), WM_NULL, WPARAM(0), LPARAM(0));
    }
}

/// Starts listening for window moves and resizes of other processes: one out-of-context
/// WinEvent hook for `EVENT_SYSTEM_MOVESIZESTART..=END`, delivered as
/// [`Event::TileMove`]. Call it on the UI thread; the hook needs that thread's message
/// loop. Idempotent. Returns false if the system refused the hook.
pub fn install_move_hook() -> bool {
    if MOVE_HOOK.load(Ordering::Relaxed) != 0 {
        return true;
    }
    // SAFETY: the callback is a plain function that only touches this thread's queue;
    // out-of-context hooks are delivered through this thread's message loop.
    let hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_MOVESIZESTART,
            EVENT_SYSTEM_MOVESIZEEND,
            Some(HMODULE::default()),
            Some(move_changed),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    };
    if hook.0.is_null() {
        return false;
    }
    MOVE_HOOK.store(hook.0 as isize, Ordering::Relaxed);
    true
}

/// Removes the hook made by [`install_move_hook`]. Does nothing if there is none.
pub fn uninstall_move_hook() {
    let raw = MOVE_HOOK.swap(0, Ordering::Relaxed);
    if raw != 0 {
        // SAFETY: `raw` is the handle `SetWinEventHook` returned and it is unhooked once.
        unsafe {
            let _ = UnhookWinEvent(HWINEVENTHOOK(raw as *mut _));
        }
    }
}

// ---- window queries -----------------------------------------------------------------

/// What the app needs to know about a window before it touches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WinInfo {
    /// `GetWindowRect`, `(x, y, w, h)` physical desktop px: the rect `SetWindowPos` uses,
    /// including the invisible resize border.
    pub rect: (i32, i32, i32, i32),
    /// The visible frame, `DWMWA_EXTENDED_FRAME_BOUNDS`, `(x, y, w, h)`. The same as
    /// `rect` when DWM can't tell.
    pub frame: (i32, i32, i32, i32),
    pub maximized: bool,
    /// A normal top-level window the app may move (see [`classify`]).
    pub eligible: bool,
}

/// A `RECT` as `(x, y, w, h)`.
fn xywh(rc: RECT) -> (i32, i32, i32, i32) {
    (rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top)
}

/// Shell windows that are never a target, matched by class name: the desktop and its
/// worker windows and the primary and secondary taskbars.
fn is_shell_class(class: &str) -> bool {
    matches!(
        class,
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    )
}

/// Whether a window is a normal top-level window Tile may move: visible, not minimised,
/// not cloaked (another virtual desktop, a suspended UWP app), a root window, not ours, not
/// a tool window, and with a title bar (`WS_CAPTION`, both of its bits) or a sizing border
/// (`WS_THICKFRAME`). The shell's own windows are excluded separately by class name, see
/// [`is_shell_class`].
fn classify(
    style: u32,
    ex_style: u32,
    visible: bool,
    iconic: bool,
    cloaked: bool,
    own_process: bool,
    is_root: bool,
) -> bool {
    visible
        && !iconic
        && !cloaked
        && !own_process
        && is_root
        && ex_style & WS_EX_TOOLWINDOW.0 == 0
        && (style & WS_CAPTION.0 == WS_CAPTION.0 || style & WS_THICKFRAME.0 != 0)
}

/// Rect, visible frame, maximized state and eligibility of a window. `None` if the handle
/// is not a window (any more) or its rect can't be read.
pub fn window_info(w: Raw) -> Option<WinInfo> {
    if w == 0 {
        return None;
    }
    let hwnd = hwnd_of(w);
    // SAFETY: plain queries on a handle that may be stale (a stale one fails the calls
    // and gives `None`), with correctly sized out-buffers.
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return None;
        }
        let mut rc = RECT::default();
        GetWindowRect(hwnd, &mut rc).ok()?;
        let rect = xywh(rc);

        let mut fr = RECT::default();
        let framed = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut fr as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        )
        .is_ok();
        let frame = if framed { xywh(fr) } else { rect };

        let mut cloaked = 0u32;
        let cloaked = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0;

        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let own_process = pid == GetCurrentProcessId();

        let mut class = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..n]);

        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let eligible = classify(
            style,
            ex_style,
            IsWindowVisible(hwnd).as_bool(),
            IsIconic(hwnd).as_bool(),
            cloaked,
            own_process,
            GetAncestor(hwnd, GA_ROOT) == hwnd,
        ) && !is_shell_class(&class);

        Some(WinInfo {
            rect,
            frame,
            maximized: IsZoomed(hwnd).as_bool(),
            eligible,
        })
    }
}

/// The foreground window, or 0 if there is none.
pub fn foreground() -> Raw {
    // SAFETY: no pointers involved.
    unsafe { GetForegroundWindow() }.0 as Raw
}

// ---- placement ----------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaceError {
    /// The window belongs to a process with a higher integrity level (an elevated app);
    /// UIPI refuses to let a normal process move it.
    AccessDenied,
    /// The window closed or the handle was never valid.
    Gone,
    Other(String),
}

/// The Win32 error code inside an `HRESULT` from `HRESULT_FROM_WIN32`, else the value
/// itself reinterpreted as unsigned.
fn win32_code(hresult: i32) -> u32 {
    let h = hresult as u32;
    if h >> 16 == 0x8007 {
        h & 0xFFFF
    } else {
        h
    }
}

/// Maps a failed `SetWindowPos` to a [`PlaceError`]. `hresult` is the error's `HRESULT`;
/// `exists` is whether the handle is still a window. A window that is gone wins over any
/// other reason, so a close mid-placement never reads as a UIPI refusal.
fn place_error(hresult: i32, exists: bool) -> PlaceError {
    let code = win32_code(hresult);
    if !exists || code == ERROR_INVALID_WINDOW_HANDLE.0 {
        PlaceError::Gone
    } else if code == ERROR_ACCESS_DENIED.0 {
        PlaceError::AccessDenied
    } else {
        PlaceError::Other(format!("SetWindowPos failed (error {code})"))
    }
}

/// Moves and sizes the window's rect (`GetWindowRect` space, so including its invisible
/// border) to `(x, y, cx, cy)` physical desktop px, without changing its z-order or
/// activating it. A maximized window is restored first, because a maximized window keeps
/// its size otherwise.
pub fn place(w: Raw, x: i32, y: i32, cx: i32, cy: i32) -> std::result::Result<(), PlaceError> {
    if w == 0 {
        return Err(PlaceError::Gone);
    }
    let hwnd = hwnd_of(w);
    // SAFETY: plain calls on a handle that may be stale; a stale handle makes them fail,
    // which is mapped to `Gone`. No pointers are passed.
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return Err(PlaceError::Gone);
        }
        if IsZoomed(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        match SetWindowPos(hwnd, None, x, y, cx, cy, SWP_NOZORDER | SWP_NOACTIVATE) {
            Ok(()) => Ok(()),
            Err(e) => Err(place_error(e.code().0, IsWindow(Some(hwnd)).as_bool())),
        }
    }
}

// ---- monitors and keys --------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorInfo {
    /// The GDI device name, e.g. `\\.\DISPLAY1`: the key of `[tile.monitors]`.
    pub device: String,
    /// The monitor, `(x, y, w, h)` physical desktop px.
    pub rect: (i32, i32, i32, i32),
    /// The work area (the monitor minus the taskbar and AppBars), `(x, y, w, h)`.
    pub work: (i32, i32, i32, i32),
    /// The effective DPI (96 is 100 %), at least 96.
    pub dpi: u32,
}

/// A NUL-terminated UTF-16 buffer as a string (the whole buffer if it has no NUL).
fn device_name(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// The primary monitor first, the rest in the order given.
fn primary_first(items: Vec<(bool, MonitorInfo)>) -> Vec<MonitorInfo> {
    let (primary, rest): (Vec<_>, Vec<_>) = items.into_iter().partition(|(p, _)| *p);
    primary.into_iter().chain(rest).map(|(_, m)| m).collect()
}

/// One monitor's facts, and whether it is the primary one.
///
/// # Safety
/// `hmon` must be a monitor handle from `EnumDisplayMonitors`.
unsafe fn query_monitor(hmon: HMONITOR) -> Option<(bool, MonitorInfo)> {
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: `info` is a fully initialised MONITORINFOEXW whose `cbSize` says so; the
    // cast points at its first field, which is the MONITORINFO the call fills.
    unsafe {
        if !GetMonitorInfoW(
            hmon,
            (&mut info as *mut MONITORINFOEXW).cast::<MONITORINFO>(),
        )
        .as_bool()
        {
            return None;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let base = info.monitorInfo;
        Some((
            base.dwFlags & MONITORINFOF_PRIMARY != 0,
            MonitorInfo {
                device: device_name(&info.szDevice),
                rect: xywh(base.rcMonitor),
                work: xywh(base.rcWork),
                dpi: dx.max(96),
            },
        ))
    }
}

/// Every attached monitor: the primary first, then the order `EnumDisplayMonitors` gives.
pub fn monitors() -> Vec<MonitorInfo> {
    unsafe extern "system" fn each(
        hmon: HMONITOR,
        _hdc: HDC,
        _rc: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: `data` is the `&mut Vec` passed below and outlives the enumeration;
        // `hmon` is valid for the duration of the callback.
        unsafe {
            let items = &mut *(data.0 as *mut Vec<(bool, MonitorInfo)>);
            if let Some(item) = query_monitor(hmon) {
                items.push(item);
            }
        }
        true.into()
    }
    let mut items: Vec<(bool, MonitorInfo)> = Vec::new();
    // SAFETY: the callback only touches `items`, which outlives the call.
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(each),
            LPARAM(&mut items as *mut Vec<(bool, MonitorInfo)> as isize),
        );
    }
    primary_first(items)
}

/// The high bit of a `GetAsyncKeyState` result: the key is down right now.
fn key_is_down(state: i16) -> bool {
    state < 0
}

/// Either Shift key is down right now (the physical state, whichever window has focus).
pub fn shift_down() -> bool {
    // SAFETY: no pointers involved.
    key_is_down(unsafe { GetAsyncKeyState(i32::from(VK_SHIFT.0)) })
}

/// The primary mouse button is held right now. `GetAsyncKeyState` reads the physical
/// buttons, so with swapped buttons the primary one is the right one.
pub fn primary_button_down() -> bool {
    // SAFETY: no pointers involved.
    unsafe {
        let vk = if GetSystemMetrics(SM_SWAPBUTTON) != 0 {
            VK_RBUTTON
        } else {
            VK_LBUTTON
        };
        key_is_down(GetAsyncKeyState(i32::from(vk.0)))
    }
}

/// `WM_NCHITTEST` results that mean a sizing border (or the size grip): a drag from there
/// resizes the window instead of moving it.
fn is_sizing_hit(code: isize) -> bool {
    matches!(code as u32, HTSIZE | HTLEFT..=HTBOTTOMRIGHT)
}

/// Whether a move/size that just started at the desktop point `(x, y)` is a resize: the
/// window's own hit test there is a sizing border. A window that doesn't answer in 50 ms
/// (hung, or out of reach behind UIPI) counts as a move.
pub fn is_resize_at(w: Raw, x: i32, y: i32) -> bool {
    if w == 0 {
        return false;
    }
    let lp = (x as u16 as u32 | (y as u16 as u32) << 16) as i32 as isize;
    let mut result = 0usize;
    // SAFETY: a message with no pointers in its parameters; `result` is a valid out
    // pointer for the duration of the call, which returns within the timeout.
    let ok = unsafe {
        SendMessageTimeoutW(
            hwnd_of(w),
            WM_NCHITTEST,
            WPARAM(0),
            LPARAM(lp),
            SMTO_ABORTIFHUNG,
            50,
            Some(&mut result),
        )
    };
    ok.0 != 0 && is_sizing_hit(result as isize)
}

/// The window's thread is still in a move/size loop (`GUI_INMOVESIZE`). Used as a
/// watchdog in case the end event never comes, so it only says no when it knows: a query
/// that fails counts as still moving; a window that is gone is not.
pub fn in_move_size(w: Raw) -> bool {
    if w == 0 {
        return false;
    }
    // SAFETY: a handle that may be stale (then the thread id is 0); `info` is a valid,
    // sized out structure for the duration of the call.
    unsafe {
        let thread = GetWindowThreadProcessId(hwnd_of(w), None);
        if thread == 0 {
            return false;
        }
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        GetGUIThreadInfo(thread, &mut info).is_err() || (info.flags & GUI_INMOVESIZE).0 != 0
    }
}

/// Restores a maximized window to its normal size (before measuring it for a zone).
pub fn restore(w: Raw) {
    if w == 0 {
        return;
    }
    // SAFETY: a plain call on a handle that may be stale (then it does nothing).
    unsafe {
        if IsZoomed(hwnd_of(w)).as_bool() {
            let _ = ShowWindow(hwnd_of(w), SW_RESTORE);
        }
    }
}

// ---- the zone overlay window --------------------------------------------------------

const OVERLAY_CLASS: PCWSTR = w!("DaijobuTileZones");

/// Topmost, never activated, out of Alt-Tab, no GDI surface (DirectComposition draws it)
/// and click-through.
fn overlay_ex_style() -> WINDOW_EX_STYLE {
    WS_EX_NOREDIRECTIONBITMAP
        | WS_EX_TOOLWINDOW
        | WS_EX_TOPMOST
        | WS_EX_NOACTIVATE
        | WS_EX_TRANSPARENT
}

/// The overlay's own answer to a message, or `None` to leave it to `DefWindowProc`.
fn overlay_answer(msg: u32) -> Option<isize> {
    match msg {
        // Input falls through to whatever is below.
        WM_NCHITTEST => Some(HTTRANSPARENT as isize),
        // Never take the foreground from the window being dragged.
        WM_MOUSEACTIVATE => Some(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => Some(1),
        // The app places and sizes the window itself: never resize it for a DPI change.
        WM_DPICHANGED => Some(0),
        // The app owns the window's lifetime.
        WM_CLOSE => Some(0),
        _ => None,
    }
}

extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(answer) = overlay_answer(msg) {
        return LRESULT(answer);
    }
    // SAFETY: forwards the unhandled message with the original arguments.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Creates the zone overlay, hidden, at `(x, y, w, h)` physical desktop px (one monitor).
/// Draw its first frame, then [`show_overlay`], so nothing flashes.
pub fn create_overlay(x: i32, y: i32, w: i32, h: i32) -> Result<HWND> {
    // SAFETY: standard class registration and window creation; all pointers are valid for
    // the duration of the calls and the class name is a static string. Registering the
    // class again on a later drag fails harmlessly.
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(overlay_proc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: OVERLAY_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        CreateWindowExW(
            overlay_ex_style(),
            OVERLAY_CLASS,
            w!("Daijobu Island Tile zones"),
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

/// Shows the overlay on top of other windows without activating it. It lands above the
/// island too; see [`show_overlay_below`] to keep the island's panel on top.
pub fn show_overlay(hwnd: HWND) {
    // SAFETY: our own window, on the thread that owns it. Nothing here activates it.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// Like [`show_overlay`], but puts the overlay just below `island` in the z-order (both
/// topmost), so the island's Tile panel is not covered by zones that reach under it.
pub fn show_overlay_below(hwnd: HWND, island: HWND) {
    // SAFETY: our own windows, on the thread that owns them. Nothing here activates
    // either one.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(
            hwnd,
            Some(island),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

pub fn hide_overlay(hwnd: HWND) {
    // SAFETY: our own window, on the thread that owns it.
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

/// Destroys the overlay (on the thread that created it).
pub fn destroy_overlay(hwnd: HWND) {
    // SAFETY: our own window, destroyed on the thread that created it.
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A normal framed app window: visible, on screen, root, not ours.
    const CAPTION: u32 = WS_CAPTION.0;
    const THICK: u32 = WS_THICKFRAME.0;

    fn eligible(style: u32, ex: u32) -> bool {
        classify(style, ex, true, false, false, false, true)
    }

    #[test]
    fn hook_events_translate_to_move_events() {
        assert_eq!(
            translate_move(EVENT_SYSTEM_MOVESIZESTART, 0x1234),
            Some(MoveEvent::Start(0x1234))
        );
        assert_eq!(
            translate_move(EVENT_SYSTEM_MOVESIZEEND, 0x1234),
            Some(MoveEvent::End(0x1234))
        );
        // Another event, or no window, is nothing.
        assert_eq!(translate_move(EVENT_SYSTEM_FOREGROUND, 0x1234), None);
        assert_eq!(translate_move(EVENT_SYSTEM_MOVESIZESTART, 0), None);
        assert_eq!(translate_move(EVENT_SYSTEM_MOVESIZEEND, 0), None);
        // A handle in the upper half of the address space survives the round trip.
        let h = hwnd_of(-8);
        assert_eq!(
            translate_move(EVENT_SYSTEM_MOVESIZEEND, h.0 as Raw),
            Some(MoveEvent::End(-8))
        );
    }

    #[test]
    fn a_normal_framed_window_is_eligible() {
        assert!(eligible(CAPTION, 0));
        assert!(eligible(CAPTION | THICK, 0));
        assert!(eligible(CAPTION | THICK | WS_VISIBLE.0, WS_EX_APPWINDOW.0));
    }

    #[test]
    fn classify_rejects_each_rule() {
        let c = |visible, iconic, cloaked, own, root| {
            classify(CAPTION | THICK, 0, visible, iconic, cloaked, own, root)
        };
        assert!(c(true, false, false, false, true));
        assert!(!c(false, false, false, false, true), "hidden");
        assert!(!c(true, true, false, false, true), "minimised");
        assert!(!c(true, false, true, false, true), "cloaked");
        assert!(!c(true, false, false, true, true), "our own window");
        assert!(!c(true, false, false, false, false), "not a root window");
    }

    #[test]
    fn tool_windows_are_not_eligible() {
        assert!(!eligible(CAPTION | THICK, WS_EX_TOOLWINDOW.0));
        // Other extended styles don't matter.
        assert!(eligible(CAPTION, WS_EX_TOPMOST.0 | WS_EX_ACCEPTFILES.0));
        assert!(!eligible(
            CAPTION,
            WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_ACCEPTFILES.0
        ));
    }

    #[test]
    fn a_window_needs_a_title_bar_or_a_sizing_border() {
        // A caption alone (a fixed-size dialog) and a sizing border alone (a frameless
        // app's resizable window) are both fine.
        assert!(eligible(CAPTION, 0));
        assert!(eligible(THICK, 0));
        // Neither: a popup, a splash screen, a tooltip.
        assert!(!eligible(0, 0));
        assert!(!eligible(WS_POPUP.0, 0));
        assert!(!eligible(WS_VISIBLE.0 | WS_POPUP.0, 0));
        // WS_CAPTION is two bits; one of them is not a title bar.
        assert!(!eligible(WS_BORDER.0, 0));
        assert!(!eligible(WS_DLGFRAME.0, 0));
        assert!(eligible(WS_BORDER.0 | WS_DLGFRAME.0, 0));
    }

    #[test]
    fn shell_classes_are_never_targets() {
        for class in [
            "Progman",
            "WorkerW",
            "Shell_TrayWnd",
            "Shell_SecondaryTrayWnd",
        ] {
            assert!(is_shell_class(class), "{class}");
        }
        for class in [
            "",
            "Chrome_WidgetWin_1",
            "CabinetWClass",
            "Notepad",
            "progman",
            "Shell_TrayWnd2",
            "DaijobuIslandWindow",
        ] {
            assert!(!is_shell_class(class), "{class}");
        }
    }

    #[test]
    fn rects_convert_to_x_y_w_h() {
        let rc = |l, t, r, b| RECT {
            left: l,
            top: t,
            right: r,
            bottom: b,
        };
        assert_eq!(xywh(rc(0, 0, 1920, 1080)), (0, 0, 1920, 1080));
        // A monitor left of the primary one starts at a negative x.
        assert_eq!(xywh(rc(-1920, 0, 0, 1080)), (-1920, 0, 1920, 1080));
        assert_eq!(xywh(rc(-1920, -300, -100, 700)), (-1920, -300, 1820, 1000));
        assert_eq!(xywh(rc(5, 5, 5, 5)), (5, 5, 0, 0));
    }

    #[test]
    fn win32_errors_map_to_place_errors() {
        // 0x80070005 is HRESULT_FROM_WIN32(ERROR_ACCESS_DENIED): UIPI.
        assert_eq!(
            place_error(0x8007_0005_u32 as i32, true),
            PlaceError::AccessDenied
        );
        // 0x80070578 is HRESULT_FROM_WIN32(ERROR_INVALID_WINDOW_HANDLE).
        assert_eq!(place_error(0x8007_0578_u32 as i32, true), PlaceError::Gone);
        // A window that no longer exists is gone, whatever the code said.
        assert_eq!(place_error(0x8007_0005_u32 as i32, false), PlaceError::Gone);
        assert_eq!(place_error(0x8007_0057_u32 as i32, false), PlaceError::Gone);
        // Anything else keeps its code for the log.
        assert_eq!(
            place_error(0x8007_0057_u32 as i32, true),
            PlaceError::Other("SetWindowPos failed (error 87)".to_string())
        );
        // An HRESULT that isn't a Win32 wrapper is reported as is.
        assert_eq!(
            place_error(0x8000_4005_u32 as i32, true),
            PlaceError::Other(format!("SetWindowPos failed (error {})", 0x8000_4005_u32))
        );
    }

    #[test]
    fn win32_code_unwraps_the_hresult() {
        assert_eq!(win32_code(0x8007_0005_u32 as i32), 5);
        assert_eq!(win32_code(0x8007_0578_u32 as i32), 1400);
        assert_eq!(win32_code(5), 5);
        assert_eq!(win32_code(0), 0);
    }

    #[test]
    fn a_missing_window_is_gone_without_touching_anything() {
        assert_eq!(window_info(0), None);
        assert_eq!(place(0, 0, 0, 100, 100), Err(PlaceError::Gone));
    }

    #[test]
    fn device_names_stop_at_the_nul() {
        let mut buf = [0u16; 32];
        for (i, c) in r"\\.\DISPLAY2".encode_utf16().enumerate() {
            buf[i] = c;
        }
        assert_eq!(device_name(&buf), r"\\.\DISPLAY2");
        assert_eq!(device_name(&[0u16; 32]), "");
        // No NUL at all: the whole buffer.
        let full: Vec<u16> = "ABC".encode_utf16().collect();
        assert_eq!(device_name(&full), "ABC");
    }

    fn mon(name: &str) -> MonitorInfo {
        MonitorInfo {
            device: name.to_string(),
            rect: (0, 0, 1920, 1080),
            work: (0, 0, 1920, 1032),
            dpi: 96,
        }
    }

    #[test]
    fn the_primary_monitor_comes_first() {
        let names = |v: Vec<MonitorInfo>| v.into_iter().map(|m| m.device).collect::<Vec<_>>();
        let items = vec![
            (false, mon("A")),
            (false, mon("B")),
            (true, mon("P")),
            (false, mon("C")),
        ];
        assert_eq!(names(primary_first(items)), ["P", "A", "B", "C"]);
        // Already first, and a lone monitor, and none.
        let items = vec![(true, mon("P")), (false, mon("A"))];
        assert_eq!(names(primary_first(items)), ["P", "A"]);
        assert_eq!(names(primary_first(vec![(true, mon("P"))])), ["P"]);
        assert!(primary_first(Vec::new()).is_empty());
    }

    #[test]
    fn monitors_report_sane_geometry() {
        // A read-only query; a machine with no display gives an empty list.
        let all = monitors();
        for m in &all {
            assert!(m.dpi >= 96, "{m:?}");
            assert!(m.rect.2 > 0 && m.rect.3 > 0, "{m:?}");
            // The work area sits inside the monitor.
            let (x, y, w, h) = m.rect;
            let (wx, wy, ww, wh) = m.work;
            assert!(
                wx >= x && wy >= y && wx + ww <= x + w && wy + wh <= y + h,
                "{m:?}"
            );
        }
    }

    #[test]
    fn the_high_bit_means_the_key_is_down() {
        assert!(key_is_down(i16::MIN));
        assert!(key_is_down(-1));
        // The low bit is "pressed since the last call", not "down".
        assert!(!key_is_down(1));
        assert!(!key_is_down(0));
    }

    #[test]
    fn only_sizing_borders_and_the_grip_count_as_a_resize() {
        for code in [
            HTLEFT,
            HTRIGHT,
            HTTOP,
            HTTOPLEFT,
            HTTOPRIGHT,
            HTBOTTOM,
            HTBOTTOMLEFT,
            HTBOTTOMRIGHT,
            HTSIZE,
        ] {
            assert!(is_sizing_hit(code as isize), "{code}");
        }
        for code in [
            HTCAPTION,
            HTCLIENT,
            HTSYSMENU,
            HTNOWHERE,
            HTTRANSPARENT as u32,
        ] {
            assert!(!is_sizing_hit(code as isize), "{code}");
        }
        assert!(!is_resize_at(0, 10, 10), "no window");
        assert!(!in_move_size(0));
    }

    #[test]
    fn the_overlay_is_click_through_and_never_activated() {
        let ex = overlay_ex_style();
        for flag in [
            WS_EX_NOREDIRECTIONBITMAP,
            WS_EX_TOOLWINDOW,
            WS_EX_TOPMOST,
            WS_EX_NOACTIVATE,
            WS_EX_TRANSPARENT,
        ] {
            assert_eq!(ex & flag, flag, "{flag:?}");
        }
        assert_eq!(overlay_answer(WM_NCHITTEST), Some(-1));
        assert_eq!(overlay_answer(WM_NCHITTEST), Some(HTTRANSPARENT as isize));
        assert_eq!(
            overlay_answer(WM_MOUSEACTIVATE),
            Some(MA_NOACTIVATE as isize)
        );
        assert_eq!(overlay_answer(WM_ERASEBKGND), Some(1));
        assert_eq!(overlay_answer(WM_DPICHANGED), Some(0));
        assert_eq!(overlay_answer(WM_CLOSE), Some(0));
        // Everything else is the default procedure's.
        for msg in [
            WM_PAINT,
            WM_MOUSEMOVE,
            WM_LBUTTONDOWN,
            WM_SETCURSOR,
            WM_SHOWWINDOW,
        ] {
            assert_eq!(overlay_answer(msg), None, "{msg:#x}");
        }
    }
}
