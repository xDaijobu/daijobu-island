//! Platform-free core of Daijobu Island: springs, themes, the feature registry,
//! layout, the island state machine, vitals, the Sweep view model, the Pomodoro timer,
//! the Snap selection model, the Snap annotation editor, the Snap preview stack, Tile
//! (layouts, zone math and the window-drag state machine), the Tile window picker (the
//! zone-by-zone fill run and its card grid), the dj CLI activity (the progress and notice
//! pills' model) and the Settings panel.
//! No `unsafe`, no Win32.
#![forbid(unsafe_code)]

pub mod color;
pub mod config;
pub mod dj;
pub mod features;
pub mod health;
pub mod island;
pub mod layout;
pub mod music;
pub mod osd;
pub mod settings;
pub mod snap;
pub mod snap_edit;
pub mod snap_stack;
pub mod spring;
pub mod sweep;
pub mod theme;
pub mod tile;
pub mod tile_assist;
pub mod timer;
pub mod vitals;
