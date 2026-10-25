//! YouTube Music engine: a hidden WebView2 that plays music.youtube.com for the island.
//! It is the one place where a web view is allowed (see AGENTS.md).

pub mod art;
mod bridge;
mod engine;

pub use engine::{Engine, MUSIC_URL};
