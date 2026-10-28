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
const CMD_MUSIC_SHOW: usize = 4;
const CMD_MUSIC_SIGNOUT: usize = 5;
const CMD_MUSIC_STOP: usize = 6;
const CMD_SNAP_PREVIEWS: usize = 7;
const CMD_SETTINGS: usize = 8;
const CMD_SNAP_PINS: usize = 9;
const CMD_THEME_BASE: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    ToggleVisible,
    Theme(usize),
    ToggleAutostart,
    MusicShow,
    MusicSignOut,
    MusicStop,
    /// Give the Snap preview stack the keyboard (deliberate keyboard access).
    SnapPreviews,
    /// Reveal pinned screenshots and give one deliberate keyboard focus.
    SnapPins,
    /// Open the Settings window.
    Settings,
    Quit,
}

pub struct MenuSpec<'a> {
    /// `(display name, checked)` per theme.
    pub themes: &'a [(String, bool)],
    pub autostart: bool,
    pub music: MusicMenu,
    /// The Snap preview stack has cards to focus; "Focus Snap previews" is greyed out
    /// otherwise.
    pub previews: bool,
    pub pins: bool,
}

/// Which YouTube Music items can be used right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MusicMenu {
    /// The hidden engine is running.
    pub running: bool,
    pub signed_in: bool,
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
            let d = if qx > 0.0 {
                (qx * qx + qy * qy).sqrt()
            } else {
                qy
            } - hh;
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
            let flags = if *checked {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            append(themes, flags, CMD_THEME_BASE + i, name);
        }
        let music = CreatePopupMenu().ok()?;
        let grey = |on: bool| if on { MF_STRING } else { MF_STRING | MF_GRAYED };
        append(music, MF_STRING, CMD_MUSIC_SHOW, "Show window");
        append(
            music,
            grey(spec.music.signed_in || spec.music.running),
            CMD_MUSIC_SIGNOUT,
            "Sign out",
        );
        append(
            music,
            grey(spec.music.running),
            CMD_MUSIC_STOP,
            "Stop engine",
        );
        append(menu, MF_STRING, CMD_TOGGLE, "Show / Hide");
        append(menu, MF_STRING, CMD_SETTINGS, "Settings…");
        let _ = AppendMenuW(menu, MF_POPUP, themes.0 as usize, w!("Theme"));
        let _ = AppendMenuW(menu, MF_POPUP, music.0 as usize, w!("YouTube Music"));
        append(
            menu,
            grey(spec.previews),
            CMD_SNAP_PREVIEWS,
            "Focus Snap previews",
        );
        append(
            menu,
            grey(spec.pins),
            CMD_SNAP_PINS,
            "Show pinned screenshots",
        );
        let flags = if spec.autostart {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
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
        command_for(cmd.0 as usize)
    }
}

/// The command a chosen menu id stands for (`TrackPopupMenu` returns 0 for no choice).
fn command_for(id: usize) -> Option<MenuCommand> {
    match id {
        CMD_TOGGLE => Some(MenuCommand::ToggleVisible),
        CMD_AUTOSTART => Some(MenuCommand::ToggleAutostart),
        CMD_MUSIC_SHOW => Some(MenuCommand::MusicShow),
        CMD_MUSIC_SIGNOUT => Some(MenuCommand::MusicSignOut),
        CMD_MUSIC_STOP => Some(MenuCommand::MusicStop),
        CMD_SNAP_PREVIEWS => Some(MenuCommand::SnapPreviews),
        CMD_SNAP_PINS => Some(MenuCommand::SnapPins),
        CMD_SETTINGS => Some(MenuCommand::Settings),
        CMD_QUIT => Some(MenuCommand::Quit),
        n if n >= CMD_THEME_BASE => Some(MenuCommand::Theme(n - CMD_THEME_BASE)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_their_commands() {
        assert_eq!(command_for(0), None);
        assert_eq!(command_for(CMD_TOGGLE), Some(MenuCommand::ToggleVisible));
        assert_eq!(
            command_for(CMD_AUTOSTART),
            Some(MenuCommand::ToggleAutostart)
        );
        assert_eq!(command_for(CMD_MUSIC_SHOW), Some(MenuCommand::MusicShow));
        assert_eq!(
            command_for(CMD_MUSIC_SIGNOUT),
            Some(MenuCommand::MusicSignOut)
        );
        assert_eq!(command_for(CMD_MUSIC_STOP), Some(MenuCommand::MusicStop));
        assert_eq!(
            command_for(CMD_SNAP_PREVIEWS),
            Some(MenuCommand::SnapPreviews)
        );
        assert_eq!(command_for(CMD_QUIT), Some(MenuCommand::Quit));
        assert_eq!(command_for(CMD_SETTINGS), Some(MenuCommand::Settings));
        assert_eq!(command_for(CMD_SNAP_PINS), Some(MenuCommand::SnapPins));
        assert_eq!(command_for(CMD_THEME_BASE), Some(MenuCommand::Theme(0)));
        assert_eq!(command_for(CMD_THEME_BASE + 3), Some(MenuCommand::Theme(3)));
        // An id in the gap between the fixed commands and the themes is nothing.
        assert_eq!(command_for(CMD_SNAP_PINS + 1), None);
        assert_eq!(command_for(CMD_THEME_BASE - 1), None);
    }

    #[test]
    fn fixed_menu_ids_are_distinct_and_below_the_themes() {
        let ids = [
            CMD_TOGGLE,
            CMD_AUTOSTART,
            CMD_QUIT,
            CMD_SETTINGS,
            CMD_MUSIC_SHOW,
            CMD_MUSIC_SIGNOUT,
            CMD_MUSIC_STOP,
            CMD_SNAP_PREVIEWS,
        ];
        for (i, a) in ids.iter().enumerate() {
            assert!(*a > 0 && *a < CMD_THEME_BASE);
            assert!(ids[i + 1..].iter().all(|b| a != b), "duplicate id {a}");
        }
    }
}
