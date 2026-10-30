//! Platform-free core of Daijobu Island: springs, themes, the feature registry,
//! layout, the island state machine, vitals, the Sweep view model, the Pomodoro timer,
//! the Snap selection model, the Snap annotation editor, the Snap preview stack, Tile
//! (layouts, zone math and the window-drag state machine), the Tile window picker (the
//! zone-by-zone fill run and its card grid), the Settings panel and the terminal panel's
//! shared types and key mapping.
//! No `unsafe`, no Win32.
#![forbid(unsafe_code)]

pub mod agent_notifications;
pub mod clipboard;
pub mod clipboard_types;
pub mod clipboard_ui;
pub mod color;
pub mod config;
pub mod dev_memory;
pub mod edge_reveal;
pub mod features;
pub mod health;
pub mod island;
pub mod layout;
pub mod line_edit;
pub mod music;
pub mod notifications;
pub mod osd;
pub mod settings;
pub mod settings_ui;
pub mod snap;
pub mod snap_drag;
pub mod snap_edit;
pub mod snap_pin;
pub mod snap_stack;
pub mod spotlight;
pub mod spring;
pub mod sweep;
pub mod term_agents;
pub mod term_tabs;
pub mod terminal;
pub mod terminal_folders;
pub mod terminal_shortcuts;
pub mod theme;
pub mod tile;
pub mod tile_assist;
pub mod timer;
pub mod tray_menu;
pub mod vitals;
