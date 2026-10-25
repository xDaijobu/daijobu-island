//! `~/.daijobu/config.toml`. Parsing only; the app does the file IO.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Theme id (file stem for user themes).
    pub theme: String,
    pub start_with_windows: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { theme: crate::theme::DEFAULT_THEME.to_string(), start_with_windows: false }
    }
}

impl Config {
    /// Lenient: anything unreadable yields the defaults.
    pub fn parse(s: &str) -> Self {
        toml::from_str(s).unwrap_or_default()
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let c = Config { theme: "paper-zine".into(), start_with_windows: true };
        assert_eq!(Config::parse(&c.to_toml()), c);
        assert_eq!(Config::parse("not [ toml"), Config::default());
        assert!(!Config::parse("theme = \"x\"").start_with_windows);
    }
}
