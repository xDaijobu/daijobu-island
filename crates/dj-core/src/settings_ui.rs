//! The Settings window (Stage V2): a large popup with a sidebar of pages, rows of
//! controls, a search field and Reset / Apply / Save. Pure: it edits a pending copy of the
//! `Config` and tells the app what to apply; the app owns the window and the side effects.
//!
//! Coordinates are window-local logical px. The window is `WIN_W` × `WIN_H` with a
//! transparent `MARGIN` all round (room for the shadow); the panel is `PANEL`.

use crate::layout::Rect;

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
    Scrollback,
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
    /// Apply this config (write it, save `config.toml`, apply side effects).
    pub apply: Option<crate::config::Config>,
    /// Close the window.
    pub close: bool,
}
