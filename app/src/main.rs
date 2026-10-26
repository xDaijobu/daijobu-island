//! Daijobu Island: a lofi-modern dynamic island for Windows 11.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod chime;
mod snap;
mod storage;
mod sweep;
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
    let mut app = app::App::new(hwnd, config, exe);
    window::run(&mut app);
}
