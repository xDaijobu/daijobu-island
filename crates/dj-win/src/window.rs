//! The overlay window and its message loop.
//!
//! The window procedure never calls into the application. It translates messages
//! into [`Event`]s on a queue that `run` drains outside of any Win32 callback, so
//! the handler is never re-entered (e.g. while a tray menu pumps messages).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, DeleteObject, GetMonitorInfoW, MonitorFromPoint, SetWindowRgn,
    HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTOPRIMARY, RGN_OR,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, ReleaseCapture, SetCapture, TrackMouseEvent, UnregisterHotKey,
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_TRAY: u32 = WM_APP + 1;
pub const WM_THEMES_CHANGED: u32 = WM_APP + 2;
/// Posted by the audio endpoint callback (any thread): the master volume or mute changed.
pub const WM_VOLUME_CHANGED: u32 = WM_APP + 3;
/// Posted by the `IMMNotificationClient` (any thread): the default render device changed.
pub const WM_DEFAULT_DEVICE_CHANGED: u32 = WM_APP + 4;
/// Posted by the low-level keyboard hook (any thread). `wparam` is the virtual-key code.
pub const WM_VOLUME_KEY: u32 = WM_APP + 5;
/// Posted from another thread to wake the loop (see [`wake`]). Ids 3..=5 belong to the
/// volume work above, so this one takes the next free slot.
pub const WM_WAKE: u32 = WM_APP + 6;
/// Posted by the vitals worker thread: a new reading is queued.
pub const WM_VITALS: u32 = WM_APP + 7;
/// Posted by the sweep worker thread: scan, clean or quick-action results are queued.
pub const WM_SWEEP: u32 = WM_APP + 8;
/// Posted by the keyboard hook: plain PrtSc was pressed (and kept from Windows).
pub const WM_PRINT_SCREEN: u32 = WM_APP + 9;
/// Not exported by the windows crate.
const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_THEMES_DEBOUNCE: usize = 0xD001;
const THEMES_DEBOUNCE_MS: u32 = 200;
const CLASS_NAME: PCWSTR = w!("DaijobuIslandWindow");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Left button pressed, client coordinates in device pixels.
    Click {
        x: i32,
        y: i32,
    },
    Move {
        x: i32,
        y: i32,
    },
    /// Pointer moved with the left button held. Captured, so it may leave the pill.
    Drag {
        x: i32,
        y: i32,
    },
    /// Left button released.
    Release,
    Leave,
    Hotkey(i32),
    TrayLeft,
    TrayRight,
    DpiChanged,
    DisplayChange,
    /// The themes directory changed (debounced 200 ms).
    ThemesChanged,
    Timer(usize),
    /// Explorer restarted: the tray icon must be re-added.
    TaskbarCreated,
    /// The master volume or mute changed, from any source.
    VolumeChanged,
    /// The default render device changed: re-acquire the volume endpoint.
    DefaultDeviceChanged,
    /// A volume key was pressed (virtual-key code). Its default action is swallowed.
    VolumeKey(u32),
    /// Plain PrtSc, taken by the keyboard hook before Windows' screen capture sees it.
    PrintScreen,
    /// The Music engine has queued messages (drain them with its `poll`).
    Music,
    /// Another thread has results ready (see [`wake`]).
    Wake,
    /// The vitals worker queued a new reading.
    VitalsReady,
    /// The sweep worker queued results.
    SweepReady,
    /// Input on the Snap overlay window (see [`crate::overlay`]).
    Snap(crate::overlay::SnapInput),
    /// Input on the Snap preview stack's window (see [`crate::previews`]).
    Previews(crate::previews::PreviewInput),
    /// The shell moved the AppBars around (see [`crate::appbar`]): set the strip again.
    AppBarMoved,
    /// Another process's window started or finished a move or a resize (see
    /// [`crate::tile`]). Raised by an out-of-context WinEvent hook, so it arrives only
    /// while the loop is pumping messages, which a thread message from the hook ensures.
    TileMove(crate::tile::MoveEvent),
    /// Input on the Tile window picker's window (see [`crate::assist`]).
    Assist(crate::assist::AssistInput),
}

pub trait Handler {
    fn event(&mut self, ev: Event);
    /// True while something is animating. When false the loop blocks in `GetMessageW`.
    fn wants_frame(&self) -> bool;
    /// Render one frame. Called only while `wants_frame` is true; paced by `Present(1)`.
    fn frame(&mut self);
}

thread_local! {
    static QUEUE: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

fn push(ev: Event) {
    QUEUE.with(|q| q.borrow_mut().push_back(ev));
}

fn pop() -> Option<Event> {
    QUEUE.with(|q| q.borrow_mut().pop_front())
}

/// Queues an event from a callback on the UI thread. It is handled after the
/// current message is dispatched.
pub fn push_event(ev: Event) {
    push(ev);
}

/// Wakes the UI loop from any thread so a `Wake` event is delivered.
pub fn wake(hwnd: isize) {
    post_to(hwnd, WM_WAKE);
}

/// Geometry of the primary monitor in device pixels.
#[derive(Clone, Copy, Debug)]
pub struct Monitor {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub dpi: u32,
}

pub fn primary_monitor() -> Monitor {
    // SAFETY: plain queries with valid out-pointers; the monitor handle is
    // returned by the system and used immediately.
    unsafe {
        let hmon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        if GetMonitorInfoW(hmon, &mut info).as_bool() {
            rc = info.rcMonitor;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        Monitor {
            left: rc.left,
            top: rc.top,
            width: rc.right - rc.left,
            dpi: dx.max(96),
        }
    }
}

/// Top-centre of the primary monitor for a window of `w` x `h` device pixels.
pub fn top_center(m: &Monitor, w: i32) -> (i32, i32) {
    (m.left + (m.width - w) / 2, m.top)
}

pub fn create_window(x: i32, y: i32, w: i32, h: i32) -> Result<HWND> {
    // SAFETY: standard class registration and window creation; all pointers are
    // valid for the duration of the calls and the class name is a static string.
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        TASKBAR_CREATED.with(|c| c.set(RegisterWindowMessageW(w!("TaskbarCreated"))));
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            CLASS_NAME,
            w!("Daijobu Island"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Ok(hwnd)
    }
}

/// Moves/resizes the window and keeps it topmost without activating it.
pub fn place(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    // SAFETY: valid window handle owned by this thread.
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
    }
}

/// Click-through outside the pill: clip the window to `(l, t, r, b)` (client px).
/// `HTTRANSPARENT` doesn't work across processes, a window region does.
pub fn set_hit_region(hwnd: HWND, l: i32, t: i32, r: i32, b: i32) {
    set_hit_rects(hwnd, &[(l, t, r, b)]);
}

/// Like [`set_hit_region`], for the union of several `(l, t, r, b)` rects (client px).
/// An empty list leaves the region as it is.
pub fn set_hit_rects(hwnd: HWND, rects: &[(i32, i32, i32, i32)]) {
    let Some(&(l, t, r, b)) = rects.first() else {
        return;
    };
    // SAFETY: every region made here is either combined into `rgn` and freed, or is
    // `rgn` itself. On success the system takes ownership of `rgn`; on failure we free
    // it ourselves.
    unsafe {
        let rgn = CreateRectRgn(l, t, r, b);
        for &(l, t, r, b) in &rects[1..] {
            let part = CreateRectRgn(l, t, r, b);
            CombineRgn(Some(rgn), Some(rgn), Some(part), RGN_OR);
            let _ = DeleteObject(HGDIOBJ(part.0));
        }
        if SetWindowRgn(hwnd, Some(rgn), false) == 0 {
            let _ = DeleteObject(HGDIOBJ(rgn.0));
        }
    }
}

pub fn set_visible(hwnd: HWND, visible: bool) {
    // SAFETY: valid window handle owned by this thread.
    unsafe {
        let _ = ShowWindow(hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
    }
}

pub fn dpi_for_window(hwnd: HWND) -> u32 {
    // SAFETY: valid window handle.
    unsafe { GetDpiForWindow(hwnd).max(96) }
}

/// Posts `msg` to the overlay window from any thread. `hwnd` is the handle's raw value.
pub fn post_to(hwnd: isize, msg: u32) {
    // SAFETY: PostMessageW is thread-safe; a stale handle just makes the post fail.
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(0), LPARAM(0));
    }
}

pub fn set_timer(hwnd: HWND, id: usize, ms: u32) {
    // SAFETY: valid window handle; timer ids below 0xD000 are reserved for the app.
    unsafe {
        SetTimer(Some(hwnd), id, ms, None);
    }
}

/// Like [`set_timer`], but Windows may delay each tick by up to `tolerance_ms` so it
/// can fire together with other timers and let the CPU sleep longer.
pub fn set_coalescable_timer(hwnd: HWND, id: usize, ms: u32, tolerance_ms: u32) {
    // SAFETY: valid window handle; timer ids below 0xD000 are reserved for the app.
    unsafe {
        SetCoalescableTimer(Some(hwnd), id, ms, None, tolerance_ms);
    }
}

pub fn kill_timer(hwnd: HWND, id: usize) {
    // SAFETY: valid window handle.
    unsafe {
        let _ = KillTimer(Some(hwnd), id);
    }
}

pub fn quit() {
    // SAFETY: posts WM_QUIT to this thread's queue.
    unsafe { PostQuitMessage(0) }
}

pub const MOD_CTRL_ALT: u32 = MOD_CONTROL.0 | MOD_ALT.0;
pub const MOD_CTRL_SHIFT: u32 = MOD_CONTROL.0 | MOD_SHIFT.0;
pub const MOD_CTRL: u32 = MOD_CONTROL.0;
/// No modifiers (PrtSc on its own).
pub const MOD_NONE: u32 = 0;

/// Registers a global hotkey. Returns false if another app already owns it.
pub fn register_hotkey(hwnd: HWND, id: i32, modifiers: u32, vk: u32) -> bool {
    try_register_hotkey(hwnd, id, modifiers, vk).is_ok()
}

/// Like [`register_hotkey`], with the reason it failed (for the log).
pub fn try_register_hotkey(
    hwnd: HWND,
    id: i32,
    modifiers: u32,
    vk: u32,
) -> std::result::Result<(), String> {
    // SAFETY: valid window handle.
    unsafe {
        RegisterHotKey(
            Some(hwnd),
            id,
            HOT_KEY_MODIFIERS(modifiers) | MOD_NOREPEAT,
            vk,
        )
    }
    .map_err(|e| e.message())
}

pub fn unregister_hotkey(hwnd: HWND, id: i32) {
    // SAFETY: valid window handle.
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), id);
    }
}

pub fn run(handler: &mut dyn Handler) {
    let mut msg = MSG::default();
    'outer: loop {
        if handler.wants_frame() {
            // SAFETY: standard message pump on the thread that owns the window.
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        break 'outer;
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        } else {
            // SAFETY: as above; blocks until a message arrives (idle CPU ~ 0).
            unsafe {
                if GetMessageW(&mut msg, None, 0, 0).0 <= 0 {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        while let Some(ev) = pop() {
            handler.event(ev);
        }
        if handler.wants_frame() {
            handler.frame();
        }
    }
}

fn lparam_xy(lp: LPARAM) -> (i32, i32) {
    let v = lp.0 as u32;
    (
        i32::from(v as u16 as i16),
        i32::from((v >> 16) as u16 as i16),
    )
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => return LRESULT(1),
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_xy(lp);
            // Capture so a bar drag keeps reporting after the pointer leaves the pill.
            // SAFETY: valid window handle owned by this thread.
            let _ = unsafe { SetCapture(hwnd) };
            push(Event::Click { x, y });
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            // SAFETY: releases only this thread's capture; harmless if we don't hold it.
            let _ = unsafe { ReleaseCapture() };
            push(Event::Release);
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
            let (x, y) = lparam_xy(lp);
            // MK_LBUTTON: the left button is held (a drag, possibly captured).
            if wp.0 & 0x0001 != 0 {
                push(Event::Drag { x, y });
            } else {
                push(Event::Move { x, y });
            }
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            TRACKING.with(|t| t.set(false));
            push(Event::Leave);
            return LRESULT(0);
        }
        WM_HOTKEY => {
            push(Event::Hotkey(wp.0 as i32));
            return LRESULT(0);
        }
        WM_TRAY => {
            match lp.0 as u32 & 0xFFFF {
                WM_RBUTTONUP | WM_CONTEXTMENU => push(Event::TrayRight),
                WM_LBUTTONUP => push(Event::TrayLeft),
                _ => {}
            }
            return LRESULT(0);
        }
        WM_DPICHANGED => {
            push(Event::DpiChanged);
            return LRESULT(0);
        }
        WM_DISPLAYCHANGE => {
            push(Event::DisplayChange);
            return LRESULT(0);
        }
        WM_WAKE => {
            push(Event::Wake);
            return LRESULT(0);
        }
        WM_THEMES_CHANGED => {
            // Debounce: every notification re-arms the timer.
            set_timer(hwnd, TIMER_THEMES_DEBOUNCE, THEMES_DEBOUNCE_MS);
            return LRESULT(0);
        }
        WM_VOLUME_CHANGED => {
            push(Event::VolumeChanged);
            return LRESULT(0);
        }
        WM_DEFAULT_DEVICE_CHANGED => {
            push(Event::DefaultDeviceChanged);
            return LRESULT(0);
        }
        WM_VOLUME_KEY => {
            push(Event::VolumeKey(wp.0 as u32));
            return LRESULT(0);
        }
        WM_PRINT_SCREEN => {
            push(Event::PrintScreen);
            return LRESULT(0);
        }
        WM_VITALS => {
            push(Event::VitalsReady);
            return LRESULT(0);
        }
        WM_SWEEP => {
            push(Event::SweepReady);
            return LRESULT(0);
        }
        crate::appbar::WM_APPBAR => {
            crate::appbar::notify(wp.0);
            return LRESULT(0);
        }
        WM_TIMER => {
            if wp.0 == TIMER_THEMES_DEBOUNCE {
                kill_timer(hwnd, TIMER_THEMES_DEBOUNCE);
                push(Event::ThemesChanged);
            } else {
                push(Event::Timer(wp.0));
            }
            return LRESULT(0);
        }
        WM_DESTROY => {
            // Closed from outside (WM_CLOSE, logoff): the top strip must not outlive us.
            crate::appbar::release(hwnd);
            quit();
            return LRESULT(0);
        }
        m if m != 0 && m == TASKBAR_CREATED.with(Cell::get) => {
            push(Event::TaskbarCreated);
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: forwards the unhandled message with the original arguments.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}
