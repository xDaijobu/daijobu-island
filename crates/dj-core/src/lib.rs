//! Platform-free core of Daijobu Island: springs, themes, the feature registry,
//! layout, the island state machine, vitals, the Sweep view model, the Pomodoro timer
//! and the Snap selection model.
//! No `unsafe`, no Win32.
#![forbid(unsafe_code)]

pub mod color;
pub mod config;
pub mod features;
pub mod health;
pub mod island;
pub mod layout;
pub mod music;
pub mod osd;
pub mod snap;
pub mod spring;
pub mod sweep;
pub mod theme;
pub mod timer;
pub mod vitals;
