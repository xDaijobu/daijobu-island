//! Low-level keyboard hook that takes over the volume keys and, for Snap, PrtSc.
//!
//! Each takeover has its own switch (`set_volume_keys`, `set_print_screen`); a key
//! whose switch is off passes through untouched. A taken key is swallowed (so the
//! system flyout or Windows' screen capture never sees it) and forwarded to the island
//! window: volume keys as `WM_VOLUME_KEY`, PrtSc as `WM_PRINT_SCREEN`. The hook
//! procedure runs on the installing thread through its message loop and must stay fast.

use crate::window::{WM_PRINT_SCREEN, WM_VOLUME_KEY};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    VK_SNAPSHOT, VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, PostMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// Window that receives the forwarded keys, stored as `HWND` bits.
static TARGET: AtomicIsize = AtomicIsize::new(0);
/// Installed hook, stored as `HHOOK` bits. Zero when not installed.
static HOOK: AtomicIsize = AtomicIsize::new(0);
/// Take over the volume keys (`[osd] replace_volume_flyout`).
static VOLUME_KEYS: AtomicBool = AtomicBool::new(false);
/// Take over plain PrtSc (`[snap] hotkey = "PrtSc"`).
static PRINT_SCREEN: AtomicBool = AtomicBool::new(false);
/// A PrtSc press was swallowed, so its release is swallowed too.
static PRINT_DOWN: AtomicBool = AtomicBool::new(false);
/// Escape was used to cancel a preparing drag; consume the matching release too.
static DRAG_ESCAPE_DOWN: AtomicBool = AtomicBool::new(false);

/// True for the virtual-key codes of the volume mute, down and up keys.
pub fn is_volume_key(vk: u32) -> bool {
    [VK_VOLUME_MUTE, VK_VOLUME_DOWN, VK_VOLUME_UP]
        .iter()
        .any(|k| u32::from(k.0) == vk)
}

/// What the hook does with a key press or release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Take {
    /// Not ours: pass it on.
    Pass,
    /// Swallow it and post this message to the island (on the press only).
    Post(u32),
    /// Swallow it without posting (the release of a taken key).
    Swallow,
}

/// The decision, pure. `mods` is true when Ctrl, Alt, Shift or Win is held: Alt+PrtSc
/// and Win+PrtSc stay Windows' own.
fn classify(vk: u32, down: bool, mods: bool, volume: bool, print: bool, print_down: bool) -> Take {
    if volume && is_volume_key(vk) {
        return if down {
            Take::Post(WM_VOLUME_KEY)
        } else {
            Take::Swallow
        };
    }
    if print && vk == u32::from(VK_SNAPSHOT.0) {
        if down && !mods {
            return Take::Post(WM_PRINT_SCREEN);
        }
        if !down && print_down {
            return Take::Swallow;
        }
    }
    Take::Pass
}

/// Turns the volume-key takeover on or off. Takes effect only while the hook is installed.
pub fn set_volume_keys(on: bool) {
    VOLUME_KEYS.store(on, Ordering::Release);
}

/// Turns the PrtSc takeover on or off. Takes effect only while the hook is installed.
pub fn set_print_screen(on: bool) {
    PRINT_SCREEN.store(on, Ordering::Release);
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
    DRAG_ESCAPE_DOWN.store(false, Ordering::Release);
    let raw = HOOK.swap(0, Ordering::AcqRel);
    if raw != 0 {
        // SAFETY: the handle came from SetWindowsHookExW and is removed only once.
        unsafe {
            let _ = UnhookWindowsHookEx(HHOOK(raw as *mut _));
        }
    }
}

/// True while Ctrl, Alt, Shift or either Win key is held.
fn modifiers_held() -> bool {
    [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
        .iter()
        // SAFETY: GetAsyncKeyState only reads the global key state.
        .any(|k: &VIRTUAL_KEY| unsafe { GetAsyncKeyState(i32::from(k.0)) } as u16 & 0x8000 != 0)
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
            if vk == u32::from(VK_ESCAPE.0) {
                if is_down
                    && (DRAG_ESCAPE_DOWN.load(Ordering::Acquire)
                        || crate::window::drag_escape_pressed())
                {
                    DRAG_ESCAPE_DOWN.store(true, Ordering::Release);
                    return LRESULT(1);
                }
                if is_up && DRAG_ESCAPE_DOWN.swap(false, Ordering::AcqRel) {
                    return LRESULT(1);
                }
            }
            let print = PRINT_SCREEN.load(Ordering::Acquire);
            let mods = print && vk == u32::from(VK_SNAPSHOT.0) && is_down && modifiers_held();
            let take = classify(
                vk,
                is_down,
                mods,
                VOLUME_KEYS.load(Ordering::Acquire),
                print,
                PRINT_DOWN.load(Ordering::Acquire),
            );
            if vk == u32::from(VK_SNAPSHOT.0) {
                if is_down {
                    PRINT_DOWN.store(matches!(take, Take::Post(_)), Ordering::Release);
                } else {
                    PRINT_DOWN.store(false, Ordering::Release);
                }
            }
            match take {
                Take::Pass => {}
                Take::Post(message) => {
                    let target = HWND(TARGET.load(Ordering::Acquire) as *mut _);
                    // SAFETY: PostMessageW is thread-safe; the target is our own window.
                    unsafe {
                        let _ = PostMessageW(Some(target), message, WPARAM(vk as usize), LPARAM(0));
                    }
                    return LRESULT(1);
                }
                Take::Swallow => return LRESULT(1),
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

    const PRTSC: u32 = 0x2C;

    #[test]
    fn classifies_only_the_three_volume_keys() {
        assert!(is_volume_key(0xAD));
        assert!(is_volume_key(0xAE));
        assert!(is_volume_key(0xAF));
        assert!(!is_volume_key(0xAC));
        assert!(!is_volume_key(0xB0));
        assert!(!is_volume_key(0x41));
    }

    #[test]
    fn volume_keys_are_taken_only_when_switched_on() {
        assert_eq!(
            classify(0xAF, true, false, true, false, false),
            Take::Post(WM_VOLUME_KEY)
        );
        assert_eq!(
            classify(0xAF, false, false, true, false, false),
            Take::Swallow
        );
        // The hook may be installed for PrtSc alone: the volume keys then pass.
        assert_eq!(classify(0xAF, true, false, false, true, false), Take::Pass);
    }

    #[test]
    fn plain_prtsc_is_taken_and_its_release_swallowed() {
        assert_eq!(
            classify(PRTSC, true, false, false, true, false),
            Take::Post(WM_PRINT_SCREEN)
        );
        assert_eq!(
            classify(PRTSC, false, false, false, true, true),
            Take::Swallow
        );
        // Switched off, PrtSc stays Windows'.
        assert_eq!(classify(PRTSC, true, false, true, false, false), Take::Pass);
        // Alt+PrtSc and Win+PrtSc stay Windows', and so does their release.
        assert_eq!(classify(PRTSC, true, true, false, true, false), Take::Pass);
        assert_eq!(
            classify(PRTSC, false, false, false, true, false),
            Take::Pass
        );
        // Other keys always pass.
        assert_eq!(classify(0x41, true, false, true, true, false), Take::Pass);
    }
}
