//! Tray icon and its context menu commands.

use crate::window::WM_TRAY;
use windows::core::Result;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const TRAY_ID: u32 = 1;

pub use dj_core::tray_menu::{MenuCommand, MenuSpec, MusicMenu};

pub fn make_icon() -> Result<HICON> {
    const N: i32 = 32;
    let mut xor = vec![0u8; (N * N * 4) as usize];
    let (body, lamp) = ([0x0F_u8, 0x11, 0x14], [0x58_u8, 0xA4, 0xE0]);
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
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
    // SAFETY: data contains our window handle and tray ID.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}
