//! `~/.daijobu/config.toml`. Parsing only; the app does the file IO.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Theme id (file stem for user themes).
    pub theme: String,
    pub start_with_windows: bool,
    pub music: MusicConfig,
    pub osd: OsdConfig,
    pub vitals: VitalsConfig,
    pub sweep: SweepConfig,
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

/// `[sweep]` section.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SweepConfig {
    /// Folders that Dev purge looks in, at most four levels deep. A leading `~` is the
    /// user profile.
    pub project_roots: Vec<String>,
}

impl Default for SweepConfig {
    fn default() -> Self {
        Self {
            project_roots: [
                "~/Documents",
                "~/Projects",
                "~/Code",
                "~/source/repos",
                "~/dev",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        }
    }
}

impl SweepConfig {
    /// The roots as paths: `~` and a leading `~/` or `~\` become `profile`. Blank entries
    /// are dropped. Pure; nothing is checked on disk.
    pub fn roots(&self, profile: &Path) -> Vec<PathBuf> {
        self.project_roots
            .iter()
            .map(|r| r.trim())
            .filter(|r| !r.is_empty())
            .map(|r| {
                if r == "~" {
                    profile.to_path_buf()
                } else if let Some(rest) = r.strip_prefix("~/").or_else(|| r.strip_prefix("~\\")) {
                    profile.join(rest)
                } else {
                    PathBuf::from(r)
                }
            })
            .collect()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: crate::theme::DEFAULT_THEME.to_string(),
            start_with_windows: false,
            music: MusicConfig::default(),
            osd: OsdConfig::default(),
            vitals: VitalsConfig::default(),
            sweep: SweepConfig::default(),
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
            sweep: SweepConfig {
                project_roots: vec!["D:/work".into()],
            },
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
    fn sweep_roots_default_and_read_the_sweep_table() {
        let d = Config::parse("theme = \"x\"").sweep.project_roots;
        assert_eq!(d.len(), 5);
        assert_eq!(d[0], "~/Documents");
        let c = Config::parse("[sweep]\nproject_roots = [\"~/src\"]\n");
        assert_eq!(c.sweep.project_roots, ["~/src"]);
    }

    #[test]
    fn sweep_roots_expand_the_profile() {
        let c = SweepConfig {
            project_roots: vec![
                "~/Projects".into(),
                r"~\Code".into(),
                "~".into(),
                "  ".into(),
                "D:/work".into(),
            ],
        };
        let p = Path::new("C:/Users/me");
        assert_eq!(
            c.roots(p),
            vec![
                p.join("Projects"),
                p.join("Code"),
                p.to_path_buf(),
                PathBuf::from("D:/work")
            ]
        );
    }

    #[test]
    fn vitals_health_lamp_defaults_on_and_reads_the_vitals_table() {
        assert!(Config::default().vitals.health_lamp);
        assert!(Config::parse("theme = \"x\"").vitals.health_lamp);
        let off = Config::parse("[vitals]\nhealth_lamp = false\n");
        assert!(!off.vitals.health_lamp);
    }
}
