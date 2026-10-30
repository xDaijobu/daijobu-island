//! One physical pixel of nonactivating input at the island's monitor top edge.
//! WinEvents invalidate fullscreen state; mouse input is local to this small HWND.
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::{w, Result};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, GetStockObject, MonitorFromWindow, BLACK_BRUSH, HBRUSH,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_EDGE: u32 = WM_APP + 0x91;
static PASSIVE_FRONT: AtomicBool = AtomicBool::new(false);
thread_local! {
    static OWNER: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static PENDING: Cell<bool> = const { Cell::new(false) };
}

/// Helps the existing foreground hook avoid reasserting a passive reveal above
/// a newly foreground fullscreen game before the app drains its queued context event.
pub fn set_passive_front(passive: bool) {
    PASSIVE_FRONT.store(passive, Ordering::Relaxed);
}

pub fn suppress_passive_raise() -> bool {
    PASSIVE_FRONT.load(Ordering::Relaxed) && blocked()
}

pub fn ack_context() {
    PENDING.with(|p| p.set(false));
}

/// Rect is physical desktop `(left, top, right, bottom)`; right/bottom exclusive.
pub type Bounds = (i32, i32, i32, i32);

pub fn edge_bounds(left: i32, top: i32, right: i32) -> Option<Bounds> {
    (right > left).then_some((left, top, right, top.saturating_add(1)))
}

fn covers(rect: Bounds, monitor: Bounds) -> bool {
    rect.0 <= monitor.0 && rect.1 <= monitor.1 && rect.2 >= monitor.2 && rect.3 >= monitor.3
}

fn tuple(r: RECT) -> Bounds {
    (r.left, r.top, r.right, r.bottom)
}

/// Native shell exclusive-D3D/presentation state plus foreground client geometry
/// for borderless/F11 fullscreen. Client bounds avoid invisible resize borders
/// incorrectly treating an ordinary maximized window as fullscreen.
pub fn blocked() -> bool {
    // SAFETY: shell query has no borrowed pointers or side effects.
    if unsafe { SHQueryUserNotificationState() }
        .is_ok_and(|s| s == QUNS_PRESENTATION_MODE || s == QUNS_RUNNING_D3D_FULL_SCREEN)
    {
        return true;
    }
    // SAFETY: native handles are used only in read-only queries; out buffers have
    // their documented sizes. Failure conservatively refuses a passive reveal.
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() {
            return false;
        }
        let mut name = [0u16; 80];
        let n = GetClassNameW(fg, &mut name);
        if matches!(
            String::from_utf16_lossy(&name[..n.max(0) as usize]).as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return false;
        }
        if !IsWindowVisible(fg).as_bool() || IsIconic(fg).as_bool() {
            return false;
        }
        window_covers_monitor(fg)
    }
}

fn window_covers_monitor(fg: HWND) -> bool {
    // SAFETY: read-only queries of an opaque window handle; all out buffers have
    // the documented sizes. Accept hidden HWNDs so geometry can be tested without
    // showing anything or manipulating the user's foreground window.
    unsafe {
        let mut client = RECT::default();
        if GetClientRect(fg, &mut client).is_err() {
            return true;
        }
        let mut origin = POINT::default();
        if !ClientToScreen(fg, &mut origin).as_bool() {
            return true;
        }
        let monitor = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return true;
        }
        client.left += origin.x;
        client.right += origin.x;
        client.top += origin.y;
        client.bottom += origin.y;
        if covers(tuple(client), tuple(info.rcMonitor)) {
            return true;
        }
        // Some borderless surfaces retain a small nonclient inset. Only accept
        // their DWM visible bounds when there is no caption; never raw window
        // bounds, which include invisible maximized resize borders.
        let mut frame = RECT::default();
        GetWindowLongPtrW(fg, GWL_STYLE) as u32 & WS_CAPTION.0 == 0
            && DwmGetWindowAttribute(
                fg,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut frame as *mut _ as *mut _,
                size_of::<RECT>() as u32,
            )
            .is_ok()
            && covers(tuple(frame), tuple(info.rcMonitor))
    }
}

/// Actual hit window, accounting for the island's shaped region and occlusion.
/// Called only on input/one-shot expiry, never on the render or idle clock loop.
pub fn pointer_over(hwnd: HWND) -> bool {
    let mut pt = POINT::default();
    // SAFETY: writable point and opaque HWND comparison only.
    let inside = unsafe { GetCursorPos(&mut pt).is_ok() && WindowFromPoint(pt) == hwnd };
    if inside {
        // A stationary pointer may have been uncovered by the sensor hiding,
        // without a mouse move on the island. Ensure its eventual leave arrives.
        let mut tracking = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            ..Default::default()
        };
        // SAFETY: valid out structure and our own UI-thread window.
        let _ = unsafe { TrackMouseEvent(&mut tracking) };
    }
    inside
}

pub struct Sensor {
    hwnd: HWND,
    hooks: [HWINEVENTHOOK; 2],
    bounds: Option<Bounds>,
    shown: bool,
}

impl Sensor {
    pub fn suspend(&mut self) {
        self.configure(self.bounds.unwrap_or((0, 0, 1, 1)), false, false);
    }

    /// Creates hidden. All operations and destruction belong to the UI thread.
    pub fn create(owner: HWND) -> Result<Self> {
        // SAFETY: static class name/procedure; no borrowed create data. The HWND
        // remains hidden until explicitly configured and never acquires focus.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(sensor_proc),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                // Stock brush is process-owned, never destroyed by this sensor.
                // Give the constant-alpha layered window an opaque backing pixel.
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                lpszClassName: w!("DaijobuEdgeReveal"),
                ..Default::default()
            };
            RegisterClassExW(&class);
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
                class.lpszClassName,
                w!(""),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance.into()),
                None,
            )?
        };
        let mut sensor = Self {
            hwnd,
            hooks: [HWINEVENTHOOK::default(); 2],
            bounds: None,
            shown: false,
        };
        // SAFETY: our hidden HWND. Alpha 1 keeps the pixel hit-testable with no
        // meaningful visible fill; capture exclusion must succeed before use.
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, owner.0 as isize);
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA)?;
            SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)?;
            OWNER.with(|o| o.set(owner));
            for (slot, event) in sensor
                .hooks
                .iter_mut()
                .zip([EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_LOCATIONCHANGE])
            {
                *slot = SetWinEventHook(
                    event,
                    event,
                    None,
                    Some(context_changed),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                );
                if slot.is_invalid() {
                    return Err(windows::core::Error::from_thread());
                }
            }
        }
        Ok(sensor)
    }

    /// `enabled` retains event invalidation while the sensor is temporarily hidden
    /// for fullscreen or an existing reveal. Position always uses device pixels.
    pub fn configure(&mut self, bounds: Bounds, enabled: bool, show: bool) {
        ENABLED.with(|e| e.set(enabled));
        let show = enabled && show;
        if self.bounds == Some(bounds) && self.shown == show {
            return;
        }
        self.bounds = Some(bounds);
        self.shown = show;
        // SAFETY: own HWND and scalar geometry; NOACTIVATE on every show/move.
        unsafe {
            if show {
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    bounds.0,
                    bounds.1,
                    bounds.2 - bounds.0,
                    1,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            } else {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }
}

impl Drop for Sensor {
    fn drop(&mut self) {
        ENABLED.with(|e| e.set(false));
        OWNER.with(|o| o.set(HWND::default()));
        PENDING.with(|p| p.set(false));
        // SAFETY: all hooks and the HWND were created on this UI thread. No
        // callback retains a Rust pointer; unhook before releasing the window.
        unsafe {
            for hook in self.hooks {
                if !hook.is_invalid() {
                    let _ = UnhookWinEvent(hook);
                }
            }
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

// SAFETY: out-of-context hook is delivered on this UI thread. It only queries
// the current foreground handle and posts one coalesced message to our owner.
unsafe extern "system" fn context_changed(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if !ENABLED.with(Cell::get) {
        return;
    }
    // SAFETY: read-only foreground query; no dereference of the event HWND.
    if event == EVENT_OBJECT_LOCATIONCHANGE
        && (object != 0 || child != 0 || hwnd != unsafe { GetForegroundWindow() })
    {
        return;
    }
    if !PENDING.with(|p| p.replace(true)) {
        OWNER.with(|o| crate::window::post_to(o.get().0 as isize, WM_EDGE));
    }
}

// SAFETY: registered HWND procedure, no pointer data retained across calls.
unsafe extern "system" fn sensor_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE if !blocked() => {
            // SAFETY: userdata holds only our owner's opaque HWND; asynchronous post.
            unsafe {
                let owner = HWND(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut _);
                let _ = PostMessageW(Some(owner), WM_EDGE, WPARAM(1), LPARAM(0));
            }
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: forward the system's unchanged message parameters.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SAFETY: test-only subclass of our own hidden window. F11 browsers can
    // retain frame style while handling NCCALCSIZE to use the entire window as
    // client space. No Rust pointer is stored or read by this procedure.
    unsafe extern "system" fn fullscreen_client_proc(
        hwnd: HWND,
        msg: u32,
        wp: WPARAM,
        lp: LPARAM,
    ) -> LRESULT {
        if msg == WM_NCCALCSIZE {
            return LRESULT(0);
        }
        // SAFETY: forward unchanged native parameters; HWND belongs to this test.
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }

    #[test]
    fn physical_edge_is_exactly_one_pixel_even_on_negative_origin_monitor() {
        assert_eq!(
            edge_bounds(-1400, -1080, -1200),
            Some((-1400, -1080, -1200, -1079))
        );
        assert_eq!(edge_bounds(50, 0, 50), None);
        assert!(!covers((0, 0, 1920, 1040), (0, 0, 1920, 1080)));
        assert!(!covers((0, 30, 1920, 1080), (0, 0, 1920, 1080)));
        assert!(covers((-1920, 0, 0, 1080), (-1920, 0, 0, 1080)));
        assert!(!covers((0, 0, 1920, 1080), (-1920, 0, 0, 1080)));
    }

    #[test]
    fn native_hidden_sensor_has_no_activation_capture_or_taskbar_and_cleans_up() {
        let sensor = Sensor::create(HWND::default()).expect("create hidden native sensor");
        let hwnd = sensor.hwnd;
        // SAFETY: read-only inspection of our hidden test HWND, no desktop input.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            assert_ne!(style & WS_EX_NOACTIVATE.0, 0);
            assert_ne!(style & WS_EX_TOOLWINDOW.0, 0);
            assert_eq!(style & WS_EX_APPWINDOW.0, 0);
            assert!(!IsWindowVisible(hwnd).as_bool());
            let mut affinity = 0;
            GetWindowDisplayAffinity(hwnd, &mut affinity).unwrap();
            assert_eq!(affinity, WDA_EXCLUDEFROMCAPTURE.0);
            assert_eq!(
                sensor_proc(hwnd, WM_MOUSEACTIVATE, WPARAM(0), LPARAM(0)),
                LRESULT(MA_NOACTIVATE as isize)
            );
        }
        drop(sensor);
        // SAFETY: IsWindow accepts a stale opaque handle, never dereferenced here.
        assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
    }

    #[test]
    fn native_fullscreen_geometry_distinguishes_full_monitor_and_work_area() {
        let sensor = Sensor::create(HWND::default()).unwrap();
        // SAFETY: monitor query and movement/style changes affect only our own
        // hidden test window. It is never shown, activated, or given input.
        unsafe {
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            assert!(GetMonitorInfoW(
                MonitorFromWindow(sensor.hwnd, MONITOR_DEFAULTTONEAREST),
                &mut info
            )
            .as_bool());
            let m = info.rcMonitor;
            let place_hidden = |r: RECT| {
                SetWindowPos(
                    sensor.hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED,
                )
                .unwrap();
                assert!(!IsWindowVisible(sensor.hwnd).as_bool());
            };
            place_hidden(m);
            assert!(
                window_covers_monitor(sensor.hwnd),
                "borderless full monitor"
            );
            let mut work = m;
            work.bottom -= 48;
            place_hidden(work);
            assert!(
                !window_covers_monitor(sensor.hwnd),
                "work area is not fullscreen"
            );
            SetWindowLongPtrW(
                sensor.hwnd,
                GWL_STYLE,
                (WS_POPUP | WS_CAPTION | WS_THICKFRAME).0 as isize,
            );
            place_hidden(m);
            assert!(
                !window_covers_monitor(sensor.hwnd),
                "captioned maximized geometry is not fullscreen"
            );
            // Model F11's custom nonclient handling directly, rather than rely
            // on system-DPI frame metrics or oversized-window tracking limits.
            SetWindowLongPtrW(
                sensor.hwnd,
                GWLP_WNDPROC,
                fullscreen_client_proc as *const () as isize,
            );
            place_hidden(m);
            assert!(
                window_covers_monitor(sensor.hwnd),
                "fullscreen client with retained frame style"
            );
        }
    }
}
