//! `~/.daijobu/config.toml`. Parsing only; the app does the file IO.

use crate::snap_edit::{ColorChoice, Prefs, Rgba, Size, Tool, DEFAULT_CUSTOM};
use crate::timer::{Settings, MAX_EVERY, MAX_MIN};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Theme id (file stem for user themes).
    pub theme: String,
    pub start_with_windows: bool,
    pub island: IslandConfig,
    pub music: MusicConfig,
    pub osd: OsdConfig,
    pub vitals: VitalsConfig,
    pub sweep: SweepConfig,
    pub timer: TimerConfig,
    pub snap: SnapConfig,
    #[serde(deserialize_with = "lenient_tile")]
    pub tile: TileConfig,
    #[serde(deserialize_with = "lenient_terminal")]
    pub terminal: TerminalConfig,
    pub clipboard: ClipboardConfig,
    pub spotlight: SpotlightConfig,
}

/// `[clipboard]` section. History begins only after explicit opt-in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardConfig {
    pub enabled: bool,
    pub paused: bool,
    /// Source executable names excluded from future captures.
    pub excluded_sources: Vec<String>,
}

/// How the island sits at the top of the screen. Only the drawn shape differs: the pill
/// rect, its content and its hit testing are the same in both styles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IslandStyle {
    /// A floating pill, `TOP_MARGIN` below the screen's top edge.
    Pill,
    /// Attached to the top edge like a hardware notch: flat top, concave shoulders.
    #[default]
    Notch,
}

impl IslandStyle {
    /// The config value and the Settings chip text.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pill => "pill",
            Self::Notch => "notch",
        }
    }

    /// Case-insensitive, surrounding spaces ignored.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "pill" => Some(Self::Pill),
            "notch" => Some(Self::Notch),
            _ => None,
        }
    }

    /// The other style (the Settings chip toggles).
    pub fn toggled(self) -> Self {
        match self {
            Self::Pill => Self::Notch,
            Self::Notch => Self::Pill,
        }
    }
}

/// `[island]` section.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IslandConfig {
    /// `"notch"` (default) or `"pill"`. Read it through `style()`.
    pub style: String,
    /// Notch style only: reserve a strip along the top of the screen (an AppBar), so
    /// maximized windows start below the notch instead of under it.
    pub reserve_top: bool,
    /// Keep the island on the desktop layer, below every app window. It comes to the
    /// front while it shows a panel (or the OSD) and sinks back when it rests.
    pub stay_behind: bool,
    /// The resting notch/pill size in logical px; read them through `notch_size()`.
    pub notch_width: u32,
    pub notch_height: u32,
}

/// Slider ranges (logical px) for the resting island size.
pub const NOTCH_W_RANGE: (u32, u32) = (96, 240);
pub const NOTCH_H_RANGE: (u32, u32) = (24, 44);
pub const NOTCH_W_DEFAULT: u32 = 128;
pub const NOTCH_H_DEFAULT: u32 = 32;

impl Default for IslandConfig {
    fn default() -> Self {
        Self {
            style: IslandStyle::default().label().to_string(),
            reserve_top: true,
            stay_behind: false,
            notch_width: NOTCH_W_DEFAULT,
            notch_height: NOTCH_H_DEFAULT,
        }
    }
}

impl IslandConfig {
    /// An unknown value falls back to the default (notch).
    pub fn style(&self) -> IslandStyle {
        IslandStyle::parse(&self.style).unwrap_or_default()
    }

    /// Whether the top strip is reserved while the island is visible: only a notch
    /// reserves, only when `reserve_top` is on, and never while it stays behind windows.
    pub fn reserves_top(&self) -> bool {
        self.reserve_top && self.style() == IslandStyle::Notch && !self.stay_behind
    }

    /// The resting size `(w, h)` in logical px, clamped to the slider ranges.
    pub fn notch_size(&self) -> (f32, f32) {
        (
            self.notch_width.clamp(NOTCH_W_RANGE.0, NOTCH_W_RANGE.1) as f32,
            self.notch_height.clamp(NOTCH_H_RANGE.0, NOTCH_H_RANGE.1) as f32,
        )
    }
}

/// The Spotlight hotkey choices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpotlightHotkey {
    #[default]
    AltSpace,
    CtrlAltSpace,
    Off,
}

impl SpotlightHotkey {
    pub const ALL: [SpotlightHotkey; 3] = [Self::AltSpace, Self::CtrlAltSpace, Self::Off];

    /// The config value.
    pub fn key(self) -> &'static str {
        match self {
            Self::AltSpace => "alt_space",
            Self::CtrlAltSpace => "ctrl_alt_space",
            Self::Off => "off",
        }
    }

    /// The Settings chip text.
    pub fn label(self) -> &'static str {
        match self {
            Self::AltSpace => "Alt+Space",
            Self::CtrlAltSpace => "Ctrl+Alt+Space",
            Self::Off => "off",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|h| h.key().eq_ignore_ascii_case(s.trim()))
    }

    /// The next choice (the Settings chip cycles).
    pub fn next(self) -> Self {
        match self {
            Self::AltSpace => Self::CtrlAltSpace,
            Self::CtrlAltSpace => Self::Off,
            Self::Off => Self::AltSpace,
        }
    }
}

/// `[spotlight]` section: the Alt+Space launcher.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpotlightConfig {
    pub enabled: bool,
    /// `"alt_space"` (default), `"ctrl_alt_space"` or `"off"`; read it through `hotkey()`.
    pub hotkey: String,
    /// Also search file names in the user's folders, not only apps.
    pub files: bool,
}

impl Default for SpotlightConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hotkey: SpotlightHotkey::default().key().to_string(),
            files: true,
        }
    }
}

impl SpotlightConfig {
    /// An unknown value falls back to the default; `Off` when Spotlight is disabled.
    pub fn hotkey(&self) -> SpotlightHotkey {
        if !self.enabled {
            return SpotlightHotkey::Off;
        }
        SpotlightHotkey::parse(&self.hotkey).unwrap_or_default()
    }
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
    /// How long a preview card stays in the Snap stack, in active seconds. Signed and
    /// read leniently (any non-integer counts as the default), so a stray value only
    /// resets this field, not the whole file. Read it through `preview_secs()`.
    #[serde(deserialize_with = "lenient_int")]
    pub preview_seconds: i64,
    /// `[snap.annotate]`: the editor's tool, colour and size, remembered across snaps.
    pub annotate: AnnotateConfig,
}

/// The default `[snap] save_dir`.
pub const SNAP_SAVE_DIR: &str = "~/Pictures/Daijobu";
/// The default `[snap] preview_seconds`.
pub const PREVIEW_SECS: i64 = 10;
/// The range `[snap] preview_seconds` accepts (seconds).
pub const PREVIEW_SECS_RANGE: std::ops::RangeInclusive<i64> = 3..=60;

impl Default for SnapConfig {
    fn default() -> Self {
        Self {
            hotkey: SnapHotkey::PrtSc.label().to_string(),
            alt_hotkey: SnapHotkey::CtrlSpace.label().to_string(),
            save_dir: SNAP_SAVE_DIR.to_string(),
            preview_seconds: PREVIEW_SECS,
            annotate: AnnotateConfig::default(),
        }
    }
}

/// An integer field that never fails the file: a value of any other type (a string, a
/// float, a table, ...) reads as `PREVIEW_SECS`, the one field this is used for.
fn lenient_int<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient {
        Int(i64),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Lenient::deserialize(d)? {
        Lenient::Int(v) => v,
        Lenient::Other(_) => PREVIEW_SECS,
    })
}

/// `[snap.annotate]`. Written by the app when the overlay closes, if they changed.
/// Unknown values fall back to the defaults one by one (see the accessors).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnnotateConfig {
    /// A tool name: `select`, `pen`, `line`, `arrow`, `rect`, `marker`, `text`,
    /// `pixelate` or `step`.
    pub tool: String,
    /// A swatch (`red`, `yellow`, `green`, `blue`, `purple`, `white`, `black`, `accent`)
    /// or `custom`.
    pub color: String,
    /// The custom swatch's colour, `#RRGGBB`.
    pub custom: String,
    /// `S`, `M` or `L`.
    pub size: String,
}

impl Default for AnnotateConfig {
    fn default() -> Self {
        Self::from_prefs(&Prefs::default())
    }
}

impl AnnotateConfig {
    pub fn tool(&self) -> Tool {
        Tool::parse(&self.tool).unwrap_or(Prefs::default().tool)
    }

    pub fn color(&self) -> ColorChoice {
        ColorChoice::parse(&self.color).unwrap_or(Prefs::default().color)
    }

    pub fn custom(&self) -> Rgba {
        Rgba::from_hex(&self.custom).unwrap_or(DEFAULT_CUSTOM)
    }

    pub fn size(&self) -> Size {
        Size::parse(&self.size).unwrap_or(Prefs::default().size)
    }

    pub fn prefs(&self) -> Prefs {
        Prefs {
            tool: self.tool(),
            color: self.color(),
            custom: self.custom(),
            size: self.size(),
        }
    }

    pub fn from_prefs(p: &Prefs) -> Self {
        Self {
            tool: p.tool.name().to_string(),
            color: p.color.name().to_string(),
            custom: p.custom.to_hex(),
            size: p.size.name().to_string(),
        }
    }
}

impl SnapConfig {
    pub fn hotkey(&self) -> SnapHotkey {
        SnapHotkey::parse(&self.hotkey).unwrap_or(SnapHotkey::PrtSc)
    }

    /// Seconds a preview card counts down: `preview_seconds` when it is in 3..=60, else 10.
    pub fn preview_secs(&self) -> u64 {
        if PREVIEW_SECS_RANGE.contains(&self.preview_seconds) {
            self.preview_seconds as u64
        } else {
            PREVIEW_SECS as u64
        }
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

/// The default `[tile] gap` (logical px) and the range it is clamped to.
pub const TILE_GAP: f32 = 8.0;
pub const TILE_GAP_RANGE: std::ops::RangeInclusive<f32> = 0.0..=48.0;

/// `[tile]` section. Every field falls back to its default on its own when its value has
/// the wrong type, so a stray value never costs the rest of the file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TileConfig {
    /// Logical px between zones and around the screen edge (× the monitor's scale).
    /// Clamped to 0..=48; an integer or a float.
    #[serde(deserialize_with = "lenient_gap", serialize_with = "short_f32")]
    pub gap: f32,
    /// A window dropped outside any zone gets back the size it had before the app put it
    /// in one.
    #[serde(deserialize_with = "lenient_true")]
    pub restore_size: bool,
    /// Ctrl+Alt+1..9 put the foreground window in that zone.
    #[serde(deserialize_with = "lenient_true")]
    pub number_keys: bool,
    /// Ctrl+Alt+arrows move it to the neighbouring zone.
    #[serde(deserialize_with = "lenient_true")]
    pub arrow_keys: bool,
    /// `[tile.monitors]`: a monitor's device name (`\\.\DISPLAY1`) to its active layout's
    /// name. Written by the app when a zone is picked from another layout. Entries whose
    /// value is not a string are dropped. Keep this last: TOML tables follow plain values.
    #[serde(deserialize_with = "lenient_monitors")]
    pub monitors: BTreeMap<String, String>,
}

impl Default for TileConfig {
    fn default() -> Self {
        Self {
            gap: TILE_GAP,
            restore_size: true,
            number_keys: true,
            arrow_keys: true,
            monitors: BTreeMap::new(),
        }
    }
}

/// A whole `[tile]` value of the wrong type (`tile = 5`) reads as the defaults.
fn lenient_tile<'de, D: serde::Deserializer<'de>>(d: D) -> Result<TileConfig, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient {
        Tile(TileConfig),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Lenient::deserialize(d)? {
        Lenient::Tile(t) => t,
        Lenient::Other(_) => TileConfig::default(),
    })
}

/// An integer or a float, clamped to `TILE_GAP_RANGE`; anything else (and NaN) is the
/// default.
fn lenient_gap<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient {
        Int(i64),
        Float(f64),
        Other(serde::de::IgnoredAny),
    }
    let v = match Lenient::deserialize(d)? {
        Lenient::Int(v) => v as f64,
        Lenient::Float(v) => v,
        Lenient::Other(_) => return Ok(TILE_GAP),
    };
    Ok(if v.is_nan() {
        TILE_GAP
    } else {
        (v as f32).clamp(*TILE_GAP_RANGE.start(), *TILE_GAP_RANGE.end())
    })
}

/// A bool that reads as `true` when the value is of another type.
fn lenient_true<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient {
        Bool(bool),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Lenient::deserialize(d)? {
        Lenient::Bool(v) => v,
        Lenient::Other(_) => true,
    })
}

/// A string-to-string table; entries with other value types are dropped, and a value that
/// is not a table at all is empty.
fn lenient_monitors<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Entry {
        Name(String),
        Other(serde::de::IgnoredAny),
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient {
        Table(BTreeMap<String, Entry>),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Lenient::deserialize(d)? {
        Lenient::Table(t) => t
            .into_iter()
            .filter_map(|(k, v)| match v {
                Entry::Name(n) => Some((k, n)),
                Entry::Other(_) => None,
            })
            .collect(),
        Lenient::Other(_) => BTreeMap::new(),
    })
}

/// The default `[terminal] font`, and the font size and scrollback with the ranges they
/// are clamped to.
pub const TERMINAL_FONT: &str = "Cascadia Mono";
pub const TERMINAL_FONT_SIZE: f32 = 12.0;
pub const TERMINAL_FONT_SIZE_RANGE: std::ops::RangeInclusive<f32> = 8.0..=24.0;
pub const TERMINAL_SCROLLBACK: u32 = 5000;
pub const TERMINAL_SCROLLBACK_RANGE: std::ops::RangeInclusive<u32> = 0..=50_000;

/// `[terminal]` section. Every field falls back to its default on its own when its value
/// has the wrong type, so a stray value never costs the rest of the file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// `""` picks `pwsh.exe` if it is on PATH, else Windows PowerShell. `"cmd"`, `"wsl"`,
    /// or a path or program name also work. Any other type reads as `""`.
    #[serde(deserialize_with = "lenient_string")]
    pub shell: String,
    /// Arguments for `shell`. Entries that are not strings are dropped, and a value that
    /// is not a list is empty.
    #[serde(deserialize_with = "lenient_strings")]
    pub args: Vec<String>,
    /// The shell's working directory; `""` is the user's home.
    #[serde(deserialize_with = "lenient_string")]
    pub cwd: String,
    /// Font family for the cells. A blank or non-string value reads as the default.
    #[serde(deserialize_with = "lenient_font")]
    pub font: String,
    /// Logical px, an integer or a float, clamped to 8..=24.
    #[serde(deserialize_with = "lenient_font_size", serialize_with = "short_f32")]
    pub font_size: f32,
    /// Hide the panel when another window is clicked (the session keeps running).
    #[serde(deserialize_with = "lenient_true")]
    pub hide_on_blur: bool,
    /// Lines of history, clamped to 0..=50000; an integer or a float.
    #[serde(deserialize_with = "lenient_scrollback")]
    pub scrollback: u32,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: String::new(),
            args: Vec::new(),
            cwd: String::new(),
            font: TERMINAL_FONT.to_string(),
            font_size: TERMINAL_FONT_SIZE,
            hide_on_blur: true,
            scrollback: TERMINAL_SCROLLBACK,
        }
    }
}

/// A whole `[terminal]` value of the wrong type (`terminal = 5`) reads as the defaults.
fn lenient_terminal<'de, D: serde::Deserializer<'de>>(d: D) -> Result<TerminalConfig, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient {
        Terminal(TerminalConfig),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Lenient::deserialize(d)? {
        Lenient::Terminal(t) => t,
        Lenient::Other(_) => TerminalConfig::default(),
    })
}

/// A string; any other type is empty.
fn lenient_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient {
        Text(String),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Lenient::deserialize(d)? {
        Lenient::Text(s) => s,
        Lenient::Other(_) => String::new(),
    })
}

/// A list of strings; entries of another type are dropped, and a value that is not a
/// list is empty.
fn lenient_strings<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Entry {
        Text(String),
        Other(serde::de::IgnoredAny),
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient {
        List(Vec<Entry>),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Lenient::deserialize(d)? {
        Lenient::List(l) => l
            .into_iter()
            .filter_map(|e| match e {
                Entry::Text(s) => Some(s),
                Entry::Other(_) => None,
            })
            .collect(),
        Lenient::Other(_) => Vec::new(),
    })
}

/// The font family: a string that is not blank (trimmed), else the default.
fn lenient_font<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let s = lenient_string(d)?;
    let s = s.trim();
    Ok(if s.is_empty() { TERMINAL_FONT } else { s }.to_string())
}

/// An integer or a float; anything else (and NaN) is `None`.
fn lenient_number<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient {
        Int(i64),
        Float(f64),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Lenient::deserialize(d)? {
        Lenient::Int(v) => Some(v as f64),
        Lenient::Float(v) if !v.is_nan() => Some(v),
        _ => None,
    })
}

/// An integer or a float, clamped to `TERMINAL_FONT_SIZE_RANGE`; anything else is 12.
fn lenient_font_size<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    Ok(lenient_number(d)?.map_or(TERMINAL_FONT_SIZE, |v| {
        (v as f32).clamp(
            *TERMINAL_FONT_SIZE_RANGE.start(),
            *TERMINAL_FONT_SIZE_RANGE.end(),
        )
    }))
}

/// An integer or a float, clamped to `TERMINAL_SCROLLBACK_RANGE` (a float is cut to a
/// whole number of lines); anything else is 5000.
fn lenient_scrollback<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    Ok(lenient_number(d)?.map_or(TERMINAL_SCROLLBACK, |v| {
        v.clamp(
            f64::from(*TERMINAL_SCROLLBACK_RANGE.start()),
            f64::from(*TERMINAL_SCROLLBACK_RANGE.end()),
        ) as u32
    }))
}

/// Writes an `f32` the way it reads (`12.1`, not `12.100000381469727`).
fn short_f32<S: serde::Serializer>(v: &f32, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_f64(v.to_string().parse().unwrap_or(*v as f64))
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: crate::theme::DEFAULT_THEME.to_string(),
            start_with_windows: false,
            island: IslandConfig::default(),
            music: MusicConfig::default(),
            osd: OsdConfig::default(),
            vitals: VitalsConfig::default(),
            sweep: SweepConfig::default(),
            timer: TimerConfig::default(),
            snap: SnapConfig::default(),
            tile: TileConfig::default(),
            terminal: TerminalConfig::default(),
            clipboard: ClipboardConfig::default(),
            spotlight: SpotlightConfig::default(),
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
    fn legacy_config_does_not_enable_clipboard_history() {
        let c = Config::parse("theme = 'paper-zine'\n[music]\nsigned_in = true\n");
        assert_eq!(c.theme, "paper-zine");
        assert!(c.music.signed_in);
        assert_eq!(c.clipboard, ClipboardConfig::default());
        assert!(!c.clipboard.enabled && !c.clipboard.paused);
        assert!(c.clipboard.excluded_sources.is_empty());
        assert_eq!(Config::parse(&c.to_toml()), c);
    }

    #[test]
    fn partial_clipboard_config_defaults_missing_fields_without_losing_other_sections() {
        let c = Config::parse("theme = 'paper-zine'\n[clipboard]\npaused = true\n");
        assert_eq!(c.theme, "paper-zine");
        assert!(!c.clipboard.enabled);
        assert!(c.clipboard.paused);
        assert!(c.clipboard.excluded_sources.is_empty());
        assert_eq!(Config::parse(&c.to_toml()), c);
        let c = Config::parse(
            "[clipboard]\nenabled = true\nexcluded_sources = ['秘密.exe', 'private.exe']\n",
        );
        assert!(c.clipboard.enabled && !c.clipboard.paused);
        assert_eq!(c.clipboard.excluded_sources, ["秘密.exe", "private.exe"]);
        assert_eq!(Config::parse(&c.to_toml()), c);
    }

    #[test]
    fn roundtrip_and_defaults() {
        let c = Config {
            theme: "paper-zine".into(),
            start_with_windows: true,
            island: IslandConfig {
                style: "pill".into(),
                reserve_top: false,
                stay_behind: true,
                notch_width: 160,
                notch_height: 36,
            },
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
                preview_seconds: 25,
                annotate: AnnotateConfig {
                    tool: "pixelate".into(),
                    color: "custom".into(),
                    custom: "#123ABC".into(),
                    size: "L".into(),
                },
            },
            tile: TileConfig {
                gap: 12.5,
                restore_size: false,
                number_keys: false,
                arrow_keys: false,
                monitors: BTreeMap::from([("\\\\.\\DISPLAY2".into(), "1+2".into())]),
            },
            terminal: TerminalConfig {
                shell: "C:\\Program Files\\PowerShell\\7\\pwsh.exe".into(),
                args: vec!["-NoLogo".into(), "-NoProfile".into()],
                cwd: "D:\\work".into(),
                font: "JetBrains Mono".into(),
                font_size: 13.5,
                hide_on_blur: false,
                scrollback: 20_000,
            },
            spotlight: SpotlightConfig {
                enabled: false,
                hotkey: "ctrl_alt_space".into(),
                files: false,
            },
            clipboard: ClipboardConfig {
                enabled: true,
                paused: true,
                excluded_sources: vec!["private.exe".into()],
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
    fn island_style_names() {
        for (s, want) in [
            ("notch", Some(IslandStyle::Notch)),
            ("pill", Some(IslandStyle::Pill)),
            ("Notch", Some(IslandStyle::Notch)),
            ("PILL", Some(IslandStyle::Pill)),
            ("  notch  ", Some(IslandStyle::Notch)),
            ("\tPill\n", Some(IslandStyle::Pill)),
            ("", None),
            ("  ", None),
            ("floating", None),
            ("notches", None),
            ("no tch", None),
        ] {
            assert_eq!(IslandStyle::parse(s), want, "{s:?}");
        }
        for s in [IslandStyle::Pill, IslandStyle::Notch] {
            assert_eq!(IslandStyle::parse(s.label()), Some(s));
            assert_ne!(s.toggled(), s);
            assert_eq!(s.toggled().toggled(), s);
        }
        assert_eq!(IslandStyle::Pill.toggled(), IslandStyle::Notch);
        assert_eq!(IslandStyle::Notch.toggled(), IslandStyle::Pill);
        assert_eq!(IslandStyle::default(), IslandStyle::Notch);
        assert_eq!(IslandStyle::Notch.label(), "notch");
        assert_eq!(IslandStyle::Pill.label(), "pill");
    }

    #[test]
    fn reserve_top_defaults_on_and_only_a_notch_reserves() {
        let d = IslandConfig::default();
        assert!(d.reserve_top);
        assert!(d.reserves_top());
        // Older configs without the key: on.
        assert!(
            Config::parse("[island]\nstyle = \"notch\"\n")
                .island
                .reserve_top
        );
        let off = Config::parse("[island]\nreserve_top = false\n");
        assert!(!off.island.reserve_top && !off.island.reserves_top());
        assert_eq!(Config::parse(&off.to_toml()), off);
        // A pill never reserves, whatever the switch says.
        let pill = Config::parse("[island]\nstyle = \"pill\"\nreserve_top = true\n");
        assert!(pill.island.reserve_top && !pill.island.reserves_top());
    }

    #[test]
    fn island_style_defaults_to_notch_and_reads_the_island_table() {
        assert_eq!(IslandConfig::default().style, "notch");
        assert_eq!(IslandConfig::default().style(), IslandStyle::Notch);
        assert_eq!(Config::default().island.style(), IslandStyle::Notch);
        // No `[island]` table: notch.
        let c = Config::parse("theme = \"x\"\n[timer]\nfocus_min = 50\n");
        assert_eq!(c.island, IslandConfig::default());
        assert_eq!(c.island.style(), IslandStyle::Notch);
        let pill = Config::parse("[island]\nstyle = \"pill\"\n");
        assert_eq!(pill.island.style(), IslandStyle::Pill);
        assert_eq!(
            Config::parse("[island]\nstyle = \" Pill \"\n")
                .island
                .style(),
            IslandStyle::Pill
        );
        assert_eq!(Config::parse(&pill.to_toml()), pill);
        // An unknown value means notch, and the rest of the file still reads.
        let odd = Config::parse("theme = \"paper-zine\"\n[island]\nstyle = \"floating\"\n");
        assert_eq!(odd.island.style(), IslandStyle::Notch);
        assert_eq!(odd.theme, "paper-zine");
        // An `[island]` table without `style` is notch too.
        assert_eq!(
            Config::parse("[island]\n").island.style(),
            IslandStyle::Notch
        );
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
    fn snap_preview_seconds_defaults_to_ten_and_reads_the_snap_table() {
        assert_eq!(SnapConfig::default().preview_seconds, 10);
        assert_eq!(SnapConfig::default().preview_secs(), 10);
        // No key, no `[snap]` table, or an older config: ten.
        assert_eq!(Config::parse("theme = \"x\"").snap.preview_secs(), 10);
        assert_eq!(
            Config::parse("[snap]\nhotkey = \"Ctrl+Alt+S\"\n")
                .snap
                .preview_secs(),
            10
        );
        let c = Config::parse("[snap]\npreview_seconds = 25\n[timer]\nfocus_min = 50\n");
        assert_eq!(c.snap.preview_secs(), 25);
        assert_eq!(c.timer.focus_min, 50);
        // Both ends of 3..=60 are valid.
        for ok in [3, 4, 10, 59, 60] {
            let c = Config::parse(&format!("[snap]\npreview_seconds = {ok}\n"));
            assert_eq!(c.snap.preview_secs(), ok as u64, "{ok}");
        }
    }

    #[test]
    fn snap_preview_seconds_out_of_range_or_wrong_type_falls_back_to_ten() {
        for bad in [
            "2", "0", "61", "-5", "3600", "\"12\"", "\"soon\"", "10.5", "true", "[1]",
        ] {
            let c = Config::parse(&format!(
                "theme = \"paper-zine\"\n[snap]\nhotkey = \"Ctrl+Alt+S\"\npreview_seconds = {bad}\nsave_dir = \"D:/shots\"\n[timer]\nfocus_min = 50\n"
            ));
            assert_eq!(c.snap.preview_secs(), 10, "{bad}");
            // The rest of the file still reads.
            assert_eq!(c.theme, "paper-zine", "{bad}");
            assert_eq!(c.snap.hotkey(), SnapHotkey::CtrlAltS, "{bad}");
            assert_eq!(c.snap.save_dir, "D:/shots", "{bad}");
            assert_eq!(c.timer.focus_min, 50, "{bad}");
        }
        let c = SnapConfig {
            preview_seconds: i64::MIN,
            ..SnapConfig::default()
        };
        assert_eq!(c.preview_secs(), 10);
        let c = SnapConfig {
            preview_seconds: i64::MAX,
            ..SnapConfig::default()
        };
        assert_eq!(c.preview_secs(), 10);
    }

    #[test]
    fn snap_preview_seconds_round_trips_with_the_other_sections() {
        let mut c = Config::parse(
            "theme = \"paper-zine\"\n[timer]\nfocus_min = 50\n[snap]\nsave_dir = \"D:/shots\"\n",
        );
        c.snap.preview_seconds = 42;
        let text = c.to_toml();
        assert!(text.contains("preview_seconds = 42"), "{text}");
        let back = Config::parse(&text);
        assert_eq!(back, c);
        assert_eq!(back.snap.preview_secs(), 42);
        assert_eq!(back.snap.save_dir, "D:/shots");
        assert_eq!(back.timer.focus_min, 50);
        // An out-of-range number is kept as written; only the accessor falls back.
        c.snap.preview_seconds = 2;
        let back = Config::parse(&c.to_toml());
        assert_eq!(back, c);
        assert_eq!(back.snap.preview_secs(), 10);
        // A wrong-typed value is written back as the default, not as text.
        let c = Config::parse("[snap]\npreview_seconds = \"abc\"\n");
        assert_eq!(c.snap.preview_seconds, 10);
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
    fn snap_annotate_defaults_reading_and_invalid_values() {
        let d = Config::parse("theme = \"x\"").snap.annotate;
        assert_eq!(d, AnnotateConfig::default());
        assert_eq!(
            (
                d.tool.as_str(),
                d.color.as_str(),
                d.custom.as_str(),
                d.size.as_str()
            ),
            ("arrow", "red", "#FF7AC6", "M")
        );
        assert_eq!(d.prefs(), Prefs::default());
        let c = Config::parse(
            "[snap]\nhotkey = \"Ctrl+Alt+S\"\n[snap.annotate]\ntool = \"Marker\"\ncolor = \"accent\"\ncustom = \"#00ff00\"\nsize = \"l\"\n",
        );
        assert_eq!(
            c.snap.hotkey(),
            SnapHotkey::CtrlAltS,
            "the [snap] keys stay"
        );
        let p = c.snap.annotate.prefs();
        assert_eq!(p.tool, Tool::Marker);
        assert_eq!(p.color, ColorChoice::Swatch(7));
        assert_eq!(p.custom, Rgba::rgb(0, 255, 0));
        assert_eq!(p.size, Size::L);
        // Each bad value falls back on its own.
        let c = Config::parse(
            "[snap.annotate]\ntool = \"brush\"\ncolor = \"pink\"\ncustom = \"#12\"\nsize = \"XL\"\n",
        );
        assert_eq!(c.snap.annotate.prefs(), Prefs::default());
        let c = Config::parse("[snap.annotate]\ntool = \"step\"\nsize = \"huge\"\n");
        assert_eq!(c.snap.annotate.tool(), Tool::Step);
        assert_eq!(c.snap.annotate.size(), Size::M);
        assert_eq!(c.snap.annotate.color(), ColorChoice::Swatch(0));
        // Prefs round-trip through the config text, keeping the other sections.
        let mut c = Config::parse("theme = \"paper-zine\"\n[timer]\nfocus_min = 50\n");
        let p = Prefs {
            tool: Tool::Text,
            color: ColorChoice::Custom,
            custom: Rgba::rgb(0x12, 0x34, 0x56),
            size: Size::S,
        };
        c.snap.annotate = AnnotateConfig::from_prefs(&p);
        let text = c.to_toml();
        assert!(text.contains("[snap.annotate]"), "{text}");
        let back = Config::parse(&text);
        assert_eq!(back.snap.annotate.prefs(), p);
        assert_eq!(back.snap.annotate.custom, "#123456");
        assert_eq!(back.theme, "paper-zine");
        assert_eq!(back.timer.focus_min, 50);
    }

    #[test]
    fn vitals_health_lamp_defaults_on_and_reads_the_vitals_table() {
        assert!(Config::default().vitals.health_lamp);
        assert!(Config::parse("theme = \"x\"").vitals.health_lamp);
        let off = Config::parse("[vitals]\nhealth_lamp = false\n");
        assert!(!off.vitals.health_lamp);
    }

    #[test]
    fn tile_defaults_and_reads_the_tile_table() {
        let d = TileConfig::default();
        assert_eq!(
            (d.gap, d.restore_size, d.number_keys, d.arrow_keys),
            (8.0, true, true, true)
        );
        assert!(d.monitors.is_empty());
        // No `[tile]` table, or an older config: the defaults.
        assert_eq!(Config::parse("theme = \"x\"").tile, d);
        assert_eq!(Config::default().tile, d);
        assert_eq!(Config::parse("[tile]\n").tile, d);
        let c = Config::parse(
            "theme = \"paper-zine\"\n[tile]\ngap = 12\nrestore_size = false\nnumber_keys = false\narrow_keys = false\n[tile.monitors]\n\"\\\\\\\\.\\\\DISPLAY1\" = \"2x2\"\n'\\\\.\\DISPLAY2' = \"1+2\"\n[timer]\nfocus_min = 50\n",
        );
        assert_eq!(c.tile.gap, 12.0);
        assert!(!c.tile.restore_size && !c.tile.number_keys && !c.tile.arrow_keys);
        assert_eq!(c.tile.monitors.len(), 2);
        assert_eq!(c.tile.monitors["\\\\.\\DISPLAY1"], "2x2");
        assert_eq!(c.tile.monitors["\\\\.\\DISPLAY2"], "1+2");
        // The other sections still read.
        assert_eq!(c.theme, "paper-zine");
        assert_eq!(c.timer.focus_min, 50);
        // Each key stands alone.
        let c = Config::parse("[tile]\nnumber_keys = false\n");
        assert_eq!(
            (
                c.tile.gap,
                c.tile.restore_size,
                c.tile.number_keys,
                c.tile.arrow_keys
            ),
            (8.0, true, false, true)
        );
        // A float gap.
        assert_eq!(Config::parse("[tile]\ngap = 6.5\n").tile.gap, 6.5);
    }

    #[test]
    fn tile_gap_is_clamped_and_bad_values_fall_back_per_field() {
        for (text, want) in [
            ("0", 0.0),
            ("48", 48.0),
            ("49", 48.0),
            ("1000", 48.0),
            ("-3", 0.0),
            ("-2.5", 0.0),
            ("47.5", 47.5),
            ("100.0", 48.0),
            ("inf", 48.0),
            ("-inf", 0.0),
            ("nan", 8.0),
            ("\"wide\"", 8.0),
            ("true", 8.0),
            ("[1]", 8.0),
            ("{ a = 1 }", 8.0),
        ] {
            let c = Config::parse(&format!(
                "theme = \"paper-zine\"\n[tile]\nrestore_size = false\ngap = {text}\n[timer]\nfocus_min = 50\n"
            ));
            assert_eq!(c.tile.gap, want, "{text}");
            // The rest of the file, and the rest of the table, still read.
            assert_eq!(c.theme, "paper-zine", "{text}");
            assert!(!c.tile.restore_size, "{text}");
            assert_eq!(c.timer.focus_min, 50, "{text}");
        }
        // A wrong-typed bool is its default (on) and leaves the others alone.
        let c = Config::parse(
            "theme = \"paper-zine\"\n[tile]\ngap = 20\nrestore_size = \"no\"\nnumber_keys = 0\narrow_keys = false\n",
        );
        assert_eq!(c.theme, "paper-zine");
        assert_eq!(c.tile.gap, 20.0);
        assert!(c.tile.restore_size && c.tile.number_keys && !c.tile.arrow_keys);
        // Unknown keys are ignored.
        assert_eq!(Config::parse("[tile]\nzoom = 3\ngap = 9\n").tile.gap, 9.0);
        // The whole table of the wrong type: defaults, the file survives.
        let c = Config::parse("theme = \"paper-zine\"\ntile = 5\n[timer]\nfocus_min = 50\n");
        assert_eq!(c.tile, TileConfig::default());
        assert_eq!(c.theme, "paper-zine");
        assert_eq!(c.timer.focus_min, 50);
    }

    #[test]
    fn tile_monitors_drop_bad_entries_and_round_trip_with_the_other_sections() {
        let c = Config::parse(
            "theme = \"paper-zine\"\n[tile]\ngap = 10\n[tile.monitors]\n\"a\" = \"2x2\"\n\"b\" = 5\n\"c\" = [1]\n\"d\" = \"1+2\"\n",
        );
        let keys: Vec<&str> = c.tile.monitors.keys().map(String::as_str).collect();
        assert_eq!(keys, ["a", "d"]);
        assert_eq!(c.tile.gap, 10.0);
        assert_eq!(c.theme, "paper-zine");
        // `monitors` not a table at all.
        let c = Config::parse("[tile]\ngap = 10\nmonitors = \"x\"\n");
        assert!(c.tile.monitors.is_empty());
        assert_eq!(c.tile.gap, 10.0);
        // The writer (the app's `to_toml`) keeps the table and the other sections; device
        // names with backslashes survive.
        let mut c = Config::parse(
            "theme = \"paper-zine\"\n[timer]\nfocus_min = 50\n[snap]\nsave_dir = \"D:/shots\"\n[tile]\ngap = 6\n",
        );
        c.tile
            .monitors
            .insert("\\\\.\\DISPLAY1".into(), "2x2".into());
        c.tile
            .monitors
            .insert("\\\\.\\DISPLAY2".into(), "code + docs".into());
        let text = c.to_toml();
        assert!(text.contains("[tile.monitors]"), "{text}");
        let back = Config::parse(&text);
        assert_eq!(back, c);
        assert_eq!(back.tile.monitors["\\\\.\\DISPLAY2"], "code + docs");
        assert_eq!(back.tile.gap, 6.0);
        assert_eq!(back.timer.focus_min, 50);
        assert_eq!(back.snap.save_dir, "D:/shots");
        assert_eq!(back.theme, "paper-zine");
        // Changing one monitor's layout (what picking a zone does) and writing again.
        c.tile
            .monitors
            .insert("\\\\.\\DISPLAY1".into(), "1+3".into());
        let back = Config::parse(&c.to_toml());
        assert_eq!(back, c);
        assert_eq!(back.tile.monitors.len(), 2);
        // A gap that is not a round float is written the way it was read.
        c.tile.gap = 12.1;
        let text = c.to_toml();
        assert!(text.contains("gap = 12.1\n"), "{text}");
        assert_eq!(Config::parse(&text).tile.gap, 12.1);
        // The defaults round trip too, with no monitors table.
        let d = Config::default();
        assert_eq!(Config::parse(&d.to_toml()), d);
    }

    #[test]
    fn terminal_defaults_and_reads_the_terminal_table() {
        let d = TerminalConfig::default();
        assert_eq!(d.shell, "");
        assert!(d.args.is_empty());
        assert_eq!(d.cwd, "");
        assert_eq!(d.font, "Cascadia Mono");
        assert_eq!(d.font_size, 12.0);
        assert!(d.hide_on_blur);
        assert_eq!(d.scrollback, 5000);
        // No `[terminal]` table, an older config, or an empty one: the defaults.
        assert_eq!(Config::parse("theme = \"x\"").terminal, d);
        assert_eq!(Config::default().terminal, d);
        assert_eq!(Config::parse("[terminal]\n").terminal, d);
        let c = Config::parse(
            "theme = \"paper-zine\"\n[tile]\ngap = 4\n[terminal]\nshell = \"cmd\"\nargs = [\"/k\", \"echo hi\"]\ncwd = \"D:/src\"\nfont = \"Consolas\"\nfont_size = 14\nhide_on_blur = false\nscrollback = 100\n[timer]\nfocus_min = 50\n",
        );
        let t = &c.terminal;
        assert_eq!(t.shell, "cmd");
        assert_eq!(t.args, ["/k", "echo hi"]);
        assert_eq!(t.cwd, "D:/src");
        assert_eq!(t.font, "Consolas");
        assert_eq!(t.font_size, 14.0);
        assert!(!t.hide_on_blur);
        assert_eq!(t.scrollback, 100);
        // The sections around it are read as before.
        assert_eq!(c.tile.gap, 4.0);
        assert_eq!(c.timer.focus_min, 50);
        // One key set: the others keep their defaults.
        let c = Config::parse("[terminal]\nhide_on_blur = false\n");
        assert_eq!(
            c.terminal,
            TerminalConfig {
                hide_on_blur: false,
                ..d
            }
        );
        // A float font size is read as it is, and an integer one too.
        assert_eq!(
            Config::parse("[terminal]\nfont_size = 10.5\n")
                .terminal
                .font_size,
            10.5
        );
    }

    #[test]
    fn terminal_font_size_and_scrollback_are_clamped() {
        for (text, want) in [
            ("8", 8.0),
            ("24", 24.0),
            ("7", 8.0),
            ("0", 8.0),
            ("-3", 8.0),
            ("25", 24.0),
            ("1000", 24.0),
            ("7.9", 8.0),
            ("24.5", 24.0),
            ("inf", 24.0),
            ("-inf", 8.0),
            ("nan", 12.0),
            ("\"big\"", 12.0),
            ("true", 12.0),
            ("[14]", 12.0),
        ] {
            let c = Config::parse(&format!(
                "theme = \"paper-zine\"\n[terminal]\nshell = \"cmd\"\nfont_size = {text}\n[timer]\nfocus_min = 50\n"
            ));
            assert_eq!(c.terminal.font_size, want, "{text}");
            // The bad value never costs the rest of the file.
            assert_eq!(c.terminal.shell, "cmd", "{text}");
            assert_eq!(c.timer.focus_min, 50, "{text}");
            assert_eq!(c.theme, "paper-zine", "{text}");
        }
        for (text, want) in [
            ("0", 0),
            ("1", 1),
            ("5000", 5000),
            ("50000", 50_000),
            ("50001", 50_000),
            ("99999999999", 50_000),
            ("-1", 0),
            ("-99999999999", 0),
            ("2500.9", 2500),
            ("1e9", 50_000),
            ("-2.5", 0),
            ("nan", 5000),
            ("\"lots\"", 5000),
            ("false", 5000),
            ("{ a = 1 }", 5000),
        ] {
            let c = Config::parse(&format!(
                "theme = \"paper-zine\"\n[terminal]\nscrollback = {text}\ncwd = \"D:/x\"\n[timer]\nfocus_min = 50\n"
            ));
            assert_eq!(c.terminal.scrollback, want, "{text}");
            assert_eq!(c.terminal.cwd, "D:/x", "{text}");
            assert_eq!(c.timer.focus_min, 50, "{text}");
        }
    }

    #[test]
    fn terminal_bad_types_fall_back_per_field() {
        let c = Config::parse(
            "theme = \"paper-zine\"\n[terminal]\nshell = 5\nargs = \"-NoLogo\"\ncwd = [1]\nfont = false\nfont_size = \"x\"\nhide_on_blur = \"no\"\nscrollback = \"x\"\n[timer]\nfocus_min = 50\n",
        );
        assert_eq!(c.terminal, TerminalConfig::default());
        assert_eq!(c.timer.focus_min, 50);
        assert_eq!(c.theme, "paper-zine");
        // Each field on its own: the good ones are kept.
        let c = Config::parse(
            "[terminal]\nshell = \"wsl\"\nargs = 7\ncwd = \"D:/a\"\nfont = 3\nfont_size = 9\nhide_on_blur = 0\nscrollback = 10\n",
        );
        let t = c.terminal;
        assert_eq!((t.shell.as_str(), t.cwd.as_str()), ("wsl", "D:/a"));
        assert!(t.args.is_empty());
        assert_eq!(t.font, "Cascadia Mono");
        assert_eq!((t.font_size, t.scrollback), (9.0, 10));
        assert!(t.hide_on_blur);
        // A list keeps its strings and drops the rest, in order.
        let c = Config::parse("[terminal]\nargs = [\"a\", 1, \"b\", [2], true, \"c\"]\n");
        assert_eq!(c.terminal.args, ["a", "b", "c"]);
        // A blank font is the default (a blank shell or cwd is meaningful: auto, home).
        let c = Config::parse("[terminal]\nfont = \"  \"\nshell = \"\"\ncwd = \"\"\n");
        assert_eq!(c.terminal, TerminalConfig::default());
        let c = Config::parse("[terminal]\nfont = \"  Fira Code \"\n");
        assert_eq!(c.terminal.font, "Fira Code");
        // Unknown keys are ignored.
        let c = Config::parse("[terminal]\ntabs = 3\nscrollback = 70\n");
        assert_eq!(c.terminal.scrollback, 70);
        // The whole value of the wrong type reads as the defaults, and the rest survives.
        let c = Config::parse("theme = \"paper-zine\"\nterminal = 5\n[timer]\nfocus_min = 50\n");
        assert_eq!(c.terminal, TerminalConfig::default());
        assert_eq!(c.timer.focus_min, 50);
        assert_eq!(c.theme, "paper-zine");
        let c = Config::parse("theme = \"paper-zine\"\nterminal = \"cmd\"\n");
        assert_eq!(c.terminal, TerminalConfig::default());
        assert_eq!(c.theme, "paper-zine");
    }

    #[test]
    fn terminal_round_trips_with_the_other_sections() {
        // Defaults write and read back the same.
        let d = Config::default();
        assert_eq!(Config::parse(&d.to_toml()), d);
        let mut c = Config::parse(
            "theme = \"paper-zine\"\n[timer]\nfocus_min = 50\n[tile]\ngap = 6\n[tile.monitors]\n\"a\" = \"2x2\"\n",
        );
        c.terminal = TerminalConfig {
            shell: "pwsh".into(),
            args: vec!["-NoLogo".into(), "-Command".into(), "echo \"hi\"".into()],
            cwd: "C:\\Users\\me\\src".into(),
            font: "Cascadia Code".into(),
            font_size: 12.1,
            hide_on_blur: false,
            scrollback: 0,
        };
        let text = c.to_toml();
        assert!(text.contains("[terminal]"), "{text}");
        // A font size that is not a round float is written the way it was read.
        assert!(text.contains("font_size = 12.1\n"), "{text}");
        let back = Config::parse(&text);
        assert_eq!(back, c);
        assert_eq!(back.terminal.args.len(), 3);
        assert_eq!(back.terminal.cwd, "C:\\Users\\me\\src");
        assert_eq!(back.timer.focus_min, 50);
        assert_eq!(back.tile.monitors["a"], "2x2");
        // Clamped values are what gets written, so the file stays in range.
        let c = Config::parse("[terminal]\nfont_size = 99\nscrollback = 999999\n");
        let back = Config::parse(&c.to_toml());
        assert_eq!(
            (back.terminal.font_size, back.terminal.scrollback),
            (24.0, 50_000)
        );
        assert_eq!(back, c);
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
        // The style chip: notch → pill and back, each read back from the file.
        assert_eq!(c.island.style(), IslandStyle::Notch);
        c.island.style = c.island.style().toggled().label().to_string();
        let back = Config::parse(&c.to_toml());
        assert_eq!(back, c);
        assert_eq!(back.island.style(), IslandStyle::Pill);
        assert_eq!(back.snap.save_dir, "D:/shots");
        c.island.style = c.island.style().toggled().label().to_string();
        assert_eq!(
            Config::parse(&c.to_toml()).island.style(),
            IslandStyle::Notch
        );
    }
}
