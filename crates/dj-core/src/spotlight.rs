//! Spotlight (Stage V3): the Alt+Space launcher. A search field under the notch with up
//! to `MAX_ROWS` results. Pure: matching, ranking, the selection and the layout; the app
//! supplies the index (apps and files) and does the launching.
//!
//! Coordinates are window-local logical px. The window is `WIN_W` × `WIN_H` with a
//! transparent `MARGIN` (shadow room); the panel's height follows the result count.

use crate::layout::Rect;
use crate::line_edit::LineEdit;
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
    /// The entry's target (the app asks for its size by it).
    pub target: String,
    /// Size (and file count for a folder), dim at the right: "…" while it loads, None
    /// when unknown.
    pub meta: Option<String>,
}

/// What is known about an entry's size; the app fills it in lazily, row by row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeInfo {
    /// Asked for, not known yet.
    Pending,
    /// Known. `files` is the file count of a folder (None for a file or an app);
    /// `capped`: the count stopped at a limit, so both are lower bounds.
    Known {
        bytes: u64,
        files: Option<u64>,
        capped: bool,
    },
    /// Not available (an app with no install size on record, an unreadable path).
    Unknown,
}

/// An app's install size from the installed programs `(display name, bytes)`: the same
/// name (ignoring case), else the closest name that starts with the other at a word
/// boundary ("Visual Studio Code" ~ "Microsoft Visual Studio Code" does not count, but
/// "Steam" ~ "Steam (x64)" does). None when nothing matches well enough.
pub fn installed_size(name: &str, installed: &[(String, u64)]) -> Option<u64> {
    let want = name.trim().to_lowercase();
    if want.chars().count() < 3 {
        return None;
    }
    let mut best: Option<(usize, u64)> = None;
    for (display, bytes) in installed {
        let have = display.trim().to_lowercase();
        if have == want {
            return Some(*bytes);
        }
        let (short, long) = if have.len() < want.len() {
            (&have, &want)
        } else {
            (&want, &have)
        };
        let boundary = long[short.len()..]
            .chars()
            .next()
            .is_some_and(|c| !c.is_alphanumeric());
        if short.chars().count() >= 4 && long.starts_with(short.as_str()) && boundary {
            let gap = long.len() - short.len();
            if best.is_none_or(|(g, _)| gap < g) {
                best = Some((gap, *bytes));
            }
        }
    }
    best.map(|(_, b)| b)
}

/// `1234567` → `"1,234,567"`.
fn grouped(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The row's size text: "…", "48 KB", "1,234 files · 4.2 GB", or with `≥` when capped.
/// None when unknown.
pub fn size_text(info: SizeInfo) -> Option<String> {
    match info {
        SizeInfo::Pending => Some("\u{2026}".to_string()),
        SizeInfo::Unknown => None,
        SizeInfo::Known {
            bytes,
            files,
            capped,
        } => {
            let at_least = if capped { "\u{2265} " } else { "" };
            let size = format!("{at_least}{}", crate::sweep::format_bytes(bytes));
            Some(match files {
                Some(n) => {
                    let unit = if n == 1 { "file" } else { "files" };
                    format!("{at_least}{} {unit} \u{b7} {size}", grouped(n))
                }
                None => size,
            })
        }
    }
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

// ---------------------------------------------------------------------------------------
// Matching and ranking

/// Score tiers; a tier's scores all sort above the tier below it.
const TIER: u32 = 100_000;
const T_EXACT: u32 = 6;
const T_PREFIX: u32 = 5;
const T_WORD: u32 = 4;
const T_SUBSTRING: u32 = 3;
const T_SUBSEQUENCE: u32 = 2;

/// Results kept by `Spotlight::refresh`.
pub const RESULT_LIMIT: usize = 50;

const VK_RETURN: u16 = 0x0D;
const VK_ESCAPE: u16 = 0x1B;
const VK_PRIOR: u16 = 0x21;
const VK_NEXT: u16 = 0x22;
const VK_UP: u16 = 0x26;
const VK_DOWN: u16 = 0x28;

/// One char, Unicode-lowercased (the first char of its lowercase form, so indices stay
/// aligned with the original text).
fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn is_separator(c: char) -> bool {
    matches!(c, ' ' | '-' | '_' | '.')
}

/// Char indices where a word starts: after a separator, or at a lower→Upper boundary.
fn word_starts(name: &[char]) -> Vec<usize> {
    let mut v = Vec::new();
    for i in 0..name.len() {
        if is_separator(name[i]) {
            continue;
        }
        if i == 0 {
            v.push(i);
            continue;
        }
        let prev = name[i - 1];
        if is_separator(prev) || (prev.is_lowercase() && name[i].is_uppercase()) {
            v.push(i);
        }
    }
    v
}

fn tier_score(tier: u32, pos: usize, name_len: usize) -> u32 {
    tier * TIER + (255 - pos.min(255) as u32) * 256 + (255 - name_len.min(255) as u32)
}

/// Matches `query` against `name`, case-insensitively. Returns a score (higher is better)
/// and the matched char indices of `name`. Tiers, best first: the whole name, a prefix, the
/// start of a word (words split on space, `-`, `_`, `.` and camelCase humps; also an
/// acronym over word starts, "vsc" → "Visual Studio Code"), a substring, a subsequence.
/// Within a tier an earlier match and a shorter name win. An empty query matches nothing.
pub fn score(query: &str, name: &str) -> Option<(u32, Vec<usize>)> {
    let q: Vec<char> = query.chars().map(lower).collect();
    if q.is_empty() {
        return None;
    }
    let orig: Vec<char> = name.chars().collect();
    let n = orig.len();
    if q.len() > n {
        return None;
    }
    let low: Vec<char> = orig.iter().map(|c| lower(*c)).collect();
    let range = |s: usize| (s..s + q.len()).collect::<Vec<_>>();

    if low == q {
        return Some((tier_score(T_EXACT, 0, n), range(0)));
    }
    if low[..q.len()] == q[..] {
        return Some((tier_score(T_PREFIX, 0, n), range(0)));
    }
    // Word start: a contiguous match beginning at a word start, else an acronym.
    let starts = word_starts(&orig);
    if let Some(&w) = starts
        .iter()
        .find(|&&w| w + q.len() <= n && low[w..w + q.len()] == q[..])
    {
        return Some((tier_score(T_WORD, w, n), range(w)));
    }
    if q.len() >= 2 {
        let mut matched = Vec::with_capacity(q.len());
        let mut next = 0;
        for &qc in &q {
            match starts[next..].iter().position(|&w| low[w] == qc) {
                Some(off) => {
                    matched.push(starts[next + off]);
                    next += off + 1;
                }
                None => {
                    matched.clear();
                    break;
                }
            }
        }
        if matched.len() == q.len() {
            let last = *matched.last().unwrap_or(&0);
            return Some((tier_score(T_WORD, last, n), matched));
        }
    }
    if let Some(s) = (0..=n - q.len()).find(|&s| low[s..s + q.len()] == q[..]) {
        return Some((tier_score(T_SUBSTRING, s, n), range(s)));
    }
    let mut matched = Vec::with_capacity(q.len());
    let mut qi = 0;
    for (i, c) in low.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            matched.push(i);
            qi += 1;
        }
    }
    if qi == q.len() {
        return Some((tier_score(T_SUBSEQUENCE, matched[0], n), matched));
    }
    None
}

/// Ranks `entries` for `query`: best match first, at most `limit`. At the same tier Apps
/// come before Folders before Files, then the better score, then the entry order, so the
/// result is deterministic. Folders and files are searched only when `include_files` is
/// set and the query has at least `FILE_MIN_QUERY` chars. Returns `(entry index, matched
/// char indices)`.
pub fn rank(
    query: &str,
    entries: &[Entry],
    include_files: bool,
    limit: usize,
) -> Vec<(usize, Vec<usize>)> {
    let files_ok = include_files && query.chars().count() >= FILE_MIN_QUERY;
    let mut hits: Vec<(u32, EntryKind, usize, Vec<usize>)> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| files_ok || e.kind == EntryKind::App)
        .filter_map(|(i, e)| score(query, &e.name).map(|(s, m)| (s, e.kind, i, m)))
        .collect();
    hits.sort_by(|a, b| {
        (b.0 / TIER)
            .cmp(&(a.0 / TIER))
            .then(a.1.cmp(&b.1))
            .then(b.0.cmp(&a.0))
            .then(a.2.cmp(&b.2))
    });
    hits.truncate(limit);
    hits.into_iter().map(|(_, _, i, m)| (i, m)).collect()
}

// ---------------------------------------------------------------------------------------
// The model

/// The Spotlight state: the query, its ranked results and the selection.
#[derive(Clone, Debug)]
pub struct Spotlight {
    query: LineEdit,
    /// `(entry index, matched chars)`, best first.
    results: Vec<(usize, Vec<usize>)>,
    /// Index into `results`.
    selected: usize,
    /// The selected entry's target, so a changed entry list can't move the selection.
    selected_target: Option<String>,
    /// Index into `results` of the first visible row.
    scroll: usize,
    /// A result row under the pointer (index into `results`).
    hover: Option<usize>,
    indexing: bool,
    include_files: bool,
    /// Unused fraction of wheel lines.
    wheel_acc: f32,
}

impl Default for Spotlight {
    fn default() -> Self {
        Self::new()
    }
}

impl Spotlight {
    pub fn new() -> Self {
        Self {
            query: LineEdit::new(),
            results: Vec::new(),
            selected: 0,
            selected_target: None,
            scroll: 0,
            hover: None,
            indexing: false,
            include_files: true,
            wheel_acc: 0.0,
        }
    }

    /// Empty query and no results: call when Spotlight opens. `indexing` stays.
    pub fn reset(&mut self) {
        self.query.clear();
        self.results.clear();
        self.selected = 0;
        self.selected_target = None;
        self.scroll = 0;
        self.hover = None;
        self.wheel_acc = 0.0;
    }

    pub fn query(&self) -> &str {
        self.query.text()
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Index of the first visible result row.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn result_count(&self) -> usize {
        self.results.len()
    }

    /// The selected entry's index in the entry list.
    pub fn selected_entry(&self) -> Option<usize> {
        self.results.get(self.selected).map(|r| r.0)
    }

    pub fn set_indexing(&mut self, indexing: bool) -> bool {
        let changed = self.indexing != indexing;
        self.indexing = indexing;
        changed
    }

    /// Re-runs the ranking for the current query (the entry list or the files setting
    /// changed). Keeps the selected entry when it is still in the results, else selects
    /// the first.
    pub fn refresh(&mut self, entries: &[Entry], include_files: bool) {
        self.include_files = include_files;
        self.rerank(entries, true);
    }

    fn rerank(&mut self, entries: &[Entry], keep_selection: bool) {
        let keep = if keep_selection {
            self.selected_target.take()
        } else {
            None
        };
        self.results = if self.query.is_empty() {
            Vec::new()
        } else {
            rank(self.query.text(), entries, self.include_files, RESULT_LIMIT)
        };
        let selected = keep
            .and_then(|t| {
                self.results
                    .iter()
                    .position(|r| entries.get(r.0).is_some_and(|e| e.target == t))
            })
            .unwrap_or(0);
        if !keep_selection {
            self.scroll = 0;
        }
        self.hover = None;
        self.set_selected(selected, entries);
    }

    fn visible_rows(&self) -> usize {
        self.results.len().min(MAX_ROWS)
    }

    /// Clamps the scroll to the results and moves it so the selection is visible.
    fn fix_scroll(&mut self) {
        let max_scroll = self.results.len().saturating_sub(MAX_ROWS);
        self.scroll = self.scroll.min(max_scroll);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + MAX_ROWS {
            self.scroll = self.selected + 1 - MAX_ROWS;
        }
    }

    fn set_selected(&mut self, i: usize, entries: &[Entry]) {
        self.selected = i;
        self.selected_target = self
            .results
            .get(i)
            .and_then(|r| entries.get(r.0))
            .map(|e| e.target.clone());
        self.fix_scroll();
    }

    fn select(&mut self, i: usize, entries: &[Entry]) -> Action {
        if i == self.selected {
            return Action::None;
        }
        self.set_selected(i, entries);
        Action::Redraw
    }

    fn selected_clone(&self, entries: &[Entry]) -> Option<Entry> {
        self.selected_entry().and_then(|i| entries.get(i)).cloned()
    }

    /// A key down. Up/Down move the selection (wrapping), PageUp/PageDown by a page, Enter
    /// launches, Ctrl+Enter reveals a file or folder (an app launches), Esc clears the
    /// query and then closes; the rest edits the query.
    pub fn key(&mut self, vk: u16, ctrl: bool, shift: bool, entries: &[Entry]) -> Action {
        let n = self.results.len();
        match vk {
            VK_ESCAPE => {
                if self.query.is_empty() {
                    Action::Close
                } else {
                    self.query.clear();
                    self.rerank(entries, false);
                    Action::Redraw
                }
            }
            VK_RETURN => match self.selected_clone(entries) {
                Some(e) if ctrl && e.kind != EntryKind::App => Action::Reveal(e),
                Some(e) => Action::Launch(e),
                None => Action::None,
            },
            VK_UP if n > 0 => self.select((self.selected + n - 1) % n, entries),
            VK_DOWN if n > 0 => self.select((self.selected + 1) % n, entries),
            VK_PRIOR if n > 0 => self.select(self.selected.saturating_sub(MAX_ROWS), entries),
            VK_NEXT if n > 0 => self.select((self.selected + MAX_ROWS).min(n - 1), entries),
            VK_UP | VK_DOWN | VK_PRIOR | VK_NEXT => Action::None,
            _ => {
                let before = self.query.text().to_string();
                if !self.query.key(vk, ctrl, shift) {
                    return Action::None;
                }
                if self.query.text() != before {
                    self.rerank(entries, false);
                }
                Action::Redraw
            }
        }
    }

    /// A typed char.
    pub fn char(&mut self, c: char, entries: &[Entry]) -> Action {
        if !self.query.insert_char(c) {
            return Action::None;
        }
        self.rerank(entries, false);
        Action::Redraw
    }

    /// Pasted text (newlines become spaces).
    pub fn paste(&mut self, s: &str, entries: &[Entry]) -> Action {
        if !self.query.insert_str(s) {
            return Action::None;
        }
        self.rerank(entries, false);
        Action::Redraw
    }

    /// The panel's rect (window-local): the field plus the rows or a note line.
    pub fn panel_rect(&self) -> Rect {
        let lines = if self.results.is_empty() {
            usize::from(self.note().is_some())
        } else {
            self.visible_rows()
        };
        let pad = if lines > 0 { PAD } else { 0.0 };
        Rect::new(
            MARGIN,
            MARGIN,
            PANEL_W,
            FIELD_H + lines as f32 * ROW_H + pad,
        )
    }

    fn row_rect(slot: usize) -> Rect {
        Rect::new(
            MARGIN,
            MARGIN + FIELD_H + slot as f32 * ROW_H,
            PANEL_W,
            ROW_H,
        )
    }

    /// The result (index into `results`) drawn at window-local `(x, y)`.
    fn result_at(&self, x: f32, y: f32) -> Option<usize> {
        (0..self.visible_rows())
            .find(|&slot| Self::row_rect(slot).contains(x, y))
            .map(|slot| self.scroll + slot)
    }

    fn note(&self) -> Option<&'static str> {
        if !self.results.is_empty() || self.query.is_empty() {
            None
        } else if self.indexing {
            Some("Indexing…")
        } else {
            Some("No results")
        }
    }

    /// The pointer moved. Redraw when the hovered row changed.
    pub fn hover(&mut self, x: f32, y: f32) -> Action {
        let h = self.result_at(x, y);
        if h == self.hover {
            return Action::None;
        }
        self.hover = h;
        Action::Redraw
    }

    /// The pointer left the window.
    pub fn leave(&mut self) -> Action {
        if self.hover.take().is_some() {
            Action::Redraw
        } else {
            Action::None
        }
    }

    /// A left click: a row launches it, the field just takes focus, outside the panel closes.
    pub fn press(&mut self, x: f32, y: f32, entries: &[Entry]) -> Action {
        if !self.panel_rect().contains(x, y) {
            return Action::Close;
        }
        match self.result_at(x, y) {
            Some(i) => {
                self.set_selected(i, entries);
                match self.selected_clone(entries) {
                    Some(e) => Action::Launch(e),
                    None => Action::None,
                }
            }
            None => Action::None,
        }
    }

    /// The wheel: positive `lines` scroll down (to later rows). The selection stays.
    pub fn wheel(&mut self, lines: f32) -> Action {
        self.wheel_acc += lines;
        let whole = self.wheel_acc.trunc();
        self.wheel_acc -= whole;
        let max_scroll = self.results.len().saturating_sub(MAX_ROWS);
        let target = (self.scroll as f32 + whole).clamp(0.0, max_scroll as f32) as usize;
        if target == self.scroll {
            return Action::None;
        }
        self.scroll = target;
        self.hover = None;
        Action::Redraw
    }

    /// Everything the window draws.
    pub fn view(&self, entries: &[Entry]) -> SpotlightView {
        self.view_sized(entries, |_| None)
    }

    /// The view with each row's size from `size` (by entry; None: not asked for yet,
    /// shown as nothing).
    pub fn view_sized(
        &self,
        entries: &[Entry],
        size: impl Fn(&Entry) -> Option<SizeInfo>,
    ) -> SpotlightView {
        let panel = self.panel_rect();
        let field = self.query.view(
            Rect::new(MARGIN, MARGIN, PANEL_W, FIELD_H),
            true,
            "Search apps and files",
        );
        let rows = (0..self.visible_rows())
            .filter_map(|slot| {
                let i = self.scroll + slot;
                let (ei, matched) = self.results.get(i)?;
                let e = entries.get(*ei)?;
                Some(ResultRow {
                    rect: Self::row_rect(slot),
                    kind: e.kind,
                    name: e.name.clone(),
                    detail: e.detail.clone(),
                    icon_key: e.icon_key(),
                    selected: i == self.selected,
                    hovered: self.hover == Some(i),
                    matched: matched.clone(),
                    target: e.target.clone(),
                    meta: size(e).and_then(size_text),
                })
            })
            .collect();
        SpotlightView {
            panel,
            field,
            rows,
            note: self.note(),
            hint: "esc",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> Entry {
        Entry {
            kind: EntryKind::App,
            name: name.into(),
            target: format!("app:{name}"),
            detail: "App".into(),
        }
    }

    fn file(name: &str) -> Entry {
        Entry {
            kind: EntryKind::File,
            name: name.into(),
            target: format!("C:\\Users\\me\\Documents\\{name}"),
            detail: "Documents".into(),
        }
    }

    fn folder(name: &str) -> Entry {
        Entry {
            kind: EntryKind::Folder,
            name: name.into(),
            target: format!("C:\\Users\\me\\{name}"),
            detail: "me".into(),
        }
    }

    fn s(q: &str, n: &str) -> u32 {
        score(q, n).unwrap().0
    }

    #[test]
    fn tiers_are_ordered() {
        let exact = s("code", "Code");
        let prefix = s("code", "Codec Pack");
        let word = s("code", "Visual Studio Code");
        let sub = s("code", "Barcode Scanner");
        let seq = s("code", "Cool Dev Editor");
        assert!(exact > prefix, "{exact} {prefix}");
        assert!(prefix > word);
        assert!(word > sub);
        assert!(sub > seq);
        assert!(score("code", "Notepad").is_none());
    }

    #[test]
    fn empty_query_and_long_query_match_nothing() {
        assert!(score("", "Chrome").is_none());
        assert!(score("chromes", "Chrome").is_none());
    }

    #[test]
    fn matched_indices_are_reported() {
        assert_eq!(score("chr", "Chrome").unwrap().1, vec![0, 1, 2]);
        assert_eq!(
            score("studio", "Visual Studio Code").unwrap().1,
            (7..13).collect::<Vec<_>>()
        );
        assert_eq!(
            score("vsc", "Visual Studio Code").unwrap().1,
            vec![0, 7, 14]
        );
        assert_eq!(score("ome", "Chrome").unwrap().1, vec![3, 4, 5]);
        assert_eq!(score("cme", "Chrome").unwrap().1, vec![0, 4, 5]);
    }

    #[test]
    fn acronyms_and_word_starts() {
        assert!(s("vsc", "Visual Studio Code") > s("vsc", "Xvscx"));
        let (sc, _) = score("vsc", "Visual Studio Code").unwrap();
        assert_eq!(sc / TIER, T_WORD);
        // A word start in the middle beats a substring inside a word.
        assert!(s("stu", "Visual Studio") > s("stu", "Bluestudy"));
        // Separators: - _ .
        for name in ["my-notes", "my_notes", "my.notes", "my notes"] {
            assert_eq!(score("notes", name).unwrap().0 / TIER, T_WORD, "{name}");
        }
    }

    #[test]
    fn camel_case_humps_are_word_starts() {
        let (sc, m) = score("code", "VisualStudioCode").unwrap();
        assert_eq!(sc / TIER, T_WORD);
        assert_eq!(m, vec![12, 13, 14, 15]);
        let (sc, _) = score("vsc", "VisualStudioCode").unwrap();
        assert_eq!(sc / TIER, T_WORD);
        // No hump in an all-lowercase name: a plain substring.
        assert_eq!(
            score("code", "visualstudiocode").unwrap().0 / TIER,
            T_SUBSTRING
        );
    }

    #[test]
    fn earlier_matches_and_shorter_names_win_within_a_tier() {
        assert!(s("pad", "Notepad") > s("pad", "Another Notepad")); // earlier substring
        assert!(s("chr", "Chrome") > s("chr", "Chrome Remote Desktop"));
        assert!(s("note", "Notes") > s("note", "Notepad++"));
    }

    #[test]
    fn matching_is_case_insensitive_and_unicode() {
        assert_eq!(s("CHROME", "chrome"), s("chrome", "CHROME"));
        assert_eq!(score("ÉCOLE", "école").unwrap().0 / TIER, T_EXACT);
        let (sc, m) = score("日記", "私の日記帳").unwrap();
        assert_eq!(sc / TIER, T_SUBSTRING);
        assert_eq!(m, vec![2, 3]);
        assert_eq!(score("straße", "Straße").unwrap().0 / TIER, T_EXACT);
    }

    #[test]
    fn rank_orders_by_tier_then_kind() {
        let entries = vec![
            file("chrome notes.txt"),
            app("Google Chrome"),
            app("Chrome Remote Desktop"),
            folder("chrome"),
            app("Chrome"),
        ];
        let r = rank("chrome", &entries, true, 10);
        let order: Vec<usize> = r.iter().map(|x| x.0).collect();
        // exact: Chrome (app 4) before the exact folder (3); prefix: app 2 before file 0;
        // word start: app 1.
        assert_eq!(order, vec![4, 3, 2, 0, 1]);
    }

    #[test]
    fn kind_breaks_ties_at_the_same_tier() {
        let entries = vec![file("report"), folder("report"), app("report")];
        let order: Vec<usize> = rank("report", &entries, true, 10)
            .iter()
            .map(|x| x.0)
            .collect();
        assert_eq!(order, vec![2, 1, 0]);
    }

    #[test]
    fn files_need_the_setting_and_two_chars() {
        let entries = vec![app("Budget"), file("budget.xlsx"), folder("Budget 2026")];
        assert_eq!(rank("b", &entries, true, 10).len(), 1);
        assert_eq!(rank("bu", &entries, true, 10).len(), 3);
        let only_apps = rank("bu", &entries, false, 10);
        assert_eq!(only_apps.len(), 1);
        assert_eq!(only_apps[0].0, 0);
    }

    #[test]
    fn rank_is_deterministic_and_limited() {
        let entries: Vec<Entry> = (0..20).map(|_| app("Same")).collect();
        let a = rank("sa", &entries, true, 5);
        let b = rank("sa", &entries, true, 5);
        assert_eq!(a, b);
        assert_eq!(
            a.iter().map(|x| x.0).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
        assert!(rank("", &entries, true, 5).is_empty());
    }

    fn many(n: usize) -> Vec<Entry> {
        (0..n).map(|i| app(&format!("App {i:02}"))).collect()
    }

    fn typed(sp: &mut Spotlight, text: &str, entries: &[Entry]) {
        for c in text.chars() {
            sp.char(c, entries);
        }
    }

    #[test]
    fn typing_ranks_and_selects_the_first() {
        let entries = vec![app("Notepad"), app("Chrome"), app("Calculator")];
        let mut sp = Spotlight::new();
        assert_eq!(sp.char('c', &entries), Action::Redraw);
        assert_eq!(sp.result_count(), 2);
        sp.key(VK_DOWN, false, false, &entries);
        assert_eq!(sp.selected(), 1);
        // Typing again goes back to the best match.
        sp.char('h', &entries);
        assert_eq!(sp.selected(), 0);
        assert_eq!(sp.selected_entry(), Some(1));
        assert_eq!(sp.char('\u{8}', &entries), Action::None);
    }

    #[test]
    fn up_and_down_wrap_and_scroll_the_window() {
        let entries = many(20);
        let mut sp = Spotlight::new();
        typed(&mut sp, "app", &entries);
        assert_eq!(sp.result_count(), 20);
        assert_eq!((sp.selected(), sp.scroll()), (0, 0));
        // Up from the first wraps to the last, scrolled so it is visible.
        assert_eq!(sp.key(VK_UP, false, false, &entries), Action::Redraw);
        assert_eq!((sp.selected(), sp.scroll()), (19, 12));
        assert_eq!(sp.key(VK_DOWN, false, false, &entries), Action::Redraw);
        assert_eq!((sp.selected(), sp.scroll()), (0, 0));
        for _ in 0..MAX_ROWS {
            sp.key(VK_DOWN, false, false, &entries);
        }
        assert_eq!((sp.selected(), sp.scroll()), (8, 1));
        sp.key(VK_UP, false, false, &entries);
        assert_eq!((sp.selected(), sp.scroll()), (7, 1));
        let v = sp.view(&entries);
        assert_eq!(v.rows.len(), MAX_ROWS);
        assert!(v.rows.iter().any(|r| r.selected));
        assert_eq!(v.rows[0].name, "App 01");
    }

    #[test]
    fn page_keys_move_by_a_page_without_wrapping() {
        let entries = many(20);
        let mut sp = Spotlight::new();
        typed(&mut sp, "app", &entries);
        sp.key(VK_NEXT, false, false, &entries);
        assert_eq!((sp.selected(), sp.scroll()), (8, 1));
        sp.key(VK_NEXT, false, false, &entries);
        sp.key(VK_NEXT, false, false, &entries);
        assert_eq!((sp.selected(), sp.scroll()), (19, 12));
        assert_eq!(sp.key(VK_NEXT, false, false, &entries), Action::None);
        sp.key(VK_PRIOR, false, false, &entries);
        assert_eq!(sp.selected(), 11);
        sp.key(VK_PRIOR, false, false, &entries);
        sp.key(VK_PRIOR, false, false, &entries);
        assert_eq!((sp.selected(), sp.scroll()), (0, 0));
        assert_eq!(sp.key(VK_PRIOR, false, false, &entries), Action::None);
    }

    #[test]
    fn navigation_without_results_does_nothing() {
        let entries = many(3);
        let mut sp = Spotlight::new();
        assert_eq!(sp.key(VK_DOWN, false, false, &entries), Action::None);
        assert_eq!(sp.key(VK_UP, false, false, &entries), Action::None);
        assert_eq!(sp.key(VK_RETURN, false, false, &entries), Action::None);
    }

    #[test]
    fn enter_launches_and_ctrl_enter_reveals_files() {
        let entries = vec![app("Budget"), file("budget.xlsx"), folder("Budget 2026")];
        let mut sp = Spotlight::new();
        typed(&mut sp, "budget", &entries);
        // App first, then the file/folder.
        assert_eq!(
            sp.key(VK_RETURN, false, false, &entries),
            Action::Launch(entries[0].clone())
        );
        // Ctrl+Enter on an app launches it.
        assert_eq!(
            sp.key(VK_RETURN, true, false, &entries),
            Action::Launch(entries[0].clone())
        );
        sp.key(VK_DOWN, false, false, &entries);
        let sel = sp.selected_entry().unwrap();
        assert_ne!(entries[sel].kind, EntryKind::App);
        assert_eq!(
            sp.key(VK_RETURN, true, false, &entries),
            Action::Reveal(entries[sel].clone())
        );
        assert_eq!(
            sp.key(VK_RETURN, false, false, &entries),
            Action::Launch(entries[sel].clone())
        );
    }

    #[test]
    fn escape_clears_then_closes() {
        let entries = many(3);
        let mut sp = Spotlight::new();
        typed(&mut sp, "app", &entries);
        assert_eq!(sp.key(VK_ESCAPE, false, false, &entries), Action::Redraw);
        assert_eq!((sp.query(), sp.result_count()), ("", 0));
        assert_eq!(sp.key(VK_ESCAPE, false, false, &entries), Action::Close);
    }

    #[test]
    fn caret_keys_edit_the_query() {
        let entries = many(3);
        let mut sp = Spotlight::new();
        typed(&mut sp, "app", &entries);
        assert_eq!(sp.key(0x24, false, false, &entries), Action::Redraw); // Home
        assert_eq!(sp.key(0x24, false, false, &entries), Action::None);
        assert_eq!(sp.result_count(), 3);
        assert_eq!(sp.key(0x2E, false, false, &entries), Action::Redraw); // Delete
        assert_eq!(sp.query(), "pp");
        assert_eq!(sp.key(0x41, false, false, &entries), Action::None);
        assert_eq!(sp.paste("x\ny", &entries), Action::Redraw);
        assert_eq!(sp.query(), "x ypp");
        assert_eq!(sp.paste("", &entries), Action::None);
    }

    #[test]
    fn refresh_keeps_the_selected_entry() {
        let mut entries = vec![app("Alpha One"), app("Alpha Two"), app("Alpha Three")];
        let mut sp = Spotlight::new();
        typed(&mut sp, "alpha", &entries);
        sp.key(VK_DOWN, false, false, &entries);
        sp.key(VK_DOWN, false, false, &entries);
        let want = sp.selected_entry().unwrap();
        // A new, better entry arrives in front.
        entries.insert(0, app("Alpha"));
        let want = want + 1;
        sp.refresh(&entries, true);
        assert_eq!(sp.selected_entry(), Some(want));
        assert_ne!(sp.selected(), 0);
        // The selected entry goes away: back to the first.
        entries.remove(want);
        sp.refresh(&entries, true);
        assert_eq!(sp.selected(), 0);
        // Files appear only with the setting.
        entries.push(file("alpha.txt"));
        sp.refresh(&entries, false);
        assert_eq!(sp.result_count(), 3);
        sp.refresh(&entries, true);
        assert_eq!(sp.result_count(), 4);
    }

    #[test]
    fn reset_clears_the_query_and_results() {
        let entries = many(3);
        let mut sp = Spotlight::new();
        typed(&mut sp, "app", &entries);
        sp.reset();
        assert_eq!((sp.query(), sp.result_count(), sp.selected()), ("", 0, 0));
        assert_eq!(sp.view(&entries).note, None);
    }

    #[test]
    fn view_geometry_follows_the_result_count() {
        let panel_h = |n: usize| {
            let entries = many(n);
            let mut sp = Spotlight::new();
            typed(&mut sp, "app", &entries);
            let v = sp.view(&entries);
            assert_eq!(v.rows.len(), n.min(MAX_ROWS));
            assert_eq!(v.note, None);
            (v.panel, v.rows)
        };
        let (p, rows) = panel_h(1);
        assert_eq!(p, Rect::new(MARGIN, MARGIN, PANEL_W, FIELD_H + ROW_H + PAD));
        assert_eq!(
            rows[0].rect,
            Rect::new(MARGIN, MARGIN + FIELD_H, PANEL_W, ROW_H)
        );
        let (p, rows) = panel_h(8);
        assert_eq!(p.h, FIELD_H + 8.0 * ROW_H + PAD);
        assert_eq!(rows[7].rect.y, MARGIN + FIELD_H + 7.0 * ROW_H);
        // 20 results still show at most 8 rows.
        let (p, _) = panel_h(20);
        assert_eq!(p.h, FIELD_H + MAX_ROWS as f32 * ROW_H + PAD);
        // Never taller than the window allows.
        assert!(p.y + p.h + MARGIN <= WIN_H);
        assert_eq!(p.w, PANEL_W);
    }

    #[test]
    fn the_field_is_the_top_of_the_panel() {
        let entries = many(3);
        let mut sp = Spotlight::new();
        typed(&mut sp, "ap", &entries);
        let v = sp.view(&entries);
        assert_eq!(v.field.rect, Rect::new(MARGIN, MARGIN, PANEL_W, FIELD_H));
        assert_eq!((v.field.text.as_str(), v.field.caret), ("ap", 2));
        assert!(v.field.focused);
        assert_eq!(v.hint, "esc");
        assert_eq!(v.rows[0].icon_key, "app:App 00");
        assert_eq!(v.rows[0].matched, vec![0, 1]);
    }

    #[test]
    fn notes_replace_the_rows() {
        let entries = many(3);
        let mut sp = Spotlight::new();
        // Empty query: just the field.
        let v = sp.view(&entries);
        assert_eq!((v.note, v.rows.len(), v.panel.h), (None, 0, FIELD_H));
        typed(&mut sp, "zzz", &entries);
        let v = sp.view(&entries);
        assert_eq!(v.note, Some("No results"));
        assert_eq!(v.panel.h, FIELD_H + ROW_H + PAD);
        assert!(sp.set_indexing(true));
        assert!(!sp.set_indexing(true));
        assert_eq!(sp.view(&entries).note, Some("Indexing…"));
        // Indexing with an empty query shows nothing; with results, the rows.
        sp.reset();
        assert_eq!(sp.view(&entries).note, None);
        typed(&mut sp, "app", &entries);
        assert_eq!(sp.view(&entries).note, None);
    }

    #[test]
    fn hover_and_press_hit_the_rows() {
        let entries = many(5);
        let mut sp = Spotlight::new();
        typed(&mut sp, "app", &entries);
        let row = |i: usize| {
            (
                MARGIN + 10.0,
                MARGIN + FIELD_H + i as f32 * ROW_H + ROW_H / 2.0,
            )
        };
        let (x, y) = row(2);
        assert_eq!(sp.hover(x, y), Action::Redraw);
        assert_eq!(sp.hover(x, y + 1.0), Action::None);
        assert!(sp.view(&entries).rows[2].hovered);
        assert_eq!(sp.hover(MARGIN + 5.0, MARGIN + 5.0), Action::Redraw);
        assert_eq!(sp.leave(), Action::None);
        sp.hover(x, y);
        assert_eq!(sp.leave(), Action::Redraw);
        assert_eq!(sp.press(x, y, &entries), Action::Launch(entries[2].clone()));
        // The field takes focus; outside the panel closes.
        assert_eq!(
            sp.press(MARGIN + 20.0, MARGIN + 10.0, &entries),
            Action::None
        );
        assert_eq!(sp.press(2.0, 2.0, &entries), Action::Close);
        // Below the last row (padding / empty window area) is outside the panel.
        let below = sp.panel_rect().y + sp.panel_rect().h + 5.0;
        assert_eq!(sp.press(MARGIN + 20.0, below, &entries), Action::Close);
    }

    #[test]
    fn the_wheel_scrolls_the_window_and_clamps() {
        let entries = many(20);
        let mut sp = Spotlight::new();
        typed(&mut sp, "app", &entries);
        assert_eq!(sp.wheel(3.0), Action::Redraw);
        assert_eq!((sp.scroll(), sp.selected()), (3, 0));
        assert_eq!(sp.wheel(-10.0), Action::Redraw);
        assert_eq!(sp.scroll(), 0);
        assert_eq!(sp.wheel(-1.0), Action::None);
        assert_eq!(sp.wheel(100.0), Action::Redraw);
        assert_eq!(sp.scroll(), 12);
        assert_eq!(sp.wheel(1.0), Action::None);
        // Fractions add up.
        sp.wheel(-12.0);
        assert_eq!(sp.wheel(0.5), Action::None);
        assert_eq!(sp.wheel(0.5), Action::Redraw);
        assert_eq!(sp.scroll(), 1);
        // A row click after scrolling picks the row that is drawn there.
        let y = MARGIN + FIELD_H + ROW_H / 2.0;
        assert_eq!(
            sp.press(MARGIN + 5.0, y, &entries),
            Action::Launch(entries[1].clone())
        );
        // Few results: nothing to scroll.
        let mut few = Spotlight::new();
        typed(&mut few, "app 00", &entries);
        assert_eq!(few.wheel(2.0), Action::None);
    }

    #[test]
    fn size_text_for_each_kind() {
        assert_eq!(size_text(SizeInfo::Pending).as_deref(), Some("\u{2026}"));
        assert_eq!(size_text(SizeInfo::Unknown), None);
        let file = SizeInfo::Known {
            bytes: 48 * 1024,
            files: None,
            capped: false,
        };
        assert_eq!(size_text(file).as_deref(), Some("48 KB"));
        let folder = SizeInfo::Known {
            bytes: 4_509_715_660,
            files: Some(1234),
            capped: false,
        };
        assert_eq!(
            size_text(folder).as_deref(),
            Some("1,234 files \u{b7} 4.2 GB")
        );
        let one = SizeInfo::Known {
            bytes: 10,
            files: Some(1),
            capped: false,
        };
        assert_eq!(size_text(one).as_deref(), Some("1 file \u{b7} 10 B"));
        let big = SizeInfo::Known {
            bytes: 13 * 1024 * 1024 * 1024,
            files: Some(200_000),
            capped: true,
        };
        assert_eq!(
            size_text(big).as_deref(),
            Some("\u{2265} 200,000 files \u{b7} \u{2265} 13 GB")
        );
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn rows_carry_their_target_and_size() {
        let entries = vec![app("Chrome"), app("Chromium")];
        let mut s = Spotlight::new();
        s.char('c', &entries);
        s.refresh(&entries, true);
        let v = s.view(&entries);
        assert!(v.rows.iter().all(|r| r.meta.is_none()));
        assert_eq!(v.rows[0].target, "app:Chrome");
        let v = s.view_sized(&entries, |e| {
            (e.name == "Chrome").then_some(SizeInfo::Pending)
        });
        assert_eq!(v.rows[0].meta.as_deref(), Some("\u{2026}"));
        assert_eq!(v.rows[1].meta, None);
    }

    #[test]
    fn installed_size_matches_names() {
        let list = vec![
            ("Google Chrome".to_string(), 500),
            ("Steam (x64)".to_string(), 300),
            ("Microsoft Visual Studio Code (User)".to_string(), 400),
            ("Microsoft Edge".to_string(), 600),
            ("Microsoft Edge WebView2 Runtime".to_string(), 700),
            ("Zoom".to_string(), 50),
        ];
        assert_eq!(installed_size("Google Chrome", &list), Some(500));
        assert_eq!(installed_size("google chrome ", &list), Some(500));
        assert_eq!(installed_size("Steam", &list), Some(300));
        assert_eq!(installed_size("Microsoft Edge", &list), Some(600));
        // Not a prefix at a word boundary: no guess.
        assert_eq!(installed_size("Visual Studio Code", &list), None);
        assert_eq!(installed_size("Zoo", &list), None);
        assert_eq!(installed_size("Notepad", &list), None);
        assert_eq!(installed_size("ab", &list), None);
    }
}
