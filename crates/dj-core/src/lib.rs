//! Platform-free core of Daijobu Island: springs, themes, the feature registry,
//! layout and the island state machine. No `unsafe`, no Win32.
#![forbid(unsafe_code)]

pub mod color;
pub mod config;
pub mod features;
pub mod health;
pub mod island;
pub mod layout;
pub mod music;
pub mod osd;
pub mod spring;
pub mod theme;
pub mod vitals;
