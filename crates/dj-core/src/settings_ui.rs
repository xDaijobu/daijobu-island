//! The Settings window (Stage V2): a large popup with a sidebar of pages, rows of
//! controls, a search field and Reset / Apply / Save. Pure: it edits a pending copy of the
//! `Config` and tells the app what to apply; the app owns the window and the side effects.
//!
//! Coordinates are window-local logical px. The window is `WIN_W` × `WIN_H` with a
//! transparent `MARGIN` all round (room for the shadow); the panel is `PANEL`.

use crate::config::{
    Config, IslandStyle, SpotlightHotkey, NOTCH_H_RANGE, NOTCH_W_RANGE, TERMINAL_FONT_SIZE_RANGE,
    TERMINAL_SCROLLBACK_RANGE,
};
use crate::layout::Rect;
use crate::line_edit::LineEdit;
use crate::settings;

// ---------------------------------------------------------------------------------------
// Shared types (Stage V skeleton — do not change without the coordinator)

pub const PANEL_W: f32 = 820.0;
pub const PANEL_H: f32 = 560.0;
pub const PANEL_R: f32 = 20.0;
pub const MARGIN: f32 = 24.0;
pub const WIN_W: f32 = PANEL_W + 2.0 * MARGIN;
pub const WIN_H: f32 = PANEL_H + 2.0 * MARGIN;
pub const PANEL: Rect = Rect::new(MARGIN, MARGIN, PANEL_W, PANEL_H);

/// Title bar (drag to move the window), inside `PANEL`.
pub const TITLE_H: f32 = 44.0;
/// Sidebar width, inside `PANEL`, below the title bar.
pub const SIDEBAR_W: f32 = 200.0;
/// Footer with the buttons, at the bottom of the content side.
pub const FOOTER_H: f32 = 56.0;
pub const ROW_H: f32 = 56.0;

/// A sidebar page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    General,
    Notch,
    Hotkeys,
    Terminal,
    Spotlight,
}

impl Page {
    pub const ALL: [Page; 5] = [
        Page::General,
        Page::Notch,
        Page::Hotkeys,
        Page::Terminal,
        Page::Spotlight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Notch => "Notch",
            Page::Hotkeys => "Hotkeys",
            Page::Terminal => "Terminal",
            Page::Spotlight => "Spotlight",
        }
    }
}

/// Every setting row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RowKey {
    // General
    Autostart,
    DeveloperMode,
    NotificationsEnabled,
    MusicLyrics,
    InternalNotificationsEnabled,
    Theme,
    // Notch
    Style,
    NotchWidth,
    NotchHeight,
    Reserve,
    StayBehind,
    // Hotkeys
    SnapHotkey,
    SnapAltHotkey,
    SpotlightHotkey,
    TerminalHotkey,
    // Terminal
    Shell,
    FontSize,
    HideOnBlur,
    FollowExplorer,
    Scrollback,
    AgentNotificationStatus,
    // Spotlight
    SpotlightEnabled,
    SpotlightFiles,
    SpotlightFolders,
}

/// What a row shows on its right side.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Toggle {
        on: bool,
    },
    /// A segmented choice; each segment's rect is in `RowView::parts`.
    Segmented {
        options: Vec<&'static str>,
        selected: usize,
    },
    /// A chip a click cycles. `warn` draws it in the warn colour (a hotkey taken by
    /// another app).
    Chip {
        text: String,
        warn: bool,
    },
    /// `value` within `min..=max`; the track is `RowView::parts[0]`, the thumb sits at
    /// `(value - min) / (max - min)` along it. `text` is the value as shown ("128 px").
    Slider {
        value: f32,
        min: f32,
        max: f32,
        text: String,
        dragging: bool,
    },
    /// A value that can't be changed here (shown dim).
    Fixed {
        text: String,
    },
    /// Read-only lines under the label (the Spotlight folders); the row is taller.
    Lines {
        lines: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowView {
    pub key: RowKey,
    pub label: &'static str,
    /// One short line under the label (dim).
    pub hint: &'static str,
    pub control: Control,
    /// The whole row (already scrolled; may be partly outside `ViewModel::content`).
    pub rect: Rect,
    /// The control's rect, and its parts (segments, or the slider track).
    pub control_rect: Rect,
    pub parts: Vec<Rect>,
    /// False: drawn dim and ignores input (e.g. Reserve top space in pill style).
    pub enabled: bool,
    pub hovered: bool,
    /// The label's matched char range while searching (draw it in the accent colour).
    pub matched: Option<(usize, usize)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NavItem {
    pub page: Page,
    pub rect: Rect,
    pub active: bool,
    pub hovered: bool,
    /// While searching: how many rows of this page match (0 hides the dot / dims it).
    pub matches: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Close,
    Reset,
    Apply,
    Save,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ButtonView {
    pub button: Button,
    pub label: &'static str,
    pub rect: Rect,
    pub enabled: bool,
    pub hovered: bool,
    /// Save is drawn filled (primary), the others as outlines.
    pub primary: bool,
}

/// A one-line text field (the search box; Spotlight uses the same type).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FieldView {
    pub rect: Rect,
    pub text: String,
    /// Caret position in chars.
    pub caret: usize,
    pub focused: bool,
    pub placeholder: &'static str,
}

/// Everything the Settings window draws.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewModel {
    /// The page title shown above the rows ("Notch"), or "Search" while searching.
    pub title: String,
    pub nav: Vec<NavItem>,
    pub search: FieldView,
    /// The scrolling area the rows are clipped to.
    pub content: Rect,
    pub rows: Vec<RowView>,
    /// 0..=1 scroll position and the visible fraction, for a thin scrollbar (None when
    /// everything fits).
    pub scrollbar: Option<(f32, f32)>,
    pub buttons: Vec<ButtonView>,
    /// Footer text on the left: "Unsaved changes", "Press ✕ again to discard", "Saved".
    pub status: Option<String>,
    /// The title bar rect (the app makes it a drag area).
    pub title_bar: Rect,
}

/// What the app must do after an input.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub redraw: bool,
    /// Apply this config at once (side effects, live). Every edit applies: there is no
    /// Apply button.
    pub apply: Option<crate::config::Config>,
    /// Also save it to `config.toml`. False while a slider is being dragged (a live
    /// preview); the release saves.
    pub persist: bool,
    /// Close the window.
    pub close: bool,
}

// ---------------------------------------------------------------------------------------
// The document

/// Hotkeys another app already holds (the app learns this when it registers them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Taken {
    pub snap: bool,
    pub snap_alt: bool,
    pub spotlight: bool,
}

/// Search field: height and the padding around the sidebar's contents.
pub const SEARCH_H: f32 = 32.0;
pub const SIDEBAR_PAD: f32 = 12.0;
pub const NAV_H: f32 = 34.0;
pub const NAV_GAP: f32 = 4.0;
/// The page title above the rows.
pub const PAGE_TITLE_H: f32 = 40.0;
/// Right (and left) padding inside a row.
pub const ROW_PAD: f32 = 24.0;
pub const CLOSE_SIZE: f32 = 28.0;
pub const BTN_W: f32 = 88.0;
pub const BTN_H: f32 = 32.0;
pub const BTN_GAP: f32 = 8.0;
/// One scroll line, in px.
pub const SCROLL_LINE: f32 = 40.0;
/// Height of one line in a `Lines` row.
pub const LINE_H: f32 = 18.0;

const TOGGLE_W: f32 = 44.0;
const TOGGLE_H: f32 = 24.0;
const CHIP_MAX_W: f32 = 160.0;
const CHIP_H: f32 = 28.0;
const SEG_MAX_W: f32 = 280.0;
const SEG_W: f32 = 80.0;
const SEG_H: f32 = 30.0;
const TRACK_W: f32 = 200.0;
const TRACK_H: f32 = 4.0;
const VALUE_W: f32 = 64.0;
const VALUE_GAP: f32 = 8.0;
/// A slider is grabbed this far outside its track, sideways.
const SLIDER_GRAB: f32 = 6.0;

const VK_ESCAPE: u16 = 0x1B;
const VK_UP: u16 = 0x26;
const VK_DOWN: u16 = 0x28;
const VK_F: u16 = 0x46;
#[cfg(test)]
const VK_S: u16 = 0x53;

const SHELLS: [(&str, &str); 4] = [
    ("PowerShell", "powershell"),
    ("pwsh", "pwsh"),
    ("cmd", "cmd"),
    ("WSL", "wsl"),
];

const FOLDERS: [&str; 6] = [
    "Desktop",
    "Documents",
    "Downloads",
    "Pictures",
    "Videos",
    "Music",
];

struct RowDef {
    key: RowKey,
    page: Page,
    label: &'static str,
    hint: &'static str,
}

const fn def(key: RowKey, page: Page, label: &'static str, hint: &'static str) -> RowDef {
    RowDef {
        key,
        page,
        label,
        hint,
    }
}

/// Every row, in page order.
const ROWS: [RowDef; 24] = [
    def(
        RowKey::MusicLyrics,
        Page::General,
        "Synced lyrics (LRCLIB)",
        "Off by default; enabling sends artist, title and duration to LRCLIB",
    ),
    def(
        RowKey::InternalNotificationsEnabled,
        Page::General,
        "Agent notifications",
        "Opt in to install notification hooks for new Claude/Codex tabs",
    ),
    def(
        RowKey::NotificationsEnabled,
        Page::General,
        "Windows notifications",
        "Opt in to memory-only history; permission status is in Notifications",
    ),
    def(
        RowKey::DeveloperMode,
        Page::General,
        "Developer mode",
        "Open memory diagnostics; disable to stop sampling",
    ),
    def(
        RowKey::Autostart,
        Page::General,
        "Start with Windows",
        "Launch Daijobu Island when you sign in",
    ),
    def(
        RowKey::Theme,
        Page::General,
        "Theme",
        "Colours and look of the island",
    ),
    def(
        RowKey::Style,
        Page::Notch,
        "Island style",
        "A notch hangs from the top edge, a pill floats",
    ),
    def(
        RowKey::NotchWidth,
        Page::Notch,
        "Notch width",
        "Width of the resting island",
    ),
    def(
        RowKey::NotchHeight,
        Page::Notch,
        "Notch height",
        "Height of the resting island",
    ),
    def(
        RowKey::Reserve,
        Page::Notch,
        "Reserve top space",
        "Maximized windows start below the notch",
    ),
    def(
        RowKey::StayBehind,
        Page::Notch,
        "Stay behind windows",
        "Reveal notch at top edge, except fullscreen; panels come forward",
    ),
    def(
        RowKey::SnapHotkey,
        Page::Hotkeys,
        "Snap",
        "Shortcut that starts a screenshot",
    ),
    def(
        RowKey::SnapAltHotkey,
        Page::Hotkeys,
        "Snap (alternate)",
        "A second screenshot shortcut",
    ),
    def(
        RowKey::SpotlightHotkey,
        Page::Hotkeys,
        "Spotlight",
        "Shortcut that opens the launcher",
    ),
    def(
        RowKey::TerminalHotkey,
        Page::Hotkeys,
        "Terminal",
        "Shortcut that opens the terminal",
    ),
    def(
        RowKey::Shell,
        Page::Terminal,
        "Shell",
        "The program the terminal starts",
    ),
    def(
        RowKey::FontSize,
        Page::Terminal,
        "Font size",
        "Size of the terminal text",
    ),
    def(
        RowKey::HideOnBlur,
        Page::Terminal,
        "Hide when another window is clicked",
        "The session keeps running",
    ),
    def(
        RowKey::Scrollback,
        Page::Terminal,
        "Scrollback",
        "Lines of history the terminal keeps",
    ),
    def(
        RowKey::FollowExplorer,
        Page::Terminal,
        "Start in active Explorer folder",
        "Off: last folder per profile, then configured folder or home",
    ),
    def(
        RowKey::AgentNotificationStatus,
        Page::Terminal,
        "Agent notification status",
        "New agent tabs only; existing provider permissions are preserved",
    ),
    def(
        RowKey::SpotlightEnabled,
        Page::Spotlight,
        "Spotlight",
        "A launcher for apps and files",
    ),
    def(
        RowKey::SpotlightFiles,
        Page::Spotlight,
        "Search files",
        "Also match file and folder names",
    ),
    def(
        RowKey::SpotlightFolders,
        Page::Spotlight,
        "Folders searched",
        "Fixed for now",
    ),
];

/// The height of a row (a `Lines` row grows with its lines).
fn row_height(key: RowKey) -> f32 {
    match key {
        RowKey::SpotlightFolders => ROW_H + LINE_H * FOLDERS.len() as f32,
        RowKey::AgentNotificationStatus => ROW_H + LINE_H * 4.0,
        _ => ROW_H,
    }
}

/// `(min, max, step)` of a slider row.
fn slider_spec(key: RowKey) -> Option<(f32, f32, f32)> {
    match key {
        RowKey::NotchWidth => Some((NOTCH_W_RANGE.0 as f32, NOTCH_W_RANGE.1 as f32, 2.0)),
        RowKey::NotchHeight => Some((NOTCH_H_RANGE.0 as f32, NOTCH_H_RANGE.1 as f32, 1.0)),
        RowKey::FontSize => Some((
            *TERMINAL_FONT_SIZE_RANGE.start(),
            *TERMINAL_FONT_SIZE_RANGE.end(),
            1.0,
        )),
        RowKey::Scrollback => Some((
            *TERMINAL_SCROLLBACK_RANGE.start() as f32,
            *TERMINAL_SCROLLBACK_RANGE.end() as f32,
            1000.0,
        )),
        _ => None,
    }
}

fn slider_value(c: &Config, key: RowKey) -> f32 {
    match key {
        RowKey::NotchWidth => c.island.notch_size().0,
        RowKey::NotchHeight => c.island.notch_size().1,
        RowKey::FontSize => c.terminal.font_size.clamp(
            *TERMINAL_FONT_SIZE_RANGE.start(),
            *TERMINAL_FONT_SIZE_RANGE.end(),
        ),
        RowKey::Scrollback => c.terminal.scrollback.clamp(
            *TERMINAL_SCROLLBACK_RANGE.start(),
            *TERMINAL_SCROLLBACK_RANGE.end(),
        ) as f32,
        _ => 0.0,
    }
}

fn set_slider_value(c: &mut Config, key: RowKey, v: f32) {
    match key {
        RowKey::NotchWidth => c.island.notch_width = v.round() as u32,
        RowKey::NotchHeight => c.island.notch_height = v.round() as u32,
        RowKey::FontSize => c.terminal.font_size = v,
        RowKey::Scrollback => c.terminal.scrollback = v.round() as u32,
        _ => {}
    }
}

fn slider_text(key: RowKey, v: f32) -> String {
    let n = v.round() as i64;
    if key == RowKey::Scrollback {
        format!("{n} lines")
    } else {
        format!("{n} px")
    }
}

/// Whether row `key`'s setting is the same in `a` and `b`.
fn field_eq(a: &Config, b: &Config, key: RowKey) -> bool {
    match key {
        RowKey::InternalNotificationsEnabled => {
            a.notifications.internal_enabled == b.notifications.internal_enabled
        }
        RowKey::NotificationsEnabled => a.notifications.enabled == b.notifications.enabled,
        RowKey::MusicLyrics => a.music.lyrics_enabled == b.music.lyrics_enabled,
        RowKey::DeveloperMode => a.developer_mode == b.developer_mode,
        RowKey::Autostart => a.start_with_windows == b.start_with_windows,
        RowKey::Theme => a.theme == b.theme,
        RowKey::Style => a.island.style == b.island.style,
        RowKey::NotchWidth => a.island.notch_width == b.island.notch_width,
        RowKey::NotchHeight => a.island.notch_height == b.island.notch_height,
        RowKey::Reserve => a.island.reserve_top == b.island.reserve_top,
        RowKey::StayBehind => a.island.stay_behind == b.island.stay_behind,
        RowKey::SnapHotkey => a.snap.hotkey == b.snap.hotkey,
        RowKey::SnapAltHotkey => a.snap.alt_hotkey == b.snap.alt_hotkey,
        RowKey::SpotlightHotkey => a.spotlight.hotkey == b.spotlight.hotkey,
        RowKey::TerminalHotkey | RowKey::SpotlightFolders | RowKey::AgentNotificationStatus => true,
        RowKey::Shell => a.terminal.shell == b.terminal.shell,
        RowKey::FontSize => a.terminal.font_size == b.terminal.font_size,
        RowKey::HideOnBlur => a.terminal.hide_on_blur == b.terminal.hide_on_blur,
        RowKey::FollowExplorer => a.terminal.follow_explorer == b.terminal.follow_explorer,
        RowKey::Scrollback => a.terminal.scrollback == b.terminal.scrollback,
        RowKey::SpotlightEnabled => a.spotlight.enabled == b.spotlight.enabled,
        RowKey::SpotlightFiles => a.spotlight.files == b.spotlight.files,
    }
}

/// Copies row `key`'s setting from `src` into `dst`.
fn copy_field(dst: &mut Config, src: &Config, key: RowKey) {
    match key {
        RowKey::InternalNotificationsEnabled => {
            dst.notifications.internal_enabled = src.notifications.internal_enabled
        }
        RowKey::NotificationsEnabled => dst.notifications.enabled = src.notifications.enabled,
        RowKey::MusicLyrics => dst.music.lyrics_enabled = src.music.lyrics_enabled,
        RowKey::DeveloperMode => dst.developer_mode = src.developer_mode,
        RowKey::Autostart => dst.start_with_windows = src.start_with_windows,
        RowKey::Theme => dst.theme = src.theme.clone(),
        RowKey::Style => dst.island.style = src.island.style.clone(),
        RowKey::NotchWidth => dst.island.notch_width = src.island.notch_width,
        RowKey::NotchHeight => dst.island.notch_height = src.island.notch_height,
        RowKey::Reserve => dst.island.reserve_top = src.island.reserve_top,
        RowKey::StayBehind => dst.island.stay_behind = src.island.stay_behind,
        RowKey::SnapHotkey => dst.snap.hotkey = src.snap.hotkey.clone(),
        RowKey::SnapAltHotkey => dst.snap.alt_hotkey = src.snap.alt_hotkey.clone(),
        RowKey::SpotlightHotkey => dst.spotlight.hotkey = src.spotlight.hotkey.clone(),
        RowKey::TerminalHotkey | RowKey::SpotlightFolders | RowKey::AgentNotificationStatus => {}
        RowKey::Shell => dst.terminal.shell = src.terminal.shell.clone(),
        RowKey::FontSize => dst.terminal.font_size = src.terminal.font_size,
        RowKey::HideOnBlur => dst.terminal.hide_on_blur = src.terminal.hide_on_blur,
        RowKey::FollowExplorer => dst.terminal.follow_explorer = src.terminal.follow_explorer,
        RowKey::Scrollback => dst.terminal.scrollback = src.terminal.scrollback,
        RowKey::SpotlightEnabled => dst.spotlight.enabled = src.spotlight.enabled,
        RowKey::SpotlightFiles => dst.spotlight.files = src.spotlight.files,
    }
}

/// Where a control sits in a row, and its parts (segments, the slider track).
fn place(control: &Control, row: Rect) -> (Rect, Vec<Rect>) {
    let right = row.x + row.w - ROW_PAD;
    let mid = row.y + ROW_H / 2.0;
    match control {
        Control::Toggle { .. } => (
            Rect::new(right - TOGGLE_W, mid - TOGGLE_H / 2.0, TOGGLE_W, TOGGLE_H),
            Vec::new(),
        ),
        Control::Chip { text, .. } => {
            let w = (text.chars().count() as f32 * 8.0 + 24.0).clamp(64.0, CHIP_MAX_W);
            (
                Rect::new(right - w, mid - CHIP_H / 2.0, w, CHIP_H),
                Vec::new(),
            )
        }
        Control::Fixed { .. } => (
            Rect::new(right - CHIP_MAX_W, mid - CHIP_H / 2.0, CHIP_MAX_W, CHIP_H),
            Vec::new(),
        ),
        Control::Segmented { options, .. } => {
            let n = options.len().max(1);
            let w = (n as f32 * SEG_W).min(SEG_MAX_W);
            let rect = Rect::new(right - w, mid - SEG_H / 2.0, w, SEG_H);
            let seg = w / n as f32;
            let parts = (0..n)
                .map(|i| Rect::new(rect.x + i as f32 * seg, rect.y, seg, SEG_H))
                .collect();
            (rect, parts)
        }
        Control::Slider { .. } => {
            let track = Rect::new(
                right - VALUE_W - VALUE_GAP - TRACK_W,
                mid - TRACK_H / 2.0,
                TRACK_W,
                TRACK_H,
            );
            let whole = Rect::new(
                track.x,
                mid - TOGGLE_H / 2.0,
                TRACK_W + VALUE_GAP + VALUE_W,
                TOGGLE_H,
            );
            (whole, vec![track])
        }
        Control::Lines { lines } => (
            Rect::new(
                row.x + ROW_PAD,
                row.y + ROW_H - 6.0,
                row.w - 2.0 * ROW_PAD,
                LINE_H * lines.len() as f32,
            ),
            Vec::new(),
        ),
    }
}

fn lower_chars(s: &str) -> Vec<char> {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

fn find_chars(hay: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

/// What the pointer is over.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    None,
    Nav(Page),
    Search,
    Button(Button),
    /// A row's control; the part is the segment index (0 for the others).
    Control(RowKey, usize),
}

/// The hovered thing that is drawn differently.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hover {
    None,
    Nav(Page),
    Row(RowKey),
    Button(Button),
}

/// The scroll slot used while searching.
const SEARCH_SLOT: usize = 5;

/// The Settings window's state: the saved and the pending config, the page, the search,
/// scroll, and the pointer. Pure; see the module docs.
#[derive(Clone, Debug)]
pub struct SettingsDoc {
    saved: Config,
    pending: Config,
    /// `(id, name)` of the loaded themes, in the order the chip cycles.
    themes: Vec<(String, String)>,
    taken: Taken,
    page: Page,
    search: LineEdit,
    search_focused: bool,
    /// Scroll offset in px per page, and one more for the search results.
    scroll: [f32; 6],
    hover: Hover,
    /// The slider being dragged.
    drag: Option<RowKey>,
    /// A button held down; it acts on release when the pointer is still over it.
    pressed: Option<Button>,
    /// A close was asked with unsaved changes; the next one discards them.
    armed: bool,
    status: Option<String>,
    agent_notification_status: Vec<String>,
}

impl SettingsDoc {
    pub fn new(config: &Config, themes: Vec<(String, String)>, taken: Taken) -> Self {
        Self {
            saved: config.clone(),
            pending: config.clone(),
            themes,
            taken,
            page: Page::General,
            search: LineEdit::new(),
            search_focused: false,
            scroll: [0.0; 6],
            hover: Hover::None,
            drag: None,
            pressed: None,
            armed: false,
            status: None,
            agent_notification_status: vec!["Agent notifications are off".into()],
        }
    }

    pub fn set_taken(&mut self, taken: Taken) {
        self.taken = taken;
    }

    pub fn set_agent_notification_status(&mut self, status: Vec<String>) {
        self.agent_notification_status = status
            .into_iter()
            .take(4)
            .map(|s| s.chars().filter(|c| !c.is_control()).take(100).collect())
            .collect();
    }

    pub fn pending(&self) -> &Config {
        &self.pending
    }

    pub fn saved(&self) -> &Config {
        &self.saved
    }

    pub fn is_dirty(&self) -> bool {
        self.pending != self.saved
    }

    pub fn page(&self) -> Page {
        self.page
    }

    /// Shows `page` and leaves the search.
    pub fn set_page(&mut self, page: Page) {
        self.page = page;
        self.search.clear();
        self.search_focused = false;
    }

    /// The title bar, which the app makes a drag area (the Close button is not part of
    /// the drag; `press` reports it).
    pub fn title_bar(&self) -> Rect {
        Rect::new(PANEL.x, PANEL.y, PANEL_W, TITLE_H)
    }

    /// The app changed the config elsewhere (a theme hotkey, say). Takes it as the saved
    /// config; rows the user has not edited follow it, edited rows keep their pending value.
    pub fn config_changed(&mut self, config: &Config) {
        let mut next = config.clone();
        for d in &ROWS {
            if !field_eq(&self.pending, &self.saved, d.key) {
                copy_field(&mut next, &self.pending, d.key);
            }
        }
        self.saved = config.clone();
        self.pending = next;
    }

    // -- layout --------------------------------------------------------------------

    fn sidebar() -> Rect {
        Rect::new(PANEL.x, PANEL.y + TITLE_H, SIDEBAR_W, PANEL_H - TITLE_H)
    }

    fn search_rect() -> Rect {
        let s = Self::sidebar();
        Rect::new(
            s.x + SIDEBAR_PAD,
            s.y + SIDEBAR_PAD,
            SIDEBAR_W - 2.0 * SIDEBAR_PAD,
            SEARCH_H,
        )
    }

    fn nav_rect(i: usize) -> Rect {
        let s = Self::search_rect();
        Rect::new(
            s.x,
            s.y + SEARCH_H + SIDEBAR_PAD + i as f32 * (NAV_H + NAV_GAP),
            s.w,
            NAV_H,
        )
    }

    fn close_rect() -> Rect {
        Rect::new(
            PANEL.x + PANEL_W - SIDEBAR_PAD - CLOSE_SIZE,
            PANEL.y + (TITLE_H - CLOSE_SIZE) / 2.0,
            CLOSE_SIZE,
            CLOSE_SIZE,
        )
    }

    fn footer() -> Rect {
        Rect::new(
            PANEL.x + SIDEBAR_W,
            PANEL.y + PANEL_H - FOOTER_H,
            PANEL_W - SIDEBAR_W,
            FOOTER_H,
        )
    }

    /// The rows' clip area: right of the sidebar, below the page title, above the footer.
    fn content() -> Rect {
        let top = PANEL.y + TITLE_H + PAGE_TITLE_H;
        Rect::new(
            PANEL.x + SIDEBAR_W,
            top,
            PANEL_W - SIDEBAR_W,
            Self::footer().y - top,
        )
    }

    fn button_rect(b: Button) -> Rect {
        if b == Button::Close {
            return Self::close_rect();
        }
        let f = Self::footer();
        // Only Reset is shown (every edit applies at once), flush with the right padding.
        // Apply and Save keep a rect for the shared type but are never shown.
        let slot = match b {
            Button::Reset => 2.0,
            Button::Apply => 1.0,
            _ => 0.0,
        };
        let right = f.x + f.w - ROW_PAD;
        let x = right - BTN_W - (2.0 - slot) * (BTN_W + BTN_GAP);
        Rect::new(x, f.y + (FOOTER_H - BTN_H) / 2.0, BTN_W, BTN_H)
    }

    // -- rows ----------------------------------------------------------------------

    fn searching(&self) -> bool {
        !self.search.text().trim().is_empty()
    }

    fn query_chars(&self) -> Vec<char> {
        lower_chars(self.search.text().trim())
    }

    /// The rows shown now with the label's matched char range: the page's rows, or while
    /// searching every row whose label or hint holds the text, in page order.
    fn visible(&self) -> Vec<(&'static RowDef, Option<(usize, usize)>)> {
        if !self.searching() {
            return ROWS
                .iter()
                .filter(|d| d.page == self.page)
                .map(|d| (d, None))
                .collect();
        }
        let q = self.query_chars();
        ROWS.iter()
            .filter_map(|d| {
                let in_label = find_chars(&lower_chars(d.label), &q);
                let in_hint = find_chars(&lower_chars(d.hint), &q);
                (in_label.is_some() || in_hint.is_some())
                    .then_some((d, in_label.map(|s| (s, s + q.len()))))
            })
            .collect()
    }

    fn slot(&self) -> usize {
        if self.searching() {
            SEARCH_SLOT
        } else {
            Page::ALL.iter().position(|p| *p == self.page).unwrap_or(0)
        }
    }

    fn content_height(&self) -> f32 {
        self.visible().iter().map(|(d, _)| row_height(d.key)).sum()
    }

    fn max_scroll(&self) -> f32 {
        (self.content_height() - Self::content().h).max(0.0)
    }

    fn scroll_now(&self) -> f32 {
        self.scroll[self.slot()].clamp(0.0, self.max_scroll())
    }

    fn enabled(&self, key: RowKey) -> bool {
        let i = &self.pending.island;
        match key {
            RowKey::Reserve => i.style() == IslandStyle::Notch && !i.stay_behind,
            RowKey::SpotlightFiles => self.pending.spotlight.enabled,
            _ => true,
        }
    }

    fn spotlight_chip(&self) -> SpotlightHotkey {
        SpotlightHotkey::parse(&self.pending.spotlight.hotkey).unwrap_or_default()
    }

    fn control(&self, key: RowKey) -> Control {
        let c = &self.pending;
        match key {
            RowKey::InternalNotificationsEnabled => Control::Toggle {
                on: c.notifications.internal_enabled,
            },
            RowKey::MusicLyrics => Control::Toggle {
                on: c.music.lyrics_enabled,
            },
            RowKey::NotificationsEnabled => Control::Toggle {
                on: c.notifications.enabled,
            },
            RowKey::DeveloperMode => Control::Toggle {
                on: c.developer_mode,
            },
            RowKey::Autostart => Control::Toggle {
                on: c.start_with_windows,
            },
            RowKey::Theme => Control::Chip {
                text: self
                    .themes
                    .iter()
                    .find(|t| t.0 == c.theme)
                    .map_or_else(|| c.theme.clone(), |t| t.1.clone()),
                warn: false,
            },
            RowKey::Style => Control::Segmented {
                options: vec!["Notch", "Pill"],
                selected: usize::from(c.island.style() == IslandStyle::Pill),
            },
            RowKey::NotchWidth | RowKey::NotchHeight | RowKey::FontSize | RowKey::Scrollback => {
                let (min, max, _) = slider_spec(key).unwrap_or((0.0, 1.0, 1.0));
                let value = slider_value(c, key);
                Control::Slider {
                    value,
                    min,
                    max,
                    text: slider_text(key, value),
                    dragging: self.drag == Some(key),
                }
            }
            RowKey::Reserve => Control::Toggle {
                on: c.island.reserve_top,
            },
            RowKey::StayBehind => Control::Toggle {
                on: c.island.stay_behind,
            },
            RowKey::SnapHotkey => Control::Chip {
                text: c.snap.hotkey().label().to_string(),
                warn: self.taken.snap,
            },
            RowKey::SnapAltHotkey => Control::Chip {
                text: settings::alt_label(c.snap.alt_hotkey()).to_string(),
                warn: self.taken.snap_alt && c.snap.alt_hotkey().is_some(),
            },
            RowKey::SpotlightHotkey => {
                let h = self.spotlight_chip();
                Control::Chip {
                    text: h.label().to_string(),
                    warn: self.taken.spotlight && h != SpotlightHotkey::Off,
                }
            }
            RowKey::TerminalHotkey => Control::Fixed {
                text: "Ctrl+Alt+`".to_string(),
            },
            RowKey::Shell => {
                let shell = c.terminal.shell.trim().to_ascii_lowercase();
                Control::Segmented {
                    options: SHELLS.iter().map(|s| s.0).collect(),
                    selected: SHELLS.iter().position(|s| s.1 == shell).unwrap_or(0),
                }
            }
            RowKey::HideOnBlur => Control::Toggle {
                on: c.terminal.hide_on_blur,
            },
            RowKey::FollowExplorer => Control::Toggle {
                on: c.terminal.follow_explorer,
            },
            RowKey::SpotlightEnabled => Control::Toggle {
                on: c.spotlight.enabled,
            },
            RowKey::SpotlightFiles => Control::Toggle {
                on: c.spotlight.files,
            },
            RowKey::SpotlightFolders => Control::Lines {
                lines: FOLDERS.iter().map(|s| s.to_string()).collect(),
            },
            RowKey::AgentNotificationStatus => Control::Lines {
                lines: self.agent_notification_status.clone(),
            },
        }
    }

    fn rows(&self) -> Vec<RowView> {
        let content = Self::content();
        let mut y = content.y - self.scroll_now();
        let mut out = Vec::new();
        for (d, matched) in self.visible() {
            let h = row_height(d.key);
            let rect = Rect::new(content.x, y, content.w, h);
            let control = self.control(d.key);
            let (control_rect, parts) = place(&control, rect);
            let enabled = self.enabled(d.key);
            let in_view = rect.y < content.y + content.h && rect.y + h > content.y;
            out.push(RowView {
                key: d.key,
                label: d.label,
                hint: d.hint,
                control,
                rect,
                control_rect,
                parts,
                enabled,
                hovered: enabled && in_view && self.hover == Hover::Row(d.key),
                matched,
            });
            y += h;
        }
        out
    }

    // -- hit testing ---------------------------------------------------------------

    fn button_enabled(&self, b: Button) -> bool {
        match b {
            Button::Close => true,
            Button::Apply | Button::Save => false,
            Button::Reset => !self.page_at_defaults(),
        }
    }

    /// True when every row shown is at its default (Reset has nothing to do).
    fn page_at_defaults(&self) -> bool {
        let d = Config::default();
        self.visible()
            .iter()
            .all(|(r, _)| field_eq(&self.pending, &d, r.key))
    }

    fn hit(&self, x: f32, y: f32) -> Hit {
        for b in [Button::Close, Button::Reset, Button::Apply, Button::Save] {
            if Self::button_rect(b).contains(x, y) {
                return Hit::Button(b);
            }
        }
        for (i, p) in Page::ALL.iter().enumerate() {
            if Self::nav_rect(i).contains(x, y) {
                return Hit::Nav(*p);
            }
        }
        if Self::search_rect().contains(x, y) {
            return Hit::Search;
        }
        if !Self::content().contains(x, y) {
            return Hit::None;
        }
        for row in self.rows() {
            if !row.rect.contains(x, y) {
                continue;
            }
            return match &row.control {
                Control::Segmented { .. } => row
                    .parts
                    .iter()
                    .position(|p| p.contains(x, y))
                    .map_or(Hit::None, |i| Hit::Control(row.key, i)),
                Control::Slider { .. } => {
                    let r = row.control_rect;
                    let grab = Rect::new(r.x - SLIDER_GRAB, r.y, r.w + 2.0 * SLIDER_GRAB, r.h);
                    if grab.contains(x, y) {
                        Hit::Control(row.key, 0)
                    } else {
                        Hit::None
                    }
                }
                Control::Fixed { .. } | Control::Lines { .. } => Hit::None,
                _ => {
                    if row.control_rect.contains(x, y) {
                        Hit::Control(row.key, 0)
                    } else {
                        Hit::None
                    }
                }
            };
        }
        Hit::None
    }

    fn hover_target(&self, x: f32, y: f32) -> Hover {
        match self.hit(x, y) {
            Hit::Nav(p) => Hover::Nav(p),
            Hit::Button(b) if self.button_enabled(b) => Hover::Button(b),
            Hit::Control(k, _) if self.enabled(k) => Hover::Row(k),
            Hit::None | Hit::Search | Hit::Button(_) | Hit::Control(..) => {
                // Anywhere on an enabled row highlights it.
                if Self::content().contains(x, y) {
                    if let Some(r) = self
                        .rows()
                        .iter()
                        .find(|r| r.rect.contains(x, y) && r.enabled)
                    {
                        return Hover::Row(r.key);
                    }
                }
                Hover::None
            }
        }
    }

    // -- editing -------------------------------------------------------------------

    /// Runs `f` on the pending config. Any real change is an edit: it disarms a pending
    /// discard and drops the "Saved" note. Returns whether anything changed.
    fn mutate(&mut self, f: impl FnOnce(&mut Config)) -> bool {
        let before = self.pending.clone();
        f(&mut self.pending);
        let changed = self.pending != before;
        if changed {
            self.armed = false;
            self.status = None;
        }
        changed
    }

    fn set_slider_from_x(&mut self, key: RowKey, x: f32) -> bool {
        let Some((min, max, step)) = slider_spec(key) else {
            return false;
        };
        let row = self.rows().into_iter().find(|r| r.key == key);
        let Some(track) = row.and_then(|r| r.parts.first().copied()) else {
            return false;
        };
        let t = ((x - track.x) / track.w).clamp(0.0, 1.0);
        let v = (min + ((t * (max - min)) / step).round() * step).clamp(min, max);
        self.mutate(|c| set_slider_value(c, key, v))
    }

    fn click_control(&mut self, key: RowKey, part: usize) -> bool {
        let themes = &self.themes;
        let next_theme = themes
            .iter()
            .position(|t| t.0 == self.pending.theme)
            .map_or(0, |i| (i + 1) % themes.len().max(1));
        let theme_id = themes.get(next_theme).map(|t| t.0.clone());
        let next_spot = self.spotlight_chip().next();
        self.mutate(|c| match key {
            RowKey::InternalNotificationsEnabled => {
                c.notifications.internal_enabled = !c.notifications.internal_enabled
            }
            RowKey::NotificationsEnabled => c.notifications.enabled = !c.notifications.enabled,
            RowKey::MusicLyrics => c.music.lyrics_enabled = !c.music.lyrics_enabled,
            RowKey::DeveloperMode => c.developer_mode = !c.developer_mode,
            RowKey::Autostart => c.start_with_windows = !c.start_with_windows,
            RowKey::Theme => {
                if let Some(id) = theme_id {
                    c.theme = id;
                }
            }
            RowKey::Style => {
                c.island.style = if part == 0 {
                    IslandStyle::Notch
                } else {
                    IslandStyle::Pill
                }
                .label()
                .to_string();
            }
            RowKey::Reserve => c.island.reserve_top = !c.island.reserve_top,
            RowKey::StayBehind => c.island.stay_behind = !c.island.stay_behind,
            RowKey::SnapHotkey => settings::cycle_hotkey(&mut c.snap),
            RowKey::SnapAltHotkey => settings::cycle_alt_hotkey(&mut c.snap),
            RowKey::SpotlightHotkey => c.spotlight.hotkey = next_spot.key().to_string(),
            RowKey::Shell => {
                if let Some(s) = SHELLS.get(part) {
                    c.terminal.shell = s.1.to_string();
                }
            }
            RowKey::HideOnBlur => c.terminal.hide_on_blur = !c.terminal.hide_on_blur,
            RowKey::FollowExplorer => c.terminal.follow_explorer = !c.terminal.follow_explorer,
            RowKey::SpotlightEnabled => c.spotlight.enabled = !c.spotlight.enabled,
            RowKey::SpotlightFiles => c.spotlight.files = !c.spotlight.files,
            _ => {}
        })
    }

    fn reset(&mut self) -> bool {
        let keys: Vec<RowKey> = self.visible().iter().map(|(d, _)| d.key).collect();
        let defaults = Config::default();
        self.mutate(|c| {
            for k in keys {
                copy_field(c, &defaults, k);
            }
            // The two Snap keys can't be the same.
            if c.snap.alt_hotkey() == Some(c.snap.hotkey()) {
                c.snap.alt_hotkey = settings::ALT_OFF.to_string();
            }
        })
    }

    /// An edit took: hand the pending config to the app to apply now. `persist` saves it
    /// (and makes it the saved config); a slider being dragged only previews.
    fn commit(&mut self, persist: bool) -> Outcome {
        if persist {
            self.saved = self.pending.clone();
            self.status = Some("Saved".to_string());
        }
        Outcome {
            redraw: true,
            apply: Some(self.pending.clone()),
            persist,
            close: false,
        }
    }

    /// Close or Esc. Everything is applied and saved already, so it just closes (a
    /// slider still being dragged is saved first).
    fn close_request(&mut self) -> Outcome {
        let mut out = if self.is_dirty() {
            self.commit(true)
        } else {
            redraw_if(true)
        };
        out.close = true;
        out
    }

    fn run_button(&mut self, b: Button) -> Outcome {
        if !self.button_enabled(b) {
            return Outcome::default();
        }
        match b {
            Button::Close => self.close_request(),
            Button::Apply | Button::Save => Outcome::default(),
            Button::Reset if self.reset() => self.commit(true),
            Button::Reset => Outcome::default(),
        }
    }

    fn search_changed(&mut self) {
        self.scroll[SEARCH_SLOT] = 0.0;
        self.hover = Hover::None;
    }

    // -- input ---------------------------------------------------------------------

    /// The pointer moved (no button down). Redraws only when the highlight changed.
    pub fn hover(&mut self, x: f32, y: f32) -> Outcome {
        let h = self.hover_target(x, y);
        if h == self.hover {
            return Outcome::default();
        }
        self.hover = h;
        redraw_if(true)
    }

    pub fn leave(&mut self) -> Outcome {
        let had = self.hover != Hover::None;
        self.hover = Hover::None;
        redraw_if(had)
    }

    /// A left press. Rows act at once (a slider also starts a drag); a button acts on
    /// `release`.
    pub fn press(&mut self, x: f32, y: f32) -> Outcome {
        let hit = self.hit(x, y);
        let was_focused = self.search_focused;
        self.search_focused = hit == Hit::Search;
        let mut redraw = was_focused != self.search_focused;
        match hit {
            Hit::Nav(p) => {
                let changed = self.page != p || self.searching();
                self.set_page(p);
                redraw |= changed;
            }
            Hit::Button(b) => {
                if self.button_enabled(b) {
                    self.pressed = Some(b);
                }
            }
            Hit::Control(key, part) if self.enabled(key) => {
                if slider_spec(key).is_some() {
                    // A live preview from the first press; the release saves.
                    self.drag = Some(key);
                    if self.set_slider_from_x(key, x) {
                        return self.commit(false);
                    }
                    redraw = true;
                } else if self.click_control(key, part) {
                    return self.commit(true);
                }
            }
            _ => {}
        }
        redraw_if(redraw)
    }

    /// The pointer moved with the button down: moves a slider being dragged, which
    /// previews live (applied, not saved yet).
    pub fn drag(&mut self, x: f32, _y: f32) -> Outcome {
        let Some(key) = self.drag else {
            return Outcome::default();
        };
        if self.set_slider_from_x(key, x) {
            self.commit(false)
        } else {
            Outcome::default()
        }
    }

    /// The button went up: ends a drag (saving what it previewed), and runs a button that
    /// is still under the pointer.
    pub fn release(&mut self, x: f32, y: f32) -> Outcome {
        let mut out = Outcome::default();
        if self.drag.take().is_some() {
            out = if self.is_dirty() {
                self.commit(true)
            } else {
                redraw_if(true)
            };
        }
        if let Some(b) = self.pressed.take() {
            if Self::button_rect(b).contains(x, y) {
                let run = self.run_button(b);
                out.redraw |= run.redraw;
                if run.apply.is_some() {
                    out.apply = run.apply;
                    out.persist = run.persist;
                }
                out.close = run.close;
            }
        }
        out
    }

    /// The wheel: positive `lines` scroll down (to later rows), `SCROLL_LINE` px each.
    pub fn wheel(&mut self, lines: f32) -> Outcome {
        let slot = self.slot();
        let max = self.max_scroll();
        let now = self.scroll[slot].clamp(0.0, max);
        let next = (now + lines * SCROLL_LINE).clamp(0.0, max);
        self.scroll[slot] = next;
        if next != now {
            self.hover = Hover::None;
        }
        redraw_if(next != now)
    }

    /// A key down (`vk` is a Windows virtual-key code).
    pub fn key(&mut self, vk: u16, ctrl: bool, shift: bool) -> Outcome {
        if ctrl && vk == VK_F {
            let changed = !self.search_focused;
            self.search_focused = true;
            return redraw_if(changed);
        }
        match vk {
            VK_ESCAPE => {
                if !self.search.is_empty() {
                    self.search.clear();
                    self.search_changed();
                    return redraw_if(true);
                }
                return self.close_request();
            }
            VK_UP | VK_DOWN if !self.searching() => {
                let i = Page::ALL.iter().position(|p| *p == self.page).unwrap_or(0);
                let j = if vk == VK_UP {
                    i.saturating_sub(1)
                } else {
                    (i + 1).min(Page::ALL.len() - 1)
                };
                let changed = i != j;
                self.page = Page::ALL[j];
                return redraw_if(changed);
            }
            _ => {}
        }
        if self.search_focused && self.search.key(vk, ctrl, shift) {
            self.search_changed();
            return redraw_if(true);
        }
        Outcome::default()
    }

    /// A typed char: it goes to the search field, which it focuses.
    pub fn char(&mut self, c: char) -> Outcome {
        if c.is_control() {
            return Outcome::default();
        }
        self.search_focused = true;
        let changed = self.search.insert_char(c);
        if changed {
            self.search_changed();
        }
        redraw_if(true)
    }

    // -- view ----------------------------------------------------------------------

    pub fn view(&self) -> ViewModel {
        let searching = self.searching();
        let rows = self.rows();
        let nav = Page::ALL
            .iter()
            .enumerate()
            .map(|(i, p)| NavItem {
                page: *p,
                rect: Self::nav_rect(i),
                active: !searching && *p == self.page,
                hovered: self.hover == Hover::Nav(*p),
                matches: searching.then(|| self.matches_on(*p)),
            })
            .collect();
        let max = self.max_scroll();
        let content_h = self.content_height();
        let scrollbar = (max > 0.0).then(|| {
            (
                (self.scroll_now() / max).clamp(0.0, 1.0),
                (Self::content().h / content_h).clamp(0.0, 1.0),
            )
        });
        let buttons = [Button::Close, Button::Reset]
            .into_iter()
            .map(|b| ButtonView {
                button: b,
                label: match b {
                    Button::Close => "\u{2715}",
                    Button::Reset => "Reset",
                    Button::Apply => "Apply",
                    Button::Save => "Save",
                },
                rect: Self::button_rect(b),
                enabled: self.button_enabled(b),
                hovered: self.hover == Hover::Button(b),
                primary: b == Button::Save,
            })
            .collect();
        // Every change applies and saves at once; the footer says so until one has.
        let status = self
            .status
            .clone()
            .or_else(|| Some("Changes apply instantly".to_string()));
        ViewModel {
            title: if searching {
                "Search".to_string()
            } else {
                self.page.label().to_string()
            },
            nav,
            search: self
                .search
                .view(Self::search_rect(), self.search_focused, "Search settings"),
            content: Self::content(),
            rows,
            scrollbar,
            buttons,
            status,
            title_bar: self.title_bar(),
        }
    }

    /// How many rows of `page` match the search.
    fn matches_on(&self, page: Page) -> usize {
        self.visible()
            .iter()
            .filter(|(d, _)| d.page == page)
            .count()
    }
}

fn redraw_if(redraw: bool) -> Outcome {
    Outcome {
        redraw,
        ..Outcome::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_setup_status_is_read_only_and_does_not_change_config() {
        let mut d = doc();
        let before = d.pending().clone();
        d.set_agent_notification_status(vec![
            "Codex question alerts unavailable".into(),
            "x\ncontrol".into(),
            "x".repeat(200),
            "four".into(),
            "ignored".into(),
        ]);
        d.set_page(Page::Terminal);
        let status = row(&mut d, RowKey::AgentNotificationStatus);
        let Control::Lines { lines } = status.control else {
            panic!("expected status lines")
        };
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[2].chars().count(), 100);
        assert!(!lines[1].contains('\n'));
        assert!(!d.click_control(RowKey::AgentNotificationStatus, 0));
        assert_eq!(d.pending(), &before);
        assert!(!d.is_dirty());
    }

    fn themes() -> Vec<(String, String)> {
        vec![
            ("a".to_string(), "Alpha".to_string()),
            ("b".to_string(), "Beta".to_string()),
        ]
    }

    fn cfg() -> Config {
        Config {
            theme: "a".to_string(),
            ..Config::default()
        }
    }

    fn doc() -> SettingsDoc {
        SettingsDoc::new(&cfg(), themes(), Taken::default())
    }

    fn custom() -> Config {
        let mut c = cfg();
        c.start_with_windows = true;
        c.theme = "b".into();
        c.island.style = "pill".into();
        c.island.notch_width = 160;
        c.island.notch_height = 36;
        c.island.reserve_top = false;
        c.island.stay_behind = true;
        c.snap.hotkey = "Ctrl+Space".into();
        c.snap.alt_hotkey = "none".into();
        c.spotlight.hotkey = "ctrl_alt_space".into();
        c.spotlight.enabled = false;
        c.spotlight.files = false;
        c.terminal.shell = "wsl".into();
        c.terminal.font_size = 14.0;
        c.terminal.hide_on_blur = false;
        c.terminal.scrollback = 10_000;
        c
    }

    fn page_of(key: RowKey) -> Page {
        ROWS.iter().find(|d| d.key == key).unwrap().page
    }

    /// The row's view, on its own page.
    fn row(d: &mut SettingsDoc, key: RowKey) -> RowView {
        if d.page() != page_of(key) || d.searching() {
            d.set_page(page_of(key));
        }
        d.view().rows.into_iter().find(|r| r.key == key).unwrap()
    }

    fn ctl(d: &mut SettingsDoc, key: RowKey) -> Control {
        row(d, key).control
    }

    fn mid(r: Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    fn click_at(d: &mut SettingsDoc, x: f32, y: f32) -> Outcome {
        let p = d.press(x, y);
        let r = d.release(x, y);
        Outcome {
            redraw: p.redraw || r.redraw,
            persist: r.persist || p.persist,
            apply: r.apply.or(p.apply),
            close: r.close || p.close,
        }
    }

    #[test]
    fn developer_mode_defaults_off_and_requires_explicit_toggle() {
        let mut d = doc();
        assert!(!d.pending().developer_mode);
        let out = click_row(&mut d, RowKey::DeveloperMode);
        assert!(out.persist);
        assert!(out.apply.unwrap().developer_mode);
        assert!(click_row(&mut d, RowKey::DeveloperMode).persist);
        assert!(!d.pending().developer_mode);
    }

    fn click_row(d: &mut SettingsDoc, key: RowKey) -> Outcome {
        let (x, y) = mid(row(d, key).control_rect);
        click_at(d, x, y)
    }

    fn click_seg(d: &mut SettingsDoc, key: RowKey, i: usize) -> Outcome {
        let (x, y) = mid(row(d, key).parts[i]);
        click_at(d, x, y)
    }

    fn click_button(d: &mut SettingsDoc, b: Button) -> Outcome {
        let (x, y) = mid(SettingsDoc::button_rect(b));
        click_at(d, x, y)
    }

    fn type_text(d: &mut SettingsDoc, s: &str) {
        for c in s.chars() {
            d.char(c);
        }
    }

    fn track_x(d: &mut SettingsDoc, key: RowKey, t: f32) -> f32 {
        let tr = row(d, key).parts[0];
        tr.x + t * tr.w
    }

    fn slider_value(d: &mut SettingsDoc, key: RowKey) -> f32 {
        match ctl(d, key) {
            Control::Slider { value, .. } => value,
            other => panic!("{other:?}"),
        }
    }

    // -- rows from a config ------------------------------------------------------------

    #[test]
    fn every_row_shows_the_config() {
        let mut d = SettingsDoc::new(&custom(), themes(), Taken::default());
        let t = |on| Control::Toggle { on };
        assert_eq!(ctl(&mut d, RowKey::Autostart), t(true));
        assert_eq!(
            ctl(&mut d, RowKey::Theme),
            Control::Chip {
                text: "Beta".into(),
                warn: false
            }
        );
        assert_eq!(
            ctl(&mut d, RowKey::Style),
            Control::Segmented {
                options: vec!["Notch", "Pill"],
                selected: 1
            }
        );
        let slider = |value: f32, min, max, text: &str| Control::Slider {
            value,
            min,
            max,
            text: text.into(),
            dragging: false,
        };
        assert_eq!(
            ctl(&mut d, RowKey::NotchWidth),
            slider(160.0, 96.0, 240.0, "160 px")
        );
        assert_eq!(
            ctl(&mut d, RowKey::NotchHeight),
            slider(36.0, 24.0, 44.0, "36 px")
        );
        assert_eq!(ctl(&mut d, RowKey::Reserve), t(false));
        assert!(!row(&mut d, RowKey::Reserve).enabled);
        assert_eq!(ctl(&mut d, RowKey::StayBehind), t(true));
        let chip = |s: &str| Control::Chip {
            text: s.into(),
            warn: false,
        };
        assert_eq!(ctl(&mut d, RowKey::SnapHotkey), chip("Ctrl+Space"));
        assert_eq!(ctl(&mut d, RowKey::SnapAltHotkey), chip("off"));
        assert_eq!(ctl(&mut d, RowKey::SpotlightHotkey), chip("Ctrl+Alt+Space"));
        assert_eq!(
            ctl(&mut d, RowKey::TerminalHotkey),
            Control::Fixed {
                text: "Ctrl+Alt+`".into()
            }
        );
        assert_eq!(
            ctl(&mut d, RowKey::Shell),
            Control::Segmented {
                options: vec!["PowerShell", "pwsh", "cmd", "WSL"],
                selected: 3
            }
        );
        assert_eq!(
            ctl(&mut d, RowKey::FontSize),
            slider(14.0, 8.0, 24.0, "14 px")
        );
        assert_eq!(ctl(&mut d, RowKey::HideOnBlur), t(false));
        assert_eq!(
            ctl(&mut d, RowKey::Scrollback),
            slider(10_000.0, 0.0, 50_000.0, "10000 lines")
        );
        assert_eq!(ctl(&mut d, RowKey::SpotlightEnabled), t(false));
        assert_eq!(ctl(&mut d, RowKey::SpotlightFiles), t(false));
        assert!(!row(&mut d, RowKey::SpotlightFiles).enabled);
        match ctl(&mut d, RowKey::SpotlightFolders) {
            Control::Lines { lines } => {
                assert_eq!(
                    lines,
                    [
                        "Desktop",
                        "Documents",
                        "Downloads",
                        "Pictures",
                        "Videos",
                        "Music"
                    ]
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(!d.is_dirty());
    }

    #[test]
    fn the_default_config_shows_the_defaults() {
        let mut d = doc();
        assert_eq!(
            ctl(&mut d, RowKey::FollowExplorer),
            Control::Toggle { on: true }
        );
        assert_eq!(
            ctl(&mut d, RowKey::Style),
            Control::Segmented {
                options: vec!["Notch", "Pill"],
                selected: 0
            }
        );
        assert!(row(&mut d, RowKey::Reserve).enabled);
        assert!(row(&mut d, RowKey::SpotlightFiles).enabled);
        assert_eq!(
            ctl(&mut d, RowKey::SnapHotkey),
            Control::Chip {
                text: "PrtSc".into(),
                warn: false
            }
        );
        // A theme that isn't in the list shows its id.
        let mut c = cfg();
        c.theme = "mine".into();
        let mut d = SettingsDoc::new(&c, themes(), Taken::default());
        assert_eq!(
            ctl(&mut d, RowKey::Theme),
            Control::Chip {
                text: "mine".into(),
                warn: false
            }
        );
        // Every row is on exactly one page and the pages cover them all.
        let mut seen = 0;
        for p in Page::ALL {
            d.set_page(p);
            let v = d.view();
            assert_eq!(v.title, p.label());
            assert!(!v.rows.is_empty());
            seen += v.rows.len();
        }
        assert_eq!(seen, ROWS.len());
    }

    #[test]
    fn taken_hotkeys_warn() {
        let mut d = doc();
        d.set_taken(Taken {
            snap: true,
            snap_alt: true,
            spotlight: true,
        });
        for k in [
            RowKey::SnapHotkey,
            RowKey::SnapAltHotkey,
            RowKey::SpotlightHotkey,
        ] {
            assert!(
                matches!(ctl(&mut d, k), Control::Chip { warn: true, .. }),
                "{k:?}"
            );
        }
        d.set_taken(Taken::default());
        assert!(matches!(
            ctl(&mut d, RowKey::SnapHotkey),
            Control::Chip { warn: false, .. }
        ));
        // An alt that is off or a Spotlight that is off can't clash.
        let mut c = cfg();
        c.snap.alt_hotkey = "none".into();
        c.spotlight.hotkey = "off".into();
        let mut d = SettingsDoc::new(
            &c,
            themes(),
            Taken {
                snap: false,
                snap_alt: true,
                spotlight: true,
            },
        );
        assert!(matches!(
            ctl(&mut d, RowKey::SnapAltHotkey),
            Control::Chip { warn: false, .. }
        ));
        assert!(matches!(
            ctl(&mut d, RowKey::SpotlightHotkey),
            Control::Chip { warn: false, .. }
        ));
    }

    // -- widgets ---------------------------------------------------------------------

    #[test]
    fn toggles_flip_on_press() {
        let mut d = doc();
        for (key, get) in [
            (
                RowKey::NotificationsEnabled,
                (|c: &Config| c.notifications.enabled) as fn(&Config) -> bool,
            ),
            (RowKey::InternalNotificationsEnabled, |c| {
                c.notifications.internal_enabled
            }),
            (
                RowKey::Autostart,
                (|c: &Config| c.start_with_windows) as fn(&Config) -> bool,
            ),
            (RowKey::Reserve, |c| c.island.reserve_top),
            (RowKey::StayBehind, |c| c.island.stay_behind),
            (RowKey::HideOnBlur, |c| c.terminal.hide_on_blur),
            (RowKey::FollowExplorer, |c| c.terminal.follow_explorer),
            (RowKey::SpotlightEnabled, |c| c.spotlight.enabled),
        ] {
            let before = get(d.pending());
            let out = click_row(&mut d, key);
            assert!(out.redraw, "{key:?}");
            assert_eq!(get(d.pending()), !before, "{key:?}");
            // Reserve is off once Stay behind is on; turn that back off to click again.
            if key == RowKey::StayBehind {
                click_row(&mut d, key);
                assert_eq!(get(d.pending()), before);
            }
        }
        // A press outside any control is nothing.
        let out = click_at(&mut d, 1.0, 1.0);
        assert!(!out.redraw);
    }

    #[test]
    fn segmented_controls_pick_the_segment_under_the_pointer() {
        let mut d = doc();
        assert!(click_seg(&mut d, RowKey::Style, 1).redraw);
        assert_eq!(d.pending().island.style(), IslandStyle::Pill);
        assert_eq!(d.pending().island.style, "pill");
        // Picking the segment that is already on changes nothing.
        assert!(!click_seg(&mut d, RowKey::Style, 1).redraw);
        click_seg(&mut d, RowKey::Style, 0);
        assert_eq!(d.pending().island.style, "notch");
        for (i, want) in ["powershell", "pwsh", "cmd", "wsl"].iter().enumerate() {
            click_seg(&mut d, RowKey::Shell, i);
            assert_eq!(d.pending().terminal.shell, *want);
            assert!(matches!(
                ctl(&mut d, RowKey::Shell),
                Control::Segmented { selected, .. } if selected == i
            ));
        }
        // The segments split the control evenly and fill it.
        let r = row(&mut d, RowKey::Shell);
        assert_eq!(r.parts.len(), 4);
        assert_eq!(r.control_rect.w, 280.0);
        let sum: f32 = r.parts.iter().map(|p| p.w).sum();
        assert!((sum - r.control_rect.w).abs() < 0.01);
        assert!(r.parts.iter().all(|p| (p.w - 70.0).abs() < 0.01));
    }

    #[test]
    fn an_unknown_shell_shows_the_first_segment_and_stays_until_picked() {
        let mut c = cfg();
        c.terminal.shell = "C:\\Tools\\fish.exe".into();
        let mut d = SettingsDoc::new(&c, themes(), Taken::default());
        assert!(matches!(
            ctl(&mut d, RowKey::Shell),
            Control::Segmented { selected: 0, .. }
        ));
        // Other edits leave it alone.
        click_row(&mut d, RowKey::HideOnBlur);
        assert_eq!(d.pending().terminal.shell, "C:\\Tools\\fish.exe");
        click_seg(&mut d, RowKey::Shell, 0);
        assert_eq!(d.pending().terminal.shell, "powershell");
        // An empty shell (auto) also reads as the first.
        let mut d = doc();
        assert!(matches!(
            ctl(&mut d, RowKey::Shell),
            Control::Segmented { selected: 0, .. }
        ));
        assert_eq!(d.pending().terminal.shell, "");
    }

    #[test]
    fn chips_cycle() {
        let mut d = doc();
        click_row(&mut d, RowKey::Theme);
        assert_eq!(d.pending().theme, "b");
        click_row(&mut d, RowKey::Theme);
        assert_eq!(d.pending().theme, "a");
        // An unknown theme starts the cycle at the first.
        let mut c = cfg();
        c.theme = "zzz".into();
        let mut d2 = SettingsDoc::new(&c, themes(), Taken::default());
        click_row(&mut d2, RowKey::Theme);
        assert_eq!(d2.pending().theme, "a");
        // No themes: nothing to cycle.
        let mut d3 = SettingsDoc::new(&cfg(), Vec::new(), Taken::default());
        assert!(!click_row(&mut d3, RowKey::Theme).redraw);

        let mut seen = vec![d.pending().snap.hotkey().label()];
        for _ in 0..4 {
            click_row(&mut d, RowKey::SnapHotkey);
            seen.push(d.pending().snap.hotkey().label());
        }
        assert_eq!(
            seen,
            ["PrtSc", "Ctrl+Space", "Ctrl+Alt+S", "Ctrl+Shift+S", "PrtSc"]
        );
        let mut d = doc();
        let mut alts = Vec::new();
        for _ in 0..4 {
            click_row(&mut d, RowKey::SnapAltHotkey);
            alts.push(settings::alt_label(d.pending().snap.alt_hotkey()));
        }
        assert_eq!(alts, ["Ctrl+Alt+S", "Ctrl+Shift+S", "off", "Ctrl+Space"]);
        let mut spots = Vec::new();
        for _ in 0..3 {
            click_row(&mut d, RowKey::SpotlightHotkey);
            spots.push(d.pending().spotlight.hotkey.clone());
        }
        assert_eq!(spots, ["ctrl_alt_space", "off", "alt_space"]);
        assert_eq!(
            ctl(&mut d, RowKey::SpotlightHotkey),
            Control::Chip {
                text: "Alt+Space".into(),
                warn: false
            }
        );
    }

    #[test]
    fn fixed_and_lines_rows_ignore_input() {
        let mut d = doc();
        for k in [RowKey::TerminalHotkey, RowKey::SpotlightFolders] {
            let r = row(&mut d, k);
            let (x, y) = mid(r.control_rect);
            assert!(!click_at(&mut d, x, y).redraw, "{k:?}");
            let (x, y) = mid(r.rect);
            assert!(!click_at(&mut d, x, y).redraw, "{k:?}");
        }
        assert!(!d.is_dirty());
    }

    #[test]
    fn a_slider_jumps_on_press_and_follows_the_drag() {
        let mut d = doc();
        let x = track_x(&mut d, RowKey::NotchWidth, 0.5);
        let (_, y) = mid(row(&mut d, RowKey::NotchWidth).control_rect);
        assert!(d.press(x, y).redraw);
        assert_eq!(d.pending().island.notch_width, 168);
        assert!(matches!(
            ctl(&mut d, RowKey::NotchWidth),
            Control::Slider { dragging: true, .. }
        ));
        // Drag: snaps to the step of 2.
        let x = track_x(&mut d, RowKey::NotchWidth, 0.3);
        assert!(d.drag(x, y).redraw);
        assert_eq!(d.pending().island.notch_width, 140);
        // The same snapped value again is no redraw.
        assert!(!d.drag(x + 0.2, y).redraw);
        // Clamped at both ends, however far the pointer goes (and off the track in y).
        assert!(d.drag(5000.0, y + 300.0).redraw);
        assert_eq!(d.pending().island.notch_width, 240);
        assert!(d.drag(-5000.0, y).redraw);
        assert_eq!(d.pending().island.notch_width, 96);
        assert!(d.release(0.0, 0.0).redraw);
        assert!(matches!(
            ctl(&mut d, RowKey::NotchWidth),
            Control::Slider {
                dragging: false,
                ..
            }
        ));
        // After release a drag does nothing.
        assert!(!d.drag(x, y).redraw);
        assert_eq!(d.pending().island.notch_width, 96);
    }

    #[test]
    fn every_slider_snaps_to_its_step() {
        let mut d = doc();
        let cases = [
            (RowKey::NotchHeight, 0.5, 34.0),
            (RowKey::NotchHeight, 1.0, 44.0),
            (RowKey::NotchHeight, 0.0, 24.0),
            (RowKey::FontSize, 0.5, 16.0),
            (RowKey::FontSize, 0.02, 8.0),
            (RowKey::Scrollback, 0.123, 6000.0),
            (RowKey::Scrollback, 1.0, 50_000.0),
            (RowKey::Scrollback, 0.0, 0.0),
        ];
        for (key, t, want) in cases {
            let (_, y) = mid(row(&mut d, key).control_rect);
            let x = track_x(&mut d, key, t);
            d.press(x, y);
            d.release(x, y);
            assert_eq!(slider_value(&mut d, key), want, "{key:?} {t}");
        }
        assert_eq!(d.pending().terminal.scrollback, 0);
        assert_eq!(d.pending().terminal.font_size, 8.0);
        assert_eq!(d.pending().island.notch_height, 24);
    }

    #[test]
    fn a_slider_can_be_grabbed_just_outside_the_track_and_on_its_value_text() {
        let mut d = doc();
        let r = row(&mut d, RowKey::FontSize);
        let tr = r.parts[0];
        assert_eq!((tr.w, tr.h), (200.0, 4.0));
        // The value text sits to the right of the track and is inside the control.
        assert!(r.control_rect.x + r.control_rect.w > tr.x + tr.w + 60.0);
        let y = tr.y + tr.h / 2.0;
        assert!(d.press(tr.x - 4.0, y).redraw);
        assert_eq!(d.pending().terminal.font_size, 8.0);
        d.release(0.0, 0.0);
        // Far from it: nothing.
        let mut d = doc();
        let before = d.pending().clone();
        d.press(tr.x - 40.0, y);
        assert_eq!(d.pending(), &before);
    }

    // -- disabled rows ---------------------------------------------------------------

    #[test]
    fn disabled_rows_ignore_input() {
        let mut c = cfg();
        c.island.style = "pill".into();
        let mut d = SettingsDoc::new(&c, themes(), Taken::default());
        assert!(!row(&mut d, RowKey::Reserve).enabled);
        let out = click_row(&mut d, RowKey::Reserve);
        assert!(!out.redraw);
        assert!(!d.is_dirty());
        // Back to notch: it works; then Stay behind turns it off again.
        click_seg(&mut d, RowKey::Style, 0);
        assert!(row(&mut d, RowKey::Reserve).enabled);
        click_row(&mut d, RowKey::StayBehind);
        assert!(!row(&mut d, RowKey::Reserve).enabled);
        let before = d.pending().clone();
        click_row(&mut d, RowKey::Reserve);
        assert_eq!(d.pending(), &before);
        // Search files follows Spotlight.
        click_row(&mut d, RowKey::SpotlightEnabled);
        assert!(!row(&mut d, RowKey::SpotlightFiles).enabled);
        let before = d.pending().clone();
        click_row(&mut d, RowKey::SpotlightFiles);
        assert_eq!(d.pending(), &before);
        // They don't highlight on hover either.
        let r = row(&mut d, RowKey::SpotlightFiles);
        let (x, y) = mid(r.control_rect);
        assert!(!d.hover(x, y).redraw);
        assert!(!row(&mut d, RowKey::SpotlightFiles).hovered);
    }

    // -- search ----------------------------------------------------------------------

    fn keys(d: &SettingsDoc) -> Vec<RowKey> {
        d.view().rows.iter().map(|r| r.key).collect()
    }

    #[test]
    fn search_filters_rows_from_every_page_in_page_order() {
        let mut d = doc();
        type_text(&mut d, "spot");
        assert_eq!(
            keys(&d),
            [RowKey::SpotlightHotkey, RowKey::SpotlightEnabled]
        );
        let v = d.view();
        assert_eq!(v.title, "Search");
        // Matches are the label's range; a hint-only hit has none.
        assert_eq!(v.rows[0].matched, Some((0, 4)));
        // Case-insensitive, and the text is trimmed.
        d.search.set_text("  SPOT ");
        assert_eq!(keys(&d).len(), 2);
        // The nav shows the counts, and nothing is active.
        let counts: Vec<_> = d.view().nav.iter().map(|n| (n.page, n.matches)).collect();
        assert_eq!(
            counts,
            [
                (Page::General, Some(0)),
                (Page::Notch, Some(0)),
                (Page::Hotkeys, Some(1)),
                (Page::Terminal, Some(0)),
                (Page::Spotlight, Some(1)),
            ]
        );
        assert!(d.view().nav.iter().all(|n| !n.active));
    }

    #[test]
    fn search_matches_hints_and_marks_label_ranges() {
        let mut d = doc();
        type_text(&mut d, "notch");
        assert_eq!(
            keys(&d),
            [
                RowKey::Style,
                RowKey::NotchWidth,
                RowKey::NotchHeight,
                RowKey::Reserve,
                RowKey::StayBehind
            ]
        );
        let r: Vec<_> = d.view().rows.iter().map(|r| r.matched).collect();
        assert_eq!(r, [None, Some((0, 5)), Some((0, 5)), None, None]);
        // A match in the middle of a label.
        d.search.set_text("width");
        let v = d.view();
        assert_eq!(v.rows.len(), 1);
        assert_eq!(v.rows[0].matched, Some((6, 11)));
        d.search.set_text("zzz");
        let v = d.view();
        assert!(v.rows.is_empty());
        assert_eq!(v.scrollbar, None);
        assert!(v.nav.iter().all(|n| n.matches == Some(0)));
    }

    #[test]
    fn search_rows_are_laid_out_and_editable() {
        let mut d = doc();
        type_text(&mut d, "spot");
        let rv = d
            .view()
            .rows
            .into_iter()
            .find(|r| r.key == RowKey::SpotlightEnabled)
            .unwrap();
        let (x, y) = mid(rv.control_rect);
        assert!(d.search.text() == "spot");
        assert!(click_at(&mut d, x, y).redraw);
        assert!(!d.pending().spotlight.enabled);
        // Rows are stacked from the top of the content.
        let v = d.view();
        assert_eq!(v.rows[0].rect.y, v.content.y);
        assert_eq!(v.rows[1].rect.y, v.content.y + ROW_H);
    }

    #[test]
    fn clicking_a_page_leaves_the_search() {
        let mut d = doc();
        type_text(&mut d, "spot");
        let nav = d.view().nav[Page::ALL.iter().position(|p| *p == Page::Terminal).unwrap()].rect;
        let (x, y) = mid(nav);
        assert!(d.press(x, y).redraw);
        assert_eq!(d.page(), Page::Terminal);
        assert_eq!(d.view().search.text, "");
        assert_eq!(d.view().title, "Terminal");
        assert!(d.view().nav[3].active);
        assert!(d.view().nav.iter().all(|n| n.matches.is_none()));
        // The same page again: no change.
        assert!(!d.press(x, y).redraw);
    }

    // -- reset -----------------------------------------------------------------------

    #[test]
    fn reset_puts_the_current_page_back_to_the_defaults() {
        let mut d = SettingsDoc::new(&custom(), themes(), Taken::default());
        let edited = d.pending().clone();
        d.set_page(Page::Notch);
        assert!(d
            .view()
            .buttons
            .iter()
            .any(|b| b.button == Button::Reset && b.enabled));
        let out = click_button(&mut d, Button::Reset);
        assert!(out.redraw && out.apply.is_some() && out.persist && !out.close);
        let def = Config::default();
        let p = d.pending();
        assert_eq!(p.island, def.island);
        // Other pages are untouched.
        assert_eq!(p.terminal, edited.terminal);
        assert_eq!(p.spotlight, edited.spotlight);
        assert_eq!(p.snap, edited.snap);
        assert_eq!(p.theme, edited.theme);
        assert!(!d.is_dirty());
        // Nothing left to reset on this page: the button is disabled and a click is nothing.
        let reset = d
            .view()
            .buttons
            .into_iter()
            .find(|b| b.button == Button::Reset)
            .unwrap();
        assert!(!reset.enabled);
        assert!(!click_button(&mut d, Button::Reset).redraw);
        // The Terminal page resets its own rows.
        d.set_page(Page::Terminal);
        click_button(&mut d, Button::Reset);
        assert_eq!(d.pending().terminal, Config::default().terminal);
        d.set_page(Page::General);
        click_button(&mut d, Button::Reset);
        assert_eq!(d.pending().theme, Config::default().theme);
        assert!(!d.pending().start_with_windows);
        // Hotkeys and Spotlight.
        d.set_page(Page::Hotkeys);
        click_button(&mut d, Button::Reset);
        d.set_page(Page::Spotlight);
        click_button(&mut d, Button::Reset);
        assert_eq!(d.pending(), &Config::default());
    }

    #[test]
    fn reset_in_search_only_touches_the_matching_rows() {
        let mut c = cfg();
        c.island.notch_width = 200;
        c.island.notch_height = 40;
        let mut d = SettingsDoc::new(&c, themes(), Taken::default());
        type_text(&mut d, "notch width");
        assert_eq!(keys(&d), [RowKey::NotchWidth]);
        click_button(&mut d, Button::Reset);
        assert_eq!(d.pending().island.notch_width, 128);
        assert_eq!(d.pending().island.notch_height, 40);
        assert_eq!(d.page(), Page::General);
    }

    #[test]
    fn reset_keeps_the_two_snap_keys_apart() {
        let mut c = cfg();
        c.snap.hotkey = "Ctrl+Space".into();
        c.snap.alt_hotkey = "Ctrl+Alt+S".into();
        let mut d = SettingsDoc::new(&c, themes(), Taken::default());
        d.set_page(Page::Hotkeys);
        click_button(&mut d, Button::Reset);
        let s = &d.pending().snap;
        assert_ne!(s.alt_hotkey(), Some(s.hotkey()));
    }

    // -- apply, save, close ----------------------------------------------------------

    #[test]
    fn every_edit_applies_and_saves_at_once() {
        let mut d = doc();
        let before = d.pending().clone();
        assert_eq!(d.view().status.as_deref(), Some("Changes apply instantly"));
        let out = click_row(&mut d, RowKey::Autostart);
        let mut want = before;
        want.start_with_windows = true;
        assert_eq!(out.apply, Some(want.clone()));
        assert!(out.persist && out.redraw && !out.close);
        assert_eq!(d.saved(), &want);
        assert!(!d.is_dirty());
        assert_eq!(d.view().status.as_deref(), Some("Saved"));
        let out = click_seg(&mut d, RowKey::Style, 1);
        want.island.style = "pill".into();
        assert_eq!(out.apply, Some(want));
        assert!(out.persist);
        // A click that changes nothing (the segment already picked) applies nothing.
        assert_eq!(click_seg(&mut d, RowKey::Style, 1).apply, None);
    }

    #[test]
    fn a_slider_previews_while_dragged_and_saves_on_release() {
        let mut d = doc();
        d.set_page(Page::Notch);
        let x0 = track_x(&mut d, RowKey::NotchWidth, 0.25);
        let (_, y) = mid(row(&mut d, RowKey::NotchWidth).control_rect);
        let out = d.press(x0, y);
        assert!(out.apply.is_some() && !out.persist, "a preview");
        let x1 = track_x(&mut d, RowKey::NotchWidth, 0.75);
        let out = d.drag(x1, y);
        let previewed = out.apply.clone().unwrap();
        assert!(!out.persist);
        assert!(d.is_dirty());
        let out = d.release(x1, y);
        assert!(out.persist);
        assert_eq!(out.apply, Some(previewed));
        assert!(!d.is_dirty());
        // A drag that ends where it began still leaves nothing unsaved.
        let out = d.press(x1, y);
        assert!(out.apply.is_none());
        let out = d.release(x1, y);
        assert!(out.apply.is_none());
    }

    #[test]
    fn only_close_and_reset_are_shown() {
        let d = doc();
        let shown: Vec<_> = d.view().buttons.iter().map(|b| b.button).collect();
        assert_eq!(shown, [Button::Close, Button::Reset]);
        assert!(d.view().buttons.iter().all(|b| !b.primary));
    }

    #[test]
    fn reset_applies_the_defaults_at_once() {
        let mut d = SettingsDoc::new(&custom(), themes(), Taken::default());
        d.set_page(Page::Notch);
        let out = click_button(&mut d, Button::Reset);
        let applied = out.apply.expect("applied");
        assert!(out.persist);
        assert_eq!(applied.island, Config::default().island);
        assert!(!d.is_dirty());
    }

    #[test]
    fn buttons_act_on_release_over_them() {
        let mut d = SettingsDoc::new(&custom(), themes(), Taken::default());
        d.set_page(Page::Notch);
        let (x, y) = mid(SettingsDoc::button_rect(Button::Reset));
        assert!(d.press(x, y).apply.is_none());
        // Released somewhere else: cancelled.
        assert!(d.release(x, y - 80.0).apply.is_none());
        // Released over it: applied.
        d.press(x, y);
        assert!(d.release(x + 1.0, y).apply.is_some());
    }

    #[test]
    fn close_and_esc_close_at_once() {
        let mut d = doc();
        assert!(click_button(&mut d, Button::Close).close);
        let mut d = doc();
        click_row(&mut d, RowKey::Autostart);
        assert!(click_button(&mut d, Button::Close).close);
        let mut d = doc();
        assert!(d.key(VK_ESCAPE, false, false).close);
        // With a search, the first Esc clears it.
        let mut d = doc();
        type_text(&mut d, "notch");
        assert!(!d.key(VK_ESCAPE, false, false).close);
        assert!(d.key(VK_ESCAPE, false, false).close);
    }

    #[test]
    fn config_changed_follows_the_new_config() {
        let mut d = doc();
        let mut c = cfg();
        c.theme = "b".into();
        d.config_changed(&c);
        assert_eq!(d.pending(), &c);
        assert_eq!(d.saved(), &c);
        assert!(!d.is_dirty());
    }

    // -- scrolling -------------------------------------------------------------------

    #[test]
    fn the_wheel_scrolls_and_clamps() {
        let mut d = doc();
        // A page that fits has nothing to scroll.
        assert!(!d.wheel(1.0).redraw);
        assert_eq!(d.view().scrollbar, None);
        // Searching "e" lists most rows, more than fit.
        type_text(&mut d, "e");
        let v = d.view();
        assert!(v.rows.len() > 6);
        let (pos, frac) = v.scrollbar.unwrap();
        assert_eq!(pos, 0.0);
        assert!(frac > 0.0 && frac < 1.0);
        let top = v.content.y;
        assert_eq!(v.rows[0].rect.y, top);
        assert!(!d.wheel(-1.0).redraw);
        assert!(d.wheel(1.0).redraw);
        assert_eq!(d.view().rows[0].rect.y, top - SCROLL_LINE);
        assert!(d.wheel(0.5).redraw);
        assert_eq!(d.view().rows[0].rect.y, top - 1.5 * SCROLL_LINE);
        // Clamped so the last row ends at the content's bottom edge.
        assert!(d.wheel(1000.0).redraw);
        let v = d.view();
        let last = v.rows.last().unwrap();
        assert_eq!(last.rect.y + last.rect.h, v.content.y + v.content.h);
        assert_eq!(v.scrollbar.unwrap().0, 1.0);
        assert!(!d.wheel(1.0).redraw);
        assert!(d.wheel(-1000.0).redraw);
        assert_eq!(d.view().rows[0].rect.y, top);
        // Typing again goes back to the top.
        d.wheel(2.0);
        d.char('x');
        assert_eq!(d.view().content.y, top);
        d.search.set_text("e");
        assert_eq!(d.view().rows[0].rect.y, top);
    }

    #[test]
    fn each_page_keeps_its_own_scroll() {
        let mut d = doc();
        type_text(&mut d, "e");
        d.wheel(3.0);
        // Leaving the search for a page that fits shows it from the top.
        d.set_page(Page::Notch);
        assert_eq!(d.view().rows[0].rect.y, d.view().content.y);
    }

    // -- keyboard --------------------------------------------------------------------

    #[test]
    fn ctrl_s_does_nothing_since_everything_is_saved() {
        let mut d = doc();
        click_row(&mut d, RowKey::Autostart);
        assert_eq!(d.key(VK_S, true, false), Outcome::default());
    }

    #[test]
    fn ctrl_f_focuses_the_search_and_typing_goes_to_it() {
        let mut d = doc();
        assert!(!d.view().search.focused);
        assert!(d.key(VK_F, true, false).redraw);
        assert!(d.view().search.focused);
        assert!(!d.key(VK_F, true, false).redraw);
        assert!(d.char('n').redraw);
        d.char('o');
        assert_eq!(d.view().search.text, "no");
        assert_eq!(d.view().search.caret, 2);
        // Editing keys.
        assert!(d.key(0x08, false, false).redraw); // Backspace
        assert_eq!(d.view().search.text, "n");
        assert!(d.key(0x25, false, false).redraw); // Left
        assert!(d.key(0x2E, false, false).redraw); // Delete
        assert_eq!(d.view().search.text, "");
        assert!(!d.key(0x08, false, false).redraw);
        // Control chars are not text.
        assert!(!d.char('\u{8}').redraw);
        assert!(!d.char('\r').redraw);
    }

    #[test]
    fn typing_focuses_the_search_when_nothing_has_focus() {
        let mut d = doc();
        assert!(!d.view().search.focused);
        d.char('t');
        assert!(d.view().search.focused);
        // Clicking elsewhere takes the focus away; clicking the field gives it back.
        click_row(&mut d, RowKey::Autostart);
        assert!(!d.view().search.focused);
        let (x, y) = mid(d.view().search.rect);
        assert!(d.press(x, y).redraw);
        assert!(d.view().search.focused);
        // Edit keys do nothing while unfocused.
        let (x, y) = mid(d.view().nav[1].rect);
        d.press(x, y);
        d.char('a');
        click_row(&mut d, RowKey::StayBehind);
        let t = d.view().search.text;
        d.key(0x08, false, false);
        assert_eq!(d.view().search.text, t);
    }

    #[test]
    fn escape_clears_the_search_before_it_closes() {
        let mut d = doc();
        type_text(&mut d, "spot");
        let out = d.key(VK_ESCAPE, false, false);
        assert!(out.redraw && !out.close);
        assert_eq!(d.view().search.text, "");
        assert_eq!(d.view().title, "General");
        let out = d.key(VK_ESCAPE, false, false);
        assert!(out.close);
    }

    #[test]
    fn up_and_down_move_between_pages() {
        let mut d = doc();
        assert!(!d.key(VK_UP, false, false).redraw);
        for p in &Page::ALL[1..] {
            assert!(d.key(VK_DOWN, false, false).redraw);
            assert_eq!(d.page(), *p);
        }
        assert!(!d.key(VK_DOWN, false, false).redraw);
        assert_eq!(d.page(), Page::Spotlight);
        assert!(d.key(VK_UP, false, false).redraw);
        assert_eq!(d.page(), Page::Terminal);
        // While searching they do nothing.
        type_text(&mut d, "x");
        assert!(!d.key(VK_UP, false, false).redraw);
        assert_eq!(d.page(), Page::Terminal);
    }

    // -- hover -----------------------------------------------------------------------

    #[test]
    fn hover_redraws_only_when_the_highlight_changes() {
        let mut d = doc();
        let nav = d.view().nav[2].rect;
        let (x, y) = mid(nav);
        assert!(d.hover(x, y).redraw);
        assert!(!d.hover(x + 3.0, y).redraw);
        assert!(d.view().nav[2].hovered);
        // A row: anywhere on it highlights it.
        let r = row(&mut d, RowKey::Autostart);
        assert!(d.hover(r.rect.x + 10.0, r.rect.y + 5.0).redraw);
        assert!(!d.hover(r.rect.x + 40.0, r.rect.y + 20.0).redraw);
        assert!(row(&mut d, RowKey::Autostart).hovered);
        assert!(!d.view().nav[2].hovered);
        // Empty space: it clears; again no change.
        assert!(d.hover(1.0, 1.0).redraw);
        assert!(!d.hover(2.0, 2.0).redraw);
        // A disabled button doesn't highlight; an enabled one does.
        let (x, y) = mid(SettingsDoc::button_rect(Button::Apply));
        assert!(!d.hover(x, y).redraw);
        let (x, y) = mid(SettingsDoc::button_rect(Button::Close));
        assert!(d.hover(x, y).redraw);
        assert!(d
            .view()
            .buttons
            .iter()
            .any(|b| b.button == Button::Close && b.hovered));
        // Leaving the window clears it once.
        assert!(d.leave().redraw);
        assert!(!d.leave().redraw);
    }

    // -- layout ----------------------------------------------------------------------

    fn inside(inner: Rect, outer: Rect) -> bool {
        inner.x >= outer.x - 0.01
            && inner.y >= outer.y - 0.01
            && inner.x + inner.w <= outer.x + outer.w + 0.01
            && inner.y + inner.h <= outer.y + outer.h + 0.01
    }

    #[test]
    fn the_layout_fits_the_panel() {
        let mut d = doc();
        let v = d.view();
        assert_eq!(v.title_bar, Rect::new(PANEL.x, PANEL.y, PANEL_W, TITLE_H));
        assert_eq!(d.title_bar(), v.title_bar);
        let sidebar = Rect::new(PANEL.x, PANEL.y + TITLE_H, SIDEBAR_W, PANEL_H - TITLE_H);
        assert!(inside(v.search.rect, sidebar));
        assert_eq!(v.search.rect.h, 32.0);
        assert_eq!(v.nav.len(), 5);
        for (i, n) in v.nav.iter().enumerate() {
            assert!(inside(n.rect, sidebar));
            assert_eq!(n.rect.h, 34.0);
            if i > 0 {
                assert_eq!(n.rect.y, v.nav[i - 1].rect.y + 34.0 + 4.0);
            }
        }
        assert!(v.nav[0].rect.y >= v.search.rect.y + v.search.rect.h);
        // The content sits right of the sidebar, under the page title, above the footer.
        assert_eq!(v.content.x, PANEL.x + SIDEBAR_W);
        assert_eq!(v.content.y, PANEL.y + TITLE_H + 40.0);
        let footer_y = PANEL.y + PANEL_H - FOOTER_H;
        assert_eq!(v.content.y + v.content.h, footer_y);
        assert_eq!(v.content.x + v.content.w, PANEL.x + PANEL_W);
        // Buttons: Close in the title bar (28x28), the others in the footer, no overlap.
        let b = |k: Button| v.buttons.iter().find(|b| b.button == k).unwrap().rect;
        assert_eq!((b(Button::Close).w, b(Button::Close).h), (28.0, 28.0));
        assert!(inside(b(Button::Close), v.title_bar));
        let footer = Rect::new(v.content.x, footer_y, v.content.w, FOOTER_H);
        // Only Reset is in the footer (every edit applies at once), flush right.
        assert!(inside(b(Button::Reset), footer));
        assert_eq!(
            b(Button::Reset).x + b(Button::Reset).w,
            PANEL.x + PANEL_W - 24.0
        );
        assert_eq!(
            v.buttons.iter().map(|b| b.label).collect::<Vec<_>>(),
            ["\u{2715}", "Reset"]
        );
        // The window fits the panel with its margin.
        assert!(inside(PANEL, Rect::new(0.0, 0.0, WIN_W, WIN_H)));
        // Every row and control on every page is inside the content width, stacked.
        for p in Page::ALL {
            d.set_page(p);
            let v = d.view();
            let mut y = v.content.y;
            for r in &v.rows {
                assert_eq!(r.rect.y, y);
                assert_eq!(r.rect.x, v.content.x);
                assert_eq!(r.rect.w, v.content.w);
                assert!(inside(r.control_rect, r.rect), "{:?}", r.key);
                for part in &r.parts {
                    assert!(inside(*part, r.rect), "{:?}", r.key);
                }
                assert!(r.control_rect.x + r.control_rect.w <= r.rect.x + r.rect.w - 24.0 + 0.01);
                y += r.rect.h;
            }
            // The pages fit without scrolling.
            assert!(y <= v.content.y + v.content.h, "{p:?}");
        }
    }

    #[test]
    fn control_sizes() {
        let mut d = doc();
        let r = row(&mut d, RowKey::Autostart);
        assert_eq!((r.control_rect.w, r.control_rect.h), (44.0, 24.0));
        let r = row(&mut d, RowKey::Theme);
        assert!(r.control_rect.w <= 160.0 && r.control_rect.h == 28.0);
        let r = row(&mut d, RowKey::Style);
        assert_eq!((r.control_rect.w, r.control_rect.h), (160.0, 30.0));
        assert_eq!(r.parts.len(), 2);
        let r = row(&mut d, RowKey::NotchWidth);
        assert_eq!((r.parts[0].w, r.parts[0].h), (200.0, 4.0));
        // The Folders row is taller by its lines.
        let r = row(&mut d, RowKey::SpotlightFolders);
        assert_eq!(r.rect.h, ROW_H + 6.0 * LINE_H);
        assert!(inside(r.control_rect, r.rect));
        assert_eq!(r.control_rect.h, 6.0 * LINE_H);
    }

    #[test]
    fn rows_outside_the_content_are_not_hit() {
        let mut d = doc();
        type_text(&mut d, "e");
        d.wheel(4.0);
        let v = d.view();
        // A row scrolled up under the title area can't be pressed there.
        let r = v
            .rows
            .iter()
            .find(|r| r.rect.y < v.content.y)
            .unwrap()
            .clone();
        let (x, _) = mid(r.control_rect);
        let before = d.pending().clone();
        d.press(x, v.content.y - 10.0);
        d.release(x, v.content.y - 10.0);
        assert_eq!(d.pending(), &before);
    }
}
