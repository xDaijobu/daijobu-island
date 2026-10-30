//! The overlay window and its message loop.
//!
//! The window procedure never calls into the application. It translates messages
//! into [`Event`]s on a queue that `run` drains outside of any Win32 callback, so
//! the handler is never re-entered (e.g. while a tray menu pumps messages).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CombineRgn, CreateRectRgn, DeleteObject, GetMonitorInfoW, MonitorFromPoint,
    SetWindowRgn, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTOPRIMARY, RGN_OR,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, GetKeyState, RegisterHotKey, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent,
    UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, TME_LEAVE,
    TRACKMOUSEEVENT, VK_CONTROL, VK_MENU, VK_SHIFT,
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// Physical top-edge sensor entry, or a foreground/fullscreen invalidation.
    EdgeReveal {
        reached: bool,
    },
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
    /// Input on a popup window (Settings or Spotlight, see [`crate::popup`]).
    Popup(crate::popup::PopupKind, crate::popup::PopupInput),
    /// Input on the Snap preview stack's window (see [`crate::previews`]).
    Previews(crate::previews::PreviewInput),
    /// Input from one independent pinned screenshot window.
    SnapPin {
        id: u64,
        input: crate::snap_pins::PinInput,
    },
    /// The shell moved the AppBars around (see [`crate::appbar`]): set the strip again.
    AppBarMoved,
    /// Another process's window started or finished a move or a resize (see
    /// [`crate::tile`]). Raised by an out-of-context WinEvent hook, so it arrives only
    /// while the loop is pumping messages, which a thread message from the hook ensures.
    TileMove(crate::tile::MoveEvent),
    /// Input on the Tile window picker's window (see [`crate::assist`]).
    Assist(crate::assist::AssistInput),
    /// A key went down on the island while it has the keyboard (see [`set_keyboard`]);
    /// auto-repeats included. `vk` is the virtual-key code and the flags are the modifier
    /// keys as they were at that message. AltGr reads as `ctrl` and `alt` both set: the
    /// receiver decides what that means. Sent only in keyboard mode.
    Key {
        vk: u16,
        ctrl: bool,
        shift: bool,
        alt: bool,
    },
    /// A character typed on the island while it has the keyboard, from `WM_CHAR` or
    /// `WM_SYSCHAR` (surrogate pairs joined; an unpaired surrogate is dropped). Control
    /// characters come through as they are (Ctrl+C is U+0003, Enter U+000D). The flags are
    /// as for [`Event::Key`]. Sent only in keyboard mode.
    Char {
        ch: char,
        ctrl: bool,
        alt: bool,
    },
    /// The wheel turned over the island while it has the keyboard. `delta` is in
    /// `WHEEL_DELTA` units (120 per notch), positive when the wheel is rolled away from the
    /// user (scroll up). `x` and `y` are client coordinates in device pixels. Sent only in
    /// keyboard mode.
    Wheel {
        delta: i32,
        x: i32,
        y: i32,
    },
    /// Right button pressed, client coordinates in device pixels. Sent only in keyboard
    /// mode.
    RightClick {
        x: i32,
        y: i32,
    },
    /// Middle button released, client coordinates in device pixels. Sent only in keyboard
    /// mode.
    MiddleClick {
        x: i32,
        y: i32,
    },
    /// The second press of a double click (the left button), client coordinates in device
    /// pixels. It follows that press's own `Click`. Sent in every mode.
    DoubleClick {
        x: i32,
        y: i32,
    },
    /// The keyboard focus or the activation went away (`WM_KILLFOCUS`, or `WM_ACTIVATE`
    /// with `WA_INACTIVE`). One deactivation can send both, so handle it idempotently.
    /// Sent only in keyboard mode.
    FocusLost,
    /// A close request, handled outside the native callback and any OLE drag loop.
    Close,
    /// Escape during asynchronous preview preparation, tied to its exact request.
    DragEscape(u64),
}

pub trait Handler {
    fn event(&mut self, ev: Event);
    /// True while something is animating. When false the loop blocks in `GetMessageW`.
    fn wants_frame(&self) -> bool;
    /// Render one frame. Called only while `wants_frame` is true; paced by `Present(1)`.
    fn frame(&mut self);
    /// Take an owned drag after all queued input has been handled. No handler borrow
    /// or application callback is retained by the native drag operation.
    fn take_drag(&mut self) -> Option<(u64, crate::drag_drop::DragPayload)> {
        None
    }
    fn drag_finished(
        &mut self,
        _request: u64,
        _result: std::result::Result<crate::drag_drop::DragOutcome, String>,
    ) {
    }
}

thread_local! {
    static QUEUE: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    /// Keyboard mode is on (see [`set_keyboard`]).
    static KEYBOARD: Cell<bool> = const { Cell::new(false) };
    /// A high surrogate from `WM_CHAR` waiting for its low half.
    static HIGH_SURROGATE: Cell<Option<u16>> = const { Cell::new(None) };
    /// Native callbacks may cancel a modal operation, but only enqueue app events.
    static ACTIVE_DRAG: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
    static QUIT_REQUESTED: Cell<bool> = const { Cell::new(false) };
    static DRAG_ESCAPE_REQUEST: Cell<Option<u64>> = const { Cell::new(None) };
}

/// The backtick key on US layouts (`VK_OEM_3`), the terminal's hotkey.
pub const VK_OEM_3: u32 = 0xC0;

fn push(ev: Event) {
    if interrupts_drag(ev) {
        cancel_modal_drag();
    }
    let modal = ACTIVE_DRAG.with(|active| active.borrow().is_some());
    QUEUE.with(|q| {
        let mut queue = q.borrow_mut();
        // These are level-triggered notifications: one drain observes all pending
        // results/current time. A long OLE drag must not accumulate thousands of
        // redundant timer/wake callbacks. Commands and input retain FIFO order.
        if modal
            && matches!(
                ev,
                Event::Timer(_)
                    | Event::Wake
                    | Event::Music
                    | Event::VitalsReady
                    | Event::SweepReady
            )
            && queue.iter().any(|queued| *queued == ev)
        {
            return;
        }
        queue.push_back(ev);
    });
}

fn interrupts_drag(ev: Event) -> bool {
    matches!(
        ev,
        Event::Hotkey(_)
            | Event::PrintScreen
            | Event::TrayLeft
            | Event::TrayRight
            | Event::DpiChanged
            | Event::DisplayChange
            | Event::AppBarMoved
            | Event::Close
            | Event::DragEscape(_)
            | Event::Previews(crate::previews::PreviewInput::CancelCapture)
    )
}

fn cancel_modal_drag() {
    let first = ACTIVE_DRAG.with(|active| {
        active
            .borrow()
            .as_ref()
            .is_some_and(|cancelled| !cancelled.swap(true, Ordering::AcqRel))
    });
    if first {
        // Do not wait for another physical pointer movement to wake a stationary
        // OLE loop. GetCapture is limited to this UI thread; WM_CANCELMODE asks its
        // capture window to cancel its mode. The token remains authoritative.
        // SAFETY: no pointer dereference; only our current thread's capture HWND
        // receives a parameter-free asynchronous message, after the TLS borrow ends.
        unsafe {
            let capture = GetCapture();
            if !capture.0.is_null() {
                let _ = PostMessageW(Some(capture), WM_CANCELMODE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

/// Capture Escape only while a preview request prepares in the background. The
/// keyboard hook records key-down immediately, so a short tap between timer ticks
/// still invalidates that exact request without activating the preview window.
pub fn watch_drag_escape(owner: HWND, request: Option<u64>) -> bool {
    if request.is_some() && !crate::hook::install(owner) {
        DRAG_ESCAPE_REQUEST.with(|active| active.set(None));
        return false;
    }
    DRAG_ESCAPE_REQUEST.with(|active| active.set(request));
    true
}

/// Called only by our UI thread's keyboard hook. It never enters application code.
pub(crate) fn drag_escape_pressed() -> bool {
    let Some(request) = DRAG_ESCAPE_REQUEST.with(Cell::get) else {
        return false;
    };
    push(Event::DragEscape(request));
    true
}

/// OLE setup is complete. Its message loop now owns Escape; preparation kept the
/// request watcher armed through any reentrant helper/COM initialization callbacks.
pub(crate) fn finish_drag_preparation() {
    DRAG_ESCAPE_REQUEST.with(|active| active.set(None));
}

/// Registers only a cancellation token for native callbacks. Neither the queue's
/// RefCell guard nor any reference to application state crosses the modal call.
fn with_drag_cancellation<T>(operation: impl FnOnce(Arc<AtomicBool>) -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ACTIVE_DRAG.with(|active| *active.borrow_mut() = None);
            finish_drag_preparation();
        }
    }
    // frame/take_drag can synchronously pump callbacks (notably capture release).
    // Include their queued cancellation before a transport is allowed to start.
    let queued_cancel = QUEUE.with(|queue| queue.borrow().iter().copied().any(interrupts_drag));
    let cancelled = Arc::new(AtomicBool::new(queued_cancel));
    ACTIVE_DRAG.with(|active| {
        assert!(active.borrow().is_none(), "nested source drag");
        *active.borrow_mut() = Some(Arc::clone(&cancelled));
    });
    let _reset = Reset;
    operation(cancelled)
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
    pub height: i32,
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
            height: rc.bottom - rc.top,
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
            // The second press of a double click arrives as WM_LBUTTONDBLCLK; the window
            // procedure turns it into a `Click` and a `DoubleClick` (see `left_press`).
            style: CS_DBLCLKS,
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

/// Which z-order layer the island lives on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// Topmost, over every window.
    Top,
    /// Directly above the desktop, under every normal window.
    Desktop,
}

/// The layer last set by [`set_layer`] (`true` = Desktop); [`place`] keeps it.
static LAYER_DESKTOP: AtomicBool = AtomicBool::new(false);

/// The layer the island is currently on.
pub fn current_layer() -> Layer {
    if LAYER_DESKTOP.load(Ordering::Relaxed) {
        Layer::Desktop
    } else {
        Layer::Top
    }
}

/// Puts the island on `layer` without moving, sizing or activating it. `Desktop` takes it
/// out of topmost and inserts it right above `Progman`; never `HWND_BOTTOM`, which hides
/// it under the desktop.
pub fn set_layer(hwnd: HWND, layer: Layer) {
    LAYER_DESKTOP.store(layer == Layer::Desktop, Ordering::Relaxed);
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
    // SAFETY: z-order calls on our own window; `Progman` and its neighbour are only
    // used as insert-after handles and may be stale (the call then just fails).
    unsafe {
        match layer {
            Layer::Top => {
                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, flags);
            }
            Layer::Desktop => {
                let _ = SetWindowPos(hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags);
                let Ok(progman) = FindWindowW(w!("Progman"), PCWSTR::null()) else {
                    return;
                };
                if let Ok(above) = GetWindow(progman, GW_HWNDPREV) {
                    if above != hwnd {
                        let _ = SetWindowPos(hwnd, Some(above), 0, 0, 0, 0, flags);
                    }
                }
            }
        }
    }
}

/// Moves/resizes the window and keeps its layer (see [`set_layer`]) without activating it.
pub fn place(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    if current_layer() == Layer::Desktop {
        // SAFETY: valid window handle owned by this thread; z-order is left alone.
        unsafe {
            let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOACTIVATE | SWP_NOZORDER);
        }
        return;
    }
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
    QUIT_REQUESTED.with(|quitting| quitting.set(true));
    cancel_modal_drag();
    // SAFETY: posts WM_QUIT to this thread's queue.
    unsafe { PostQuitMessage(0) }
}

/// Shutdown was requested on this UI thread; no deferred application work may run.
pub fn quitting() -> bool {
    QUIT_REQUESTED.with(Cell::get)
}

pub const MOD_CTRL_ALT: u32 = MOD_CONTROL.0 | MOD_ALT.0;
pub const MOD_CTRL_SHIFT: u32 = MOD_CONTROL.0 | MOD_SHIFT.0;
pub const MOD_CTRL: u32 = MOD_CONTROL.0;
/// Alt on its own (Alt+Space, Spotlight).
pub const MOD_ALT_ONLY: u32 = MOD_ALT.0;
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

// ---- keyboard mode ------------------------------------------------------------------

/// The extended style with `WS_EX_NOACTIVATE` removed (`keyboard_on`) or put back.
fn ex_style_for(ex: isize, keyboard_on: bool) -> isize {
    let no_activate = WS_EX_NOACTIVATE.0 as isize;
    if keyboard_on {
        ex & !no_activate
    } else {
        ex | no_activate
    }
}

/// The answer to `WM_MOUSEACTIVATE`: a click activates the island only in keyboard mode.
fn mouse_activate(keyboard_on: bool) -> u32 {
    if keyboard_on {
        MA_ACTIVATE
    } else {
        MA_NOACTIVATE
    }
}

/// Turns keyboard mode on or off for the island window.
///
/// Off (the default) is the island as it always was: `WS_EX_NOACTIVATE` is set, a click
/// does not activate it, and no keyboard, wheel, right-click or focus events are sent.
/// On, the style is removed and a click activates the window, so it can take the keyboard
/// (see [`take_keyboard`]), and the events above are sent. Turning it off puts
/// `WS_EX_NOACTIVATE` back; it does not move the foreground (see [`give_back_keyboard`]).
/// While on, key messages never reach `DefWindowProc`, so Alt+F4, F10 and Alt+Space can't
/// close the island or open its system menu.
pub fn set_keyboard(hwnd: HWND, on: bool) {
    KEYBOARD.with(|k| k.set(on));
    HIGH_SURROGATE.with(|h| h.set(None));
    // SAFETY: plain style reads and writes on a window owned by this thread. The style
    // is not a frame style, so no `SWP_FRAMECHANGED` is needed for it to take effect.
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let new = ex_style_for(ex, on);
        if new != ex {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new);
        }
    }
}

/// Gives the island window the foreground and the keyboard focus, and returns the window
/// that had the foreground (its raw handle, 0 when there was none) for
/// [`give_back_keyboard`]. Call it once per opening, after `set_keyboard(hwnd, true)`, and
/// keep the first value: a second call would return the island itself. Use it only right
/// after the user's hotkey or click, which is what lets this process take the foreground.
pub fn take_keyboard(hwnd: HWND) -> isize {
    // SAFETY: plain calls on our own window, on the thread that owns it. The previous
    // foreground handle is only stored, never dereferenced.
    unsafe {
        let prev = GetForegroundWindow();
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        prev.0 as isize
    }
}

/// `hwnd` is the foreground window (it has the keyboard).
pub fn is_foreground(hwnd: HWND) -> bool {
    // SAFETY: no arguments; the returned handle is only compared.
    unsafe { GetForegroundWindow() == hwnd }
}

/// Activates `prev` (from [`take_keyboard`]) if it is still a window and visible; a stale
/// or zero handle does nothing. Best effort: the system may refuse (the foreground lock).
pub fn give_back_keyboard(prev: isize) {
    if prev == 0 {
        return;
    }
    let hwnd = HWND(prev as *mut _);
    // SAFETY: plain calls on a handle that may be stale (then `IsWindow` says so).
    unsafe {
        if IsWindow(Some(hwnd)).as_bool() && IsWindowVisible(hwnd).as_bool() {
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// Modifier keys at the time of a message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Mods {
    ctrl: bool,
    shift: bool,
    alt: bool,
}

/// What `translate` needs besides the message itself.
#[derive(Clone, Copy, Debug, Default)]
struct Ctx {
    keyboard: bool,
    mods: Mods,
    /// Where the client area's (0, 0) is on the screen, for `WM_MOUSEWHEEL`'s screen
    /// coordinates.
    origin: (i32, i32),
}

fn key_down(vk: u16) -> bool {
    // SAFETY: no pointers involved. `GetKeyState` reports the state as of the message
    // being handled, which is what a typed character needs.
    unsafe { GetKeyState(i32::from(vk)) < 0 }
}

/// The modifier keys now. AltGr is how Windows reports Ctrl+Alt+... layouts (a left Ctrl
/// plus a right Alt), so it shows as `ctrl` and `alt` both down; callers decide.
fn read_mods() -> Mods {
    Mods {
        ctrl: key_down(VK_CONTROL.0),
        shift: key_down(VK_SHIFT.0),
        alt: key_down(VK_MENU.0),
    }
}

/// Adds one UTF-16 unit from `WM_CHAR` to `high` (the pending high surrogate) and returns
/// the character it completes. A high surrogate waits; a low one joins it. Unpaired
/// surrogates are dropped: a high one followed by something else, and a low one on its own.
fn join_utf16(high: &mut Option<u16>, unit: u16) -> Option<char> {
    match unit {
        0xD800..=0xDBFF => {
            *high = Some(unit);
            None
        }
        0xDC00..=0xDFFF => {
            let hi = high.take()?;
            char::decode_utf16([hi, unit]).next()?.ok()
        }
        _ => {
            *high = None;
            char::from_u32(u32::from(unit))
        }
    }
}

/// Signed client coordinates from a mouse message's `lparam`.
fn lparam_xy(lp: isize) -> (i32, i32) {
    let v = lp as u32;
    (
        i32::from(v as u16 as i16),
        i32::from((v >> 16) as u16 as i16),
    )
}

/// The events one left-button press makes: always a `Click` (the second press of a double
/// click too, with the capture the caller takes, so every feature that only knows `Click`
/// behaves as before `CS_DBLCLKS`), and a `DoubleClick` after it for `WM_LBUTTONDBLCLK`.
fn left_press(msg: u32, lp: isize) -> (Event, Option<Event>) {
    let (x, y) = lparam_xy(lp);
    let double = (msg == WM_LBUTTONDBLCLK).then_some(Event::DoubleClick { x, y });
    (Event::Click { x, y }, double)
}

/// The messages that keyboard mode takes for itself: the window procedure answers them
/// with 0 and never calls `DefWindowProc`. For the key messages that is what keeps Alt+F4
/// (`WM_SYSKEYDOWN` → `SC_CLOSE`), F10 and Alt+Space (→ `SC_KEYMENU`) from the system;
/// `WM_SYSCOMMAND`'s `SC_KEYMENU` is also refused in case one arrives another way.
/// `TranslateMessage` has already made the `WM_CHAR`s by then. Always false outside
/// keyboard mode.
fn consumed(msg: u32, wp: usize, keyboard_on: bool) -> bool {
    keyboard_on
        && match msg {
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_SYSKEYUP | WM_CHAR | WM_SYSCHAR | WM_MOUSEWHEEL
            | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN | WM_MBUTTONUP => true,
            WM_SYSCOMMAND => (wp & 0xFFF0) as u32 == SC_KEYMENU,
            _ => false,
        }
}

/// Maps the messages that carry keyboard-mode input (and `WM_KILLFOCUS` / `WM_ACTIVATE`)
/// to their event. Everything is `None` outside keyboard mode. `high` is the pending high
/// surrogate (see `join_utf16`). The left button is not here: see `left_press`.
fn translate(msg: u32, wp: usize, lp: isize, ctx: &Ctx, high: &mut Option<u16>) -> Option<Event> {
    if !ctx.keyboard {
        return None;
    }
    let Mods { ctrl, shift, alt } = ctx.mods;
    match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => Some(Event::Key {
            vk: wp as u16,
            ctrl,
            shift,
            alt,
        }),
        WM_CHAR | WM_SYSCHAR => {
            let ch = join_utf16(high, wp as u16)?;
            Some(Event::Char { ch, ctrl, alt })
        }
        WM_MOUSEWHEEL => {
            // The high word of `wparam` is the signed delta, and the position in `lparam`
            // is in screen coordinates.
            let delta = i32::from((wp >> 16) as u16 as i16);
            let (sx, sy) = lparam_xy(lp);
            Some(Event::Wheel {
                delta,
                x: sx - ctx.origin.0,
                y: sy - ctx.origin.1,
            })
        }
        WM_RBUTTONDOWN => {
            let (x, y) = lparam_xy(lp);
            Some(Event::RightClick { x, y })
        }
        // On the release, as browsers close a tab: a press that drags elsewhere is no click.
        WM_MBUTTONUP => {
            let (x, y) = lparam_xy(lp);
            Some(Event::MiddleClick { x, y })
        }
        WM_KILLFOCUS => Some(Event::FocusLost),
        // The low word of `wparam` is the activation state; the high word is "minimized".
        WM_ACTIVATE if (wp & 0xFFFF) as u32 == WA_INACTIVE => Some(Event::FocusLost),
        _ => None,
    }
}

/// `translate` for a live message: reads the thread's keyboard-mode state, the modifier
/// keys and the client origin it needs (and only when it needs them).
fn translate_live(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<Event> {
    let keyboard = KEYBOARD.with(Cell::get);
    if !keyboard {
        return None;
    }
    let mods = if matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN | WM_CHAR | WM_SYSCHAR) {
        read_mods()
    } else {
        Mods::default()
    };
    let origin = if msg == WM_MOUSEWHEEL {
        let mut pt = POINT { x: 0, y: 0 };
        // SAFETY: valid window handle and out-pointer. On failure the origin stays (0, 0).
        let _ = unsafe { ClientToScreen(hwnd, &mut pt) };
        (pt.x, pt.y)
    } else {
        (0, 0)
    };
    let ctx = Ctx {
        keyboard,
        mods,
        origin,
    };
    let mut high = HIGH_SURROGATE.with(Cell::get);
    let ev = translate(msg, wp.0, lp.0, &ctx, &mut high);
    HIGH_SURROGATE.with(|h| h.set(high));
    ev
}

fn drain_events(handler: &mut impl Handler) -> bool {
    while !quitting() {
        let Some(event) = pop() else {
            return true;
        };
        handler.event(event);
    }
    false
}

pub fn run(mut handler: impl Handler) {
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
        loop {
            if !drain_events(&mut handler) {
                break 'outer;
            }
            if handler.wants_frame() {
                handler.frame();
            }
            if quitting() {
                break 'outer;
            }
            let Some((request, payload)) = handler.take_drag() else {
                break;
            };
            let result = with_drag_cancellation(|cancelled| {
                if cancelled.load(Ordering::Acquire) {
                    Ok(crate::drag_drop::DragOutcome::Cancelled)
                } else {
                    crate::drag_drop::run(payload, cancelled)
                }
            });
            handler.drag_finished(request, result);
            // OLE pumped native callbacks. Drain their queued events before blocking
            // again, and only after the drag controller has consumed its result.
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Keyboard mode: these messages are the island's own and never reach DefWindowProc.
    if consumed(msg, wp.0, KEYBOARD.with(Cell::get)) {
        if let Some(ev) = translate_live(hwnd, msg, wp, lp) {
            push(ev);
        }
        return LRESULT(0);
    }
    match msg {
        WM_MOUSEACTIVATE => {
            return LRESULT(mouse_activate(KEYBOARD.with(Cell::get)) as isize);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            // Capture so a bar drag keeps reporting after the pointer leaves the pill.
            // SAFETY: valid window handle owned by this thread.
            let _ = unsafe { SetCapture(hwnd) };
            let (click, double) = left_press(msg, lp.0);
            push(click);
            if let Some(double) = double {
                push(double);
            }
            return LRESULT(0);
        }
        // Reported, then `DefWindowProc` still runs: it gives a newly activated window the
        // keyboard focus. The events are sent in keyboard mode only.
        WM_KILLFOCUS | WM_ACTIVATE => {
            if let Some(ev) = translate_live(hwnd, msg, wp, lp) {
                push(ev);
            }
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
            let (x, y) = lparam_xy(lp.0);
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
        crate::edge_reveal::WM_EDGE => {
            push(Event::EdgeReveal { reached: wp.0 == 1 });
            return LRESULT(0);
        }
        WM_TRAY => {
            let event = match lp.0 as u32 & 0xFFFF {
                WM_RBUTTONUP | WM_CONTEXTMENU => Some(Event::TrayRight),
                WM_LBUTTONUP => Some(Event::TrayLeft),
                _ => None,
            };
            if let Some(event) = event {
                push(event);
                post_to(hwnd.0 as isize, WM_NULL);
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
        WM_CLOSE => {
            push(Event::Close);
            return LRESULT(0);
        }
        WM_DESTROY => {
            cancel_modal_drag();
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

/// The foreground window's raw handle (0 when there is none), to give the keyboard back
/// to later with [`give_back_keyboard`].
pub fn foreground() -> isize {
    // SAFETY: no arguments; the handle is only stored, never dereferenced here.
    unsafe { GetForegroundWindow().0 as isize }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_callbacks_post_a_wake_without_waiting_for_input_or_timers() {
        // SAFETY: this message-only window is never shown, has no application state,
        // and is created, queried and destroyed on this test thread.
        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("DaijobuTrayWakeTest"),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
            .unwrap();
            for (message, event) in [
                (WM_RBUTTONUP, Event::TrayRight),
                (WM_CONTEXTMENU, Event::TrayRight),
                (WM_LBUTTONUP, Event::TrayLeft),
            ] {
                assert!(pop().is_none());
                wndproc(hwnd, WM_TRAY, WPARAM(1), LPARAM(message as isize));
                let mut wake = MSG::default();
                assert!(PeekMessageW(&mut wake, Some(hwnd), WM_NULL, WM_NULL, PM_REMOVE).as_bool());
                assert_eq!(wake.message, WM_NULL);
                assert_eq!(pop(), Some(event));
                assert!(pop().is_none());
            }
            wndproc(hwnd, WM_TRAY, WPARAM(1), LPARAM(WM_MOUSEMOVE as isize));
            let mut wake = MSG::default();
            assert!(!PeekMessageW(&mut wake, Some(hwnd), WM_NULL, WM_NULL, PM_REMOVE).as_bool());
            assert!(pop().is_none());
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn short_escape_tap_is_queued_with_its_preparation_generation() {
        assert!(!drag_escape_pressed());
        DRAG_ESCAPE_REQUEST.with(|active| active.set(Some(7)));
        assert!(drag_escape_pressed());
        // Releasing Escape before the next timer does not erase this event.
        DRAG_ESCAPE_REQUEST.with(|active| active.set(None));
        assert_eq!(pop(), Some(Event::DragEscape(7)));
        assert!(pop().is_none());
        assert!(!drag_escape_pressed());
    }

    #[test]
    fn escape_tap_during_reentrant_native_setup_cancels_before_modal_entry() {
        DRAG_ESCAPE_REQUEST.with(|active| active.set(Some(9)));
        with_drag_cancellation(|cancelled| {
            // Synthetic helper-setup callback: Escape goes down and up before the
            // final physical-state check. The request event and token retain it.
            assert!(drag_escape_pressed());
            finish_drag_preparation();
            assert!(cancelled.load(Ordering::Acquire));
        });
        assert_eq!(pop(), Some(Event::DragEscape(9)));
        assert!(pop().is_none());
        assert!(!drag_escape_pressed());
    }

    #[test]
    fn modal_notifications_coalesce_without_losing_ordered_commands() {
        with_drag_cancellation(|_| {
            for _ in 0..200 {
                push_event(Event::Timer(1));
                push_event(Event::Timer(9));
                push_event(Event::Wake);
            }
            push_event(Event::TrayLeft);
            push_event(Event::TrayLeft);
        });
        assert_eq!(pop(), Some(Event::Timer(1)));
        assert_eq!(pop(), Some(Event::Timer(9)));
        assert_eq!(pop(), Some(Event::Wake));
        assert_eq!(pop(), Some(Event::TrayLeft));
        assert_eq!(pop(), Some(Event::TrayLeft));
        assert!(pop().is_none());
    }

    #[test]
    fn deferred_quit_stops_snap_and_wake_replay_before_any_further_handler_call() {
        struct FakeHandler(Vec<Event>);
        impl Handler for FakeHandler {
            fn event(&mut self, event: Event) {
                self.0.push(event);
                if event == Event::Close {
                    // Only set the pure latch: this test posts no OS messages.
                    QUIT_REQUESTED.with(|quitting| quitting.set(true));
                }
            }
            fn wants_frame(&self) -> bool {
                false
            }
            fn frame(&mut self) {
                panic!("shutdown must not render");
            }
        }
        let mut handler = FakeHandler(Vec::new());
        push_event(Event::Close);
        push_event(Event::PrintScreen);
        push_event(Event::Wake);
        assert!(!drain_events(&mut handler));
        assert_eq!(handler.0, vec![Event::Close]);
        assert_eq!(pop(), Some(Event::PrintScreen));
        assert_eq!(pop(), Some(Event::Wake));
        assert!(pop().is_none());
        QUIT_REQUESTED.with(|quitting| quitting.set(false));
    }

    #[test]
    fn modal_drag_defers_disruptive_events_and_only_sets_a_cancel_token() {
        for event in [
            Event::Hotkey(1),
            Event::PrintScreen,
            Event::TrayLeft,
            Event::TrayRight,
            Event::DpiChanged,
            Event::DisplayChange,
            Event::AppBarMoved,
            Event::Close,
            Event::Previews(crate::previews::PreviewInput::CancelCapture),
        ] {
            assert!(pop().is_none());
            with_drag_cancellation(|cancelled| {
                assert!(!cancelled.load(Ordering::Acquire));
                push_event(event);
                assert!(cancelled.load(Ordering::Acquire));
                // An app callback is never invoked by push_event: the event stays
                // queued until the outer dispatcher resumes after the modal call.
                QUEUE.with(|queue| assert_eq!(queue.borrow().len(), 1));
            });
            ACTIVE_DRAG.with(|active| assert!(active.borrow().is_none()));
            assert_eq!(pop(), Some(event));
            assert!(pop().is_none());
        }
    }

    #[test]
    fn modal_drag_keeps_regular_wakes_and_timers_without_cancelling() {
        with_drag_cancellation(|cancelled| {
            push_event(Event::Wake);
            push_event(Event::Timer(9));
            assert!(!cancelled.load(Ordering::Acquire));
        });
        assert_eq!(pop(), Some(Event::Wake));
        assert_eq!(pop(), Some(Event::Timer(9)));
        assert!(pop().is_none());
    }

    #[test]
    fn cancellation_queued_during_frame_or_capture_release_blocks_transport_entry() {
        for event in [Event::Close, Event::DisplayChange] {
            // Simulates a native callback while frame/take_drag runs, after the
            // outer dispatcher last drained its queue but before the token exists.
            push_event(event);
            let transport_called =
                with_drag_cancellation(|cancelled| !cancelled.load(Ordering::Acquire));
            assert!(!transport_called);
            assert_eq!(pop(), Some(event));
            assert!(pop().is_none());
        }
    }

    #[test]
    fn modal_cancellation_token_is_cleared_after_error_and_unwind() {
        let result: std::result::Result<(), ()> = with_drag_cancellation(|_| Err(()));
        assert!(result.is_err());
        ACTIVE_DRAG.with(|active| assert!(active.borrow().is_none()));
        assert!(std::panic::catch_unwind(|| {
            with_drag_cancellation(|_| panic!("synthetic transport panic"));
        })
        .is_err());
        ACTIVE_DRAG.with(|active| assert!(active.borrow().is_none()));
        with_drag_cancellation(|cancelled| assert!(!cancelled.load(Ordering::Acquire)));
    }

    fn lp(x: i16, y: i16) -> isize {
        (u32::from(y as u16) << 16 | u32::from(x as u16)) as isize
    }

    fn mods(ctrl: bool, shift: bool, alt: bool) -> Mods {
        Mods { ctrl, shift, alt }
    }

    fn ctx(keyboard: bool, m: Mods) -> Ctx {
        Ctx {
            keyboard,
            mods: m,
            origin: (0, 0),
        }
    }

    /// `translate` with no pending surrogate.
    fn tr(msg: u32, wp: usize, lparam: isize, c: &Ctx) -> Option<Event> {
        translate(msg, wp, lparam, c, &mut None)
    }

    /// `wp` for a `WM_MOUSEWHEEL`: the delta in the high word, key flags in the low one.
    fn wheel_wp(delta: i16, keys: u16) -> usize {
        (usize::from(delta as u16) << 16) | usize::from(keys)
    }

    fn key(vk: u16, ctrl: bool, shift: bool, alt: bool) -> Option<Event> {
        Some(Event::Key {
            vk,
            ctrl,
            shift,
            alt,
        })
    }

    fn chr(ch: char, ctrl: bool, alt: bool) -> Option<Event> {
        Some(Event::Char { ch, ctrl, alt })
    }

    const ON: bool = true;
    const OFF: bool = false;

    #[test]
    fn the_backtick_key_code() {
        assert_eq!(VK_OEM_3, 0xC0);
    }

    // ---- keys ----

    #[test]
    fn keys_carry_the_virtual_key_and_modifiers() {
        for msg in [WM_KEYDOWN, WM_SYSKEYDOWN] {
            let c = ctx(ON, Mods::default());
            assert_eq!(tr(msg, 0x41, 0, &c), key(0x41, false, false, false));
            let c = ctx(ON, mods(true, true, false));
            assert_eq!(tr(msg, 0x25, 0, &c), key(0x25, true, true, false));
            let c = ctx(ON, mods(false, false, true));
            assert_eq!(tr(msg, 0x73, 0, &c), key(0x73, false, false, true));
        }
        // The key code is the low 16 bits of wparam.
        assert_eq!(
            tr(WM_KEYDOWN, 0x1_0027, 0, &ctx(ON, Mods::default())),
            key(0x27, false, false, false)
        );
    }

    #[test]
    fn altgr_is_reported_as_ctrl_and_alt_together() {
        let altgr = ctx(ON, mods(true, false, true));
        assert_eq!(
            tr(WM_KEYDOWN, 0x51, 0, &altgr),
            key(0x51, true, false, true)
        );
        // German `@` is Ctrl+Alt+Q: the char keeps both flags, for the core to decide.
        let at = u32::from('@') as usize;
        assert_eq!(tr(WM_CHAR, at, 0, &altgr), chr('@', true, true));
        assert_eq!(tr(WM_SYSCHAR, at, 0, &altgr), chr('@', true, true));
    }

    #[test]
    fn chars_carry_the_character_and_modifiers() {
        let plain = ctx(ON, Mods::default());
        assert_eq!(tr(WM_CHAR, 0x61, 0, &plain), chr('a', false, false));
        // Shift is not part of a char: it is already in the character.
        let shift = ctx(ON, mods(false, true, false));
        assert_eq!(tr(WM_CHAR, 0x41, 0, &shift), chr('A', false, false));
        // Ctrl+C arrives as U+0003 with ctrl set.
        let ctrl = ctx(ON, mods(true, false, false));
        assert_eq!(tr(WM_CHAR, 0x03, 0, &ctrl), chr('\u{3}', true, false));
        // Alt+x is a WM_SYSCHAR.
        let alt = ctx(ON, mods(false, false, true));
        assert_eq!(tr(WM_SYSCHAR, 0x78, 0, &alt), chr('x', false, true));
        // Enter, Esc and Tab come through as the control characters they are.
        for (unit, ch) in [(0x0D, '\r'), (0x1B, '\u{1b}'), (0x09, '\t')] {
            assert_eq!(tr(WM_CHAR, unit, 0, &plain), chr(ch, false, false));
        }
        // A BMP character that isn't ASCII.
        assert_eq!(tr(WM_CHAR, 0x00E9, 0, &plain), chr('é', false, false));
    }

    // ---- surrogates ----

    #[test]
    fn a_surrogate_pair_joins_into_one_char() {
        let c = ctx(ON, Mods::default());
        let mut high = None;
        // U+1F600 is D83D DE00.
        assert_eq!(translate(WM_CHAR, 0xD83D, 0, &c, &mut high), None);
        assert_eq!(high, Some(0xD83D));
        assert_eq!(
            translate(WM_CHAR, 0xDE00, 0, &c, &mut high),
            chr('\u{1F600}', false, false)
        );
        assert_eq!(high, None);
    }

    #[test]
    fn unpaired_surrogates_are_dropped() {
        let c = ctx(ON, Mods::default());
        // A low surrogate on its own.
        let mut high = None;
        assert_eq!(translate(WM_CHAR, 0xDE00, 0, &c, &mut high), None);
        // A high surrogate followed by an ordinary char: the high one is dropped and the
        // char is not.
        assert_eq!(translate(WM_CHAR, 0xD83D, 0, &c, &mut high), None);
        assert_eq!(
            translate(WM_CHAR, 0x62, 0, &c, &mut high),
            chr('b', false, false)
        );
        assert_eq!(high, None);
        // ...and a low one after that finds nothing to join.
        assert_eq!(translate(WM_CHAR, 0xDE00, 0, &c, &mut high), None);
        // Two highs in a row: the later one wins.
        assert_eq!(translate(WM_CHAR, 0xD800, 0, &c, &mut high), None);
        assert_eq!(translate(WM_CHAR, 0xD83D, 0, &c, &mut high), None);
        assert_eq!(
            translate(WM_CHAR, 0xDE00, 0, &c, &mut high),
            chr('\u{1F600}', false, false)
        );
    }

    #[test]
    fn a_surrogate_pair_works_through_wm_syschar() {
        let c = ctx(ON, mods(false, false, true));
        let mut high = None;
        assert_eq!(translate(WM_SYSCHAR, 0xD83D, 0, &c, &mut high), None);
        assert_eq!(
            translate(WM_SYSCHAR, 0xDE00, 0, &c, &mut high),
            chr('\u{1F600}', false, true)
        );
    }

    // ---- the wheel ----

    fn wheel_ev(delta: i32, x: i32, y: i32) -> Option<Event> {
        Some(Event::Wheel { delta, x, y })
    }

    #[test]
    fn the_wheel_keeps_the_sign_of_its_delta() {
        let c = ctx(ON, Mods::default());
        let wheel = |d: i16| tr(WM_MOUSEWHEEL, wheel_wp(d, 0), lp(10, 20), &c);
        // Rolled away from the user (up): positive.
        assert_eq!(wheel(120), wheel_ev(120, 10, 20));
        assert_eq!(wheel(-120), wheel_ev(-120, 10, 20));
        // A high-resolution wheel sends fractions of a notch.
        assert_eq!(wheel(-30), wheel_ev(-30, 10, 20));
        assert_eq!(wheel(i16::MIN), wheel_ev(-32768, 10, 20));
        assert_eq!(wheel(i16::MAX), wheel_ev(32767, 10, 20));
        // The key flags in the low word and the upper half of a 64-bit wparam don't leak in.
        let wp = wheel_wp(-120, 0x0008 | 0x0004) | (0xABCD_usize << 32);
        assert_eq!(
            tr(WM_MOUSEWHEEL, wp, lp(10, 20), &c),
            wheel_ev(-120, 10, 20)
        );
    }

    #[test]
    fn the_wheel_position_is_converted_to_client_coordinates() {
        let at = |origin: (i32, i32), screen: (i16, i16)| {
            let c = Ctx {
                origin,
                ..ctx(ON, Mods::default())
            };
            tr(WM_MOUSEWHEEL, wheel_wp(120, 0), lp(screen.0, screen.1), &c)
        };
        // The window at (656, 0) on the primary monitor.
        assert_eq!(at((656, 0), (700, 90)), wheel_ev(120, 44, 90));
        // A pointer left of and above the client area reads negative.
        assert_eq!(at((656, 10), (600, 4)), wheel_ev(120, -56, -6));
        // A monitor to the left of the primary has negative screen coordinates.
        assert_eq!(at((-1264, 0), (-1000, 50)), wheel_ev(120, 264, 50));
    }

    // ---- the right and left buttons ----

    #[test]
    fn a_right_press_is_a_right_click() {
        let c = ctx(ON, Mods::default());
        assert_eq!(
            tr(WM_RBUTTONDOWN, 2, lp(-3, 40), &c),
            Some(Event::RightClick { x: -3, y: 40 })
        );
        // The release makes no event of its own.
        assert_eq!(tr(WM_RBUTTONUP, 0, lp(-3, 40), &c), None);
    }

    #[test]
    fn a_middle_release_is_a_middle_click() {
        let c = ctx(ON, Mods::default());
        assert_eq!(tr(WM_MBUTTONDOWN, 0x10, lp(7, 9), &c), None);
        assert_eq!(
            tr(WM_MBUTTONUP, 0, lp(7, 9), &c),
            Some(Event::MiddleClick { x: 7, y: 9 })
        );
        assert_eq!(
            tr(WM_MBUTTONUP, 0, lp(7, 9), &ctx(OFF, Mods::default())),
            None
        );
        assert!(consumed(WM_MBUTTONUP, 0, ON) && consumed(WM_MBUTTONDOWN, 0, ON));
        assert!(!consumed(WM_MBUTTONUP, 0, OFF));
    }

    #[test]
    fn a_double_click_is_a_click_and_a_double_click() {
        // The plain press: a Click, nothing else.
        assert_eq!(
            left_press(WM_LBUTTONDOWN, lp(12, -4)),
            (Event::Click { x: 12, y: -4 }, None)
        );
        // The second press of a double click: the same Click as before CS_DBLCLKS, then
        // the DoubleClick. `left_press` doesn't look at keyboard mode, so this holds in
        // every mode.
        assert_eq!(
            left_press(WM_LBUTTONDBLCLK, lp(12, -4)),
            (
                Event::Click { x: 12, y: -4 },
                Some(Event::DoubleClick { x: 12, y: -4 })
            )
        );
    }

    // ---- focus ----

    #[test]
    fn losing_the_keyboard_focus_is_reported() {
        assert_eq!(
            tr(WM_KILLFOCUS, 0, 0, &ctx(ON, Mods::default())),
            Some(Event::FocusLost)
        );
    }

    #[test]
    fn only_deactivation_loses_focus() {
        let c = ctx(ON, Mods::default());
        assert_eq!(
            tr(WM_ACTIVATE, WA_INACTIVE as usize, 0, &c),
            Some(Event::FocusLost)
        );
        // Deactivated while minimized: the high word is set, the low word is still 0.
        assert_eq!(tr(WM_ACTIVATE, 0x0001_0000, 0, &c), Some(Event::FocusLost));
        // WA_ACTIVE (1) and WA_CLICKACTIVE (2) are activations.
        assert_eq!(tr(WM_ACTIVATE, 1, 0, &c), None);
        assert_eq!(tr(WM_ACTIVATE, 2, 0, &c), None);
        assert_eq!(tr(WM_ACTIVATE, 0x0001_0001, 0, &c), None);
    }

    // ---- outside keyboard mode ----

    #[test]
    fn nothing_is_sent_outside_keyboard_mode() {
        let c = ctx(OFF, mods(true, true, true));
        for (msg, wp) in [
            (WM_KEYDOWN, 0x41),
            (WM_SYSKEYDOWN, 0x73),
            (WM_CHAR, 0x61),
            (WM_SYSCHAR, 0x61),
            (WM_MOUSEWHEEL, wheel_wp(120, 0)),
            (WM_RBUTTONDOWN, 2),
            (WM_KILLFOCUS, 0),
            (WM_ACTIVATE, WA_INACTIVE as usize),
        ] {
            assert_eq!(tr(msg, wp, lp(5, 5), &c), None, "{msg:#x}");
        }
        // A surrogate half doesn't wait for its pair when nothing is listening.
        let mut high = None;
        assert_eq!(translate(WM_CHAR, 0xD83D, 0, &c, &mut high), None);
        assert_eq!(high, None);
    }

    #[test]
    fn unrelated_messages_make_no_event() {
        let c = ctx(ON, Mods::default());
        for msg in [
            WM_MOUSEMOVE,
            WM_LBUTTONDOWN,
            WM_LBUTTONUP,
            WM_KEYUP,
            WM_SYSKEYUP,
            WM_SETFOCUS,
            WM_PAINT,
            WM_MOUSEHWHEEL,
        ] {
            assert_eq!(tr(msg, 1, lp(1, 1), &c), None, "{msg:#x}");
        }
    }

    // ---- what keyboard mode takes for itself ----

    #[test]
    fn keyboard_mode_consumes_the_key_messages_so_alt_f4_cannot_close() {
        for msg in [
            WM_KEYDOWN,
            WM_SYSKEYDOWN,
            WM_SYSKEYUP,
            WM_CHAR,
            WM_SYSCHAR,
            WM_MOUSEWHEEL,
            WM_RBUTTONDOWN,
            WM_RBUTTONUP,
        ] {
            assert!(consumed(msg, 0x73, ON), "{msg:#x}");
            assert!(!consumed(msg, 0x73, OFF), "{msg:#x} outside keyboard mode");
        }
        // The system menu by keyboard (F10, Alt+Space), however it arrives. The low four
        // bits of wparam are used by the system.
        assert!(consumed(WM_SYSCOMMAND, SC_KEYMENU as usize, ON));
        assert!(consumed(WM_SYSCOMMAND, SC_KEYMENU as usize | 0x2, ON));
        assert!(!consumed(WM_SYSCOMMAND, SC_KEYMENU as usize, OFF));
        // Other system commands are not ours to take (an outside WM_CLOSE still closes).
        assert!(!consumed(WM_SYSCOMMAND, SC_CLOSE as usize, ON));
        assert!(!consumed(WM_CLOSE, 0, ON));
    }

    #[test]
    fn keyboard_mode_leaves_everything_else_alone() {
        for msg in [
            WM_MOUSEMOVE,
            WM_LBUTTONDOWN,
            WM_LBUTTONDBLCLK,
            WM_LBUTTONUP,
            WM_HOTKEY,
            WM_KILLFOCUS,
            WM_ACTIVATE,
            WM_TIMER,
            WM_DESTROY,
        ] {
            assert!(!consumed(msg, 0, ON), "{msg:#x}");
        }
    }

    #[test]
    fn a_click_activates_the_island_only_in_keyboard_mode() {
        assert_eq!(mouse_activate(OFF), MA_NOACTIVATE);
        assert_eq!(mouse_activate(ON), MA_ACTIVATE);
    }

    #[test]
    fn keyboard_mode_toggles_only_no_activate_and_restores_it_exactly() {
        let no_activate = WS_EX_NOACTIVATE.0 as isize;
        // The style `create_window` gives the island.
        let created =
            (WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE).0
                as isize;
        let on = ex_style_for(created, ON);
        assert_eq!(on, created & !no_activate, "on removes NOACTIVATE only");
        // Off puts back exactly what create_window set.
        assert_eq!(ex_style_for(on, OFF), created);
        // Idempotent both ways.
        assert_eq!(ex_style_for(on, ON), on);
        assert_eq!(ex_style_for(created, OFF), created);
    }

    // ---- lparam ----

    #[test]
    fn lparam_coordinates_are_signed() {
        assert_eq!(lparam_xy(lp(12, 34)), (12, 34));
        assert_eq!(lparam_xy(lp(-5, 7)), (-5, 7));
        assert_eq!(lparam_xy(lp(9, -300)), (9, -300));
        assert_eq!(lparam_xy(lp(i16::MIN, i16::MAX)), (-32768, 32767));
        // The upper half of a 64-bit lparam is ignored.
        assert_eq!(lparam_xy(0x1234_0000_0003_0004), (4, 3));
    }
}
