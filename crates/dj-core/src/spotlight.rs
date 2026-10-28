//! Spotlight (Stage V3): the Alt+Space launcher. A search field under the notch with up
//! to `MAX_ROWS` results. Pure: matching, ranking, the selection and the layout; the app
//! supplies the index (apps and files) and does the launching.
//!
//! Coordinates are window-local logical px. The window is `WIN_W` × `WIN_H` with a
//! transparent `MARGIN` (shadow room); the panel's height follows the result count.

use crate::layout::Rect;
use crate::settings_ui::FieldView;

// ---------------------------------------------------------------------------------------
// Shared types (Stage V skeleton — do not change without the coordinator)

pub const PANEL_W: f32 = 680.0;
pub const FIELD_H: f32 = 52.0;
pub const ROW_H: f32 = 44.0;
pub const MAX_ROWS: usize = 8;
/// Padding below the last row.
pub const PAD: f32 = 8.0;
pub const PANEL_R: f32 = 18.0;
pub const MARGIN: f32 = 24.0;
pub const WIN_W: f32 = PANEL_W + 2.0 * MARGIN;
pub const WIN_H: f32 = FIELD_H + MAX_ROWS as f32 * ROW_H + PAD + 2.0 * MARGIN;
/// The window's top edge sits this far below the island's resting bottom edge.
pub const GAP_BELOW_NOTCH: f32 = 24.0;
/// Files are searched only from this many query chars.
pub const FILE_MIN_QUERY: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EntryKind {
    App,
    Folder,
    File,
}

/// One searchable item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: EntryKind,
    /// What is shown and matched ("Google Chrome", "budget.xlsx").
    pub name: String,
    /// What the app launches: an AppsFolder parsing name for an app, a full path for a
    /// file or folder.
    pub target: String,
    /// The dim second line: the folder for a file, "App" for an app.
    pub detail: String,
}

impl Entry {
    /// The icon cache key: the target for an app, the lowercase extension (with the dot,
    /// or "<folder>") for files and folders.
    pub fn icon_key(&self) -> String {
        match self.kind {
            EntryKind::App => self.target.clone(),
            EntryKind::Folder => "<folder>".to_string(),
            EntryKind::File => std::path::Path::new(&self.target)
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase()))
                .unwrap_or_else(|| "<file>".to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResultRow {
    pub rect: Rect,
    pub kind: EntryKind,
    pub name: String,
    pub detail: String,
    pub icon_key: String,
    pub selected: bool,
    pub hovered: bool,
    /// Char indices of `name` that matched the query (drawn bold/accent).
    pub matched: Vec<usize>,
}

/// Everything the Spotlight window draws.
#[derive(Clone, Debug, PartialEq)]
pub struct SpotlightView {
    /// The panel (its height follows the rows).
    pub panel: Rect,
    pub field: FieldView,
    pub rows: Vec<ResultRow>,
    /// A dim line in place of rows: "No results", "Indexing…" (None when there are rows or
    /// the query is empty).
    pub note: Option<&'static str>,
    /// Keyboard hints at the right of the field ("esc").
    pub hint: &'static str,
}

/// What the app must do after an input.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    None,
    Redraw,
    /// Open the entry, then close Spotlight.
    Launch(Entry),
    /// Show the file in Explorer (`explorer /select,`), then close.
    Reveal(Entry),
    Close,
}
