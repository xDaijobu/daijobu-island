//! The Snap preview stack's native companion window: a topmost, click-through-by-region
//! `WS_POPUP` that sits below the island and hosts the stack's DirectComposition surface.
//!
//! Unlike the Snap overlay ([`crate::overlay`]) it must never take activation on its own:
//! it appears while the user works in another app, so it is `WS_EX_NOACTIVATE`, has no
//! taskbar or Alt-Tab entry and answers `WM_MOUSEACTIVATE` with `MA_NOACTIVATE`. Keyboard
//! access is deliberate: only [`focus`] (called from the tray command) takes the foreground.
//!
//! Like the other windows, the window procedure never calls into the app. Its messages
//! become [`Event::Previews`] on the shared queue and the app handles them outside any
//! Win32 callback. Positions are client px (physical) and signed, because a captured
//! drag can leave the window on any side.

use crate::window::{push_event, Event};
use std::cell::Cell;
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, GetMonitorInfoW, MonitorFromPoint,
    SetWindowRgn, HGDIOBJ, HRGN, MONITORINFO, MONITOR_DEFAULTTONEAREST, RGN_OR,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE,
    TRACKMOUSEEVENT, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: PCWSTR = w!("DaijobuSnapPreviews");
/// Not exported by the windows crate.
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Input on the preview window. Positions are client px (physical); add the window's
/// origin for desktop coordinates. The app turns these into hover, press and key
/// handling for the stack (see `dj_core::snap_stack`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewInput {
    /// The pointer moved over (or, while captured, away from) the window.
    Move { x: i32, y: i32 },
    /// Left button pressed. The window captures the pointer until the release.
    Press { x: i32, y: i32 },
    /// Left button released.
    Release { x: i32, y: i32 },
    /// The pointer left the window (`WM_MOUSELEAVE`), or the pointer capture was lost to
    /// something else, or the system cancelled the mouse mode (`WM_CANCELMODE`). Treat it
    /// as "no hover and no press in progress", so a lost mouse-up never leaves a press stuck.
    Leave,
    /// A key went down while the window has the keyboard (see [`focus`]). `ctrl` is false
    /// while Alt is held too, so AltGr (Ctrl+Alt) characters never read as Ctrl shortcuts.
    Key { vk: u16, ctrl: bool, shift: bool },
    /// The keyboard focus or the activation went away (`WM_KILLFOCUS`, or `WM_ACTIVATE`
    /// with `WA_INACTIVE`). One deactivation can send both, so handle it idempotently.
    FocusLost,
}

thread_local! {
    /// A `WM_MOUSELEAVE` is armed. Separate from the island window's own flag.
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    /// We are releasing the pointer capture ourselves, so the `WM_CAPTURECHANGED` it
    /// sends is not a loss to report.
    static RELEASING: Cell<bool> = const { Cell::new(false) };
}

// ---- pure helpers: message translation and argument preparation ---------------------

/// Signed client coordinates from a mouse message's `lparam` (low word x, high word y,
/// both signed so a captured pointer left or above the window reads negative).
fn lparam_xy(lp: isize) -> (i32, i32) {
    let v = lp as u32;
    (
        i32::from(v as u16 as i16),
        i32::from((v >> 16) as u16 as i16),
    )
}

/// The `Key` input for a `WM_KEYDOWN`. Ctrl only counts while Alt is up.
fn key_input(wp: usize, ctrl_down: bool, alt_down: bool, shift_down: bool) -> PreviewInput {
    PreviewInput::Key {
        vk: wp as u16,
        ctrl: ctrl_down && !alt_down,
        shift: shift_down,
    }
}

/// Maps the messages that need no side effect (and no key state) to their input.
/// `WM_CAPTURECHANGED`, `WM_KEYDOWN` and `WM_MOUSEMOVE`'s tracking are handled by the
/// window procedure itself.
fn translate(msg: u32, wp: usize, lp: isize) -> Option<PreviewInput> {
    let (x, y) = lparam_xy(lp);
    match msg {
        WM_LBUTTONDOWN => Some(PreviewInput::Press { x, y }),
        WM_LBUTTONUP => Some(PreviewInput::Release { x, y }),
        WM_MOUSEMOVE => Some(PreviewInput::Move { x, y }),
        WM_MOUSELEAVE | WM_CANCELMODE => Some(PreviewInput::Leave),
        WM_KILLFOCUS => Some(PreviewInput::FocusLost),
        // The low word of `wparam` is the activation state; the high word is "minimized".
        WM_ACTIVATE if (wp & 0xFFFF) as u32 == WA_INACTIVE => Some(PreviewInput::FocusLost),
        _ => None,
    }
}

/// Whether a `WM_CAPTURECHANGED` is a real loss: not our own release, and not the capture
/// being handed to this very window. `gaining` is the message's `lparam`, the window
/// that is taking the capture (0 when nobody is).
fn capture_lost(releasing: bool, gaining: isize, own: isize) -> bool {
    !releasing && gaining != own
}

/// One rounded rect of the input region, in client px.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RegionPart {
    l: i32,
    t: i32,
    r: i32,
    b: i32,
    /// Corner ellipse diameter (what `CreateRoundRectRgn` takes); 0 means square corners.
    ellipse: i32,
}

/// Drops rects that cover nothing (zero or negative size) and works out each corner
/// ellipse: `radius * 2`, never below 0 and never above the rect's shorter side.
fn region_parts(rects: &[(i32, i32, i32, i32)], radius: i32) -> Vec<RegionPart> {
    rects
        .iter()
        .filter(|&&(l, t, r, b)| r > l && b > t)
        .map(|&(l, t, r, b)| {
            let shorter = (i64::from(r) - i64::from(l)).min(i64::from(b) - i64::from(t));
            let ellipse = (i64::from(radius.max(0)) * 2).min(shorter);
            RegionPart {
                l,
                t,
                r,
                b,
                ellipse: ellipse as i32,
            }
        })
        .collect()
}

// ---- GDI regions --------------------------------------------------------------------

/// Builds the union of `parts` as a GDI region. No parts gives an empty region. Returns
/// an invalid handle if even the empty region could not be made. The caller owns the
/// region: it either hands it to `SetWindowRgn` or deletes it.
fn build_region(parts: &[RegionPart]) -> HRGN {
    // SAFETY: plain GDI object creation. Every part region made here is freed before the
    // function returns; only the combined `rgn` is returned, to be owned by the caller.
    unsafe {
        let rgn = CreateRectRgn(0, 0, 0, 0);
        if rgn.is_invalid() {
            return rgn;
        }
        for p in parts {
            let part = if p.ellipse > 0 {
                // `CreateRoundRectRgn` leaves out the last column and row (a rect region
                // stops before `r` and `b`, this one a pixel earlier), so ask for one
                // more to cover exactly `[l, r) x [t, b)` like the square case.
                CreateRoundRectRgn(
                    p.l,
                    p.t,
                    p.r.saturating_add(1),
                    p.b.saturating_add(1),
                    p.ellipse,
                    p.ellipse,
                )
            } else {
                CreateRectRgn(p.l, p.t, p.r, p.b)
            };
            if part.is_invalid() {
                continue;
            }
            CombineRgn(Some(rgn), Some(rgn), Some(part), RGN_OR);
            let _ = DeleteObject(HGDIOBJ(part.0));
        }
        rgn
    }
}

/// Clips the window's input (and, with it, its visible area) to the union of `rects`
/// (client px, `(l, t, r, b)`), each with corners of `radius` px. Everything outside the
/// union is click-through: the pointer reaches the window beneath.
///
/// An EMPTY slice sets an empty region, so no part of the window takes input. That is
/// the opposite of [`crate::window::set_hit_rects`], where an empty list is a no-op.
/// Rects that cover nothing are ignored, so a slice of only those is empty too.
pub fn set_regions(hwnd: HWND, rects: &[(i32, i32, i32, i32)], radius: i32) {
    let rgn = build_region(&region_parts(rects, radius));
    if rgn.is_invalid() {
        return;
    }
    // SAFETY: valid window handle owned by this thread. On success the system owns
    // `rgn` and frees it; on failure it is still ours, so we free it.
    unsafe {
        if SetWindowRgn(hwnd, Some(rgn), false) == 0 {
            let _ = DeleteObject(HGDIOBJ(rgn.0));
        }
    }
}

// ---- window -------------------------------------------------------------------------

/// Creates the preview window, hidden, at `(x, y, w, h)` desktop px. It starts with an
/// empty input region, so until [`set_regions`] is called it takes no input and shows
/// nothing; show it with [`show`] once its first frame is drawn.
pub fn create(x: i32, y: i32, w: i32, h: i32) -> Result<HWND> {
    TRACKING.with(|t| t.set(false));
    RELEASING.with(|r| r.set(false));
    // SAFETY: standard class registration and window creation; all pointers are valid
    // for the duration of the calls and the class name is a static string. Registering
    // the class again (the window is recreated on a DPI change) fails harmlessly.
    let hwnd = unsafe {
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(preview_proc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            CLASS_NAME,
            w!("Daijobu Island Snap previews"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinst.into()),
            None,
        )?
    };
    set_regions(hwnd, &[], 0);
    Ok(hwnd)
}

/// Shows the window on top of other windows without activating it.
pub fn show(hwnd: HWND) {
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

/// Hides the window. If it held the pointer capture or the keyboard focus, Windows takes
/// them away, which arrives as `Leave` and `FocusLost`.
pub fn hide(hwnd: HWND) {
    // The hidden window can't deliver the pending `WM_MOUSELEAVE` reliably, so re-arm on
    // the next move instead of waiting for it.
    TRACKING.with(|t| t.set(false));
    // SAFETY: our own window, on the thread that owns it.
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

/// Moves and resizes the window (desktop px) and keeps it topmost, without activating it.
pub fn place(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    // SAFETY: valid window handle owned by this thread.
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
    }
}

/// Gives the window the foreground and the keyboard, so `Key` input starts to arrive.
/// This is the one place the window takes activation, for deliberate keyboard access:
/// call it only from the tray command. It does nothing useful on a hidden window.
pub fn focus(hwnd: HWND) {
    // SAFETY: our own window, on the thread that owns it. The tray menu just ran, so
    // this process may take the foreground.
    unsafe {
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
    }
}

/// The window is the foreground window (it has the keyboard after [`focus`]).
pub fn has_foreground(hwnd: HWND) -> bool {
    // SAFETY: no pointers involved; only compares handles.
    unsafe { GetForegroundWindow() == hwnd }
}

/// Destroys the window (on the thread that created it).
pub fn destroy(hwnd: HWND) {
    TRACKING.with(|t| t.set(false));
    release_capture_quietly();
    // SAFETY: our own window, destroyed on the thread that created it.
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

/// Waits for the next DWM composition pass. Call it after [`hide`] and before a screen
/// freeze, so the hidden stack is gone from the composed desktop when it is captured.
/// Blocks for up to about one refresh interval, so call it only when needed.
pub fn flush_composition() {
    // SAFETY: no pointers involved. The error is ignored on purpose: `DwmFlush` fails
    // only when composition is unavailable or the DWM is restarting. Then there is
    // nothing to wait for, and the caller's next step (the capture) is best effort
    // anyway.
    unsafe {
        let _ = DwmFlush();
    }
}

/// The work area (monitor minus taskbar and AppBars) of the monitor nearest to the
/// desktop point `(x, y)`, as `(l, t, r, b)` in physical desktop px. `l` and `t` can be
/// negative on a monitor left of or above the primary one.
pub fn work_area_at(x: i32, y: i32) -> (i32, i32, i32, i32) {
    // SAFETY: plain queries with valid out-pointers; the monitor handle comes from the
    // system and is used immediately.
    unsafe {
        let hmon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let rc = if GetMonitorInfoW(hmon, &mut info).as_bool() {
            info.rcWork
        } else {
            // Can't happen with DEFAULTTONEAREST; fall back to the primary screen.
            RECT {
                left: 0,
                top: 0,
                right: GetSystemMetrics(SM_CXSCREEN),
                bottom: GetSystemMetrics(SM_CYSCREEN),
            }
        };
        (rc.left, rc.top, rc.right, rc.bottom)
    }
}

/// The effective DPI of the monitor nearest to the desktop point `(x, y)`, at least 96.
pub fn dpi_at(x: i32, y: i32) -> u32 {
    // SAFETY: plain queries with valid out-pointers; the monitor handle comes from the
    // system and is used immediately.
    unsafe {
        let hmon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        dx.max(96)
    }
}

fn key_down(vk: u16) -> bool {
    // SAFETY: no pointers involved.
    unsafe { GetKeyState(i32::from(vk)) < 0 }
}

/// Releases the pointer capture without the `WM_CAPTURECHANGED` it sends being reported
/// as a loss. Windows delivers that message synchronously, inside the call.
fn release_capture_quietly() {
    RELEASING.with(|r| r.set(true));
    // SAFETY: releases only this thread's capture; harmless if we don't hold it.
    let _ = unsafe { ReleaseCapture() };
    RELEASING.with(|r| r.set(false));
}

fn send(input: PreviewInput) {
    push_event(Event::Previews(input));
}

extern "system" fn preview_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        // Clicking a card must not pull the foreground away from the user's app.
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        // The class has no `CS_DBLCLKS`: a quick second click is another `Press`.
        WM_LBUTTONDOWN => {
            // SAFETY: our own window; capture keeps the release coming even when the
            // pointer leaves the window during a press.
            let _ = unsafe { SetCapture(hwnd) };
            if let Some(input) = translate(msg, wp.0, lp.0) {
                send(input);
            }
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            release_capture_quietly();
            if let Some(input) = translate(msg, wp.0, lp.0) {
                send(input);
            }
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            if !TRACKING.with(Cell::get) {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                // SAFETY: tme is fully initialised and the window is ours.
                if unsafe { TrackMouseEvent(&mut tme) }.is_ok() {
                    TRACKING.with(|t| t.set(true));
                }
            }
            if let Some(input) = translate(msg, wp.0, lp.0) {
                send(input);
            }
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            TRACKING.with(|t| t.set(false));
            send(PreviewInput::Leave);
            return LRESULT(0);
        }
        // Capture went to another window or was cancelled by the system: a press in
        // progress would never see its release, so report it as a leave.
        WM_CAPTURECHANGED => {
            if capture_lost(RELEASING.with(Cell::get), lp.0, hwnd.0 as isize) {
                send(PreviewInput::Leave);
            }
            return LRESULT(0);
        }
        WM_CANCELMODE => {
            // SAFETY: no pointers involved; we only compare handles.
            if unsafe { GetCapture() } == hwnd {
                release_capture_quietly();
            }
            send(PreviewInput::Leave);
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            send(key_input(
                wp.0,
                key_down(VK_CONTROL.0),
                key_down(VK_MENU.0),
                key_down(VK_SHIFT.0),
            ));
            return LRESULT(0);
        }
        // Report, then let `DefWindowProc` run: it gives a newly activated window the
        // keyboard focus.
        WM_KILLFOCUS | WM_ACTIVATE => {
            if let Some(input) = translate(msg, wp.0, lp.0) {
                send(input);
            }
        }
        // The app owns the window's lifetime: Alt+F4 or a taskbar close does nothing.
        WM_CLOSE => return LRESULT(0),
        // The app places and sizes the window itself, from the island's own events.
        WM_DPICHANGED => return LRESULT(0),
        _ => {}
    }
    // SAFETY: forwards the unhandled message with the original arguments.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::{GetRgnBox, PtInRegion, NULLREGION};

    fn lp(x: i16, y: i16) -> isize {
        (u32::from(y as u16) << 16 | u32::from(x as u16)) as isize
    }

    #[test]
    fn lparam_coordinates_are_signed() {
        assert_eq!(lparam_xy(lp(12, 34)), (12, 34));
        assert_eq!(lparam_xy(lp(-5, 7)), (-5, 7));
        assert_eq!(lparam_xy(lp(9, -300)), (9, -300));
        assert_eq!(lparam_xy(lp(i16::MIN, i16::MAX)), (-32768, 32767));
        // The upper half of a 64-bit lparam is ignored.
        assert_eq!(lparam_xy(0x1234_0000_0003_0004), (4, 3));
    }

    #[test]
    fn ctrl_only_counts_while_alt_is_up() {
        let k = |c, a, s| key_input(0x41, c, a, s);
        assert_eq!(
            k(true, false, false),
            PreviewInput::Key {
                vk: 0x41,
                ctrl: true,
                shift: false
            }
        );
        // AltGr is Ctrl+Alt: not a Ctrl shortcut.
        assert_eq!(
            k(true, true, false),
            PreviewInput::Key {
                vk: 0x41,
                ctrl: false,
                shift: false
            }
        );
        assert_eq!(
            k(false, false, true),
            PreviewInput::Key {
                vk: 0x41,
                ctrl: false,
                shift: true
            }
        );
        // The key code is the low 16 bits of wparam.
        assert_eq!(
            key_input(0x1_0027, false, false, false),
            PreviewInput::Key {
                vk: 0x27,
                ctrl: false,
                shift: false
            }
        );
    }

    #[test]
    fn messages_map_to_inputs() {
        let l = lp(-3, 40);
        assert_eq!(
            translate(WM_LBUTTONDOWN, 1, l),
            Some(PreviewInput::Press { x: -3, y: 40 })
        );
        assert_eq!(
            translate(WM_LBUTTONUP, 0, l),
            Some(PreviewInput::Release { x: -3, y: 40 })
        );
        assert_eq!(
            translate(WM_MOUSEMOVE, 1, l),
            Some(PreviewInput::Move { x: -3, y: 40 })
        );
        assert_eq!(translate(WM_MOUSELEAVE, 0, 0), Some(PreviewInput::Leave));
        assert_eq!(translate(WM_CANCELMODE, 0, 0), Some(PreviewInput::Leave));
        assert_eq!(translate(WM_KILLFOCUS, 0, 0), Some(PreviewInput::FocusLost));
        assert_eq!(translate(WM_KEYDOWN, 0x41, 0), None);
        assert_eq!(translate(WM_CAPTURECHANGED, 0, 5), None);
        assert_eq!(translate(WM_RBUTTONDOWN, 2, l), None);
    }

    #[test]
    fn only_deactivation_loses_focus() {
        assert_eq!(
            translate(WM_ACTIVATE, WA_INACTIVE as usize, 0),
            Some(PreviewInput::FocusLost)
        );
        // Deactivated while minimized: the high word is set, the low word is still 0.
        assert_eq!(
            translate(WM_ACTIVATE, 0x0001_0000, 0),
            Some(PreviewInput::FocusLost)
        );
        // WA_ACTIVE (1) and WA_CLICKACTIVE (2) are activations.
        assert_eq!(translate(WM_ACTIVATE, 1, 0), None);
        assert_eq!(translate(WM_ACTIVATE, 2, 0), None);
    }

    #[test]
    fn capture_loss_ignores_our_own_release() {
        // Taken by another window: a real loss.
        assert!(capture_lost(false, 0x1234, 0x99));
        // Cancelled with nobody taking it.
        assert!(capture_lost(false, 0, 0x99));
        // Our own ReleaseCapture, or a SetCapture on ourselves again: not a loss.
        assert!(!capture_lost(true, 0, 0x99));
        assert!(!capture_lost(true, 0x1234, 0x99));
        assert!(!capture_lost(false, 0x99, 0x99));
    }

    #[test]
    fn region_parts_drop_empty_rects_and_clamp_the_corner() {
        let parts = region_parts(
            &[
                (10, 10, 134, 90),    // a card
                (0, 0, 0, 0),         // empty
                (5, 5, 5, 50),        // zero width
                (50, 50, 20, 80),     // inverted
                (100, 100, 110, 140), // narrow: corner can't exceed the 10 px side
            ],
            12,
        );
        assert_eq!(
            parts,
            vec![
                RegionPart {
                    l: 10,
                    t: 10,
                    r: 134,
                    b: 90,
                    ellipse: 24
                },
                RegionPart {
                    l: 100,
                    t: 100,
                    r: 110,
                    b: 140,
                    ellipse: 10
                },
            ]
        );
        // A negative radius means square corners; a huge one doesn't overflow.
        assert_eq!(region_parts(&[(0, 0, 40, 40)], -4)[0].ellipse, 0);
        assert_eq!(region_parts(&[(0, 0, 40, 40)], i32::MAX)[0].ellipse, 40);
        assert!(region_parts(&[], 12).is_empty());
        // Negative client coordinates are fine (a window can sit on a negative monitor).
        assert_eq!(region_parts(&[(-20, -20, 20, 20)], 4).len(), 1);
    }

    /// Runs `f` on a region built from `rects`, then frees it. Regions are plain GDI
    /// objects: no window, input or hook is involved.
    fn with_region<R>(rects: &[(i32, i32, i32, i32)], radius: i32, f: impl FnOnce(HRGN) -> R) -> R {
        let rgn = build_region(&region_parts(rects, radius));
        assert!(!rgn.is_invalid());
        let out = f(rgn);
        // SAFETY: `rgn` was made by `build_region` and is not used again.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(rgn.0));
        }
        out
    }

    fn inside(rgn: HRGN, x: i32, y: i32) -> bool {
        // SAFETY: `rgn` is a live region owned by the caller.
        unsafe { PtInRegion(rgn, x, y).as_bool() }
    }

    fn bounds(rgn: HRGN) -> (i32, i32, i32, i32, i32) {
        let mut rc = RECT::default();
        // SAFETY: valid out-pointer and a live region.
        let kind = unsafe { GetRgnBox(rgn, &mut rc) };
        (kind.0, rc.left, rc.top, rc.right, rc.bottom)
    }

    #[test]
    fn empty_input_gives_an_empty_region() {
        with_region(&[], 12, |rgn| {
            assert_eq!(bounds(rgn).0, NULLREGION.0);
            assert!(!inside(rgn, 0, 0));
        });
        // Only degenerate rects: also empty.
        with_region(&[(5, 5, 5, 5), (9, 9, 3, 3)], 12, |rgn| {
            assert_eq!(bounds(rgn).0, NULLREGION.0);
        });
    }

    #[test]
    fn a_rounded_rect_covers_its_pixels_and_cuts_the_corners() {
        with_region(&[(20, 30, 144, 110)], 12, |rgn| {
            // The box is exactly the requested rect.
            let (_, l, t, r, b) = bounds(rgn);
            assert_eq!((l, t, r, b), (20, 30, 144, 110));
            // Centre and all four edge midpoints are inside, one pixel out is not.
            assert!(inside(rgn, 80, 70));
            assert!(inside(rgn, 20, 70) && inside(rgn, 143, 70));
            assert!(inside(rgn, 80, 30) && inside(rgn, 80, 109));
            assert!(!inside(rgn, 19, 70) && !inside(rgn, 144, 70));
            assert!(!inside(rgn, 80, 29) && !inside(rgn, 80, 110));
            // The sharp corners are cut away.
            assert!(!inside(rgn, 20, 30));
            assert!(!inside(rgn, 143, 30));
            assert!(!inside(rgn, 20, 109));
            assert!(!inside(rgn, 143, 109));
            // Just inside each corner arc is covered again.
            assert!(inside(rgn, 20 + 12, 30 + 1));
            assert!(inside(rgn, 20 + 1, 30 + 12));
        });
    }

    #[test]
    fn zero_radius_gives_square_corners() {
        with_region(&[(0, 0, 50, 40)], 0, |rgn| {
            assert!(inside(rgn, 0, 0) && inside(rgn, 49, 39));
            assert!(!inside(rgn, 50, 39) && !inside(rgn, 49, 40));
        });
    }

    #[test]
    fn several_rects_form_a_union_with_a_gap() {
        // Two cards 10 px apart: the gap between them must not take input.
        with_region(&[(10, 10, 134, 90), (144, 10, 268, 90)], 12, |rgn| {
            assert!(inside(rgn, 70, 50));
            assert!(inside(rgn, 200, 50));
            assert!(!inside(rgn, 139, 50));
            let (_, l, t, r, b) = bounds(rgn);
            assert_eq!((l, t, r, b), (10, 10, 268, 90));
        });
    }

    #[test]
    fn overlapping_rects_merge_and_negative_origins_work() {
        with_region(&[(-30, -30, 10, 10), (0, 0, 40, 40)], 0, |rgn| {
            assert!(inside(rgn, -30, -30) && inside(rgn, 39, 39));
            assert!(inside(rgn, 5, 5));
            assert!(!inside(rgn, -31, 0));
            let (_, l, t, r, b) = bounds(rgn);
            assert_eq!((l, t, r, b), (-30, -30, 40, 40));
        });
    }
}
