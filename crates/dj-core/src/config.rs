//! `~/.daijobu/config.toml`. Parsing only; the app does the file IO.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Theme id (file stem for user themes).
    pub theme: String,
    pub start_with_windows: bool,
    pub music: MusicConfig,
    pub osd: OsdConfig,
    pub vitals: VitalsConfig,
}

/// `[music]` section. The YouTube Music profile itself lives in `~/.daijobu/ytmusic/`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicConfig {
    pub signed_in: bool,
}

/// `[osd]` section.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OsdConfig {
    /// Swallow the volume keys and show the island's pill instead of the Windows flyout.
    pub replace_volume_flyout: bool,
}

/// `[vitals]` section.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VitalsConfig {
    /// Tint the idle pill's lamp by the health band (warn colour when the band is low).
    pub health_lamp: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: crate::theme::DEFAULT_THEME.to_string(),
            start_with_windows: false,
            music: MusicConfig::default(),
            osd: OsdConfig::default(),
            vitals: VitalsConfig::default(),
        }
    }
}

impl Default for OsdConfig {
    fn default() -> Self {
        Self {
            replace_volume_flyout: true,
        }
    }
}

impl Default for VitalsConfig {
    fn default() -> Self {
        Self { health_lamp: true }
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
        let c = Config {
            theme: "paper-zine".into(),
            start_with_windows: true,
            music: MusicConfig { signed_in: true },
            osd: OsdConfig {
                replace_volume_flyout: false,
            },
            vitals: VitalsConfig { health_lamp: false },
        };
        assert_eq!(Config::parse(&c.to_toml()), c);
        assert_eq!(Config::parse("not [ toml"), Config::default());
        assert!(!Config::parse("theme = \"x\"").start_with_windows);
        assert!(!Config::parse("theme = \"x\"").music.signed_in);
        let c = Config::parse("[music]\nsigned_in = true\n");
        assert!(c.music.signed_in);
        assert_eq!(Config::parse(&c.to_toml()), c);
    }

    #[test]
    fn osd_flyout_replacement_defaults_on_and_reads_the_osd_table() {
        assert!(Config::default().osd.replace_volume_flyout);
        assert!(Config::parse("theme = \"x\"").osd.replace_volume_flyout);
        let off = Config::parse("[osd]\nreplace_volume_flyout = false\n");
        assert!(!off.osd.replace_volume_flyout);
    }

    #[test]
    fn vitals_health_lamp_defaults_on_and_reads_the_vitals_table() {
        assert!(Config::default().vitals.health_lamp);
        assert!(Config::parse("theme = \"x\"").vitals.health_lamp);
        let off = Config::parse("[vitals]\nhealth_lamp = false\n");
        assert!(!off.vitals.health_lamp);
    }
}
