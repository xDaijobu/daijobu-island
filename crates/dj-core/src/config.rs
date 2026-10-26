//! `~/.daijobu/config.toml`. Parsing only; the app does the file IO.

use crate::timer::{Settings, MAX_EVERY, MAX_MIN};
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
    pub timer: TimerConfig,
    pub snap: SnapConfig,
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
    /// The roots as paths: `~` and a leading `~/` or `~\` become `profile`. Blank and
    /// relative entries are dropped (a relative root would depend on the working
    /// directory). Pure; nothing is checked on disk.
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
            .filter(|p| p.is_absolute())
            .collect()
    }
}

/// `[timer]` section. Minutes are signed so a stray negative value only resets that
/// field, not the whole file. `settings()` applies the ranges.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimerConfig {
    pub focus_min: i64,
    pub break_min: i64,
    pub long_break_min: i64,
    /// Every `every`-th focus is followed by the long break.
    pub every: i64,
    /// Play the chime when a session ends.
    pub chime: bool,
}

impl Default for TimerConfig {
    fn default() -> Self {
        Self {
            focus_min: 25,
            break_min: 5,
            long_break_min: 15,
            every: 4,
            chime: true,
        }
    }
}

impl TimerConfig {
    /// The Pomodoro settings. A minute value outside 1..=180 or an `every` outside
    /// 1..=12 falls back to its default.
    pub fn settings(&self) -> Settings {
        let d = Self::default();
        let pick = |v: i64, max: i64, fallback: i64| {
            if (1..=max).contains(&v) {
                v as u32
            } else {
                fallback as u32
            }
        };
        Settings::minutes(
            pick(self.focus_min, MAX_MIN, d.focus_min),
            pick(self.break_min, MAX_MIN, d.break_min),
            pick(self.long_break_min, MAX_MIN, d.long_break_min),
            pick(self.every, MAX_EVERY, d.every),
        )
    }
}

/// A Snap hotkey the config can name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapHotkey {
    /// PrtSc, no modifiers.
    PrtSc,
    CtrlShiftS,
    CtrlAltS,
    CtrlSpace,
}

impl SnapHotkey {
    /// Reads `"PrtSc"`, `"Ctrl+Shift+S"`, `"Ctrl+Alt+S"` or `"Ctrl+Space"`. Case, spaces
    /// and the order of the modifiers don't matter. Anything else is None.
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts: Vec<String> = s
            .split('+')
            .map(|p| p.trim().to_ascii_lowercase())
            .collect();
        let key = parts.pop()?;
        parts.sort();
        match (parts.as_slice(), key.as_str()) {
            ([], "prtsc" | "printscreen" | "print") => Some(Self::PrtSc),
            ([a, b], "s") if a == "ctrl" && b == "shift" => Some(Self::CtrlShiftS),
            ([a, b], "s") if a == "alt" && b == "ctrl" => Some(Self::CtrlAltS),
            ([a], "space") if a == "ctrl" => Some(Self::CtrlSpace),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PrtSc => "PrtSc",
            Self::CtrlShiftS => "Ctrl+Shift+S",
            Self::CtrlAltS => "Ctrl+Alt+S",
            Self::CtrlSpace => "Ctrl+Space",
        }
    }
}

/// `[snap]` section.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SnapConfig {
    /// `"PrtSc"`, `"Ctrl+Shift+S"`, `"Ctrl+Alt+S"` or `"Ctrl+Space"`. Anything else means
    /// PrtSc. PrtSc is taken through the keyboard hook, so Windows' own "Print screen
    /// opens screen capture" never sees it.
    pub hotkey: String,
    /// A second shortcut that is always registered next to `hotkey`, from the same names.
    /// `"none"` (or blank, or an unknown name) means no second shortcut.
    pub alt_hotkey: String,
    /// Where **save** writes. A leading `~` is the user profile; a relative or blank
    /// path falls back to the default.
    pub save_dir: String,
}

/// The default `[snap] save_dir`.
pub const SNAP_SAVE_DIR: &str = "~/Pictures/Daijobu";

impl Default for SnapConfig {
    fn default() -> Self {
        Self {
            hotkey: SnapHotkey::PrtSc.label().to_string(),
            alt_hotkey: SnapHotkey::CtrlSpace.label().to_string(),
            save_dir: SNAP_SAVE_DIR.to_string(),
        }
    }
}

impl SnapConfig {
    pub fn hotkey(&self) -> SnapHotkey {
        SnapHotkey::parse(&self.hotkey).unwrap_or(SnapHotkey::PrtSc)
    }

    /// The second shortcut, unless it is off, unknown or the same as `hotkey`.
    pub fn alt_hotkey(&self) -> Option<SnapHotkey> {
        SnapHotkey::parse(&self.alt_hotkey).filter(|k| *k != self.hotkey())
    }

    /// The save folder: `~` becomes `profile`, and a relative or blank path (or `~`
    /// without a profile) gives `profile\Pictures\Daijobu`. Pure; nothing is created.
    pub fn save_dir(&self, profile: &Path) -> PathBuf {
        let default = profile.join("Pictures").join("Daijobu");
        let expand = |s: &str| -> PathBuf {
            if s == "~" {
                profile.to_path_buf()
            } else if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
                profile.join(rest)
            } else {
                PathBuf::from(s)
            }
        };
        let p = expand(self.save_dir.trim());
        if p.is_absolute() {
            p
        } else {
            default
        }
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
            timer: TimerConfig::default(),
            snap: SnapConfig::default(),
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
            timer: TimerConfig {
                focus_min: 50,
                chime: false,
                ..TimerConfig::default()
            },
            snap: SnapConfig {
                hotkey: "Ctrl+Shift+S".into(),
                alt_hotkey: "none".into(),
                save_dir: "D:/shots".into(),
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
                "src".into(),
                r"..\up".into(),
                "C:no-slash".into(),
                r"\no-drive".into(),
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
        // Without a profile, `~` roots would be relative, so they are dropped too.
        assert_eq!(c.roots(Path::new("")), vec![PathBuf::from("D:/work")]);
    }

    #[test]
    fn timer_defaults_and_reads_the_timer_table() {
        let d = Config::parse("theme = \"x\"").timer;
        assert_eq!(d, TimerConfig::default());
        assert!(d.chime);
        assert_eq!(d.settings(), Settings::minutes(25, 5, 15, 4));
        let c = Config::parse(
            "[timer]\nfocus_min = 50\nbreak_min = 10\nlong_break_min = 30\nevery = 3\nchime = false\n",
        );
        assert!(!c.timer.chime);
        assert_eq!(c.timer.settings(), Settings::minutes(50, 10, 30, 3));
        assert_eq!(c.theme, crate::theme::DEFAULT_THEME);
    }

    #[test]
    fn timer_out_of_range_values_fall_back_per_field() {
        let c = Config::parse(
            "theme = \"paper-zine\"\n[timer]\nfocus_min = 0\nbreak_min = 181\nlong_break_min = -5\nevery = 0\n",
        );
        // The rest of the file still reads.
        assert_eq!(c.theme, "paper-zine");
        assert_eq!(c.timer.settings(), Settings::minutes(25, 5, 15, 4));
        let edge = Config::parse("[timer]\nfocus_min = 180\nbreak_min = 1\nevery = 13\n");
        assert_eq!(edge.timer.settings(), Settings::minutes(180, 1, 15, 4));
        let top = Config::parse("[timer]\nevery = 12\n");
        assert_eq!(top.timer.settings().every, 12);
    }

    #[test]
    fn snap_defaults_and_reads_the_snap_table() {
        let d = Config::parse("theme = \"x\"").snap;
        assert_eq!(d, SnapConfig::default());
        assert_eq!(d.hotkey(), SnapHotkey::PrtSc);
        assert_eq!(d.save_dir, "~/Pictures/Daijobu");
        let c = Config::parse(
            "theme = \"paper-zine\"\n[snap]\nhotkey = \"Ctrl+Alt+S\"\nsave_dir = \"D:/shots\"\n",
        );
        assert_eq!(c.snap.hotkey(), SnapHotkey::CtrlAltS);
        assert_eq!(
            c.snap.save_dir(Path::new("C:/Users/me")),
            Path::new("D:/shots")
        );
        assert_eq!(c.theme, "paper-zine");
        assert_eq!(Config::parse(&c.to_toml()), c);
    }

    #[test]
    fn snap_hotkey_names() {
        for (s, want) in [
            ("PrtSc", Some(SnapHotkey::PrtSc)),
            ("prtsc", Some(SnapHotkey::PrtSc)),
            ("Ctrl+Shift+S", Some(SnapHotkey::CtrlShiftS)),
            ("shift + ctrl + s", Some(SnapHotkey::CtrlShiftS)),
            ("Ctrl+Alt+S", Some(SnapHotkey::CtrlAltS)),
            ("Alt+Ctrl+s", Some(SnapHotkey::CtrlAltS)),
            ("Ctrl+Space", Some(SnapHotkey::CtrlSpace)),
            ("ctrl + space", Some(SnapHotkey::CtrlSpace)),
            ("Space", None),
            ("Ctrl+S", None),
            ("Ctrl+Shift+X", None),
            ("Win+Shift+S", None),
            ("", None),
        ] {
            assert_eq!(SnapHotkey::parse(s), want, "{s:?}");
        }
        // An unknown name means PrtSc, and the rest of the file still reads.
        let c = Config::parse("[snap]\nhotkey = \"F13\"\n[timer]\nfocus_min = 50\n");
        assert_eq!(c.snap.hotkey(), SnapHotkey::PrtSc);
        assert_eq!(c.timer.focus_min, 50);
        for k in [
            SnapHotkey::PrtSc,
            SnapHotkey::CtrlShiftS,
            SnapHotkey::CtrlAltS,
            SnapHotkey::CtrlSpace,
        ] {
            assert_eq!(SnapHotkey::parse(k.label()), Some(k));
        }
    }

    #[test]
    fn snap_alt_hotkey_defaults_to_ctrl_space_and_can_be_turned_off() {
        let d = SnapConfig::default();
        assert_eq!(d.alt_hotkey, "Ctrl+Space");
        assert_eq!(d.alt_hotkey(), Some(SnapHotkey::CtrlSpace));
        let alt = |hotkey: &str, alt: &str| {
            SnapConfig {
                hotkey: hotkey.into(),
                alt_hotkey: alt.into(),
                ..SnapConfig::default()
            }
            .alt_hotkey()
        };
        assert_eq!(alt("PrtSc", "Ctrl+Alt+S"), Some(SnapHotkey::CtrlAltS));
        for off in ["", "  ", "none", "Off", "F13"] {
            assert_eq!(alt("PrtSc", off), None, "{off:?}");
        }
        // The same key as the main hotkey is not registered twice.
        assert_eq!(alt("Ctrl+Space", "ctrl + space"), None);
        let c = Config::parse("[snap]\nalt_hotkey = \"none\"\n");
        assert_eq!(c.snap.alt_hotkey(), None);
        assert_eq!(c.snap.hotkey(), SnapHotkey::PrtSc);
    }

    #[test]
    fn snap_save_dir_expands_the_profile_and_falls_back() {
        let p = Path::new("C:/Users/me");
        let dir = |s: &str| {
            SnapConfig {
                save_dir: s.into(),
                ..SnapConfig::default()
            }
            .save_dir(p)
        };
        let default = p.join("Pictures").join("Daijobu");
        assert_eq!(dir("~/Pictures/Daijobu"), default);
        assert_eq!(dir(r"~\Shots"), p.join("Shots"));
        assert_eq!(dir("~"), p.to_path_buf());
        assert_eq!(dir("E:/snaps"), PathBuf::from("E:/snaps"));
        for relative in ["", "  ", "shots", r"..\up", "C:no-slash", r"\no-drive"] {
            assert_eq!(dir(relative), default, "{relative:?}");
        }
        // No profile: `~` paths are relative, so they fall back too (to a relative
        // default, which the app never gets: it always has USERPROFILE).
        assert_eq!(
            SnapConfig::default().save_dir(Path::new("")),
            Path::new("Pictures").join("Daijobu")
        );
    }

    #[test]
    fn vitals_health_lamp_defaults_on_and_reads_the_vitals_table() {
        assert!(Config::default().vitals.health_lamp);
        assert!(Config::parse("theme = \"x\"").vitals.health_lamp);
        let off = Config::parse("[vitals]\nhealth_lamp = false\n");
        assert!(!off.vitals.health_lamp);
    }

    #[test]
    fn settings_changes_round_trip_and_keep_the_other_sections() {
        let mut c = Config::parse(
            "theme = \"paper-zine\"\n[timer]\nfocus_min = 50\n[snap]\nsave_dir = \"D:/shots\"\n[sweep]\nproject_roots = [\"~/src\"]\n",
        );
        c.start_with_windows = true;
        c.snap.hotkey = crate::settings::next_hotkey(c.snap.hotkey())
            .label()
            .to_string();
        c.snap.alt_hotkey = crate::settings::alt_config(crate::settings::next_alt_hotkey(
            c.snap.alt_hotkey(),
            c.snap.hotkey(),
        ))
        .to_string();
        let back = Config::parse(&c.to_toml());
        assert_eq!(back, c);
        assert!(back.start_with_windows);
        assert_eq!(back.snap.hotkey(), SnapHotkey::CtrlSpace);
        // Ctrl+Space is now the main key, so the alt chip moves on from it.
        assert_eq!(back.snap.alt_hotkey(), Some(SnapHotkey::CtrlAltS));
        assert_eq!(back.theme, "paper-zine");
        assert_eq!(back.timer.focus_min, 50);
        assert_eq!(back.snap.save_dir, "D:/shots");
        assert_eq!(back.sweep.project_roots, vec!["~/src".to_string()]);
        // Turning the alt off writes "none" and reads back as off.
        c.snap.alt_hotkey = crate::settings::alt_config(None).to_string();
        assert_eq!(Config::parse(&c.to_toml()).snap.alt_hotkey(), None);
    }
}
