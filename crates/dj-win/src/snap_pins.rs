//! Independent, nonactivating native screenshot references.
//!
//! Each HWND owns an `Rc<State>` in GWLP_USERDATA, detached at WM_NCDESTROY. The
//! public owner keeps another Rc, making it thread-bound and invalidating its
//! handle even if Windows destroys the HWND first. Callbacks hold only shared
//! state with Cells across native calls: capture, focus and destruction can all
//! synchronously reenter the window procedure. Only owned events reach the app.

use crate::window::{push_event, Event};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, FILETIME, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, EndPaint, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow,
    ScreenToClient, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetCapture, GetKeyState, ReleaseCapture, SetCapture, SetFocus,
    TrackMouseEvent, TME_CANCEL, TME_LEAVE, TRACKMOUSEEVENT, VK_CONTROL, VK_LBUTTON, VK_MENU,
    VK_RBUTTON, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: PCWSTR = w!("DaijobuSnapPin");
const WM_MOUSELEAVE: u32 = 0x02a3;
const DRAG_SAMPLE_MS: u32 = 16;
// A sampled release needs this many consecutive up readings while capture is still
// held. One stale or blind reading must not end a real drag.
const RELEASE_SAMPLES: u8 = 2;
// Never recycle a timer ID, even if an HWND is reused after destruction. KillTimer
// does not remove already posted WM_TIMER messages. Exhaustion refuses the press.
static NEXT_TIMER: AtomicUsize = AtomicUsize::new(1);
const EX_STYLE: WINDOW_EX_STYLE = WINDOW_EX_STYLE(
    WS_EX_NOREDIRECTIONBITMAP.0 | WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0 | WS_EX_NOACTIVATE.0,
);

/// Positions are physical pixels. Native messages preserve their signed client
/// coordinates; drag samples contain one owned snapshot of cursor and button state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PinInput {
    Move {
        x: i32,
        y: i32,
        screen_x: i32,
        screen_y: i32,
    },
    Down {
        x: i32,
        y: i32,
        screen_x: i32,
        screen_y: i32,
    },
    Up {
        x: i32,
        y: i32,
        screen_x: i32,
        screen_y: i32,
    },
    Leave,
    CancelCapture,
    FocusLost,
    Paint,
    Close,
    Destroyed,
    DisplayChanged,
    Key {
        vk: u16,
        ctrl: bool,
        shift: bool,
        alt: bool,
    },
    Dpi {
        dpi: u32,
        suggested: (i32, i32, i32, i32),
    },
}

/// An opaque focus destination. The process birth time protects against PID reuse;
/// restoration also rechecks the window and its owning thread.
#[derive(Clone, Copy, Debug)]
pub struct FocusTarget {
    hwnd: HWND,
    identity: Identity,
}

impl FocusTarget {
    /// Returns the destination only while its captured process/thread identity remains valid.
    pub fn validated_window(&self) -> Option<HWND> {
        same_target(self.identity, identity(self.hwnd)).then_some(self.hwnd)
    }

    /// Compares the opaque destination with a controller-owned pin HWND.
    pub fn is_window(&self, hwnd: HWND) -> bool {
        self.hwnd == hwnd
    }
}

/// Snapshot the validated foreground before a tray popup activates its owner.
/// Clicking a notification-area icon makes Explorer's taskbar the foreground, so a
/// shell tray window is never returned: the most recent non-shell foreground that
/// is still alive and visible stands in for it, or `None` when there is none.
pub fn focus_target() -> Option<FocusTarget> {
    LAST_FOREGROUND.target(&NativeDesktop)
}

/// Records a foreground change from the system's EVENT_SYSTEM_FOREGROUND hook
/// (delivered on the UI thread). Shell tray windows never replace the record.
pub(crate) fn note_foreground(hwnd: HWND) {
    LAST_FOREGROUND.note(hwnd, &NativeDesktop);
}

/// Window classes of Explorer's tray and its overflow flyouts. Window class names
/// compare case-insensitively.
const SHELL_TRAY_CLASSES: [&str; 4] = [
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
];

fn is_shell_tray_class(class: &str) -> bool {
    SHELL_TRAY_CLASSES
        .iter()
        .any(|shell| shell.eq_ignore_ascii_case(class))
}

/// Read-only desktop queries behind `focus_target`; faked in tests.
trait Desktop {
    fn foreground(&self) -> HWND;
    fn class_name(&self, hwnd: HWND) -> Option<String>;
    /// The window still exists and is visible.
    fn usable(&self, hwnd: HWND) -> bool;
    fn identity(&self, hwnd: HWND) -> Option<Identity>;
}

struct NativeDesktop;

impl Desktop for NativeDesktop {
    fn foreground(&self) -> HWND {
        // SAFETY: read-only foreground query returning a transient handle.
        unsafe { GetForegroundWindow() }
    }

    fn class_name(&self, hwnd: HWND) -> Option<String> {
        let mut class = [0u16; 64];
        // SAFETY: the buffer is a valid, writable slice; a stale HWND only makes the
        // call fail with 0, which maps to None.
        let len = unsafe { GetClassNameW(hwnd, &mut class) };
        (len > 0).then(|| String::from_utf16_lossy(&class[..len as usize]))
    }

    fn usable(&self, hwnd: HWND) -> bool {
        // SAFETY: read-only validation of a possibly stale handle.
        !hwnd.is_invalid()
            && unsafe { IsWindow(Some(hwnd)) }.as_bool()
            && unsafe { IsWindowVisible(hwnd) }.as_bool()
    }

    fn identity(&self, hwnd: HWND) -> Option<Identity> {
        identity(hwnd)
    }
}

/// The raw HWND of the latest non-shell foreground window; 0 when none is known.
struct LastForeground(AtomicIsize);

static LAST_FOREGROUND: LastForeground = LastForeground(AtomicIsize::new(0));

impl LastForeground {
    fn note(&self, hwnd: HWND, desktop: &impl Desktop) {
        if hwnd.is_invalid() {
            return;
        }
        // A window whose class cannot be read is already gone: do not remember it.
        match desktop.class_name(hwnd) {
            Some(class) if !is_shell_tray_class(&class) => {
                self.0.store(hwnd.0 as isize, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    fn target(&self, desktop: &impl Desktop) -> Option<FocusTarget> {
        let foreground = desktop.foreground();
        let shell = desktop
            .class_name(foreground)
            .is_some_and(|class| is_shell_tray_class(&class));
        if !shell {
            return desktop.identity(foreground).map(|identity| FocusTarget {
                hwnd: foreground,
                identity,
            });
        }
        let recorded = HWND(self.0.load(Ordering::Relaxed) as *mut _);
        // The record may be stale: revalidate, and compute its identity now.
        if !desktop.usable(recorded)
            || desktop
                .class_name(recorded)
                .is_none_or(|class| is_shell_tray_class(&class))
        {
            return None;
        }
        desktop.identity(recorded).map(|identity| FocusTarget {
            hwnd: recorded,
            identity,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    pid: u32,
    thread: u32,
    created: u64,
}

struct State {
    id: u64,
    hwnd: Cell<HWND>,
    tracking: Cell<bool>,
    releasing: Cell<bool>,
    destroyed: Cell<bool>,
    focus_lost: Cell<bool>,
    press: Cell<Option<Press>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Press {
    timer: usize,
    physical_button: u16,
    screen: (i32, i32),
    /// GetAsyncKeyState read "up" while the press's own WM_LBUTTONDOWN was being
    /// handled, so it cannot see this button (e.g. an elevated foreground window).
    /// Such a press never synthesizes an Up from samples.
    button_blind: bool,
    up_samples: u8,
}

#[derive(Clone, Copy, Debug)]
struct PointerSample {
    client: (i32, i32),
    screen: (i32, i32),
    down: bool,
}

impl State {
    fn new(id: u64) -> Self {
        Self {
            id,
            hwnd: Cell::new(HWND::default()),
            tracking: Cell::new(false),
            releasing: Cell::new(false),
            destroyed: Cell::new(false),
            focus_lost: Cell::new(false),
            press: Cell::new(None),
        }
    }

    fn live(&self) -> bool {
        !self.destroyed.get() && !self.hwnd.get().is_invalid()
    }
}

/// Owns exactly one native window on the creating UI thread (`!Send`/`!Sync`).
pub struct Window {
    state: Rc<State>,
}

impl Window {
    /// Creates a hidden topmost tool window. Draw its first frame before showing it.
    pub fn new(id: u64, bounds: (i32, i32, i32, i32)) -> Result<Self, String> {
        if !valid_bounds(bounds) {
            return Err("Invalid screen pin bounds".into());
        }
        let state = Rc::new(State::new(id));
        let (x, y, width, height) = bounds;
        // SAFETY: class strings are static and the creation argument borrows the
        // local Rc only for synchronous CreateWindowExW. WM_NCCREATE clones it into
        // independently owned storage; WM_NCDESTROY releases that storage.
        let result = unsafe {
            (|| {
                let module = GetModuleHandleW(None)?;
                let class = WNDCLASSEXW {
                    cbSize: size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(pin_proc),
                    hInstance: module.into(),
                    hCursor: LoadCursorW(None, IDC_ARROW)?,
                    lpszClassName: CLASS_NAME,
                    ..Default::default()
                };
                // Repeated registration of our process-local class is harmless.
                RegisterClassExW(&class);
                CreateWindowExW(
                    EX_STYLE,
                    CLASS_NAME,
                    w!("Daijobu Island pinned screenshot"),
                    WS_POPUP,
                    x,
                    y,
                    width,
                    height,
                    None,
                    None,
                    Some(module.into()),
                    Some((&state as *const Rc<State>).cast()),
                )
            })()
        };
        result.map_err(|error| format!("Could not create screen pin: {error}"))?;
        if !state.live() {
            return Err("Screen pin was destroyed during creation".into());
        }
        Ok(Self { state })
    }

    pub fn hwnd(&self) -> HWND {
        self.state.hwnd.get()
    }

    pub fn show(&self) {
        if !self.state.live() {
            return;
        }
        let hwnd = self.hwnd();
        // SAFETY: the Rc invalidates the handle during native destruction. These
        // calls run on its owning thread and explicitly suppress activation.
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            if self.state.live() {
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
    }

    pub fn hide(&self) {
        if !self.state.live() {
            return;
        }
        cancel_active(&self.state, &Native);
        if self.state.live() {
            // SAFETY: a live window owned by this UI thread; hiding never activates.
            unsafe {
                let _ = ShowWindow(self.hwnd(), SW_HIDE);
            }
        }
    }

    /// Updates native bounds only. Moving does not allocate any drawing resources.
    pub fn place(&self, bounds: (i32, i32, i32, i32)) {
        if !self.state.live() || !valid_bounds(bounds) {
            return;
        }
        let (x, y, width, height) = bounds;
        // SAFETY: this thread owns the live HWND and bounds have checked extents.
        unsafe {
            let _ = SetWindowPos(
                self.hwnd(),
                Some(HWND_TOPMOST),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE,
            );
        }
    }

    pub fn capture(&self) -> bool {
        // Down already acquired capture and armed its sampler. The controller's
        // acknowledgement must never reacquire a finished/cancelled press.
        self.state.live() && self.state.press.get().is_some() && Native.owns_capture(self.hwnd())
    }

    /// Releases this pin's capture without mistaking synchronous capture-change
    /// messages for an external cancellation.
    pub fn release_capture(&self) {
        release_quietly(&self.state, &Native);
    }

    pub fn has_foreground(&self) -> bool {
        // SAFETY: read-only query, with no retained pointer or handle ownership.
        self.state.live() && unsafe { GetForegroundWindow() } == self.hwnd()
    }

    /// Deliberate keyboard recovery, called only from the tray/controller action.
    /// Ordinary creation, mouse clicks, show and place do not take the foreground.
    pub fn focus(&self) -> Option<FocusTarget> {
        if !self.state.live() {
            return None;
        }
        // SAFETY: query followed by explicit activation of this thread's live HWND.
        // No application borrow or mutable native state survives either focus call.
        unsafe {
            let target = focus_target().filter(|target| !target.is_window(self.hwnd()));
            self.show();
            if self.state.live() && SetForegroundWindow(self.hwnd()).as_bool() && self.state.live()
            {
                let _ = SetFocus(Some(self.hwnd()));
            }
            target
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        if !self.state.live() {
            return;
        }
        cancel_active(&self.state, &Native);
        if self.state.live() {
            // SAFETY: exactly our HWND, on its creating thread. WM_NCDESTROY clears
            // both the native attachment and this owner's handle before returning.
            unsafe {
                let _ = DestroyWindow(self.hwnd());
            }
        }
    }
}

fn valid_bounds((x, y, width, height): (i32, i32, i32, i32)) -> bool {
    width > 0 && height > 0 && x.checked_add(width).is_some() && y.checked_add(height).is_some()
}

fn rect_bounds(rect: RECT) -> (i32, i32, i32, i32) {
    (
        rect.left,
        rect.top,
        rect.right.saturating_sub(rect.left).max(1),
        rect.bottom.saturating_sub(rect.top).max(1),
    )
}

/// Monitor work area as physical x/y/width/height, plus effective monitor DPI.
pub fn work_area_at(point: (i32, i32)) -> ((i32, i32, i32, i32), u32) {
    // SAFETY: a pure system query with no retained resources.
    let monitor = unsafe {
        MonitorFromPoint(
            POINT {
                x: point.0,
                y: point.1,
            },
            MONITOR_DEFAULTTONEAREST,
        )
    };
    monitor_work_area(monitor)
}

pub fn work_area_for(hwnd: HWND) -> ((i32, i32, i32, i32), u32) {
    // SAFETY: read-only validation of an opaque caller-supplied HWND.
    if hwnd.is_invalid() || !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        return work_area_at((0, 0));
    }
    // SAFETY: read-only query of a validated HWND. If its display has disappeared,
    // DEFAULTTONEAREST selects a remaining monitor for the controller's clamp.
    monitor_work_area(unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) })
}

fn monitor_work_area(monitor: HMONITOR) -> ((i32, i32, i32, i32), u32) {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let (mut dpi_x, mut dpi_y) = (96, 96);
    // SAFETY: monitor is a transient system handle and all out-buffers are valid.
    let rect = unsafe {
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            info.rcWork
        } else {
            let mut fallback = RECT::default();
            if SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some((&mut fallback as *mut RECT).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
            .is_ok()
            {
                fallback
            } else {
                RECT {
                    left: 0,
                    top: 0,
                    right: GetSystemMetrics(SM_CXSCREEN).max(1),
                    bottom: GetSystemMetrics(SM_CYSCREEN).max(1),
                }
            }
        }
    };
    (rect_bounds(rect), dpi_x.max(96))
}

fn identity(hwnd: HWND) -> Option<Identity> {
    // SAFETY: read-only validation and queries. The process handle is closed on
    // every path after GetProcessTimes; no borrowed system storage is retained.
    unsafe {
        if hwnd.is_invalid() || !IsWindow(Some(hwnd)).as_bool() {
            return None;
        }
        let mut pid = 0;
        let thread = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || thread == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut created, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let queried = GetProcessTimes(process, &mut created, &mut exit, &mut kernel, &mut user);
        let _ = CloseHandle(process);
        queried.ok()?;
        Some(Identity {
            pid,
            thread,
            created: (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
        })
    }
}

fn same_target(expected: Identity, current: Option<Identity>) -> bool {
    current == Some(expected)
}

/// The controller calls this only while closing a pin that still owns foreground.
/// Invalid, inaccessible or replaced destinations are left alone.
pub fn restore_focus(target: FocusTarget) {
    if !same_target(target.identity, identity(target.hwnd)) {
        return;
    }
    // SAFETY: explicit restoration to a freshly validated live HWND. Foreground
    // policy still applies; failure is harmless and never injects keyboard input.
    unsafe {
        let _ = SetForegroundWindow(target.hwnd);
    }
}

// The same routing path runs with Native in production and Fake in tests. Native
// operations may reenter; State is shared and never mutably borrowed across them.
trait Platform {
    fn send(&self, id: u64, input: PinInput);
    fn screen_point(&self, hwnd: HWND, point: (i32, i32)) -> Option<(i32, i32)>;
    fn track(&self, hwnd: HWND, cancel: bool) -> bool;
    fn capture(&self, hwnd: HWND) -> bool;
    fn owns_capture(&self, hwnd: HWND) -> bool;
    fn release(&self);
    fn paint(&self, hwnd: HWND);
    fn primary_button(&self) -> u16;
    fn button_down(&self, physical_button: u16) -> bool;
    fn start_timer(&self, hwnd: HWND, timer: usize) -> bool;
    fn stop_timer(&self, hwnd: HWND, timer: usize);
    fn sample(&self, hwnd: HWND, physical_button: u16) -> Option<PointerSample>;
}

struct Native;

impl Platform for Native {
    fn send(&self, id: u64, input: PinInput) {
        push_event(Event::SnapPin { id, input });
    }

    fn screen_point(&self, hwnd: HWND, (x, y): (i32, i32)) -> Option<(i32, i32)> {
        let mut point = POINT { x, y };
        // SAFETY: hwnd belongs to this thread and point is a valid out-pointer.
        unsafe { ClientToScreen(hwnd, &mut point) }
            .as_bool()
            .then_some((point.x, point.y))
    }

    fn track(&self, hwnd: HWND, cancel: bool) -> bool {
        let mut tracking = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: if cancel {
                TME_LEAVE | TME_CANCEL
            } else {
                TME_LEAVE
            },
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        // SAFETY: the fully initialized request names this thread's live HWND.
        unsafe { TrackMouseEvent(&mut tracking) }.is_ok()
    }

    fn capture(&self, hwnd: HWND) -> bool {
        // SAFETY: a live HWND on this UI thread. SetCapture's return value is the
        // previous owner, so confirm success by querying the new owner instead.
        unsafe {
            let _ = SetCapture(hwnd);
            GetCapture() == hwnd
        }
    }

    fn owns_capture(&self, hwnd: HWND) -> bool {
        // SAFETY: read-only query of the calling thread's capture.
        unsafe { GetCapture() == hwnd }
    }

    fn release(&self) {
        // SAFETY: callers verified that the capture belongs to their exact HWND.
        unsafe {
            let _ = ReleaseCapture();
        }
    }

    fn paint(&self, hwnd: HWND) {
        let mut paint = PAINTSTRUCT::default();
        // SAFETY: paired BeginPaint/EndPaint with a valid out-buffer on this HWND's
        // WM_PAINT. Rendering happens later; validating now avoids a paint loop.
        unsafe {
            let _ = BeginPaint(hwnd, &mut paint);
            let _ = EndPaint(hwnd, &paint);
        }
    }

    fn primary_button(&self) -> u16 {
        // SAFETY: read-only mapping query. GetAsyncKeyState uses physical buttons:
        // https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-getasynckeystate
        primary_button(unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0)
    }

    fn button_down(&self, physical_button: u16) -> bool {
        // SAFETY: read-only physical button snapshot; no pointer or handle is used.
        unsafe { GetAsyncKeyState(i32::from(physical_button)) < 0 }
    }

    fn start_timer(&self, hwnd: HWND, timer: usize) -> bool {
        // SAFETY: HWND belongs to this UI thread; no TIMERPROC or borrowed pointer.
        unsafe { SetTimer(Some(hwnd), timer, DRAG_SAMPLE_MS, None) != 0 }
    }

    fn stop_timer(&self, hwnd: HWND, timer: usize) {
        // SAFETY: cancel only this HWND's exact press generation, on its UI thread.
        unsafe {
            let _ = KillTimer(Some(hwnd), timer);
        }
    }

    fn sample(&self, hwnd: HWND, physical_button: u16) -> Option<PointerSample> {
        let mut screen = POINT::default();
        // SAFETY: read-only snapshots and valid stack out-buffers. No input is sent
        // and no foreground/focus APIs are used by the drag sampler.
        unsafe {
            GetCursorPos(&mut screen).ok()?;
            let mut client = screen;
            if !ScreenToClient(hwnd, &mut client).as_bool() {
                return None;
            }
            Some(PointerSample {
                client: (client.x, client.y),
                screen: (screen.x, screen.y),
                down: self.button_down(physical_button),
            })
        }
    }
}

fn primary_button(swapped: bool) -> u16 {
    if swapped {
        VK_RBUTTON.0
    } else {
        VK_LBUTTON.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Message {
    Pointer(u32, (i32, i32)),
    Timer(usize),
    Hidden,
    Leave,
    CaptureChanged(HWND),
    Cancel,
    FocusLost,
    FocusGained,
    Input(PinInput),
    Paint,
    Destroy,
}

fn emit(state: &State, platform: &impl Platform, input: PinInput) {
    if state.live() {
        platform.send(state.id, input);
    }
}

fn release_quietly(state: &State, platform: &impl Platform) {
    stop_sampling(state, platform);
    let hwnd = state.hwnd.get();
    if hwnd.is_invalid() || !platform.owns_capture(hwnd) {
        return;
    }
    let previous = state.releasing.replace(true);
    platform.release();
    state.releasing.set(previous);
}

fn stop_sampling(state: &State, platform: &impl Platform) {
    if let Some(press) = state.press.take() {
        platform.stop_timer(state.hwnd.get(), press.timer);
    }
}

fn begin_press(state: &State, screen: (i32, i32), platform: &impl Platform) -> bool {
    // Background SetCapture does not promise outside-window motion or release:
    // https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-setcapture
    // Keep capture for native delivery, supplementing it only during this press.
    let Ok(timer) =
        NEXT_TIMER.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
    else {
        cancel_pointer(state, platform);
        return false;
    };
    let hwnd = state.hwnd.get();
    let physical_button = platform.primary_button();
    let press = Press {
        timer,
        physical_button,
        screen,
        // This runs while handling the WM_LBUTTONDOWN that just happened, so the
        // button must read down. If not, async state is hidden from this thread.
        button_blind: !platform.button_down(physical_button),
        up_samples: 0,
    };
    state.press.set(Some(press));
    let captured = platform.capture(hwnd);
    // Capture can synchronously cancel or destroy this very HWND.
    if !state.live() || state.press.get() != Some(press) {
        return false;
    }
    if !captured || !platform.start_timer(hwnd, timer) {
        cancel_pointer(state, platform);
        return false;
    }
    true
}

fn stop_tracking(state: &State, platform: &impl Platform) {
    if state.tracking.replace(false) && !state.hwnd.get().is_invalid() {
        let _ = platform.track(state.hwnd.get(), true);
    }
}

fn cancel_pointer(state: &State, platform: &impl Platform) {
    stop_tracking(state, platform);
    release_quietly(state, platform);
    emit(state, platform, PinInput::CancelCapture);
}

/// Ends pointer interaction only if there is something to end: an active press or
/// hover tracking emits CancelCapture; a stray capture is released silently.
fn cancel_active(state: &State, platform: &impl Platform) {
    if state.press.get().is_some() || state.tracking.get() {
        cancel_pointer(state, platform);
    } else {
        release_quietly(state, platform);
    }
}

fn route(state: &State, message: Message, platform: &impl Platform) {
    if !state.live() {
        return;
    }
    let hwnd = state.hwnd.get();
    match message {
        Message::Pointer(msg, point) => {
            if msg == WM_LBUTTONUP && state.press.get().is_none() {
                return; // A sampled release or cancellation already ended it.
            }
            // Snapshot screen coordinates before capture can synchronously cause
            // another window procedure to run (and potentially move this window).
            let screen = platform.screen_point(hwnd, point);
            if !state.live() {
                return;
            }
            if msg == WM_LBUTTONDOWN {
                if state.press.get().is_some() {
                    return;
                }
                let Some(screen) = screen else {
                    cancel_pointer(state, platform);
                    return;
                };
                if !begin_press(state, screen, platform) {
                    return;
                }
            } else if msg == WM_LBUTTONUP {
                release_quietly(state, platform);
            } else if msg == WM_MOUSEMOVE
                && !state.tracking.replace(true)
                && !platform.track(hwnd, false)
            {
                state.tracking.set(false);
            }
            let Some((screen_x, screen_y)) = screen else {
                cancel_pointer(state, platform);
                return;
            };
            let (x, y) = point;
            if msg == WM_MOUSEMOVE {
                if let Some(mut press) = state.press.get() {
                    if press.screen == (screen_x, screen_y) {
                        return;
                    }
                    press.screen = (screen_x, screen_y);
                    state.press.set(Some(press));
                }
            }
            let input = match msg {
                WM_LBUTTONDOWN => PinInput::Down {
                    x,
                    y,
                    screen_x,
                    screen_y,
                },
                WM_LBUTTONUP => PinInput::Up {
                    x,
                    y,
                    screen_x,
                    screen_y,
                },
                _ => PinInput::Move {
                    x,
                    y,
                    screen_x,
                    screen_y,
                },
            };
            emit(state, platform, input);
        }
        Message::Timer(timer) => {
            let Some(press) = state.press.get().filter(|press| press.timer == timer) else {
                return;
            };
            // Sample before judging capture: a release outside the window may drop
            // background capture first, and that is an Up, not a cancellation.
            let Some(sample) = platform.sample(hwnd, press.physical_button) else {
                cancel_pointer(state, platform);
                return;
            };
            if !state.live() || state.press.get() != Some(press) {
                return;
            }
            let captured = platform.owns_capture(hwnd);
            // A blind press can never observe a release; only native messages end it.
            let released = !press.button_blind && !sample.down;
            if !captured && !released {
                cancel_pointer(state, platform);
                return;
            }
            let (x, y) = sample.client;
            let (screen_x, screen_y) = sample.screen;
            let mut next = Press {
                up_samples: if released {
                    press.up_samples.saturating_add(1)
                } else {
                    0
                },
                ..press
            };
            // Capture already lost is corroboration enough; otherwise demand two
            // consecutive up readings before synthesizing the release.
            let finished = released && (!captured || next.up_samples >= RELEASE_SAMPLES);
            let moved = sample.screen != press.screen;
            if moved {
                next.screen = sample.screen;
            }
            state.press.set(Some(next));
            if moved {
                // Preserve the final drag position even if the last motion and the
                // release happened outside between two timer ticks.
                emit(
                    state,
                    platform,
                    PinInput::Move {
                        x,
                        y,
                        screen_x,
                        screen_y,
                    },
                );
            }
            if finished {
                release_quietly(state, platform);
                emit(
                    state,
                    platform,
                    PinInput::Up {
                        x,
                        y,
                        screen_x,
                        screen_y,
                    },
                );
            }
        }
        Message::Leave => {
            state.tracking.set(false);
            emit(state, platform, PinInput::Leave);
        }
        Message::CaptureChanged(gaining) => {
            if !state.releasing.get() && gaining != hwnd {
                cancel_pointer(state, platform);
            }
        }
        Message::Cancel => cancel_pointer(state, platform),
        Message::FocusLost => {
            cancel_active(state, platform);
            // WM_ACTIVATE(inactive) and WM_KILLFOCUS both describe one loss. Report
            // it once until focus or activation returns.
            if !state.focus_lost.replace(true) {
                emit(state, platform, PinInput::FocusLost);
            }
        }
        Message::FocusGained => state.focus_lost.set(false),
        Message::Hidden => cancel_active(state, platform),
        Message::Input(input) => {
            if input == PinInput::Close && state.press.get().is_some() {
                cancel_pointer(state, platform);
            }
            emit(state, platform, input);
        }
        Message::Paint => {
            platform.paint(hwnd);
            emit(state, platform, PinInput::Paint);
        }
        Message::Destroy => {
            // Clear the public handle before any reentrant call. The native
            // attachment is already detached; nested messages cannot find it.
            state.destroyed.set(true);
            let pressed = state.press.get().is_some();
            stop_sampling(state, platform);
            let tracking = state.tracking.replace(false);
            state.hwnd.set(HWND::default());
            if tracking {
                let _ = platform.track(hwnd, true);
            }
            let captured = platform.owns_capture(hwnd);
            if captured {
                platform.release();
            }
            if pressed || captured {
                platform.send(state.id, PinInput::CancelCapture);
            }
            platform.send(state.id, PinInput::Destroyed);
        }
    }
}

fn lparam_xy(lp: isize) -> (i32, i32) {
    let value = lp as u32;
    (
        i32::from(value as u16 as i16),
        i32::from((value >> 16) as u16 as i16),
    )
}

fn key_input(vk: usize, ctrl: bool, shift: bool, alt: bool) -> PinInput {
    PinInput::Key {
        vk: vk as u16,
        ctrl,
        shift,
        alt,
    }
}

/// Messages that contain only owned scalars. Key state and the borrowed DPI RECT
/// are copied separately by the native procedure before entering the router.
fn translate(msg: u32, wp: usize, lp: isize) -> Option<Message> {
    match msg {
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP => Some(Message::Pointer(msg, lparam_xy(lp))),
        WM_TIMER => Some(Message::Timer(wp)),
        WM_SHOWWINDOW if wp == 0 => Some(Message::Hidden),
        WM_MOUSELEAVE => Some(Message::Leave),
        WM_CAPTURECHANGED => Some(Message::CaptureChanged(HWND(lp as *mut _))),
        WM_CANCELMODE => Some(Message::Cancel),
        WM_KILLFOCUS => Some(Message::FocusLost),
        WM_ACTIVATE if (wp & 0xffff) as u32 == WA_INACTIVE => Some(Message::FocusLost),
        WM_ACTIVATE | WM_SETFOCUS => Some(Message::FocusGained),
        WM_PAINT => Some(Message::Paint),
        WM_CLOSE => Some(Message::Input(PinInput::Close)),
        WM_DISPLAYCHANGE => Some(Message::Input(PinInput::DisplayChanged)),
        // Broadcast for theme, policy, mouse and many other settings; only a work
        // area change affects pin placement.
        WM_SETTINGCHANGE if wp == SPI_SETWORKAREA.0 as usize => {
            Some(Message::Input(PinInput::DisplayChanged))
        }
        WM_NCDESTROY => Some(Message::Destroy),
        _ => None,
    }
}

extern "system" fn pin_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        // SAFETY: WM_NCCREATE supplies a CREATESTRUCTW valid for this callback.
        // Its argument is the local Rc in Window::new, still alive synchronously.
        // If the default handler fails, the attachment is reclaimed exactly once
        // here, because no WM_NCDESTROY is guaranteed after a failed creation.
        return unsafe {
            let creation = lp.0 as *const CREATESTRUCTW;
            if creation.is_null() || (*creation).lpCreateParams.is_null() {
                return LRESULT(0);
            }
            let source = &*((*creation).lpCreateParams as *const Rc<State>);
            let state = Rc::clone(source);
            state.hwnd.set(hwnd);
            let attached = Box::into_raw(Box::new(state));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, attached as isize);
            if GetWindowLongPtrW(hwnd, GWLP_USERDATA) != attached as isize {
                let state = Box::from_raw(attached);
                state.hwnd.set(HWND::default());
                return LRESULT(0);
            }
            // The default handler applies the CREATESTRUCT window text (title).
            let created = DefWindowProcW(hwnd, msg, wp, lp);
            if created.0 == 0 {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let state = Box::from_raw(attached);
                state.hwnd.set(HWND::default());
            }
            created
        };
    }
    // SAFETY: only WM_NCCREATE installs this pointer. Only WM_NCDESTROY removes
    // and frees it, on this same thread. Clone before any reentrant native call.
    let state = unsafe {
        let attached = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Rc<State>;
        if attached.is_null() {
            None
        } else {
            let state = Rc::clone(&*attached);
            if msg == WM_NCDESTROY {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(attached));
            }
            Some(state)
        }
    };
    if let Some(state) = state {
        let message = match msg {
            WM_ERASEBKGND => return LRESULT(1),
            WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
            WM_GETDLGCODE => return LRESULT((DLGC_WANTALLKEYS | DLGC_WANTTAB) as isize),
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                // SAFETY: read-only state snapshots on the receiving UI thread.
                let key = unsafe {
                    key_input(
                        wp.0,
                        GetKeyState(i32::from(VK_CONTROL.0)) < 0,
                        GetKeyState(i32::from(VK_SHIFT.0)) < 0,
                        GetKeyState(i32::from(VK_MENU.0)) < 0,
                    )
                };
                Some(Message::Input(key))
            }
            WM_DPICHANGED if lp.0 != 0 => {
                // SAFETY: Windows supplies a RECT valid only during WM_DPICHANGED;
                // copy it now, never queue its pointer or apply native placement here.
                let suggested = unsafe { *(lp.0 as *const RECT) };
                Some(Message::Input(PinInput::Dpi {
                    dpi: (wp.0 as u16 as u32).max(96),
                    suggested: rect_bounds(suggested),
                }))
            }
            _ => translate(msg, wp.0, lp.0),
        };
        if let Some(message) = message {
            route(&state, message, &Native);
            // Default activation sets keyboard focus; default system-key handling
            // retains Alt+F4's WM_CLOSE. NC teardown must also reach DefWindowProc.
            if !matches!(
                msg,
                WM_ACTIVATE | WM_SETFOCUS | WM_KILLFOCUS | WM_SYSKEYDOWN | WM_NCDESTROY
            ) {
                return LRESULT(0);
            }
        }
    }
    // SAFETY: unhandled messages retain their original native arguments.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    type Hook = Box<dyn Fn(HWND)>;

    #[derive(Default)]
    struct Fake {
        events: RefCell<Vec<(u64, PinInput)>>,
        origins: RefCell<HashMap<isize, (i32, i32)>>,
        capture: Cell<HWND>,
        tracks: RefCell<Vec<(HWND, bool)>>,
        paints: RefCell<Vec<HWND>>,
        release_hook: RefCell<Option<Hook>>,
        capture_hook: RefCell<Option<Hook>>,
        fail_track: Cell<bool>,
        fail_capture: Cell<bool>,
        timers: RefCell<HashMap<(isize, usize), u32>>,
        stopped: RefCell<Vec<(HWND, usize)>>,
        samples: RefCell<HashMap<isize, PointerSample>>,
        sampled: RefCell<Vec<(HWND, u16)>>,
        swapped: Cell<bool>,
        fail_timer: Cell<bool>,
        // The async button probe reads "up" (e.g. elevated foreground window).
        button_hidden: Cell<bool>,
        probed: RefCell<Vec<u16>>,
    }

    impl Platform for Fake {
        fn send(&self, id: u64, input: PinInput) {
            self.events.borrow_mut().push((id, input));
        }
        fn screen_point(&self, hwnd: HWND, (x, y): (i32, i32)) -> Option<(i32, i32)> {
            self.origins
                .borrow()
                .get(&(hwnd.0 as isize))
                .map(|&(sx, sy)| (sx + x, sy + y))
        }
        fn track(&self, hwnd: HWND, cancel: bool) -> bool {
            self.tracks.borrow_mut().push((hwnd, cancel));
            !self.fail_track.get()
        }
        fn capture(&self, hwnd: HWND) -> bool {
            if self.fail_capture.get() {
                return false;
            }
            self.capture.set(hwnd);
            // Take the hook before invoking it: no test RefCell borrow survives
            // reentrancy either, mirroring the production routing requirement.
            let hook = self.capture_hook.borrow_mut().take();
            if let Some(hook) = hook {
                hook(hwnd);
            }
            self.capture.get() == hwnd
        }
        fn owns_capture(&self, hwnd: HWND) -> bool {
            self.capture.get() == hwnd
        }
        fn release(&self) {
            let prior = self.capture.replace(HWND::default());
            let hook = self.release_hook.borrow_mut().take();
            if let Some(hook) = hook {
                hook(prior);
            }
        }
        fn paint(&self, hwnd: HWND) {
            self.paints.borrow_mut().push(hwnd);
        }
        fn primary_button(&self) -> u16 {
            primary_button(self.swapped.get())
        }
        fn button_down(&self, physical_button: u16) -> bool {
            self.probed.borrow_mut().push(physical_button);
            !self.button_hidden.get()
        }
        fn start_timer(&self, hwnd: HWND, timer: usize) -> bool {
            if self.fail_timer.get() {
                return false;
            }
            self.timers
                .borrow_mut()
                .insert((hwnd.0 as isize, timer), DRAG_SAMPLE_MS);
            true
        }
        fn stop_timer(&self, hwnd: HWND, timer: usize) {
            self.timers.borrow_mut().remove(&(hwnd.0 as isize, timer));
            self.stopped.borrow_mut().push((hwnd, timer));
        }
        fn sample(&self, hwnd: HWND, physical_button: u16) -> Option<PointerSample> {
            self.sampled.borrow_mut().push((hwnd, physical_button));
            self.samples.borrow().get(&(hwnd.0 as isize)).copied()
        }
    }

    fn state(id: u64, raw: isize) -> Rc<State> {
        let state = Rc::new(State::new(id));
        state.hwnd.set(HWND(raw as *mut _));
        state
    }

    fn press(state: &State, native: &Fake) -> usize {
        native
            .origins
            .borrow_mut()
            .insert(state.hwnd.get().0 as isize, (-100, 20));
        route(state, Message::Pointer(WM_LBUTTONDOWN, (10, 10)), native);
        state.press.get().unwrap().timer
    }

    #[test]
    fn sampler_follows_pointer_outside_and_releases_once_without_native_delivery() {
        let native = Fake::default();
        let a = state(1, 101);
        native.swapped.set(true);
        let timer = press(&a, &native);
        assert_eq!(native.timers.borrow().len(), 1);
        assert_eq!(native.timers.borrow()[&(101, timer)], 16);
        native.samples.borrow_mut().insert(
            101,
            PointerSample {
                client: (-500, 900),
                screen: (-600, 920),
                down: true,
            },
        );
        route(&a, Message::Timer(timer), &native);
        route(&a, Message::Timer(timer), &native); // Stationary samples are quiet.
        route(&a, Message::Pointer(WM_MOUSEMOVE, (-500, 900)), &native);
        native.samples.borrow_mut().get_mut(&101).unwrap().down = false;
        route(&a, Message::Timer(timer), &native); // First up reading: still held.
        assert!(a.press.get().is_some());
        route(&a, Message::Timer(timer), &native);
        route(&a, Message::Pointer(WM_LBUTTONUP, (-500, 900)), &native);
        let sampled = native.sampled.borrow().len();
        route(&a, Message::Timer(timer), &native); // Already posted before KillTimer.
        assert_eq!(native.sampled.borrow().len(), sampled);
        assert!(native
            .sampled
            .borrow()
            .iter()
            .all(|&(_, key)| key == VK_RBUTTON.0));
        assert!(a.press.get().is_none());
        assert!(native.timers.borrow().is_empty());
        assert!(native.capture.get().is_invalid());
        assert_eq!(
            *native.events.borrow(),
            vec![
                (
                    1,
                    PinInput::Down {
                        x: 10,
                        y: 10,
                        screen_x: -90,
                        screen_y: 30
                    }
                ),
                (
                    1,
                    PinInput::Move {
                        x: -500,
                        y: 900,
                        screen_x: -600,
                        screen_y: 920
                    }
                ),
                (
                    1,
                    PinInput::Up {
                        x: -500,
                        y: 900,
                        screen_x: -600,
                        screen_y: 920
                    }
                ),
            ]
        );
    }

    #[test]
    fn native_release_disarms_sampler_and_native_motion_shares_sample_deduplication() {
        let native = Fake::default();
        let a = state(1, 101);
        let timer = press(&a, &native);
        route(&a, Message::Pointer(WM_MOUSEMOVE, (20, 15)), &native);
        native.samples.borrow_mut().insert(
            101,
            PointerSample {
                client: (20, 15),
                screen: (-80, 35),
                down: true,
            },
        );
        route(&a, Message::Timer(timer), &native);
        route(&a, Message::Pointer(WM_LBUTTONUP, (21, 16)), &native);
        let sampled = native.sampled.borrow().len();
        route(&a, Message::Timer(timer), &native);
        assert_eq!(native.sampled.borrow().len(), sampled);
        assert_eq!(native.events.borrow().len(), 3);
        assert_eq!(
            native.events.borrow()[2],
            (
                1,
                PinInput::Up {
                    x: 21,
                    y: 16,
                    screen_x: -79,
                    screen_y: 36,
                }
            )
        );
        assert_eq!(*native.stopped.borrow(), vec![(a.hwnd.get(), timer)]);
        assert!(native.timers.borrow().is_empty());
        assert_eq!(primary_button(false), VK_LBUTTON.0);
        assert_eq!(primary_button(true), VK_RBUTTON.0);
    }

    #[test]
    fn every_cancellation_path_disarms_only_its_owned_timer() {
        for message in [
            Message::Cancel,
            Message::FocusLost,
            Message::Hidden,
            Message::Input(PinInput::Close),
            Message::Destroy,
            Message::CaptureChanged(HWND(999 as *mut _)),
        ] {
            let native = Fake::default();
            let a = state(1, 101);
            let b = state(2, 102);
            let timer_a = press(&a, &native);
            let timer_b = press(&b, &native);
            native.events.borrow_mut().clear();
            route(&a, message, &native);
            route(&a, Message::Timer(timer_a), &native);
            route(&a, Message::Pointer(WM_LBUTTONUP, (10, 10)), &native);
            assert!(a.press.get().is_none(), "{message:?}");
            assert_eq!(native.capture.get(), b.hwnd.get(), "{message:?}");
            assert_eq!(native.timers.borrow().len(), 1, "{message:?}");
            assert!(native.timers.borrow().contains_key(&(102, timer_b)));
            assert!(native.sampled.borrow().is_empty());
            assert_eq!(
                native
                    .events
                    .borrow()
                    .iter()
                    .filter(|(_, event)| *event == PinInput::CancelCapture)
                    .count(),
                1,
                "{message:?}"
            );
            assert!(!native
                .events
                .borrow()
                .iter()
                .any(|(_, event)| matches!(event, PinInput::Move { .. } | PinInput::Up { .. })));
        }
    }

    #[test]
    fn stale_timer_generations_cannot_sample_a_new_press_or_reused_hwnd() {
        let native = Fake::default();
        let a = state(1, 101);
        let old = press(&a, &native);
        route(&a, Message::Cancel, &native);
        let current = press(&a, &native);
        assert_ne!(old, current);
        route(&a, Message::Timer(old), &native);
        assert!(native.sampled.borrow().is_empty());
        route(&a, Message::Destroy, &native);
        let replacement = state(2, 101);
        let new = press(&replacement, &native);
        assert_ne!(new, current);
        native.events.borrow_mut().clear();
        route(&replacement, Message::Timer(old), &native);
        route(&replacement, Message::Timer(current), &native);
        assert!(native.sampled.borrow().is_empty());
        assert!(native.events.borrow().is_empty());
        assert_eq!(native.timers.borrow().len(), 1);
    }

    #[test]
    fn timer_failure_sample_failure_and_unreported_capture_loss_cancel_safely() {
        let native = Fake::default();
        let a = state(1, 101);
        native.origins.borrow_mut().insert(101, (0, 0));
        native.fail_timer.set(true);
        route(&a, Message::Pointer(WM_LBUTTONDOWN, (1, 2)), &native);
        assert!(a.press.get().is_none());
        assert!(native.capture.get().is_invalid());
        assert_eq!(*native.events.borrow(), vec![(1, PinInput::CancelCapture)]);
        native.fail_timer.set(false);
        let timer = press(&a, &native);
        route(&a, Message::Timer(timer), &native); // Failed cursor conversion.
        assert!(native.capture.get().is_invalid());
        let timer = press(&a, &native);
        let foreign = HWND(999 as *mut _);
        native.capture.set(foreign);
        native.samples.borrow_mut().insert(
            101,
            PointerSample {
                client: (10, 10),
                screen: (-90, 30),
                down: true,
            },
        );
        let queries = native.sampled.borrow().len();
        route(&a, Message::Timer(timer), &native);
        assert_eq!(native.sampled.borrow().len(), queries + 1); // Sampled first.
        assert_eq!(native.capture.get(), foreign);
        assert!(a.press.get().is_none());
        assert!(native.timers.borrow().is_empty());
    }

    #[test]
    fn independent_timer_ownership_ignores_other_pin_generation() {
        let native = Fake::default();
        let a = state(1, 101);
        let b = state(2, 102);
        let timer_a = press(&a, &native);
        let timer_b = press(&b, &native);
        route(&b, Message::Timer(timer_a), &native);
        release_quietly(&a, &native);
        assert!(native.sampled.borrow().is_empty());
        assert_eq!(native.capture.get(), b.hwnd.get());
        assert_eq!(native.timers.borrow().len(), 1);
        assert!(native.timers.borrow().contains_key(&(102, timer_b)));
        assert!(native
            .events
            .borrow()
            .iter()
            .all(|(_, event)| !matches!(event, PinInput::Up { .. } | PinInput::CancelCapture)));
    }

    #[test]
    fn outside_motion_and_release_between_ticks_keep_the_final_position() {
        let native = Fake::default();
        let a = state(1, 101);
        let timer = press(&a, &native);
        native.events.borrow_mut().clear();
        native.samples.borrow_mut().insert(
            101,
            PointerSample {
                client: (-800, -900),
                screen: (-900, -880),
                down: false,
            },
        );
        route(&a, Message::Timer(timer), &native);
        assert_eq!(native.events.borrow().len(), 1); // Move only; Up awaits a 2nd reading.
        route(&a, Message::Timer(timer), &native);
        assert_eq!(
            *native.events.borrow(),
            vec![
                (
                    1,
                    PinInput::Move {
                        x: -800,
                        y: -900,
                        screen_x: -900,
                        screen_y: -880
                    }
                ),
                (
                    1,
                    PinInput::Up {
                        x: -800,
                        y: -900,
                        screen_x: -900,
                        screen_y: -880
                    }
                ),
            ]
        );
        assert!(native.timers.borrow().is_empty());
    }

    #[test]
    fn windows_route_to_their_own_ids_and_signed_screen_coordinates() {
        let native = Fake::default();
        let a = state(7, 101);
        let b = state(8, 102);
        native
            .origins
            .borrow_mut()
            .extend([(101, (-1920, -200)), (102, (300, 400))]);
        route(&a, Message::Pointer(WM_MOUSEMOVE, (-4, 6)), &native);
        route(&b, Message::Pointer(WM_LBUTTONDOWN, (10, -2)), &native);
        assert_eq!(
            *native.events.borrow(),
            vec![
                (
                    7,
                    PinInput::Move {
                        x: -4,
                        y: 6,
                        screen_x: -1924,
                        screen_y: -194
                    }
                ),
                (
                    8,
                    PinInput::Down {
                        x: 10,
                        y: -2,
                        screen_x: 310,
                        screen_y: 398
                    }
                ),
            ]
        );
        assert!(a.tracking.get());
        assert!(!b.tracking.get());
        assert_eq!(native.capture.get(), b.hwnd.get());
    }

    #[test]
    fn tracking_leave_hide_and_retry_are_per_window() {
        let native = Fake::default();
        let a = state(1, 101);
        let b = state(2, 102);
        native
            .origins
            .borrow_mut()
            .extend([(101, (0, 0)), (102, (0, 0))]);
        for window in [&a, &b, &a] {
            route(window, Message::Pointer(WM_MOUSEMOVE, (0, 0)), &native);
        }
        assert_eq!(native.tracks.borrow().len(), 2);
        route(&a, Message::Leave, &native);
        assert!(!a.tracking.get());
        assert!(b.tracking.get());
        native.fail_track.set(true);
        route(&a, Message::Pointer(WM_MOUSEMOVE, (0, 0)), &native);
        assert!(!a.tracking.get());
        native.fail_track.set(false);
        route(&a, Message::Pointer(WM_MOUSEMOVE, (0, 0)), &native);
        cancel_pointer(&a, &native);
        assert!(!a.tracking.get());
        assert!(b.tracking.get());
        assert_eq!(native.tracks.borrow().last(), Some(&(a.hwnd.get(), true)));
    }

    #[test]
    fn synchronous_own_release_is_quiet_but_external_capture_loss_cancels() {
        let native = Rc::new(Fake::default());
        let a = state(1, 101);
        let b = state(2, 102);
        native.capture.set(a.hwnd.get());
        let owner = a.clone();
        let platform = Rc::downgrade(&native);
        *native.release_hook.borrow_mut() = Some(Box::new(move |_| {
            route(
                &owner,
                Message::CaptureChanged(HWND::default()),
                platform.upgrade().unwrap().as_ref(),
            );
        }));
        release_quietly(&b, native.as_ref());
        assert_eq!(native.capture.get(), a.hwnd.get());
        release_quietly(&a, native.as_ref());
        assert!(native.events.borrow().is_empty());
        assert!(!a.releasing.get());
        route(&a, Message::CaptureChanged(b.hwnd.get()), native.as_ref());
        route(&b, Message::CaptureChanged(b.hwnd.get()), native.as_ref());
        assert_eq!(*native.events.borrow(), vec![(1, PinInput::CancelCapture)]);
    }

    #[test]
    fn mouse_up_releases_only_its_owner_and_keeps_message_coordinates() {
        let native = Fake::default();
        let a = state(1, 101);
        let b = state(2, 102);
        native
            .origins
            .borrow_mut()
            .extend([(101, (-700, 40)), (102, (20, 30))]);
        route(&a, Message::Pointer(WM_LBUTTONDOWN, (0, 0)), &native);
        route(&b, Message::Pointer(WM_LBUTTONDOWN, (0, 0)), &native);
        native.events.borrow_mut().clear();
        route(&a, Message::Pointer(WM_LBUTTONUP, (-8, 12)), &native);
        assert_eq!(native.capture.get(), b.hwnd.get());
        route(&b, Message::Pointer(WM_LBUTTONUP, (4, 5)), &native);
        assert!(native.capture.get().is_invalid());
        assert_eq!(
            *native.events.borrow(),
            vec![
                (
                    1,
                    PinInput::Up {
                        x: -8,
                        y: 12,
                        screen_x: -708,
                        screen_y: 52
                    }
                ),
                (
                    2,
                    PinInput::Up {
                        x: 4,
                        y: 5,
                        screen_x: 24,
                        screen_y: 35
                    }
                ),
            ]
        );
    }

    #[test]
    fn failed_capture_or_coordinate_conversion_cancels_without_starting_drag() {
        let native = Fake::default();
        let a = state(1, 101);
        native.origins.borrow_mut().insert(101, (0, 0));
        native.fail_capture.set(true);
        route(&a, Message::Pointer(WM_LBUTTONDOWN, (2, 3)), &native);
        native.fail_capture.set(false);
        native.origins.borrow_mut().clear();
        route(&a, Message::Pointer(WM_LBUTTONDOWN, (2, 3)), &native);
        assert!(native.capture.get().is_invalid());
        assert_eq!(
            *native.events.borrow(),
            vec![(1, PinInput::CancelCapture), (1, PinInput::CancelCapture)]
        );
    }

    #[test]
    fn destruction_is_exactly_once_invalidates_owner_and_preserves_other_windows() {
        let native = Fake::default();
        let a = state(1, 101);
        let b = state(2, 102);
        native.capture.set(a.hwnd.get());
        a.tracking.set(true);
        b.tracking.set(true);
        route(&a, Message::Destroy, &native);
        route(&a, Message::Destroy, &native);
        route(&a, Message::Input(PinInput::Close), &native);
        route(&b, Message::Input(PinInput::Close), &native);
        assert!(!a.live());
        assert!(a.hwnd.get().is_invalid());
        assert!(!a.tracking.get());
        assert!(b.live());
        assert!(b.tracking.get());
        assert!(native.capture.get().is_invalid());
        assert_eq!(
            *native.events.borrow(),
            vec![
                (1, PinInput::CancelCapture),
                (1, PinInput::Destroyed),
                (2, PinInput::Close),
            ]
        );
    }

    #[test]
    fn destruction_during_reentrant_capture_does_not_emit_a_late_press() {
        let native = Rc::new(Fake::default());
        let a = state(1, 101);
        native.origins.borrow_mut().insert(101, (0, 0));
        let owner = a.clone();
        let platform = Rc::downgrade(&native);
        *native.capture_hook.borrow_mut() = Some(Box::new(move |_| {
            route(
                &owner,
                Message::Destroy,
                platform.upgrade().unwrap().as_ref(),
            );
        }));
        route(
            &a,
            Message::Pointer(WM_LBUTTONDOWN, (1, 2)),
            native.as_ref(),
        );
        assert_eq!(
            *native.events.borrow(),
            vec![(1, PinInput::CancelCapture), (1, PinInput::Destroyed)]
        );
        assert!(!a.live());
    }

    #[test]
    fn paint_is_validated_once_and_dpi_display_keyboard_events_remain_isolated() {
        let native = Fake::default();
        let a = state(1, 101);
        let b = state(2, 102);
        route(&a, Message::Paint, &native);
        let key = PinInput::Key {
            vk: 0x43,
            ctrl: true,
            shift: true,
            alt: true,
        };
        route(&b, Message::Input(key), &native);
        for dpi in [96, 144, 192] {
            route(
                &a,
                Message::Input(PinInput::Dpi {
                    dpi,
                    suggested: (-1900, -30, 640, 480),
                }),
                &native,
            );
        }
        route(&b, Message::Input(PinInput::DisplayChanged), &native);
        assert_eq!(*native.paints.borrow(), vec![a.hwnd.get()]);
        let events = native.events.borrow();
        assert_eq!(events[0], (1, PinInput::Paint));
        assert_eq!(events[1], (2, key));
        assert_eq!(events.len(), 6);
        assert_eq!(events[5], (2, PinInput::DisplayChanged));
    }

    #[test]
    fn focus_loss_and_cancel_cleanup_capture_without_touching_other_pins() {
        let native = Fake::default();
        let a = state(1, 101);
        let b = state(2, 102);
        native.capture.set(b.hwnd.get());
        route(&a, Message::FocusLost, &native);
        assert_eq!(native.capture.get(), b.hwnd.get());
        route(&b, Message::Cancel, &native);
        assert!(native.capture.get().is_invalid());
        assert_eq!(
            *native.events.borrow(),
            vec![(1, PinInput::FocusLost), (2, PinInput::CancelCapture),]
        );
    }

    fn sample_at(native: &Fake, raw: isize, screen: (i32, i32), down: bool) {
        native.samples.borrow_mut().insert(
            raw,
            PointerSample {
                client: (screen.0 + 100, screen.1 - 20),
                screen,
                down,
            },
        );
    }

    #[test]
    fn press_probes_the_physical_primary_button_once_while_handling_down() {
        let native = Fake::default();
        let a = state(1, 101);
        native.swapped.set(true);
        let timer = press(&a, &native);
        assert_eq!(*native.probed.borrow(), vec![VK_RBUTTON.0]);
        assert!(!a.press.get().unwrap().button_blind);
        sample_at(&native, 101, (-80, 40), true);
        route(&a, Message::Timer(timer), &native);
        assert_eq!(native.probed.borrow().len(), 1); // Ticks use sample(), not the probe.
    }

    #[test]
    fn blind_press_tracks_motion_but_only_native_release_ends_it() {
        let native = Fake::default();
        let a = state(1, 101);
        native.button_hidden.set(true);
        let timer = press(&a, &native);
        assert!(a.press.get().unwrap().button_blind);
        native.events.borrow_mut().clear();
        // The async state reads "up" on every tick, yet the drag keeps following.
        for step in 1..=5 {
            sample_at(&native, 101, (-100 - step * 10, 20), false);
            route(&a, Message::Timer(timer), &native);
        }
        assert!(a.press.get().is_some());
        assert_eq!(native.timers.borrow().len(), 1);
        assert_eq!(native.capture.get(), a.hwnd.get());
        let events = native.events.borrow().clone();
        assert_eq!(events.len(), 5);
        assert!(events
            .iter()
            .all(|(_, event)| matches!(event, PinInput::Move { .. })));
        drop(events);
        route(&a, Message::Pointer(WM_LBUTTONUP, (-60, 0)), &native);
        let events = native.events.borrow();
        assert!(matches!(events.last(), Some((1, PinInput::Up { .. }))));
        assert_eq!(
            events
                .iter()
                .filter(|(_, event)| matches!(event, PinInput::Up { .. }))
                .count(),
            1
        );
        assert!(a.press.get().is_none());
        assert!(native.timers.borrow().is_empty());
    }

    #[test]
    fn blind_press_still_cancels_when_capture_is_lost() {
        let native = Fake::default();
        let a = state(1, 101);
        native.button_hidden.set(true);
        let timer = press(&a, &native);
        native.events.borrow_mut().clear();
        native.capture.set(HWND(999 as *mut _));
        sample_at(&native, 101, (-50, 60), false);
        route(&a, Message::Timer(timer), &native);
        assert_eq!(*native.events.borrow(), vec![(1, PinInput::CancelCapture)]);
        assert!(a.press.get().is_none());
        assert!(native.timers.borrow().is_empty());
    }

    #[test]
    fn sampled_release_needs_two_consecutive_up_readings() {
        let native = Fake::default();
        let a = state(1, 101);
        let timer = press(&a, &native);
        native.events.borrow_mut().clear();
        sample_at(&native, 101, (-90, 30), false); // Stationary; one stale reading.
        route(&a, Message::Timer(timer), &native);
        assert!(a.press.get().is_some());
        assert!(native.events.borrow().is_empty());
        sample_at(&native, 101, (-90, 30), true); // The button is really still down.
        route(&a, Message::Timer(timer), &native);
        assert_eq!(a.press.get().unwrap().up_samples, 0);
        sample_at(&native, 101, (-90, 30), false);
        route(&a, Message::Timer(timer), &native);
        assert!(native.events.borrow().is_empty(), "the up count was reset");
        route(&a, Message::Timer(timer), &native);
        assert_eq!(native.events.borrow().len(), 1);
        assert!(matches!(
            native.events.borrow()[0],
            (1, PinInput::Up { .. })
        ));
        assert!(a.press.get().is_none());
        assert!(native.timers.borrow().is_empty());
    }

    #[test]
    fn button_up_with_lost_capture_is_a_final_move_and_up_not_a_cancel() {
        let native = Fake::default();
        let a = state(1, 101);
        let timer = press(&a, &native);
        native.events.borrow_mut().clear();
        native.capture.set(HWND::default()); // Windows dropped capture at release.
        sample_at(&native, 101, (-700, 300), false);
        route(&a, Message::Timer(timer), &native);
        assert_eq!(
            *native.events.borrow(),
            vec![
                (
                    1,
                    PinInput::Move {
                        x: -600,
                        y: 280,
                        screen_x: -700,
                        screen_y: 300
                    }
                ),
                (
                    1,
                    PinInput::Up {
                        x: -600,
                        y: 280,
                        screen_x: -700,
                        screen_y: 300
                    }
                ),
            ]
        );
        assert!(a.press.get().is_none());
        assert!(native.timers.borrow().is_empty());
        route(&a, Message::Pointer(WM_LBUTTONUP, (0, 0)), &native);
        assert_eq!(native.events.borrow().len(), 2, "exactly one Up per press");
    }

    #[test]
    fn button_still_down_with_lost_capture_cancels_after_sampling() {
        let native = Fake::default();
        let a = state(1, 101);
        let timer = press(&a, &native);
        native.events.borrow_mut().clear();
        native.capture.set(HWND(999 as *mut _));
        sample_at(&native, 101, (-70, 25), true);
        route(&a, Message::Timer(timer), &native);
        assert_eq!(native.sampled.borrow().len(), 1);
        assert_eq!(*native.events.borrow(), vec![(1, PinInput::CancelCapture)]);
        assert!(a.press.get().is_none());
    }

    #[test]
    fn hide_and_drop_cancel_only_an_active_press_or_tracking() {
        let native = Fake::default();
        let a = state(1, 101);
        cancel_active(&a, &native);
        assert!(native.events.borrow().is_empty());
        assert!(native.tracks.borrow().is_empty());
        route(&a, Message::Hidden, &native);
        assert!(native.events.borrow().is_empty());
        a.tracking.set(true);
        cancel_active(&a, &native);
        cancel_active(&a, &native);
        assert_eq!(*native.events.borrow(), vec![(1, PinInput::CancelCapture)]);
        native.events.borrow_mut().clear();
        press(&a, &native);
        native.events.borrow_mut().clear();
        cancel_active(&a, &native);
        cancel_active(&a, &native);
        assert_eq!(*native.events.borrow(), vec![(1, PinInput::CancelCapture)]);
        assert!(native.capture.get().is_invalid());
        assert!(native.timers.borrow().is_empty());
        // A stray capture without a press is released without any event.
        native.events.borrow_mut().clear();
        native.capture.set(a.hwnd.get());
        cancel_active(&a, &native);
        assert!(native.capture.get().is_invalid());
        assert!(native.events.borrow().is_empty());
    }

    #[test]
    fn one_deactivation_reports_one_focus_loss_until_focus_returns() {
        let native = Fake::default();
        let a = state(1, 101);
        let b = state(2, 102);
        // WM_ACTIVATE(inactive) followed by WM_KILLFOCUS.
        route(&a, Message::FocusLost, &native);
        route(&a, Message::FocusLost, &native);
        route(&b, Message::FocusLost, &native);
        assert_eq!(
            *native.events.borrow(),
            vec![(1, PinInput::FocusLost), (2, PinInput::FocusLost)]
        );
        // A later real loss after regaining is reported again.
        route(&a, Message::FocusGained, &native);
        route(&a, Message::FocusLost, &native);
        route(&a, Message::FocusLost, &native);
        assert_eq!(native.events.borrow().len(), 3);
        assert_eq!(native.events.borrow()[2], (1, PinInput::FocusLost));
        // A press in flight is still cancelled once, before the single FocusLost.
        route(&b, Message::FocusGained, &native);
        native.events.borrow_mut().clear();
        press(&b, &native);
        native.events.borrow_mut().clear();
        route(&b, Message::FocusLost, &native);
        route(&b, Message::FocusLost, &native);
        assert_eq!(
            *native.events.borrow(),
            vec![(2, PinInput::CancelCapture), (2, PinInput::FocusLost)]
        );
    }

    #[derive(Default)]
    struct FakeDesktop {
        foreground: Cell<isize>,
        classes: RefCell<HashMap<isize, &'static str>>,
        hidden: RefCell<Vec<isize>>,
        identities: RefCell<HashMap<isize, Identity>>,
    }

    impl FakeDesktop {
        fn window(&self, raw: isize, class: &'static str) {
            self.classes.borrow_mut().insert(raw, class);
            self.identities.borrow_mut().insert(
                raw,
                Identity {
                    pid: raw as u32,
                    thread: 1,
                    created: 5,
                },
            );
        }

        fn destroy(&self, raw: isize) {
            self.classes.borrow_mut().remove(&raw);
            self.identities.borrow_mut().remove(&raw);
        }
    }

    impl Desktop for FakeDesktop {
        fn foreground(&self) -> HWND {
            HWND(self.foreground.get() as *mut _)
        }
        fn class_name(&self, hwnd: HWND) -> Option<String> {
            self.classes
                .borrow()
                .get(&(hwnd.0 as isize))
                .map(|class| class.to_string())
        }
        fn usable(&self, hwnd: HWND) -> bool {
            let raw = hwnd.0 as isize;
            self.classes.borrow().contains_key(&raw) && !self.hidden.borrow().contains(&raw)
        }
        fn identity(&self, hwnd: HWND) -> Option<Identity> {
            self.identities.borrow().get(&(hwnd.0 as isize)).copied()
        }
    }

    fn last(raw: isize) -> LastForeground {
        LastForeground(AtomicIsize::new(raw))
    }

    #[test]
    fn shell_tray_classes_are_matched_exactly_and_case_insensitively() {
        for class in [
            "Shell_TrayWnd",
            "Shell_SecondaryTrayWnd",
            "NotifyIconOverflowWindow",
            "TopLevelWindowForOverflowXamlIsland",
            "shell_traywnd",
        ] {
            assert!(is_shell_tray_class(class), "{class}");
        }
        for class in [
            "",
            "Notepad",
            "DaijobuSnapPin",
            "Shell_TrayWnd2",
            "Shell_Tray",
            "Progman",
            "WorkerW",
        ] {
            assert!(!is_shell_tray_class(class), "{class}");
        }
    }

    #[test]
    fn a_normal_foreground_is_the_target_and_ignores_the_record() {
        let desktop = FakeDesktop::default();
        desktop.window(10, "Notepad");
        desktop.window(20, "Chrome_WidgetWin_1");
        desktop.foreground.set(10);
        let target = last(20).target(&desktop).unwrap();
        assert!(target.is_window(HWND(10 as *mut _)));
        assert_eq!(target.identity.pid, 10);
    }

    #[test]
    fn a_shell_foreground_falls_back_to_the_recorded_window_with_fresh_identity() {
        let desktop = FakeDesktop::default();
        desktop.window(20, "Chrome_WidgetWin_1");
        desktop.window(30, "Shell_TrayWnd");
        desktop.foreground.set(30);
        let record = last(0);
        record.note(HWND(20 as *mut _), &desktop);
        // The recorded process is replaced before the snapshot: identity is current.
        desktop.identities.borrow_mut().insert(
            20,
            Identity {
                pid: 77,
                thread: 2,
                created: 99,
            },
        );
        let target = record.target(&desktop).unwrap();
        assert!(target.is_window(HWND(20 as *mut _)));
        assert_eq!(target.identity.pid, 77);
        assert_eq!(target.identity.created, 99);
    }

    #[test]
    fn shell_windows_never_overwrite_the_record_and_never_become_the_target() {
        let desktop = FakeDesktop::default();
        for (raw, class) in [
            (20, "Notepad"),
            (30, "Shell_TrayWnd"),
            (31, "Shell_SecondaryTrayWnd"),
            (32, "NotifyIconOverflowWindow"),
            (33, "TopLevelWindowForOverflowXamlIsland"),
        ] {
            desktop.window(raw, class);
        }
        let record = last(0);
        record.note(HWND(20 as *mut _), &desktop);
        for raw in [30, 31, 32, 33] {
            record.note(HWND(raw as *mut _), &desktop);
            desktop.foreground.set(raw);
            let target = record.target(&desktop).unwrap();
            assert!(target.is_window(HWND(20 as *mut _)), "{raw}");
        }
        assert_eq!(record.0.load(Ordering::Relaxed), 20);
    }

    #[test]
    fn a_pin_foreground_is_recorded_so_repeated_tray_recovery_can_cycle() {
        let desktop = FakeDesktop::default();
        desktop.window(40, "DaijobuSnapPin");
        desktop.window(30, "Shell_TrayWnd");
        let record = last(0);
        record.note(HWND(40 as *mut _), &desktop);
        desktop.foreground.set(30);
        assert!(record
            .target(&desktop)
            .unwrap()
            .is_window(HWND(40 as *mut _)));
    }

    #[test]
    fn missing_stale_hidden_or_shell_records_yield_no_target() {
        let desktop = FakeDesktop::default();
        desktop.window(30, "Shell_TrayWnd");
        desktop.window(20, "Notepad");
        desktop.foreground.set(30);
        assert!(last(0).target(&desktop).is_none()); // Nothing recorded.
        assert!(last(555).target(&desktop).is_none()); // Never existed.
        let record = last(0);
        record.note(HWND(20 as *mut _), &desktop);
        desktop.hidden.borrow_mut().push(20);
        assert!(record.target(&desktop).is_none()); // No longer visible.
        desktop.hidden.borrow_mut().clear();
        assert!(record.target(&desktop).is_some());
        desktop.destroy(20);
        assert!(record.target(&desktop).is_none()); // Destroyed.
                                                    // A recycled handle that now has a shell class is not trusted.
        desktop.window(20, "Shell_SecondaryTrayWnd");
        assert!(record.target(&desktop).is_none());
        // Unrecordable handles leave the record untouched.
        let fresh = last(0);
        fresh.note(HWND::default(), &desktop);
        fresh.note(HWND(999 as *mut _), &desktop);
        assert_eq!(fresh.0.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn an_invalid_or_unidentifiable_foreground_is_none_not_a_fallback() {
        let desktop = FakeDesktop::default();
        desktop.window(20, "Notepad");
        let record = last(20);
        desktop.foreground.set(0);
        assert!(record.target(&desktop).is_none());
        desktop.window(10, "Notepad");
        desktop.identities.borrow_mut().remove(&10);
        desktop.foreground.set(10);
        assert!(record.target(&desktop).is_none());
    }

    #[test]
    fn focus_identity_rejects_missing_reused_process_and_replaced_thread() {
        let expected = Identity {
            pid: 7,
            thread: 9,
            created: 1234,
        };
        let target = FocusTarget {
            hwnd: HWND(101 as *mut _),
            identity: expected,
        };
        assert!(target.is_window(HWND(101 as *mut _)));
        assert!(!target.is_window(HWND(102 as *mut _)));
        assert!(!target.is_window(HWND::default()));
        assert!(same_target(expected, Some(expected)));
        assert!(!same_target(expected, None));
        assert!(!same_target(
            expected,
            Some(Identity { pid: 8, ..expected })
        ));
        assert!(!same_target(
            expected,
            Some(Identity {
                thread: 10,
                ..expected
            })
        ));
        assert!(!same_target(
            expected,
            Some(Identity {
                created: 1235,
                ..expected
            })
        ));
    }

    #[test]
    fn raw_messages_decode_without_native_windows_or_cursor_queries() {
        assert_eq!(translate(WM_TIMER, 1234, 0), Some(Message::Timer(1234)));
        assert_eq!(translate(WM_SHOWWINDOW, 0, 0), Some(Message::Hidden));
        assert_eq!(translate(WM_SHOWWINDOW, 1, 0), None);
        assert_eq!(
            translate(WM_LBUTTONDOWN, 0, 0xfff6_fffc),
            Some(Message::Pointer(WM_LBUTTONDOWN, (-4, -10)))
        );
        assert_eq!(
            translate(WM_LBUTTONUP, 0, 0x0002_0001),
            Some(Message::Pointer(WM_LBUTTONUP, (1, 2)))
        );
        assert_eq!(
            translate(WM_MOUSEMOVE, 0, 0x0005_0003),
            Some(Message::Pointer(WM_MOUSEMOVE, (3, 5)))
        );
        assert_eq!(
            translate(WM_ACTIVATE, 0x0001_0000, 0),
            Some(Message::FocusLost)
        );
        assert_eq!(translate(WM_ACTIVATE, 1, 0), Some(Message::FocusGained));
        assert_eq!(translate(WM_ACTIVATE, 2, 0), Some(Message::FocusGained));
        assert_eq!(translate(WM_SETFOCUS, 0, 0), Some(Message::FocusGained));
        assert_eq!(translate(WM_KILLFOCUS, 0, 0), Some(Message::FocusLost));
        assert_eq!(translate(WM_CANCELMODE, 0, 0), Some(Message::Cancel));
        assert_eq!(
            translate(WM_CAPTURECHANGED, 0, 123),
            Some(Message::CaptureChanged(HWND(123 as *mut _)))
        );
        assert_eq!(translate(WM_MOUSELEAVE, 0, 0), Some(Message::Leave));
        assert_eq!(translate(WM_PAINT, 0, 0), Some(Message::Paint));
        assert_eq!(
            translate(WM_CLOSE, 0, 0),
            Some(Message::Input(PinInput::Close))
        );
        assert_eq!(
            translate(WM_DISPLAYCHANGE, 0, 0),
            Some(Message::Input(PinInput::DisplayChanged))
        );
        assert_eq!(
            translate(WM_SETTINGCHANGE, SPI_SETWORKAREA.0 as usize, 0),
            Some(Message::Input(PinInput::DisplayChanged))
        );
        // Theme, policy, mouse and other unrelated settings broadcasts are ignored.
        for wparam in [0, 0x2a, SPI_SETWORKAREA.0 as usize + 1] {
            assert_eq!(translate(WM_SETTINGCHANGE, wparam, 0), None);
        }
        assert_eq!(translate(WM_NCDESTROY, 0, 0), Some(Message::Destroy));
        // These require native snapshots and must never guess state here.
        assert_eq!(translate(WM_KEYDOWN, 0x43, 0), None);
        assert_eq!(translate(WM_DPICHANGED, 144, 0), None);
        assert_eq!(translate(WM_RBUTTONDOWN, 0, 0), None);
    }

    #[test]
    fn keyboard_preserves_altgr_flags_and_the_virtual_key_low_word() {
        assert_eq!(
            key_input(0x1_0043, true, false, false),
            PinInput::Key {
                vk: 0x43,
                ctrl: true,
                shift: false,
                alt: false,
            }
        );
        assert_eq!(
            key_input(0x43, true, true, true),
            PinInput::Key {
                vk: 0x43,
                ctrl: true,
                shift: true,
                alt: true,
            }
        );
        assert_eq!(
            key_input(0x09, false, true, false),
            PinInput::Key {
                vk: 0x09,
                ctrl: false,
                shift: true,
                alt: false,
            }
        );
    }

    #[test]
    fn geometry_is_xywh_with_negative_origins_and_checked_extents() {
        assert_eq!(
            rect_bounds(RECT {
                left: -1920,
                top: -1080,
                right: 0,
                bottom: -40
            }),
            (-1920, -1080, 1920, 1040)
        );
        assert!(valid_bounds((-1920, -1080, 640, 480)));
        assert!(!valid_bounds((0, 0, 0, 1)));
        assert!(!valid_bounds((0, 0, 1, -1)));
        assert!(!valid_bounds((i32::MAX, 0, 1, 1)));
        assert!(!valid_bounds((0, i32::MAX, 1, 1)));
        assert_eq!(lparam_xy(0x1234_0000_8000_ffff), (-1, -32768));
        assert_eq!(lparam_xy(0x7fff_0009), (9, 32767));
    }

    #[test]
    fn window_styles_exclude_activation_taskbar_and_alt_tab() {
        assert_ne!(EX_STYLE.0 & WS_EX_NOACTIVATE.0, 0);
        assert_ne!(EX_STYLE.0 & WS_EX_TOOLWINDOW.0, 0);
        assert_ne!(EX_STYLE.0 & WS_EX_TOPMOST.0, 0);
        assert_eq!(EX_STYLE.0 & WS_EX_APPWINDOW.0, 0);
    }
}
