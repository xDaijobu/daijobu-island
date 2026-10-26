//! Win32 plumbing for Daijobu Island: the overlay window, message loop, tray,
//! hotkeys, DPI, autostart, single instance, theme-directory watching, the
//! default-device volume, the volume-key hook, in-memory sound playback, the Snap
//! overlay window, the Snap preview stack's companion window and the top AppBar strip.

#[cfg(not(windows))]
compile_error!("dj-win only supports Windows");

pub mod appbar;
pub mod hook;
pub mod overlay;
pub mod previews;
pub mod sound;
pub mod system;
pub mod tray;
pub mod volume;
pub mod window;

pub use window::{Event, Handler};

pub use windows::Win32::Foundation::HWND;
pub use windows::Win32::UI::WindowsAndMessaging::HICON;
