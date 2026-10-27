//! Win32 side of the Tile window picker ("assist"): the list of windows a user would switch
//! to, their icons, live DWM thumbnails, restoring and activating a picked window, and the
//! picker window itself.
//!
//! This module only wraps system calls. Which windows show up, in which zone and in which
//! order is decided by `dj_core::tile_assist` and the app's controller. The pure parts, the
//! Alt-Tab filter, the icon pixel maths, the thumbnail properties and the message
//! translation, are plain functions with tests; nothing here is exercised against a real
//! window in tests, and no thumbnail is registered there.
//!
//! Like the other windows, the picker's window procedure never calls into the app. Its
//! messages become [`Event::Assist`] on the shared queue and the app handles them outside
//! any Win32 callback. Positions are client px (physical) and signed, because a captured
//! drag can leave the window on any side.

use crate::window::{push_event, Event};
use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::c_void;
use windows::core::{w, Result, BOOL, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmQueryThumbnailSourceSize, DwmRegisterThumbnail,
    DwmUnregisterThumbnail, DwmUpdateThumbnailProperties, DWMWA_CLOAKED, DWM_THUMBNAIL_PROPERTIES,
    DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE,
    TRACKMOUSEEVENT, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// A window handle as a plain value, the same as [`crate::tile::Raw`].
pub type Raw = crate::tile::Raw;

fn hwnd_of(w: Raw) -> HWND {
    HWND(w as *mut _)
}

// ---- the window list ----------------------------------------------------------------

/// One window the user could pick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowEntry {
    pub w: Raw,
    pub title: String,
    /// The window is minimised (it is listed, and restored when picked).
    pub minimized: bool,
    /// The owning process runs elevated, or its token is out of our reach (see
    /// [`list_windows`]). A normal process can't move it (UIPI).
    pub elevated: bool,
}

/// Shell windows that are never listed, matched by class name: the desktop and its worker
/// windows and the primary and secondary taskbars.
fn is_shell_class(class: &str) -> bool {
    matches!(
        class,
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    )
}

/// What the Alt-Tab filter needs to know about a visible top-level window.
#[derive(Clone, Copy, Debug)]
struct Facts<'a> {
    style: u32,
    ex_style: u32,
    visible: bool,
    cloaked: bool,
    own_process: bool,
    /// `GetWindow(GW_OWNER)` is not null: an owned window (a dialog, a popup).
    has_owner: bool,
    class: &'a str,
    title: &'a str,
}

/// A title bar (`WS_CAPTION`, both of its bits) or a sizing border (`WS_THICKFRAME`): the
/// same test as `tile::classify`, because a window Tile can't place is no use in the picker.
fn has_frame(style: u32) -> bool {
    style & WS_CAPTION.0 == WS_CAPTION.0 || style & WS_THICKFRAME.0 != 0
}

/// Whether a window is one a user would switch to, by the Alt-Tab rules: visible, with a
/// title, not cloaked (another virtual desktop, a suspended UWP app), not ours, not a shell
/// window, either unowned or asking for a taskbar button (`WS_EX_APPWINDOW`). Never a tool
/// window, even one with that button: placement (`tile::classify`) refuses tool windows,
/// and a card that can't be placed would only frustrate. On top of Alt-Tab it must have a
/// title bar or a sizing border (see [`has_frame`]), which also drops the shell's popups,
/// overlays and splash screens. Minimised windows pass.
fn switchable(f: &Facts) -> bool {
    let app_window = f.ex_style & WS_EX_APPWINDOW.0 != 0;
    f.visible
        && !f.cloaked
        && !f.own_process
        && !f.title.trim().is_empty()
        && !is_shell_class(f.class)
        && f.style & WS_CHILD.0 == 0
        && f.ex_style & WS_EX_TOOLWINDOW.0 == 0
        && (!f.has_owner || app_window)
        && has_frame(f.style)
}

/// Whether a failed process or token query means "elevated": we were refused. Any other
/// failure (the process is gone, a bad id) says nothing, so the window counts as normal.
/// `hresult` is the error's `HRESULT`, `HRESULT_FROM_WIN32` of the Win32 code.
fn elevated_on_error(hresult: i32) -> bool {
    hresult as u32 == 0x8007_0000 | ERROR_ACCESS_DENIED.0
}

/// A kernel handle closed on drop.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful `OpenProcess`/`OpenProcessToken` and is
        // closed once, here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Whether process `pid` runs elevated (`TokenElevation`). A process or a token we may not
/// open counts as elevated: that is what UIPI looks like from a normal-integrity process.
fn process_elevated(pid: u32) -> bool {
    // SAFETY: plain queries; every handle is closed by its guard and `elevation` is a
    // valid, correctly sized out-buffer for `TokenElevation`.
    unsafe {
        let process = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => OwnedHandle(h),
            Err(e) => return elevated_on_error(e.code().0),
        };
        let mut token = HANDLE::default();
        if let Err(e) = OpenProcessToken(process.0, TOKEN_QUERY, &mut token) {
            return elevated_on_error(e.code().0);
        }
        let token = OwnedHandle(token);
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        match GetTokenInformation(
            token.0,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast::<c_void>()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        ) {
            Ok(()) => elevation.TokenIsElevated != 0,
            Err(e) => elevated_on_error(e.code().0),
        }
    }
}

struct Walk {
    own_pid: u32,
    entries: Vec<WindowEntry>,
    /// One token query per process, not per window.
    elevated: HashMap<u32, bool>,
}

/// The entry for `hwnd` if it is one the user would switch to.
///
/// # Safety
/// `hwnd` must be a window handle from `EnumWindows`.
unsafe fn entry(hwnd: HWND, walk: &mut Walk) -> Option<WindowEntry> {
    // SAFETY: plain queries on a handle that may have gone stale since it was enumerated
    // (a stale one fails the calls and the window is skipped), with correctly sized
    // out-buffers.
    unsafe {
        // The cheap test first: most of the enumerated windows are hidden.
        if !IsWindowVisible(hwnd).as_bool() {
            return None;
        }
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

        let mut class = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..n]);

        let mut text = [0u16; 512];
        let n = GetWindowTextW(hwnd, &mut text).max(0) as usize;
        let title = String::from_utf16_lossy(&text[..n]);

        let facts = Facts {
            style: GetWindowLongW(hwnd, GWL_STYLE) as u32,
            ex_style: GetWindowLongW(hwnd, GWL_EXSTYLE) as u32,
            visible: true,
            cloaked,
            own_process: pid == walk.own_pid,
            has_owner: GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.0.is_null()),
            class: &class,
            title: &title,
        };
        if !switchable(&facts) {
            return None;
        }
        let elevated = *walk
            .elevated
            .entry(pid)
            .or_insert_with(|| pid != 0 && process_elevated(pid));
        Some(WindowEntry {
            w: hwnd.0 as Raw,
            title,
            minimized: IsIconic(hwnd).as_bool(),
            elevated,
        })
    }
}

/// Top-level windows a user would switch to, newest (top of the z-order) first: visible,
/// titled, not cloaked (other virtual desktops), not tool windows, not ours, not the shell
/// (Progman, WorkerW, the taskbars), owned by nobody (Alt-Tab's rule; `WS_EX_APPWINDOW`
/// overrides the tool and owner tests) and with a title bar or sizing border. Minimised
/// windows are included. `EnumWindows` walks the z-order from the top, so that is the order.
///
/// `elevated` is the owning process's `TokenElevation`; a token we may not open counts as
/// elevated. It is absolute, not relative to us: when Daijobu itself runs elevated, every
/// elevated window still reads `true`.
pub fn list_windows() -> Vec<WindowEntry> {
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `&mut Walk` passed below and outlives the enumeration;
        // `hwnd` is valid for the duration of the callback.
        unsafe {
            let walk = &mut *(data.0 as *mut Walk);
            if let Some(e) = entry(hwnd, walk) {
                walk.entries.push(e);
            }
        }
        true.into()
    }
    let mut walk = Walk {
        // SAFETY: no arguments.
        own_pid: unsafe { GetCurrentProcessId() },
        entries: Vec::new(),
        elevated: HashMap::new(),
    };
    // SAFETY: the callback only touches `walk`, which outlives the call.
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut walk as *mut Walk as isize));
    }
    walk.entries
}

// ---- icons --------------------------------------------------------------------------

/// The largest icon `icon_bgra` draws (px); a larger request gives `None`.
const MAX_ICON: u32 = 256;
/// How long a window may take to answer `WM_GETICON` (ms). A hung one is skipped at once.
const ICON_TIMEOUT_MS: u32 = 50;

/// `WM_GETICON` kinds in the order to try them for an icon `size` px wide: the big icon
/// first (scaling down looks better than scaling up) unless the request is as small as the
/// small icon.
fn icon_order(size: u32) -> [u32; 3] {
    if size <= 16 {
        [ICON_SMALL2, ICON_SMALL, ICON_BIG]
    } else {
        [ICON_BIG, ICON_SMALL2, ICON_SMALL]
    }
}

/// The class icon slots in the order to try them, like [`icon_order`].
fn class_icon_order(size: u32) -> [GET_CLASS_LONG_INDEX; 2] {
    if size <= 16 {
        [GCLP_HICONSM, GCLP_HICON]
    } else {
        [GCLP_HICON, GCLP_HICONSM]
    }
}

/// Every icon handle the window and its class offer, in the order to try them. The handles
/// belong to the window or the class: never destroy them.
fn icon_candidates(w: Raw, size: u32) -> Vec<HICON> {
    let hwnd = hwnd_of(w);
    let mut out = Vec::new();
    // SAFETY: `WM_GETICON` takes no pointers; `result` is a valid out-pointer for the
    // duration of each call, which returns within the timeout (at once for a hung window).
    // `GetClassLongPtrW` is a plain query that fails on a stale handle.
    unsafe {
        for kind in icon_order(size) {
            let mut result = 0usize;
            let ok = SendMessageTimeoutW(
                hwnd,
                WM_GETICON,
                WPARAM(kind as usize),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                ICON_TIMEOUT_MS,
                Some(&mut result),
            );
            if ok.0 != 0 {
                push_unique(&mut out, result);
            }
        }
        for index in class_icon_order(size) {
            push_unique(&mut out, GetClassLongPtrW(hwnd, index));
        }
    }
    out
}

/// Adds the icon handle `raw` to `out` unless it is 0 (no icon) or already there (a window
/// often reports one icon for several kinds).
fn push_unique(out: &mut Vec<HICON>, raw: usize) {
    if raw != 0 && !out.iter().any(|h| h.0 as usize == raw) {
        out.push(HICON(raw as *mut c_void));
    }
}

/// A top-down 32-bit DIB section in its own memory DC. Everything is freed on drop.
struct Dib {
    dc: HDC,
    bmp: HBITMAP,
    /// The bitmap that was selected in `dc` before ours.
    old: HGDIOBJ,
    bits: *mut u8,
    size: u32,
}

impl Dib {
    fn new(size: u32) -> Option<Dib> {
        let header = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size as i32,
            // Negative: top-down rows.
            biHeight: -(size as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let info = BITMAPINFO {
            bmiHeader: header,
            ..Default::default()
        };
        // SAFETY: plain GDI object creation; `info` is fully initialised and `bits` is a
        // valid out-pointer. Each early return frees what was made so far.
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return None;
            }
            let mut bits: *mut c_void = std::ptr::null_mut();
            let Ok(bmp) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            else {
                let _ = DeleteDC(dc);
                return None;
            };
            if bits.is_null() {
                let _ = DeleteObject(HGDIOBJ(bmp.0));
                let _ = DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, HGDIOBJ(bmp.0));
            Some(Dib {
                dc,
                bmp,
                old,
                bits: bits.cast(),
                size,
            })
        }
    }

    fn len(&self) -> usize {
        self.size as usize * self.size as usize * 4
    }

    /// Sets every byte (all four channels) to `byte`.
    fn fill(&mut self, byte: u8) {
        // SAFETY: `bits` points at `len()` bytes owned by the DIB section, alive as long
        // as `self`. `GdiFlush` first, so no batched GDI call is still to write into it.
        unsafe {
            let _ = GdiFlush();
            std::ptr::write_bytes(self.bits, byte, self.len());
        }
    }

    /// Draws `icon` over the whole bitmap, scaled to its size.
    fn draw(&mut self, icon: HICON) -> bool {
        let s = self.size as i32;
        // SAFETY: `dc` is our memory DC with the DIB selected and `icon` is a handle that
        // may have gone stale (the call then fails).
        unsafe { DrawIconEx(self.dc, 0, 0, icon, s, s, 0, None, DI_NORMAL).is_ok() }
    }

    fn pixels(&self) -> Vec<u8> {
        // SAFETY: as in `fill`; the bytes are copied out before anything draws again.
        unsafe {
            let _ = GdiFlush();
            std::slice::from_raw_parts(self.bits, self.len()).to_vec()
        }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        // SAFETY: all three were made by `Dib::new` and are released once: the DIB is
        // unselected before it is deleted, and the DC after both.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(HGDIOBJ(self.bmp.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Premultiplied BGRA from the same icon drawn over black and over white (both BGRA, equal
/// length). GDI writes no reliable alpha into a DIB, but a pixel with coverage `a` reads
/// `p` over black and `p + 255 (1 - a)` over white, so `a` falls out of the difference and
/// `p` (already premultiplied) is the black pass. This holds for icons with an alpha
/// channel and for old mask icons alike.
fn compose_alpha(on_black: &[u8], on_white: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(on_black.len());
    for (b, w) in on_black
        .as_chunks::<4>()
        .0
        .iter()
        .zip(on_white.as_chunks::<4>().0)
    {
        let diff: u32 = (0..3).map(|i| u32::from(w[i].saturating_sub(b[i]))).sum();
        let a = (255 - ((diff + 1) / 3).min(255)) as u8;
        // A channel can't exceed the alpha of a premultiplied pixel.
        out.extend_from_slice(&[b[0].min(a), b[1].min(a), b[2].min(a), a]);
    }
    out
}

/// Every pixel is fully transparent: the icon drew nothing.
fn is_blank(bgra: &[u8]) -> bool {
    bgra.as_chunks::<4>().0.iter().all(|px| px[3] == 0)
}

/// `icon` drawn into `size` x `size` premultiplied BGRA, or `None` if it draws nothing.
fn draw_icon(icon: HICON, size: u32) -> Option<Vec<u8>> {
    let mut dib = Dib::new(size)?;
    dib.fill(0x00);
    if !dib.draw(icon) {
        return None;
    }
    let on_black = dib.pixels();
    dib.fill(0xFF);
    if !dib.draw(icon) {
        return None;
    }
    let on_white = dib.pixels();
    let out = compose_alpha(&on_black, &on_white);
    (!is_blank(&out)).then_some(out)
}

/// The window's icon (`WM_GETICON` big, small2 and small, else the class icon) drawn into
/// `size` x `size` px as premultiplied BGRA, rows top-down. `None` if the window offers no
/// icon, or only one that draws nothing, or `size` is 0 or above 256. Takes a few
/// milliseconds at most per window (a window that doesn't answer is skipped), so cache the
/// result per window.
pub fn icon_bgra(w: Raw, size: u32) -> Option<Vec<u8>> {
    if w == 0 || size == 0 || size > MAX_ICON {
        return None;
    }
    icon_candidates(w, size)
        .into_iter()
        .find_map(|icon| draw_icon(icon, size))
}

// ---- thumbnails ---------------------------------------------------------------------

/// The properties that put a thumbnail at `rect`, `(x, y, w, h)` in the destination's
/// client px: that rect, fully opaque, the whole source window (not only its client area)
/// and `visible`. The source rect is left unset, so DWM uses the whole source.
fn thumb_props(rect: (i32, i32, i32, i32), visible: bool) -> DWM_THUMBNAIL_PROPERTIES {
    let (x, y, w, h) = rect;
    DWM_THUMBNAIL_PROPERTIES {
        dwFlags: DWM_TNP_RECTDESTINATION
            | DWM_TNP_VISIBLE
            | DWM_TNP_SOURCECLIENTAREAONLY
            | DWM_TNP_OPACITY,
        rcDestination: RECT {
            left: x,
            top: y,
            right: x.saturating_add(w.max(0)),
            bottom: y.saturating_add(h.max(0)),
        },
        rcSource: RECT::default(),
        opacity: 255,
        fVisible: visible.into(),
        fSourceClientAreaOnly: false.into(),
    }
}

/// A size with something in it.
fn positive_size(cx: i32, cy: i32) -> Option<(i32, i32)> {
    (cx > 0 && cy > 0).then_some((cx, cy))
}

/// A live DWM thumbnail of another window, drawn by DWM on top of a destination window.
/// Unregistered on drop.
///
/// DWM scales the source to fill the whole destination rect, so fit the rect to
/// [`Thumbnail::source_size`]'s aspect first.
pub struct Thumbnail {
    /// The `HTHUMBNAIL`.
    id: isize,
}

impl Thumbnail {
    /// Registers a thumbnail of `src` on `dest`. It is hidden until [`Thumbnail::show`].
    /// `None` if DWM refuses (the window is gone, DWM is off, the window is ours and
    /// `dest`'s ancestor, ...) or either handle is null.
    pub fn new(dest: HWND, src: Raw) -> Option<Thumbnail> {
        if dest.0.is_null() || src == 0 {
            return None;
        }
        // SAFETY: both handles may be stale (the call then fails); no pointers involved.
        let id = unsafe { DwmRegisterThumbnail(dest, hwnd_of(src)) }.ok()?;
        Some(Thumbnail { id })
    }

    /// The source window's size in px as DWM sees it, or `None` if it has none (a
    /// minimised window, a refused one).
    pub fn source_size(&self) -> Option<(i32, i32)> {
        // SAFETY: `id` is a live thumbnail handle owned by `self`.
        let size = unsafe { DwmQueryThumbnailSourceSize(self.id) }.ok()?;
        positive_size(size.cx, size.cy)
    }

    /// Puts the thumbnail at `rect` = `(x, y, w, h)` in the destination's client px;
    /// `visible` false hides it. Errors are ignored: a thumbnail that can't be updated just
    /// stays where it was.
    pub fn show(&self, rect: (i32, i32, i32, i32), visible: bool) {
        let props = thumb_props(rect, visible);
        // SAFETY: `id` is a live thumbnail handle owned by `self` and `props` is a fully
        // initialised structure that outlives the call.
        unsafe {
            let _ = DwmUpdateThumbnailProperties(self.id, &props);
        }
    }
}

impl Drop for Thumbnail {
    fn drop(&mut self) {
        // SAFETY: `id` came from `DwmRegisterThumbnail` and is unregistered once. If the
        // destination window is already gone DWM has dropped it and this just fails.
        unsafe {
            let _ = DwmUnregisterThumbnail(self.id);
        }
    }
}

// ---- restoring and activating a window ----------------------------------------------

/// Whether a window still has to be restored before it is placed.
fn needs_restore(iconic: bool, zoomed: bool) -> bool {
    iconic || zoomed
}

/// Un-minimises and un-maximises a window (`SW_RESTORE`) so it can be placed. A window that
/// was maximised before it was minimised needs two restores: the first brings it back
/// maximised. A hung or vanished window is left alone, so the call never blocks the UI.
/// Restoring a minimised window activates it, which takes the foreground from the picker.
pub fn restore_any(w: Raw) {
    if w == 0 {
        return;
    }
    let hwnd = hwnd_of(w);
    // SAFETY: plain calls on a handle that may be stale (then they do nothing).
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() || IsHungAppWindow(hwnd).as_bool() {
            return;
        }
        for _ in 0..2 {
            if !needs_restore(IsIconic(hwnd).as_bool(), IsZoomed(hwnd).as_bool()) {
                break;
            }
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
    }
}

/// Brings a window to the foreground. Best effort: the system may refuse (the foreground
/// lock), and then nothing happens.
pub fn activate(w: Raw) {
    if w == 0 {
        return;
    }
    // SAFETY: a plain call on a handle that may be stale (then it fails).
    unsafe {
        let _ = SetForegroundWindow(hwnd_of(w));
    }
}

/// Puts a window at the top of the (non-topmost) z-order without activating it: the
/// picker was left by a click elsewhere, so the keyboard stays where the user put it.
pub fn raise(w: Raw) {
    if w == 0 {
        return;
    }
    // SAFETY: a plain call on a handle that may be stale (then it fails).
    unsafe {
        let _ = SetWindowPos(
            hwnd_of(w),
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

// ---- the picker window --------------------------------------------------------------

const CLASS_NAME: PCWSTR = w!("DaijobuTileAssist");
/// Not exported by the windows crate.
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Input on the picker window. Positions are client px (physical); add the window's origin
/// for desktop coordinates. The app turns these into hover, clicks and keys for
/// `dj_core::tile_assist`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssistInput {
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
    /// A key went down while the window has the keyboard (see [`show`]). `ctrl` is false
    /// while Alt is held too, so AltGr (Ctrl+Alt) characters never read as Ctrl shortcuts.
    Key { vk: u16, ctrl: bool, shift: bool },
    /// The keyboard focus or the activation went away (`WM_KILLFOCUS`, or `WM_ACTIVATE`
    /// with `WA_INACTIVE`). One deactivation can send both, so handle it idempotently.
    FocusLost,
}

thread_local! {
    /// A `WM_MOUSELEAVE` is armed. Separate from the island's and the previews' flags.
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    /// We are releasing the pointer capture ourselves, so the `WM_CAPTURECHANGED` it
    /// sends is not a loss to report.
    static RELEASING: Cell<bool> = const { Cell::new(false) };
}

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
fn key_input(wp: usize, ctrl_down: bool, alt_down: bool, shift_down: bool) -> AssistInput {
    AssistInput::Key {
        vk: wp as u16,
        ctrl: ctrl_down && !alt_down,
        shift: shift_down,
    }
}

/// Maps the messages that need no side effect (and no key state) to their input.
/// `WM_CAPTURECHANGED`, `WM_KEYDOWN` and `WM_MOUSEMOVE`'s tracking are handled by the
/// window procedure itself.
fn translate(msg: u32, wp: usize, lp: isize) -> Option<AssistInput> {
    let (x, y) = lparam_xy(lp);
    match msg {
        WM_LBUTTONDOWN => Some(AssistInput::Press { x, y }),
        WM_LBUTTONUP => Some(AssistInput::Release { x, y }),
        WM_MOUSEMOVE => Some(AssistInput::Move { x, y }),
        WM_MOUSELEAVE | WM_CANCELMODE => Some(AssistInput::Leave),
        WM_KILLFOCUS => Some(AssistInput::FocusLost),
        // The low word of `wparam` is the activation state; the high word is "minimized".
        WM_ACTIVATE if (wp & 0xFFFF) as u32 == WA_INACTIVE => Some(AssistInput::FocusLost),
        _ => None,
    }
}

/// Whether a `WM_CAPTURECHANGED` is a real loss: not our own release, and not the capture
/// being handed to this very window. `gaining` is the message's `lparam`, the window
/// that is taking the capture (0 when nobody is).
fn capture_lost(releasing: bool, gaining: isize, own: isize) -> bool {
    !releasing && gaining != own
}

/// Activatable (it needs the keyboard), topmost, out of Alt-Tab and the taskbar, and no GDI
/// surface (DirectComposition draws it).
fn picker_ex_style() -> WINDOW_EX_STYLE {
    WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST
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

fn send(input: AssistInput) {
    push_event(Event::Assist(input));
}

extern "system" fn assist_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
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
            send(AssistInput::Leave);
            return LRESULT(0);
        }
        // Capture went to another window or was cancelled by the system: a press in
        // progress would never see its release, so report it as a leave.
        WM_CAPTURECHANGED => {
            if capture_lost(RELEASING.with(Cell::get), lp.0, hwnd.0 as isize) {
                send(AssistInput::Leave);
            }
            return LRESULT(0);
        }
        WM_CANCELMODE => {
            // SAFETY: no pointers involved; we only compare handles.
            if unsafe { GetCapture() } == hwnd {
                release_capture_quietly();
            }
            send(AssistInput::Leave);
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
        // keyboard focus. A click activates the window (no `WM_MOUSEACTIVATE` answer).
        WM_KILLFOCUS | WM_ACTIVATE => {
            if let Some(input) = translate(msg, wp.0, lp.0) {
                send(input);
            }
        }
        // The app owns the window's lifetime: Alt+F4 or a taskbar close does nothing.
        WM_CLOSE => return LRESULT(0),
        // The app places and sizes the window itself, zone by zone.
        WM_DPICHANGED => return LRESULT(0),
        _ => {}
    }
    // SAFETY: forwards the unhandled message with the original arguments.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Creates the picker window, hidden, at `(x, y, w, h)` physical desktop px (the zone).
/// Draw its first frame, then [`show`], so nothing flashes.
pub fn create(x: i32, y: i32, w: i32, h: i32) -> Result<HWND> {
    TRACKING.with(|t| t.set(false));
    RELEASING.with(|r| r.set(false));
    // SAFETY: standard class registration and window creation; all pointers are valid for
    // the duration of the calls and the class name is a static string. Registering the
    // class again on a later run fails harmlessly.
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(assist_proc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        CreateWindowExW(
            picker_ex_style(),
            CLASS_NAME,
            w!("Daijobu Island Tile assist"),
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

/// Moves and resizes the window (desktop px) and keeps it topmost, without activating it.
/// Used to move the picker to the next zone.
pub fn place(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    // SAFETY: valid window handle owned by this thread.
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
    }
}

/// Shows the window on top of everything and gives it the foreground and the keyboard, so
/// `Key` input starts to arrive. Call it again after another window took the foreground
/// (restoring a minimised window does).
pub fn show(hwnd: HWND) {
    // SAFETY: our own window, on the thread that owns it. A click on the island or on the
    // picker just came in, so this process may take the foreground.
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

/// Destroys the window (on the thread that created it). Losing the focus and the capture
/// arrives as `FocusLost` and `Leave` afterwards, so ignore those once the run is over.
pub fn destroy(hwnd: HWND) {
    TRACKING.with(|t| t.set(false));
    release_capture_quietly();
    // SAFETY: our own window, destroyed on the thread that created it.
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPTION: u32 = WS_CAPTION.0;
    const THICK: u32 = WS_THICKFRAME.0;

    /// A normal framed app window: visible, titled, unowned, not ours.
    fn facts() -> Facts<'static> {
        Facts {
            style: CAPTION | THICK,
            ex_style: 0,
            visible: true,
            cloaked: false,
            own_process: false,
            has_owner: false,
            class: "Notepad",
            title: "notes.txt - Notepad",
        }
    }

    #[test]
    fn a_normal_window_is_listed() {
        assert!(switchable(&facts()));
        // Other extended styles don't matter.
        let f = Facts {
            ex_style: WS_EX_TOPMOST.0 | WS_EX_ACCEPTFILES.0 | WS_EX_WINDOWEDGE.0,
            ..facts()
        };
        assert!(switchable(&f));
    }

    #[test]
    fn each_rule_rejects() {
        let no = |f: Facts, why: &str| assert!(!switchable(&f), "{why}");
        no(
            Facts {
                visible: false,
                ..facts()
            },
            "hidden",
        );
        no(
            Facts {
                cloaked: true,
                ..facts()
            },
            "cloaked: another virtual desktop",
        );
        no(
            Facts {
                own_process: true,
                ..facts()
            },
            "ours",
        );
        no(
            Facts {
                title: "",
                ..facts()
            },
            "no title",
        );
        no(
            Facts {
                title: "  \t ",
                ..facts()
            },
            "a blank title",
        );
        no(
            Facts {
                style: facts().style | WS_CHILD.0,
                ..facts()
            },
            "a child window",
        );
        no(
            Facts {
                has_owner: true,
                ..facts()
            },
            "owned (a dialog)",
        );
        no(
            Facts {
                ex_style: WS_EX_TOOLWINDOW.0,
                ..facts()
            },
            "tool window",
        );
    }

    #[test]
    fn the_app_window_style_overrides_the_owner_but_not_a_tool_window() {
        let app = WS_EX_APPWINDOW.0;
        // Alt-Tab lists an owned window that asks for a taskbar button.
        assert!(switchable(&Facts {
            has_owner: true,
            ex_style: app,
            ..facts()
        }));
        // A tool window can't be placed (tile::classify), so it is never a card.
        assert!(!switchable(&Facts {
            ex_style: WS_EX_TOOLWINDOW.0 | app,
            ..facts()
        }));
        assert!(!switchable(&Facts {
            has_owner: true,
            ex_style: WS_EX_TOOLWINDOW.0 | app,
            ..facts()
        }));
        // It overrides nothing else.
        assert!(!switchable(&Facts {
            cloaked: true,
            ex_style: app,
            ..facts()
        }));
        assert!(!switchable(&Facts {
            own_process: true,
            ex_style: app,
            ..facts()
        }));
        assert!(!switchable(&Facts {
            visible: false,
            ex_style: app,
            ..facts()
        }));
        assert!(!switchable(&Facts {
            title: "",
            ex_style: app,
            ..facts()
        }));
    }

    #[test]
    fn a_window_needs_a_title_bar_or_a_sizing_border() {
        let s = |style| switchable(&Facts { style, ..facts() });
        assert!(s(CAPTION));
        assert!(s(THICK));
        assert!(s(CAPTION | THICK | WS_VISIBLE.0 | WS_MINIMIZE.0));
        // Neither: a popup, a splash screen, the shell's flyouts.
        assert!(!s(0));
        assert!(!s(WS_POPUP.0));
        assert!(!s(WS_VISIBLE.0 | WS_POPUP.0));
        // WS_CAPTION is two bits; one of them is not a title bar.
        assert!(!s(WS_BORDER.0));
        assert!(!s(WS_DLGFRAME.0));
        assert!(s(WS_BORDER.0 | WS_DLGFRAME.0));
    }

    #[test]
    fn shell_classes_are_never_listed() {
        for class in [
            "Progman",
            "WorkerW",
            "Shell_TrayWnd",
            "Shell_SecondaryTrayWnd",
        ] {
            assert!(is_shell_class(class), "{class}");
            assert!(
                !switchable(&Facts {
                    class,
                    title: "Program Manager",
                    ..facts()
                }),
                "{class}"
            );
        }
        for class in [
            "",
            "Chrome_WidgetWin_1",
            "CabinetWClass",
            "ApplicationFrameWindow",
            "progman",
            "Shell_TrayWnd2",
            "DaijobuTileAssist",
        ] {
            assert!(!is_shell_class(class), "{class}");
        }
    }

    #[test]
    fn a_minimised_window_is_not_excluded() {
        // `switchable` has no minimised input at all: WS_MINIMIZE in the style is fine.
        assert!(switchable(&Facts {
            style: CAPTION | THICK | WS_MINIMIZE.0,
            ..facts()
        }));
    }

    #[test]
    fn only_a_refusal_means_elevated() {
        // 0x80070005 is HRESULT_FROM_WIN32(ERROR_ACCESS_DENIED).
        assert!(elevated_on_error(0x8007_0005_u32 as i32));
        // ERROR_INVALID_PARAMETER (87): the process id is gone.
        assert!(!elevated_on_error(0x8007_0057_u32 as i32));
        // ERROR_INVALID_HANDLE, a generic failure, success.
        assert!(!elevated_on_error(0x8007_0006_u32 as i32));
        assert!(!elevated_on_error(0x8000_4005_u32 as i32));
        assert!(!elevated_on_error(0));
        // The bare Win32 code is not an HRESULT.
        assert!(!elevated_on_error(5));
    }

    #[test]
    fn icons_are_tried_big_first_unless_the_request_is_small() {
        assert_eq!(icon_order(20), [ICON_BIG, ICON_SMALL2, ICON_SMALL]);
        assert_eq!(icon_order(32), [ICON_BIG, ICON_SMALL2, ICON_SMALL]);
        assert_eq!(icon_order(16), [ICON_SMALL2, ICON_SMALL, ICON_BIG]);
        assert_eq!(icon_order(1), [ICON_SMALL2, ICON_SMALL, ICON_BIG]);
        assert_eq!(class_icon_order(20), [GCLP_HICON, GCLP_HICONSM]);
        assert_eq!(class_icon_order(16), [GCLP_HICONSM, GCLP_HICON]);
    }

    #[test]
    fn alpha_comes_from_the_black_and_white_passes() {
        // Four pixels (BGRA): opaque red, transparent, half-covered white, opaque black.
        let on_black = [
            0, 0, 255, 0, // red, over black
            0, 0, 0, 0, // nothing
            128, 128, 128, 0, // white at ~50 %, premultiplied
            0, 0, 0, 0, // opaque black
        ];
        let on_white = [
            0, 0, 255, 0, // red covers the white completely
            255, 255, 255, 0, // nothing: the background shows
            255, 255, 255, 0, // white over white is white
            0, 0, 0, 0, // black covers it too
        ];
        let out = compose_alpha(&on_black, &on_white);
        assert_eq!(&out[0..4], &[0, 0, 255, 255], "opaque red");
        assert_eq!(&out[4..8], &[0, 0, 0, 0], "transparent");
        // 255 - (255 - 128) = 128: half covered, colour kept premultiplied.
        assert_eq!(&out[8..12], &[128, 128, 128, 128], "half white");
        // Opaque black: alpha 255 even though the colour is 0.
        assert_eq!(&out[12..16], &[0, 0, 0, 255], "opaque black");
    }

    #[test]
    fn composed_pixels_stay_premultiplied() {
        // A channel brighter than the alpha (rounding noise, a broken blend) is clamped.
        let out = compose_alpha(&[200, 10, 90, 0], &[230, 40, 120, 0]);
        // diff = 30 on every channel: alpha 225; 200 <= 225 stays, nothing exceeds it.
        assert_eq!(out, vec![200, 10, 90, 225]);
        let out = compose_alpha(&[250, 0, 0, 0], &[255, 255, 255, 0]);
        // diff = (5 + 255 + 255) / 3 = 172: alpha 83, so 250 is clamped to 83.
        assert_eq!(out[3], 83);
        assert_eq!(out[0], 83);
        // The alpha byte GDI left in the DIBs is ignored.
        let a = compose_alpha(&[1, 2, 3, 0], &[1, 2, 3, 0]);
        let b = compose_alpha(&[1, 2, 3, 77], &[1, 2, 3, 200]);
        assert_eq!(a, b);
        // No pixels, no output; a stray partial pixel is dropped.
        assert!(compose_alpha(&[], &[]).is_empty());
        assert!(compose_alpha(&[1, 2, 3], &[1, 2, 3]).is_empty());
    }

    #[test]
    fn icon_handles_are_listed_once_and_zero_is_no_icon() {
        let mut out = Vec::new();
        push_unique(&mut out, 0);
        assert!(out.is_empty());
        push_unique(&mut out, 0x10);
        push_unique(&mut out, 0x20);
        push_unique(&mut out, 0x10);
        push_unique(&mut out, 0);
        let raw: Vec<usize> = out.iter().map(|h| h.0 as usize).collect();
        assert_eq!(raw, [0x10, 0x20]);
    }

    #[test]
    fn a_drawing_with_no_coverage_is_blank() {
        assert!(is_blank(&[]));
        assert!(is_blank(&[9, 9, 9, 0, 0, 0, 0, 0]));
        assert!(!is_blank(&[0, 0, 0, 0, 0, 0, 0, 1]));
    }

    #[test]
    fn icons_reject_a_missing_window_and_odd_sizes() {
        assert_eq!(icon_bgra(0, 20), None);
        assert_eq!(icon_bgra(0x1234, 0), None);
        assert_eq!(icon_bgra(0x1234, MAX_ICON + 1), None);
    }

    #[test]
    fn thumbnail_properties_place_and_show() {
        let p = thumb_props((10, 20, 300, 188), true);
        // Copy the fields out: the structure is packed.
        let (flags, dest, opacity, visible, client_only) = (
            p.dwFlags,
            p.rcDestination,
            p.opacity,
            p.fVisible,
            p.fSourceClientAreaOnly,
        );
        assert_eq!(
            flags,
            DWM_TNP_RECTDESTINATION
                | DWM_TNP_VISIBLE
                | DWM_TNP_SOURCECLIENTAREAONLY
                | DWM_TNP_OPACITY
        );
        // `(x, y, w, h)` becomes `(left, top, right, bottom)`.
        assert_eq!(
            (dest.left, dest.top, dest.right, dest.bottom),
            (10, 20, 310, 208)
        );
        assert_eq!(opacity, 255);
        assert!(visible.as_bool());
        // The whole window, frame and all.
        assert!(!client_only.as_bool());

        let hidden = thumb_props((0, 0, 10, 10), false);
        let visible = hidden.fVisible;
        assert!(!visible.as_bool());
        // A negative origin (a monitor left of the primary one is not a client coordinate,
        // but a rect can sit partly outside the window) and a negative size are safe.
        let p = thumb_props((-5, -6, -10, 4), true);
        let dest = p.rcDestination;
        assert_eq!(
            (dest.left, dest.top, dest.right, dest.bottom),
            (-5, -6, -5, -2)
        );
        let p = thumb_props((i32::MAX, 0, 100, 100), true);
        let dest = p.rcDestination;
        assert_eq!(dest.right, i32::MAX);
    }

    #[test]
    fn a_source_needs_a_size() {
        assert_eq!(positive_size(1280, 720), Some((1280, 720)));
        assert_eq!(positive_size(0, 720), None);
        assert_eq!(positive_size(1280, 0), None);
        assert_eq!(positive_size(-1, -1), None);
    }

    #[test]
    fn no_thumbnail_is_registered_for_a_null_handle() {
        // Both guards return before DWM is called.
        assert!(Thumbnail::new(hwnd_of(0x1234), 0).is_none());
        assert!(Thumbnail::new(HWND::default(), 0x1234).is_none());
    }

    #[test]
    fn maximised_or_minimised_needs_a_restore() {
        assert!(needs_restore(true, false));
        assert!(needs_restore(false, true));
        // Minimised after being maximised: both flags are set.
        assert!(needs_restore(true, true));
        assert!(!needs_restore(false, false));
        // Nothing to touch without a window.
        restore_any(0);
        activate(0);
    }

    #[test]
    fn listing_windows_is_a_read_only_query() {
        // Enumerates the real desktop (a machine with none gives an empty list) and only
        // checks what the filter promises.
        let all = list_windows();
        let mut seen = std::collections::HashSet::new();
        for e in &all {
            assert!(e.w != 0, "{e:?}");
            assert!(!e.title.trim().is_empty(), "{e:?}");
            assert!(seen.insert(e.w), "listed twice: {e:?}");
        }
    }

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
            AssistInput::Key {
                vk: 0x41,
                ctrl: true,
                shift: false
            }
        );
        // AltGr is Ctrl+Alt: not a Ctrl shortcut.
        assert_eq!(
            k(true, true, false),
            AssistInput::Key {
                vk: 0x41,
                ctrl: false,
                shift: false
            }
        );
        assert_eq!(
            k(false, false, true),
            AssistInput::Key {
                vk: 0x41,
                ctrl: false,
                shift: true
            }
        );
        // The key code is the low 16 bits of wparam.
        assert_eq!(
            key_input(0x1_0027, false, false, false),
            AssistInput::Key {
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
            Some(AssistInput::Press { x: -3, y: 40 })
        );
        assert_eq!(
            translate(WM_LBUTTONUP, 0, l),
            Some(AssistInput::Release { x: -3, y: 40 })
        );
        assert_eq!(
            translate(WM_MOUSEMOVE, 1, l),
            Some(AssistInput::Move { x: -3, y: 40 })
        );
        assert_eq!(translate(WM_MOUSELEAVE, 0, 0), Some(AssistInput::Leave));
        assert_eq!(translate(WM_CANCELMODE, 0, 0), Some(AssistInput::Leave));
        assert_eq!(translate(WM_KILLFOCUS, 0, 0), Some(AssistInput::FocusLost));
        assert_eq!(translate(WM_KEYDOWN, 0x41, 0), None);
        assert_eq!(translate(WM_CAPTURECHANGED, 0, 5), None);
        assert_eq!(translate(WM_RBUTTONDOWN, 2, l), None);
    }

    #[test]
    fn only_deactivation_loses_focus() {
        assert_eq!(
            translate(WM_ACTIVATE, WA_INACTIVE as usize, 0),
            Some(AssistInput::FocusLost)
        );
        // Deactivated while minimized: the high word is set, the low word is still 0.
        assert_eq!(
            translate(WM_ACTIVATE, 0x0001_0000, 0),
            Some(AssistInput::FocusLost)
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
    fn the_picker_takes_the_keyboard() {
        let ex = picker_ex_style();
        for flag in [WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST] {
            assert_eq!(ex & flag, flag, "{flag:?}");
        }
        // Unlike the zone overlay and the preview stack it is activatable and takes input.
        for flag in [WS_EX_NOACTIVATE, WS_EX_TRANSPARENT] {
            assert_eq!(ex & flag, WINDOW_EX_STYLE(0), "{flag:?}");
        }
    }
}
