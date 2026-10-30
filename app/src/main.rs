//! Daijobu Island: a lofi-modern dynamic island for Windows 11.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod chime;
mod clipboard;
mod clipboard_transfer;
mod dev_memory;
mod notch;
mod notifications;
mod popups;
mod preview_store;
mod previews;
mod snap;
mod snap_drag_store;
mod snap_pins;
mod storage;
mod sweep;
mod terminal;
mod tile;
mod timer;
mod vitals;

use dj_win::{system, window};

fn main() {
    if !system::acquire_single_instance() {
        return;
    }
    storage::ensure_dirs();
    let config = storage::load_config();
    let exe = std::env::current_exe().unwrap_or_default();

    let hwnd = match window::create_window(0, 0, 1, 1) {
        Ok(h) => h,
        Err(e) => {
            storage::log(&format!("cannot create window: {e}"));
            return;
        }
    };
    let app = app::App::new(hwnd, config, exe);
    window::run(app);
}
