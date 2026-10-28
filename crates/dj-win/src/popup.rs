//! A generic activatable popup window, used by the Settings window and Spotlight. Like
//! the Snap overlay it takes activation (it needs the keyboard) and its messages become
//! [`Event::Popup`] on the island's queue, so the app handles them outside any Win32
//! callback. Skia/DirectComposition draws into it (`WS_EX_NOREDIRECTIONBITMAP`).

use crate::overlay::join_utf16;
use crate::window::{push_event, Event};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
    VK_CONTROL, VK_F4, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Not exported by the windows crate.
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Which popup a message belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupKind {
    Settings,
    Spotlight,
    Tray,
}

impl PopupKind {
    fn class(self) -> PCWSTR {
        match self {
            PopupKind::Settings => w!("DaijobuSettingsWindow"),
            PopupKind::Spotlight => w!("DaijobuSpotlightWindow"),
            PopupKind::Tray => w!("DaijobuTrayMenu"),
        }
    }
    fn title(self) -> PCWSTR {
        match self {
            PopupKind::Settings => w!("Daijobu Island Settings"),
            PopupKind::Spotlight => w!("Daijobu Island Spotlight"),
            PopupKind::Tray => w!("Daijobu Island menu"),
        }
    }
    fn from_param(p: isize) -> Self {
        match p {
            1 => PopupKind::Spotlight,
            2 => PopupKind::Tray,
            _ => PopupKind::Settings,
        }
    }
    fn as_param(self) -> isize {
        match self {
            PopupKind::Settings => 0,
            PopupKind::Spotlight => 1,
            PopupKind::Tray => 2,
        }
    }
}

/// Input on a popup. Positions are client px (physical).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PopupInput {
    Press {
        x: f32,
        y: f32,
    },
    /// A right button press (the row menus).
    RightPress {
        x: f32,
        y: f32,
    },
    /// The second press of a double click (comes instead of a second `Press`).
    DoubleClick {
        x: f32,
        y: f32,
    },
    Motion {
        x: f32,
        y: f32,
    },
    Release {
        x: f32,
        y: f32,
    },
    /// The pointer left the window.
    Leave,
    /// Mouse wheel in lines (one notch is 1.0, positive is away from the user).
    Wheel {
        lines: f32,
    },
    /// A key went down. `ctrl` is false while Alt is also held (AltGr never reads as a
    /// Ctrl shortcut) and `alt` is false while Ctrl is also held.
    Key {
        vk: u16,
        ctrl: bool,
        shift: bool,
        alt: bool,
    },
    /// A typed printable character (surrogate pairs joined).
    Char(char),
    /// The window was asked to close (Alt+F4, the system menu). It is not destroyed.
    Close,
    /// The window was deactivated.
    FocusLost,
    /// The window moved to a monitor with another DPI (the suggested rect is applied).
    Dpi {
        dpi: u32,
    },
}

/// The popup's mouse cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Cursor {
    Arrow = 0,
    Hand,
    IBeam,
    SizeWe,
}

static CURSOR: AtomicU8 = AtomicU8::new(Cursor::Arrow as u8);

thread_local! {
    static HIGH_SURROGATE: Cell<Option<u16>> = const { Cell::new(None) };
    /// Per window: the client rect that drags the window (`HTCAPTION`).
    static DRAG_RECTS: RefCell<HashMap<isize, RECT>> = RefCell::new(HashMap::new());
}

fn cursor_id(c: u8) -> PCWSTR {
    match c {
        x if x == Cursor::Hand as u8 => IDC_HAND,
        x if x == Cursor::IBeam as u8 => IDC_IBEAM,
        x if x == Cursor::SizeWe as u8 => IDC_SIZEWE,
        _ => IDC_ARROW,
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

/// Sets the popup cursor (UI thread). It changes at once, not only on the next move.
pub fn set_cursor(c: Cursor) {
    if CURSOR.swap(c as u8, Ordering::Relaxed) != c as u8 {
        apply_cursor(c as u8);
    }
}

/// `WM_MOUSEWHEEL` high-word delta to lines (120 is one line).
pub fn wheel_lines(delta: i16) -> f32 {
    f32::from(delta) / WHEEL_DELTA as f32
}

fn lparam_xy(lp: LPARAM) -> (f32, f32) {
    let v = lp.0 as u32;
    (
        f32::from(v as u16 as i16),
        f32::from((v >> 16) as u16 as i16),
    )
}

fn key_down(vk: u16) -> bool {
    // SAFETY: no pointers involved.
    unsafe { GetKeyState(i32::from(vk)) < 0 }
}

/// Ctrl, Shift and Alt as `Key` reports them: Ctrl+Alt together (AltGr) counts as neither
/// Ctrl nor Alt.
pub fn key_mods(ctrl: bool, shift: bool, alt: bool) -> (bool, bool, bool) {
    (ctrl && !alt, shift, alt && !ctrl)
}

/// Creates the popup hidden at `(x, y, w, h)` desktop px. Show it with [`show`] once its
/// first frame is drawn. Settings is in Alt-Tab and the taskbar (with the tray's icon);
/// Spotlight is a topmost tool window.
pub fn create(kind: PopupKind, x: i32, y: i32, w: i32, h: i32) -> Result<HWND> {
    HIGH_SURROGATE.with(|s| s.set(None));
    let ex = match kind {
        PopupKind::Settings => WS_EX_NOREDIRECTIONBITMAP | WS_EX_APPWINDOW,
        PopupKind::Spotlight | PopupKind::Tray => {
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST
        }
    };
    // SAFETY: standard class registration and window creation; all pointers are valid
    // for the calls and the class and title names are static. Registering a class again
    // fails harmlessly. The create parameter is a plain integer, never dereferenced.
    let hwnd = unsafe {
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(popup_proc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: kind.class(),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        CreateWindowExW(
            ex,
            kind.class(),
            kind.title(),
            WS_POPUP,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinst.into()),
            Some(kind.as_param() as *const std::ffi::c_void),
        )?
    };
    if kind == PopupKind::Settings {
        if let Ok(icon) = crate::tray::make_icon() {
            // SAFETY: our own window; the icon is destroyed in `destroy`.
            unsafe {
                let _ = SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(ICON_BIG as usize)),
                    Some(LPARAM(icon.0 as isize)),
                );
                let _ = SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(ICON_SMALL as usize)),
                    Some(LPARAM(icon.0 as isize)),
                );
            }
        }
    }
    Ok(hwnd)
}

/// Shows the popup and gives it the foreground and the keyboard. Call it right after the
/// user's hotkey or click, which is what lets this process take the foreground (the same
/// way `window::take_keyboard` does).
pub fn show(hwnd: HWND) {
    // SAFETY: plain calls on our own window, on the thread that owns it.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        if PopupKind::from_param(GetWindowLongPtrW(hwnd, GWLP_USERDATA)) == PopupKind::Tray {
            SetCapture(hwnd);
        }
    }
}

/// Destroys the popup (and the icon it carries).
pub fn destroy(hwnd: HWND) {
    DRAG_RECTS.with(|d| d.borrow_mut().remove(&(hwnd.0 as isize)));
    // SAFETY: our own window, destroyed on the thread that created it. The icon from
    // `create` is fetched before the window goes and freed after.
    unsafe {
        let icon = SendMessageW(hwnd, WM_GETICON, Some(WPARAM(ICON_BIG as usize)), None).0;
        let _ = ReleaseCapture();
        let _ = DestroyWindow(hwnd);
        if icon != 0 {
            let _ = DestroyIcon(HICON(icon as *mut _));
        }
    }
}

/// Moves the popup without resizing or activating it.
pub fn move_to(hwnd: HWND, x: i32, y: i32) {
    // SAFETY: valid window handle owned by this thread.
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

pub fn set_bounds(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) -> Result<()> {
    // SAFETY: valid window handle owned by this thread; no activation or z-order change.
    unsafe { SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE) }
}

/// The client rect (device px) that drags the window, or `None` for no such area.
pub fn set_drag_rect(hwnd: HWND, rect: Option<RECT>) {
    DRAG_RECTS.with(|d| {
        let mut d = d.borrow_mut();
        match rect {
            Some(r) => d.insert(hwnd.0 as isize, r),
            None => d.remove(&(hwnd.0 as isize)),
        };
    });
}

/// The clipboard's text (for paste), if any.
pub fn clipboard_text() -> Option<String> {
    crate::clipboard::get_text(0)
}

fn kind_of(hwnd: HWND) -> PopupKind {
    // SAFETY: plain read of a window long on our own window.
    PopupKind::from_param(unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) })
}

fn send(hwnd: HWND, input: PopupInput) {
    push_event(Event::Popup(kind_of(hwnd), input));
}

extern "system" fn popup_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            // SAFETY: for WM_NCCREATE `lp` points to the CREATESTRUCTW of this call.
            unsafe {
                let cs = &*(lp.0 as *const CREATESTRUCTW);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            }
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_NCHITTEST => {
            let mut p = POINT {
                x: i32::from(lp.0 as u32 as u16 as i16),
                y: i32::from((lp.0 as u32 >> 16) as u16 as i16),
            };
            // SAFETY: valid window and point.
            let _ = unsafe { ScreenToClient(hwnd, &mut p) };
            let hit = DRAG_RECTS.with(|d| {
                d.borrow().get(&(hwnd.0 as isize)).is_some_and(|r| {
                    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
                })
            });
            return LRESULT(if hit { HTCAPTION } else { HTCLIENT } as isize);
        }
        WM_SETCURSOR if (lp.0 as u32 & 0xFFFF) == HTCLIENT => {
            apply_cursor(CURSOR.load(Ordering::Relaxed));
            return LRESULT(1);
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN if kind_of(hwnd) == PopupKind::Tray => {
            let (x, y) = lparam_xy(lp);
            let mut rect = RECT::default();
            // SAFETY: valid window and writable client rectangle.
            let _ = unsafe { GetClientRect(hwnd, &mut rect) };
            if x < 0.0 || y < 0.0 || x >= rect.right as f32 || y >= rect.bottom as f32 {
                send(hwnd, PopupInput::Close);
            } else if msg != WM_RBUTTONDOWN {
                send(hwnd, PopupInput::Press { x, y });
            }
            return LRESULT(0);
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            // SAFETY: our own window; capture keeps the release coming.
            let _ = unsafe { SetCapture(hwnd) };
            let (x, y) = lparam_xy(lp);
            send(
                hwnd,
                if msg == WM_LBUTTONDOWN {
                    PopupInput::Press { x, y }
                } else {
                    PopupInput::DoubleClick { x, y }
                },
            );
            return LRESULT(0);
        }
        WM_RBUTTONDOWN => {
            let (x, y) = lparam_xy(lp);
            send(hwnd, PopupInput::RightPress { x, y });
            return LRESULT(0);
        }
        // No system context menu on a right click (the app draws its own).
        WM_CONTEXTMENU => return LRESULT(0),
        WM_MOUSEMOVE => {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            // SAFETY: valid struct; asking again while tracking is harmless.
            let _ = unsafe { TrackMouseEvent(&mut tme) };
            let (x, y) = lparam_xy(lp);
            send(hwnd, PopupInput::Motion { x, y });
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            send(hwnd, PopupInput::Leave);
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            // SAFETY: releases only this thread's capture.
            if kind_of(hwnd) != PopupKind::Tray {
                let _ = unsafe { ReleaseCapture() };
            }
            let (x, y) = lparam_xy(lp);
            send(hwnd, PopupInput::Release { x, y });
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            let lines = wheel_lines((wp.0 >> 16) as u16 as i16);
            if lines != 0.0 {
                send(hwnd, PopupInput::Wheel { lines });
            }
            return LRESULT(0);
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wp.0 as u16;
            let alt = key_down(VK_MENU.0);
            // Alt+F4 still closes (through WM_CLOSE); every other key is ours, so the
            // system menu (Alt+Space, F10) never opens.
            if msg == WM_SYSKEYDOWN && vk == VK_F4.0 {
                // SAFETY: forwards the unhandled message with the original arguments.
                return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
            }
            let (ctrl, shift, alt) = key_mods(key_down(VK_CONTROL.0), key_down(VK_SHIFT.0), alt);
            send(
                hwnd,
                PopupInput::Key {
                    vk,
                    ctrl,
                    shift,
                    alt,
                },
            );
            return LRESULT(0);
        }
        WM_SYSCHAR => return LRESULT(0),
        WM_CHAR => {
            let pending = HIGH_SURROGATE.with(Cell::get);
            let (ch, pending) = join_utf16(pending, wp.0 as u16);
            HIGH_SURROGATE.with(|h| h.set(pending));
            if let Some(c) = ch {
                send(hwnd, PopupInput::Char(c));
            }
            return LRESULT(0);
        }
        WM_CLOSE => {
            send(hwnd, PopupInput::Close);
            return LRESULT(0);
        }
        WM_CAPTURECHANGED if kind_of(hwnd) == PopupKind::Tray => {
            send(hwnd, PopupInput::Close);
            return LRESULT(0);
        }
        WM_ACTIVATE => {
            if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE {
                send(hwnd, PopupInput::FocusLost);
            }
            return LRESULT(0);
        }
        WM_DPICHANGED => {
            // SAFETY: for WM_DPICHANGED `lp` points to the suggested RECT.
            unsafe {
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            send(
                hwnd,
                PopupInput::Dpi {
                    dpi: (wp.0 & 0xFFFF) as u32,
                },
            );
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: forwards the unhandled message with the original arguments.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Hides the popup (it keeps its window and renderer for the next time).
pub fn hide(hwnd: HWND) {
    // SAFETY: our own window, on the thread that owns it.
    unsafe {
        let _ = ReleaseCapture();
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_keeps_fractions() {
        assert_eq!(wheel_lines(120), 1.0);
        assert_eq!(wheel_lines(-240), -2.0);
        assert_eq!(wheel_lines(30), 0.25);
    }

    #[test]
    fn altgr_is_neither_ctrl_nor_alt() {
        assert_eq!(key_mods(true, false, false), (true, false, false));
        assert_eq!(key_mods(false, true, true), (false, true, true));
        assert_eq!(key_mods(true, false, true), (false, false, false));
    }

    #[test]
    fn reused_hidden_popup_resizes_without_activation() {
        let hwnd = create(PopupKind::Tray, 0, 0, 300, 480).unwrap();
        // SAFETY: this test only queries its own hidden window and destroys it without showing it.
        unsafe {
            let foreground = GetForegroundWindow();
            for (w, h) in [(300, 256), (300, 480)] {
                set_bounds(hwnd, 0, 0, w, h).unwrap();
                let mut rect = RECT::default();
                GetClientRect(hwnd, &mut rect).unwrap();
                assert_eq!((rect.right - rect.left, rect.bottom - rect.top), (w, h));
                assert!(!IsWindowVisible(hwnd).as_bool());
                assert_eq!(GetForegroundWindow(), foreground);
            }
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn kind_param_round_trips() {
        for k in [PopupKind::Settings, PopupKind::Spotlight, PopupKind::Tray] {
            assert_eq!(PopupKind::from_param(k.as_param()), k);
        }
    }
}
