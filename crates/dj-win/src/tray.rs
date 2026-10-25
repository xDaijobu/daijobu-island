//! Tray icon and its context menu.

use crate::window::WM_TRAY;
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const TRAY_ID: u32 = 1;
const CMD_TOGGLE: usize = 1;
const CMD_AUTOSTART: usize = 2;
const CMD_QUIT: usize = 3;
const CMD_THEME_BASE: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    ToggleVisible,
    Theme(usize),
    ToggleAutostart,
    Quit,
}

pub struct MenuSpec<'a> {
    /// `(display name, checked)` per theme.
    pub themes: &'a [(String, bool)],
    pub autostart: bool,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Draws a small pill glyph (dark body, amber lamp) as a 32x32 icon.
pub fn make_icon() -> Result<HICON> {
    const N: i32 = 32;
    let mut xor = vec![0u8; (N * N * 4) as usize];
    let (body, lamp) = ([0x0F_u8, 0x11, 0x14], [0x58_u8, 0xA4, 0xE0]); // BGR
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Rounded pill 28 x 14 centred in the icon, plus a lamp dot on its left.
            let (cx, cy, hw, hh) = (16.0_f32, 16.0_f32, 14.0_f32, 7.0_f32);
            let (qx, qy) = ((fx - cx).abs() - (hw - hh), (fy - cy).abs());
            let d = if qx > 0.0 { (qx * qx + qy * qy).sqrt() } else { qy } - hh;
            let a = (0.5 - d).clamp(0.0, 1.0);
            let ld = ((fx - 9.5).powi(2) + (fy - cy).powi(2)).sqrt() - 2.6;
            let l = (0.5 - ld).clamp(0.0, 1.0);
            let i = ((y * N + x) * 4) as usize;
            for c in 0..3 {
                xor[i + c] = (body[c] as f32 * (1.0 - l) + lamp[c] as f32 * l) as u8;
            }
            xor[i + 3] = (a * 255.0) as u8;
        }
    }
    let and = vec![0u8; (N * N / 8) as usize];
    // SAFETY: both buffers outlive the call and have the sizes CreateIcon expects
    // (32x32 at 32 bpp, and a 1 bpp mask).
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        CreateIcon(Some(hinst.into()), N, N, 1, 32, and.as_ptr(), xor.as_ptr())
    }
}

fn nid(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        ..Default::default()
    }
}

pub fn add(hwnd: HWND, icon: HICON, tip: &str) -> bool {
    let mut data = nid(hwnd);
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = icon;
    for (d, s) in data.szTip.iter_mut().zip(tip.encode_utf16().take(127)) {
        *d = s;
    }
    // SAFETY: data is fully initialised and the window handle is ours.
    unsafe { Shell_NotifyIconW(NIM_ADD, &data).as_bool() }
}

pub fn remove(hwnd: HWND) {
    let data = nid(hwnd);
    // SAFETY: as above.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

/// Shows the context menu at the cursor and returns the chosen command.
pub fn show_menu(hwnd: HWND, spec: &MenuSpec) -> Option<MenuCommand> {
    // SAFETY: the menu handles are created and destroyed in this function; every
    // string buffer lives until after the corresponding call returns.
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let themes = CreatePopupMenu().ok()?;
        let append = |m: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
            let t = wide(text);
            let _ = AppendMenuW(m, flags, id, PCWSTR(t.as_ptr()));
        };
        for (i, (name, checked)) in spec.themes.iter().enumerate() {
            let flags = if *checked { MF_STRING | MF_CHECKED } else { MF_STRING };
            append(themes, flags, CMD_THEME_BASE + i, name);
        }
        append(menu, MF_STRING, CMD_TOGGLE, "Show / Hide");
        let _ = AppendMenuW(menu, MF_POPUP, themes.0 as usize, w!("Theme"));
        let flags = if spec.autostart { MF_STRING | MF_CHECKED } else { MF_STRING };
        append(menu, flags, CMD_AUTOSTART, "Start with Windows");
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        append(menu, MF_STRING, CMD_QUIT, "Quit");

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu dismisses when the user clicks elsewhere.
        let _ = windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_RIGHTALIGN,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, Default::default(), Default::default());
        let _ = DestroyMenu(menu); // also destroys the submenu
        match cmd.0 as usize {
            0 => None,
            CMD_TOGGLE => Some(MenuCommand::ToggleVisible),
            CMD_AUTOSTART => Some(MenuCommand::ToggleAutostart),
            CMD_QUIT => Some(MenuCommand::Quit),
            n if n >= CMD_THEME_BASE => Some(MenuCommand::Theme(n - CMD_THEME_BASE)),
            _ => None,
        }
    }
}
