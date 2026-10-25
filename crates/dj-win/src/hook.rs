//! Low-level keyboard hook that takes over the three volume keys.
//!
//! Volume key presses are swallowed (so the system flyout never sees them) and
//! forwarded to the overlay window as `WM_VOLUME_KEY`. The hook procedure runs
//! on the installing thread through its message loop and must stay fast.

use crate::window::WM_VOLUME_KEY;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, PostMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// Window that receives `WM_VOLUME_KEY`, stored as `HWND` bits.
static TARGET: AtomicIsize = AtomicIsize::new(0);
/// Installed hook, stored as `HHOOK` bits. Zero when not installed.
static HOOK: AtomicIsize = AtomicIsize::new(0);

/// True for the virtual-key codes of the volume mute, down and up keys.
pub fn is_volume_key(vk: u32) -> bool {
    [VK_VOLUME_MUTE, VK_VOLUME_DOWN, VK_VOLUME_UP]
        .iter()
        .any(|k| u32::from(k.0) == vk)
}

/// Installs a WH_KEYBOARD_LL hook on the calling thread. The thread must pump messages
/// (the app's GetMessageW loop does). Stores `hwnd` for posting. Returns false on failure.
pub fn install(hwnd: HWND) -> bool {
    if HOOK.load(Ordering::Acquire) != 0 {
        return true;
    }
    TARGET.store(hwnd.0 as isize, Ordering::Release);
    // SAFETY: the module handle is the current executable; the hook proc is a plain
    // function with the required signature and is only called by the system.
    let installed = unsafe {
        let Ok(module) = GetModuleHandleW(None) else {
            return false;
        };
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_proc),
            Some(HINSTANCE::from(module)),
            0,
        )
    };
    match installed {
        Ok(hhook) => {
            HOOK.store(hhook.0 as isize, Ordering::Release);
            true
        }
        Err(_) => false,
    }
}

/// Removes the hook if installed. Safe to call twice.
pub fn uninstall() {
    let raw = HOOK.swap(0, Ordering::AcqRel);
    if raw != 0 {
        // SAFETY: the handle came from SetWindowsHookExW and is removed only once.
        unsafe {
            let _ = UnhookWindowsHookEx(HHOOK(raw as *mut _));
        }
    }
}

// SAFETY: only ever invoked by the system as a HOOKPROC for WH_KEYBOARD_LL.
unsafe extern "system" fn keyboard_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let msg = wp.0 as u32;
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
        if is_down || is_up {
            // SAFETY: for WH_KEYBOARD_LL with code >= 0, lParam points to a
            // KBDLLHOOKSTRUCT that stays valid for the duration of this call.
            let vk = unsafe { (*(lp.0 as *const KBDLLHOOKSTRUCT)).vkCode };
            if is_volume_key(vk) {
                if is_down {
                    let target = HWND(TARGET.load(Ordering::Acquire) as *mut _);
                    // SAFETY: PostMessageW is thread-safe; the target is our own window.
                    unsafe {
                        let _ = PostMessageW(
                            Some(target),
                            WM_VOLUME_KEY,
                            WPARAM(vk as usize),
                            LPARAM(0),
                        );
                    }
                }
                // Swallow both down and up so the volume flyout never sees the key.
                return LRESULT(1);
            }
        }
    }
    let raw = HOOK.load(Ordering::Acquire);
    let next = (raw != 0).then_some(HHOOK(raw as *mut _));
    // SAFETY: forwards the hook arguments unchanged to the next hook in the chain.
    unsafe { CallNextHookEx(next, code, wp, lp) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_only_the_three_volume_keys() {
        assert!(is_volume_key(0xAD));
        assert!(is_volume_key(0xAE));
        assert!(is_volume_key(0xAF));
        assert!(!is_volume_key(0xAC));
        assert!(!is_volume_key(0xB0));
        assert!(!is_volume_key(0x41));
    }
}
