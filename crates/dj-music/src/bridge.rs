//! The page bridge script, embedded at build time.

/// Injected before any page script runs. See `bridge.js` for what it posts.
pub const BRIDGE_JS: &str = include_str!("bridge.js");
