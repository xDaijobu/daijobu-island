//! Win32 plumbing for Daijobu Island: the overlay window, message loop, tray,
//! hotkeys, DPI, autostart, single instance, theme-directory watching, the
//! default-device volume, the volume-key hook, in-memory sound playback, the Snap
//! overlay window, the Snap preview stack's companion window, the top AppBar strip and
//! Tile's window-move hook, placement calls and zone overlay window, and the Tile window
//! picker's window list, icons, DWM thumbnails and input window. The island window can
//! also take the keyboard (the terminal panel), and the text clipboard lives in
//! [`clipboard`].

#[cfg(not(windows))]
compile_error!("dj-win only supports Windows");

pub mod appbar;
pub mod assist;
pub mod clipboard;
pub mod clipboard_transfer;
pub mod dev_memory;
pub mod drag_drop;
pub mod folder_picker;
pub mod hook;
pub mod launcher;
pub mod overlay;
pub mod popup;
pub mod previews;
pub mod snap_pins;
pub mod sound;
pub mod system;
pub mod tile;
pub mod tray;
pub mod volume;
pub mod window;

pub use window::{Event, Handler};

pub use windows::Win32::Foundation::{HWND, RECT};
pub use windows::Win32::UI::WindowsAndMessaging::HICON;
