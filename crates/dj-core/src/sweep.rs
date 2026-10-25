//! Sweep, the cleaner inside the Vitals panel: categories, the reclaimable-space cache,
//! byte formatting and the Sweep view's state and hit testing. Pure; the scanners
//! and the delete code live in `dj-sweep`.

use crate::layout::{self, Rect};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The reclaimable estimate is refreshed when it is older than this (and the panel opens).
pub const CACHE_MAX_AGE_SECS: u64 = 6 * 60 * 60;
/// Rows per page in a category preview list.
pub const PAGE: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    /// User `%TEMP%` (older than 7 days), WER reports and old crash dumps.
    Temp,
    /// Chromium-family and Firefox disk caches.
    Browser,
    /// Electron app caches and package-manager caches.
    AppCache,
    /// Build artifacts in old projects (`node_modules`, `target`, ...).
    DevPurge,
    /// The largest files in the profile. Handled one by one, never by "Clean".
    BigFiles,
}

/// How a category's items leave the disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    /// Temp and cache entries only.
    Permanent,
    /// To the Recycle Bin, so the user can restore them.
    Recycle,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Temp,
        Category::Browser,
        Category::AppCache,
        Category::DevPurge,
        Category::BigFiles,
    ];

    /// Row label in the Sweep view.
    pub fn label(self) -> &'static str {
        match self {
            Category::Temp => "temp & reports",
            Category::Browser => "browser caches",
            Category::AppCache => "app & dev caches",
            Category::DevPurge => "dev purge",
            Category::BigFiles => "big files",
        }
    }

    /// Key in `sweep-cache.toml`.
    pub fn key(self) -> &'static str {
        match self {
            Category::Temp => "temp",
            Category::Browser => "browser",
            Category::AppCache => "app_cache",
            Category::DevPurge => "dev_purge",
            Category::BigFiles => "big_files",
        }
    }

    pub fn removal(self) -> Removal {
        match self {
            Category::Temp | Category::Browser | Category::AppCache => Removal::Permanent,
            Category::DevPurge | Category::BigFiles => Removal::Recycle,
        }
    }

    /// True for the categories that "Clean" handles and that count as reclaimable.
    /// Big files are only ever recycled one at a time, by hand.
    pub fn in_clean(self) -> bool {
        self != Category::BigFiles
    }

    /// Ticked when the view opens. Dev purge moves projects' folders, so it is opt-in.
    pub fn default_checked(self) -> bool {
        matches!(
            self,
            Category::Temp | Category::Browser | Category::AppCache
        )
    }

    fn index(self) -> usize {
        match self {
            Category::Temp => 0,
            Category::Browser => 1,
            Category::AppCache => 2,
            Category::DevPurge => 3,
            Category::BigFiles => 4,
        }
    }
}

/// Formats a byte count with a 1024 base: "B", "KB", "MB", "GB" or "TB".
/// One decimal below 10 units, none from 10 up; plain bytes never have decimals.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if v < 9.95 {
        format!("{v:.1} {}", UNITS[unit])
    } else {
        format!("{v:.0} {}", UNITS[unit])
    }
}

/// `~/.daijobu/sweep-cache.toml`: the last background estimate, per category.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SweepCache {
    /// Unix seconds of the scan. 0 means never.
    pub updated_unix: u64,
    /// Bytes per category key (see `Category::key`).
    pub bytes: BTreeMap<String, u64>,
}

impl SweepCache {
    /// Lenient: an unreadable file is the same as no cache.
    pub fn parse(s: &str) -> Self {
        toml::from_str(s).unwrap_or_default()
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_totals(now_unix: u64, totals: &[(Category, u64)]) -> Self {
        Self {
            updated_unix: now_unix,
            bytes: totals
                .iter()
                .map(|(c, b)| (c.key().to_string(), *b))
                .collect(),
        }
    }

    /// True when the estimate should be redone: never scanned, older than six hours,
    /// or stamped in the future (the clock moved back).
    pub fn is_stale(&self, now_unix: u64) -> bool {
        self.updated_unix == 0
            || now_unix < self.updated_unix
            || now_unix - self.updated_unix >= CACHE_MAX_AGE_SECS
    }

    /// Bytes for one category, 0 if unknown.
    pub fn get(&self, cat: Category) -> u64 {
        self.bytes.get(cat.key()).copied().unwrap_or(0)
    }

    /// The "X reclaimable" figure: every category that "Clean" handles. None before
    /// the first scan.
    pub fn reclaimable(&self) -> Option<u64> {
        (self.updated_unix != 0).then(|| {
            Category::ALL
                .iter()
                .filter(|c| c.in_clean())
                .map(|c| self.get(*c))
                .fold(0u64, u64::saturating_add)
        })
    }
}

/// One previewed entry: what would be removed and how big it is.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemView {
    pub path: PathBuf,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatView {
    pub cat: Category,
    pub checked: bool,
    /// Largest first.
    pub items: Vec<ItemView>,
    pub bytes: u64,
    /// True once this category's preview scan has finished.
    pub scanned: bool,
    /// Why something was left out, e.g. "chrome is running".
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// The preview scan is running. Rows fill in as their category finishes.
    Scanning,
    /// The preview is complete. "Clean" is enabled when something is selected.
    Ready,
    Cleaning { done: usize, total: usize },
    /// The result line: bytes freed and items that could not be removed.
    Done { freed: u64, failed: usize },
}

/// A pending two-step action: the first click arms it, the second runs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirm {
    EmptyBin,
    /// Index into the big-files list.
    Recycle(usize),
}

/// What the app must do after a click in the Sweep view.
#[derive(Clone, Debug, PartialEq)]
pub enum SweepCmd {
    /// Back to the Vitals panel.
    Back,
    /// Remove every previewed item of the ticked categories.
    Clean,
    FlushDns,
    EmptyBin,
    Reveal(PathBuf),
    Recycle(PathBuf),
}

/// The Sweep view's state. The app owns it, feeds it scan results and runs the
/// commands that `click` returns.
#[derive(Clone, Debug, PartialEq)]
pub struct SweepView {
    pub phase: Phase,
    /// One per `Category::ALL`, in that order.
    pub cats: Vec<CatView>,
    /// The category whose item list is open, and its page.
    pub preview: Option<(Category, usize)>,
    pub confirm: Option<Confirm>,
    /// One status line for quick actions and confirmations.
    pub status: Option<String>,
}

impl Default for SweepView {
    fn default() -> Self {
        Self::new()
    }
}

impl SweepView {
    pub fn new() -> Self {
        Self {
            phase: Phase::Scanning,
            cats: Category::ALL
                .iter()
                .map(|&cat| CatView {
                    cat,
                    checked: cat.default_checked(),
                    items: Vec::new(),
                    bytes: 0,
                    scanned: false,
                    note: None,
                })
                .collect(),
            preview: None,
            confirm: None,
            status: None,
        }
    }

    pub fn cat(&self, cat: Category) -> &CatView {
        &self.cats[cat.index()]
    }

    fn cat_mut(&mut self, cat: Category) -> &mut CatView {
        &mut self.cats[cat.index()]
    }

    /// Clears the previous preview and enters `Scanning`. Ticks are kept.
    pub fn start_scan(&mut self) {
        for c in &mut self.cats {
            c.items.clear();
            c.bytes = 0;
            c.scanned = false;
            c.note = None;
        }
        self.phase = Phase::Scanning;
        self.preview = None;
        self.confirm = None;
        self.status = None;
    }

    /// Stores one category's preview. Items are sorted largest first.
    pub fn set_category(&mut self, cat: Category, mut items: Vec<ItemView>, note: Option<String>) {
        items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
        let c = self.cat_mut(cat);
        c.bytes = items.iter().map(|i| i.bytes).fold(0, u64::saturating_add);
        c.items = items;
        c.scanned = true;
        c.note = note;
    }

    /// The preview is complete: "Clean" may enable.
    pub fn finish_scan(&mut self) {
        if self.phase == Phase::Scanning {
            self.phase = Phase::Ready;
        }
    }

    /// Bytes in the ticked categories that "Clean" would handle.
    pub fn selected_bytes(&self) -> u64 {
        self.cats
            .iter()
            .filter(|c| c.checked && c.cat.in_clean())
            .map(|c| c.bytes)
            .fold(0, u64::saturating_add)
    }

    /// Bytes in every category that "Clean" handles, ticked or not.
    pub fn reclaimable_bytes(&self) -> u64 {
        self.cats
            .iter()
            .filter(|c| c.cat.in_clean())
            .map(|c| c.bytes)
            .fold(0, u64::saturating_add)
    }

    /// Per-category totals, for the cache.
    pub fn totals(&self) -> Vec<(Category, u64)> {
        self.cats.iter().map(|c| (c.cat, c.bytes)).collect()
    }

    /// Clean is enabled only after a finished preview, with something ticked.
    pub fn can_clean(&self) -> bool {
        self.phase == Phase::Ready && self.selected_bytes() > 0
    }

    /// The previewed items that "Clean" would remove, with their category.
    pub fn selection(&self) -> Vec<(Category, ItemView)> {
        self.cats
            .iter()
            .filter(|c| c.checked && c.cat.in_clean())
            .flat_map(|c| c.items.iter().map(move |i| (c.cat, i.clone())))
            .collect()
    }

    pub fn begin_clean(&mut self, total: usize) {
        self.phase = Phase::Cleaning { done: 0, total };
        self.preview = None;
        self.confirm = None;
        self.status = None;
    }

    pub fn clean_progress(&mut self, done: usize, total: usize) {
        if matches!(self.phase, Phase::Cleaning { .. }) {
            self.phase = Phase::Cleaning { done, total };
        }
    }

    /// Shows the result and drops the items that are gone from the lists.
    pub fn finish_clean(&mut self, freed: u64, failed: usize, gone: &[PathBuf]) {
        for p in gone {
            self.forget(p);
        }
        self.phase = Phase::Done { freed, failed };
    }

    /// Drops one item (after it was recycled on its own).
    pub fn forget(&mut self, path: &PathBuf) {
        for c in &mut self.cats {
            if let Some(i) = c.items.iter().position(|it| &it.path == path) {
                let it = c.items.remove(i);
                c.bytes = c.bytes.saturating_sub(it.bytes);
            }
        }
    }

    /// Number of pages in a category's list (at least 1).
    pub fn pages(&self, cat: Category) -> usize {
        self.cat(cat).items.len().div_ceil(PAGE).max(1)
    }

    /// The rows shown on the open preview page, with their index in the full list.
    pub fn page_items(&self) -> Vec<(usize, &ItemView)> {
        match self.preview {
            Some((cat, page)) => self
                .cat(cat)
                .items
                .iter()
                .enumerate()
                .skip(page * PAGE)
                .take(PAGE)
                .collect(),
            None => Vec::new(),
        }
    }

    /// Handles a click at pill-local `(x, y)`. UI-only clicks (ticks, previews, pages,
    /// the first click of a confirm) change the view and return None.
    pub fn click(&mut self, x: f32, y: f32) -> Option<SweepCmd> {
        let armed = self.confirm.take();
        let hit = |r: Rect| r.contains(x, y);
        let busy = matches!(self.phase, Phase::Cleaning { .. });

        if hit(layout::SW_BACK) {
            if self.preview.take().is_some() {
                return None;
            }
            return Some(SweepCmd::Back);
        }
        if hit(layout::SW_CLEAN) {
            return match self.phase {
                Phase::Done { .. } => Some(SweepCmd::Back),
                _ if self.can_clean() => Some(SweepCmd::Clean),
                _ => None,
            };
        }

        if let Some((cat, page)) = self.preview {
            return self.preview_click(cat, page, x, y, armed, busy);
        }

        if !busy {
            if hit(layout::SW_DNS) {
                return Some(SweepCmd::FlushDns);
            }
            if hit(layout::SW_BIN) {
                if armed == Some(Confirm::EmptyBin) {
                    self.status = None;
                    return Some(SweepCmd::EmptyBin);
                }
                self.confirm = Some(Confirm::EmptyBin);
                self.status = Some("click again to empty the bin".into());
                return None;
            }
        }
        for (i, cat) in Category::ALL.iter().enumerate() {
            let row = layout::sw_row(i);
            if !hit(row) {
                continue;
            }
            let wants_list = !cat.in_clean() || x >= layout::SW_ROW_SPLIT;
            if wants_list {
                if self.cat(*cat).scanned && !busy {
                    self.preview = Some((*cat, 0));
                }
            } else if !busy {
                let c = self.cat_mut(*cat);
                c.checked = !c.checked;
            }
            return None;
        }
        if armed.is_some() {
            self.status = None;
        }
        None
    }

    fn preview_click(
        &mut self,
        cat: Category,
        page: usize,
        x: f32,
        y: f32,
        armed: Option<Confirm>,
        busy: bool,
    ) -> Option<SweepCmd> {
        let hit = |r: Rect| r.contains(x, y);
        let pages = self.pages(cat);
        if hit(layout::SW_PREV) {
            self.preview = Some((cat, page.saturating_sub(1)));
            return None;
        }
        if hit(layout::SW_NEXT) {
            self.preview = Some((cat, (page + 1).min(pages - 1)));
            return None;
        }
        if cat != Category::BigFiles || busy {
            return None;
        }
        let rows: Vec<(usize, PathBuf)> = self
            .page_items()
            .into_iter()
            .map(|(i, it)| (i, it.path.clone()))
            .collect();
        for (slot, (index, path)) in rows.into_iter().enumerate() {
            if hit(layout::sw_reveal(slot)) {
                return Some(SweepCmd::Reveal(path));
            }
            if hit(layout::sw_recycle(slot)) {
                if armed == Some(Confirm::Recycle(index)) {
                    self.status = None;
                    return Some(SweepCmd::Recycle(path));
                }
                self.confirm = Some(Confirm::Recycle(index));
                self.status = Some("click again to recycle".into());
                return None;
            }
        }
        if armed.is_some() {
            self.status = None;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(p: &str, bytes: u64) -> ItemView {
        ItemView {
            path: PathBuf::from(p),
            bytes,
        }
    }

    fn mid(r: Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    fn ready_view() -> SweepView {
        let mut v = SweepView::new();
        v.set_category(Category::Temp, vec![item("t1", 100), item("t2", 300)], None);
        v.set_category(Category::Browser, vec![item("b1", 50)], None);
        v.set_category(Category::AppCache, vec![], None);
        v.set_category(Category::DevPurge, vec![item("p1", 1000)], None);
        v.set_category(
            Category::BigFiles,
            (0..8).map(|i| item(&format!("big{i}"), 5000 - i)).collect(),
            None,
        );
        v.finish_scan();
        v
    }

    #[test]
    fn bytes_format_like_the_mockup() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(412 * 1024 * 1024), "412 MB");
        assert_eq!(format_bytes((6.8 * 1024.0 * 1024.0 * 1024.0) as u64), "6.8 GB");
        assert_eq!(format_bytes(u64::MAX), "16777216 TB");
    }

    #[test]
    fn categories_removal_and_defaults() {
        assert_eq!(Category::Temp.removal(), Removal::Permanent);
        assert_eq!(Category::Browser.removal(), Removal::Permanent);
        assert_eq!(Category::AppCache.removal(), Removal::Permanent);
        assert_eq!(Category::DevPurge.removal(), Removal::Recycle);
        assert_eq!(Category::BigFiles.removal(), Removal::Recycle);
        assert!(!Category::BigFiles.in_clean());
        assert!(!Category::DevPurge.default_checked());
        assert!(Category::Temp.default_checked());
    }

    #[test]
    fn cache_roundtrip_staleness_and_total() {
        let c = SweepCache::from_totals(
            1_000_000,
            &[
                (Category::Temp, 10),
                (Category::DevPurge, 5),
                (Category::BigFiles, 999),
            ],
        );
        assert_eq!(SweepCache::parse(&c.to_toml()), c);
        assert_eq!(c.reclaimable(), Some(15));
        assert!(!c.is_stale(1_000_000 + CACHE_MAX_AGE_SECS - 1));
        assert!(c.is_stale(1_000_000 + CACHE_MAX_AGE_SECS));
        assert!(c.is_stale(999_999), "a stamp in the future is stale");
        let none = SweepCache::default();
        assert!(none.is_stale(5));
        assert_eq!(none.reclaimable(), None);
        assert_eq!(SweepCache::parse("garbage ["), SweepCache::default());
    }

    #[test]
    fn set_category_sorts_and_sums() {
        let v = ready_view();
        let t = v.cat(Category::Temp);
        assert_eq!(t.bytes, 400);
        assert_eq!(t.items[0].path, PathBuf::from("t2"));
        // Temp + Browser are ticked; dev purge is not; big files never count.
        assert_eq!(v.selected_bytes(), 450);
        assert_eq!(v.reclaimable_bytes(), 1450);
        assert_eq!(v.selection().len(), 3);
    }

    #[test]
    fn clean_needs_a_finished_preview() {
        let mut v = SweepView::new();
        v.set_category(Category::Temp, vec![item("t", 10)], None);
        assert!(!v.can_clean(), "still scanning");
        assert_eq!(v.click(mid(layout::SW_CLEAN).0, mid(layout::SW_CLEAN).1), None);
        v.finish_scan();
        assert!(v.can_clean());
        let (x, y) = mid(layout::SW_CLEAN);
        assert_eq!(v.click(x, y), Some(SweepCmd::Clean));
    }

    #[test]
    fn nothing_selected_keeps_clean_disabled() {
        let mut v = ready_view();
        for i in 0..3 {
            let r = layout::sw_row(i);
            v.click(r.x + 10.0, r.y + r.h / 2.0);
        }
        assert_eq!(v.selected_bytes(), 0);
        assert!(!v.can_clean());
    }

    #[test]
    fn row_left_toggles_and_row_right_opens_the_list() {
        let mut v = ready_view();
        let r = layout::sw_row(3);
        assert_eq!(v.click(r.x + 10.0, r.y + 5.0), None);
        assert!(v.cat(Category::DevPurge).checked);
        assert_eq!(v.selected_bytes(), 1450);
        assert_eq!(v.click(layout::SW_ROW_SPLIT + 10.0, r.y + 5.0), None);
        assert_eq!(v.preview, Some((Category::DevPurge, 0)));
        // Back closes the list first, then leaves the view.
        let (bx, by) = mid(layout::SW_BACK);
        assert_eq!(v.click(bx, by), None);
        assert_eq!(v.preview, None);
        assert_eq!(v.click(bx, by), Some(SweepCmd::Back));
    }

    #[test]
    fn big_files_row_only_opens_the_list() {
        let mut v = ready_view();
        let r = layout::sw_row(4);
        v.click(r.x + 10.0, r.y + 5.0);
        assert!(!v.cat(Category::BigFiles).checked);
        assert_eq!(v.preview, Some((Category::BigFiles, 0)));
    }

    #[test]
    fn pages_clamp() {
        let mut v = ready_view();
        v.preview = Some((Category::BigFiles, 0));
        assert_eq!(v.pages(Category::BigFiles), 2);
        assert_eq!(v.page_items().len(), PAGE);
        let (nx, ny) = mid(layout::SW_NEXT);
        v.click(nx, ny);
        v.click(nx, ny);
        assert_eq!(v.preview, Some((Category::BigFiles, 1)));
        assert_eq!(v.page_items().len(), 2);
        assert_eq!(v.page_items()[0].0, 6);
        let (px, py) = mid(layout::SW_PREV);
        v.click(px, py);
        v.click(px, py);
        assert_eq!(v.preview, Some((Category::BigFiles, 0)));
        assert_eq!(v.pages(Category::AppCache), 1);
    }

    #[test]
    fn recycle_and_empty_bin_need_two_clicks() {
        let mut v = ready_view();
        let (bx, by) = mid(layout::SW_BIN);
        assert_eq!(v.click(bx, by), None);
        assert_eq!(v.confirm, Some(Confirm::EmptyBin));
        assert_eq!(v.click(bx, by), Some(SweepCmd::EmptyBin));
        // Another click in between disarms it.
        v.click(bx, by);
        v.click(5.0, 230.0);
        assert_eq!(v.confirm, None);
        assert_eq!(v.click(bx, by), None);

        v.preview = Some((Category::BigFiles, 0));
        v.confirm = None;
        let (rx, ry) = mid(layout::sw_recycle(1));
        assert_eq!(v.click(rx, ry), None);
        assert_eq!(v.confirm, Some(Confirm::Recycle(1)));
        assert_eq!(
            v.click(rx, ry),
            Some(SweepCmd::Recycle(PathBuf::from("big1")))
        );
        let (gx, gy) = mid(layout::sw_reveal(0));
        assert_eq!(v.click(gx, gy), Some(SweepCmd::Reveal(PathBuf::from("big0"))));
    }

    #[test]
    fn dns_is_one_click() {
        let mut v = ready_view();
        let (x, y) = mid(layout::SW_DNS);
        assert_eq!(v.click(x, y), Some(SweepCmd::FlushDns));
    }

    #[test]
    fn cleaning_locks_the_controls_and_done_returns() {
        let mut v = ready_view();
        v.begin_clean(3);
        let r = layout::sw_row(0);
        v.click(r.x + 10.0, r.y + 5.0);
        assert!(v.cat(Category::Temp).checked, "ticks are locked while cleaning");
        let (x, y) = mid(layout::SW_DNS);
        assert_eq!(v.click(x, y), None);
        v.clean_progress(2, 3);
        assert_eq!(v.phase, Phase::Cleaning { done: 2, total: 3 });
        v.finish_clean(400, 1, &[PathBuf::from("t1"), PathBuf::from("t2")]);
        assert_eq!(v.phase, Phase::Done { freed: 400, failed: 1 });
        assert_eq!(v.cat(Category::Temp).bytes, 0);
        assert!(!v.can_clean());
        let (cx, cy) = mid(layout::SW_CLEAN);
        assert_eq!(v.click(cx, cy), Some(SweepCmd::Back));
    }

    #[test]
    fn start_scan_resets_the_preview_but_keeps_ticks() {
        let mut v = ready_view();
        let r = layout::sw_row(3);
        v.click(r.x + 10.0, r.y + 5.0);
        v.start_scan();
        assert_eq!(v.phase, Phase::Scanning);
        assert!(v.cat(Category::DevPurge).checked);
        assert_eq!(v.reclaimable_bytes(), 0);
        assert!(v.cats.iter().all(|c| !c.scanned));
        // Unscanned rows do not open a list.
        let r = layout::sw_row(0);
        v.click(layout::SW_ROW_SPLIT + 5.0, r.y + 5.0);
        assert_eq!(v.preview, None);
    }
}
