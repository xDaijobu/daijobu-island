//! A shell inside the island: `alacritty_terminal` (Apache-2.0) for the emulation and the
//! ConPTY, and this crate for the glue.
//!
//! - [`Session`] starts a shell in a ConPTY and keeps its screen up to date on a thread of its
//!   own. The UI asks it for a [`dj_core::terminal::Screen`] (colours already resolved through
//!   the theme's [`dj_core::terminal::Palette`]) and sends it bytes.
//! - [`resolve_shell`] turns the `[terminal] shell` setting into a program and arguments.
//! - Nothing polls: the UI is woken (once, until it calls [`Session::take_events`]) when the
//!   shell prints, sets a title, rings or exits.
//!
//! `unsafe` is confined to `child.rs` (our handle on the shell process) and `tap.rs` (one
//! delegating call).

#![deny(unsafe_code)]

#[allow(unsafe_code)]
mod child;
mod listener;
mod osc;
mod screen;
mod session;
mod shell;
#[allow(unsafe_code)]
mod tap;
mod wheel;

pub use listener::TermEvent;
pub use session::{Session, SpawnOptions};
pub use shell::{is_powershell, resolve_shell};

#[cfg(test)]
mod tests;
