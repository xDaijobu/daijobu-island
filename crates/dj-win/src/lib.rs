//! Win32 plumbing for Daijobu Island: the overlay window, message loop, tray,
//! hotkeys, DPI, autostart, single instance, theme-directory watching, the
//! default-device volume, the volume-key hook, in-memory sound playback and the Snap
//! overlay window.

#[cfg(not(windows))]
compile_error!("dj-win only supports Windows");

pub mod hook;
pub mod overlay;
pub mod sound;
pub mod system;
pub mod tray;
pub mod volume;
pub mod window;

pub use window::{Event, Handler};

pub use windows::Win32::Foundation::HWND;
pub use windows::Win32::UI::WindowsAndMessaging::HICON;
