//! Screen capture for Snap: freezing every monitor (DXGI Desktop Duplication, GDI
//! fallback), the window rects a click can snap to, PNG through WIC, and the clipboard
//! (`CF_DIB` + `"PNG"`). Win32 only; the geometry and cropping are in `dj_core::snap`.
//!
//! Nothing here runs outside a snap: there are no threads, hooks or cached devices.
//! The unit tests use synthetic bitmaps only; they never read the screen or write the
//! clipboard.

#[cfg(not(windows))]
compile_error!("dj-capture only supports Windows");

pub mod capture;
pub mod clipboard;
pub mod dib;
pub mod png;
pub mod targets;

pub use capture::{freeze, FreezeReport};
pub use clipboard::put_image;
pub use targets::window_rects;
