//! The terminal's tabs: their order, pinning, the tab bar's layout and hit testing,
//! drag-to-reorder and the tab menu. Pure: the app keys its shell sessions by [`TabId`] and
//! `dj-render` draws a [`TabBarView`].

use crate::layout::{Rect, TERM_W};
use serde::{Deserialize, Serialize};
use toml::Value;

/// At most this many tabs; "+" is gone at the limit.
pub const MAX_TABS: usize = 12;

/// The strip the tabs and "+" use in the header (panel-local px): after the lamp, before
/// the panel's close button.
pub const TABS: Rect = Rect::new(30.0, 6.0, TERM_W - 30.0 - 46.0, 24.0);
/// A pinned tab's width; a regular tab's widest and narrowest (tabs shrink to fit).
pub const PINNED_W: f32 = 76.0;
pub const TAB_MAX_W: f32 = 168.0;
pub const TAB_MIN_W: f32 = 64.0;
/// Between tabs, and "+"'s square.
pub const TAB_GAP: f32 = 2.0;
pub const PLUS_W: f32 = 24.0;
/// A press has to move this far (panel px) before it drags the tab.
pub const DRAG_SLOP: f32 = 4.0;
/// The tab menu: its width and one row's height.
pub const MENU_W: f32 = 168.0;
pub const MENU_ROW_H: f32 = 26.0;

/// Rename dialog and controls, in terminal panel-local logical pixels.
pub const RENAME_DIALOG: Rect = Rect::new(190.0, 52.0, 380.0, 154.0);
pub const RENAME_FIELD: Rect = Rect::new(206.0, 88.0, 348.0, 34.0);
pub const RENAME_SAVE: Rect = Rect::new(466.0, 162.0, 88.0, 28.0);
pub const RENAME_CANCEL: Rect = Rect::new(370.0, 162.0, 88.0, 28.0);

#[derive(Clone, Debug, PartialEq)]
pub struct RenameView {
    pub field: crate::settings_ui::FieldView,
    pub selected: bool,
}

/// A tab's stable id; the app keys its sessions by it.
pub type TabId = u64;

/// What a point on the tab bar is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarHit {
    /// Display index.
    Tab(usize),
    /// The ✕ of the tab at this display index.
    Close(usize),
    Plus,
}

/// One tab as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct TabSlot {
    pub id: TabId,
    /// Panel-local; while dragged, it follows the pointer.
    pub rect: Rect,
    pub title: String,
    pub pinned: bool,
    pub active: bool,
    /// New output since the tab was last shown.
    pub activity: bool,
    /// Its shell has exited.
    pub exited: bool,
    /// The ✕, when shown (regular tabs, active or hovered).
    pub close: Option<Rect>,
    /// Being dragged (drawn last, lifted).
    pub dragging: bool,
}

/// A menu entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Pin,
    Unpin,
    Close,
    CloseOthers,
    CloseRight,
    Rename,
    /// The "+" menu: a new shell tab.
    NewShell,
    /// The "+" menu: a new tab running Claude (yolo).
    NewClaude,
    /// The "+" menu: a new tab running Codex (yolo).
    NewCodex,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuItemView {
    pub action: MenuAction,
    pub label: &'static str,
    /// A dim hint drawn on the right ("" for none).
    pub hint: &'static str,
    pub rect: Rect,
    pub enabled: bool,
}

/// The open tab menu (panel-local).
#[derive(Clone, Debug, PartialEq)]
pub struct MenuView {
    pub rect: Rect,
    pub items: Vec<MenuItemView>,
    pub hover: Option<usize>,
}

/// Everything the header draws for the tabs.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TabBarView {
    pub tabs: Vec<TabSlot>,
    /// `None` at `MAX_TABS`.
    pub plus: Option<Rect>,
    pub hover: Option<BarHit>,
    pub menu: Option<MenuView>,
    pub rename: Option<RenameView>,
}

/// A tab shortcut (the shell never sees these keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKey {
    New,
    Close,
    Next,
    Prev,
    /// Display index (Ctrl+Shift+1 is 0).
    Jump(usize),
}

// ---------------------------------------------------------------------------------------
// The model

/// The ✕ of a regular tab: a square this big, this far from the tab's right edge and
/// centred vertically.
const CLOSE_SIZE: f32 = 16.0;
const CLOSE_INSET: f32 = 6.0;
/// The menu keeps this far from the panel's left and right edges (room for its shadow).
const MENU_MARGIN: f32 = 8.0;

/// Virtual-key codes the shortcuts use.
const VK_TAB: u16 = 0x09;
const VK_1: u16 = 0x31;
const VK_9: u16 = 0x39;
const VK_T: u16 = 0x54;
const VK_W: u16 = 0x57;

/// What the pointer is over, by id so it survives a reorder or a close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Over {
    Tab(TabId),
    Close(TabId),
    Plus,
}

#[derive(Clone, Debug)]
struct Tab {
    id: TabId,
    title: String,
    pinned: bool,
    activity: bool,
    exited: bool,
}

/// A left press on a tab that has not been released yet.
#[derive(Clone, Copy, Debug)]
struct Held {
    id: TabId,
    /// Where the press landed.
    from: (f32, f32),
    /// The pointer's x inside the tab at the press.
    grab: f32,
    /// The press was on the tab's ✕: released on it (and not turned into a drag), it closes.
    on_close: bool,
    /// Past `DRAG_SLOP`: the tab follows the pointer and this is no longer a click.
    dragging: bool,
    /// The dragged tab's left edge.
    x: f32,
}

/// The open menu: whose it is (a tab's, or the "+" menu), where it sits and the row under
/// the pointer.
#[derive(Clone, Copy, Debug)]
struct Menu {
    /// The tab it is for; unused when `plus`.
    id: TabId,
    /// The new-tab menu under "+".
    plus: bool,
    x: f32,
    y: f32,
    hover: Option<usize>,
}

/// The terminal's tabs. Display order is the vector's order and pinned tabs always come
/// first. A tab's id is never reused, so the app can key its shell sessions by it.
#[derive(Clone, Debug)]
pub struct Tabs {
    tabs: Vec<Tab>,
    active: Option<TabId>,
    next_id: TabId,
    hover: Option<Over>,
    held: Option<Held>,
    menu: Option<Menu>,
}

impl Default for Tabs {
    fn default() -> Self {
        Self::new()
    }
}

/// What a left press did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// Nothing (the header's empty space, or a press on a ✕, which waits for the release).
    None,
    /// A tab was pressed and is now the active one (also when it already was).
    Activated(TabId),
    /// "+" was pressed and its new-tab menu is now open (nothing is added yet).
    PlusMenu,
    /// A menu row was pressed; it closed the menu. The action is for that tab and is the
    /// caller's to carry out (`Tabs` has not changed). For the "+" menu's rows
    /// (`NewShell`, `NewClaude`, `NewCodex`) the id is the active tab, or 0 if there is none,
    /// and means nothing.
    Menu(MenuAction, TabId),
    /// The menu was open and the press closed it without acting: it landed outside the menu
    /// or on a disabled row. Whatever is under the pointer did not get the press.
    MenuDismissed,
}

/// What a button release did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Release {
    /// Nothing to do (a click, or no press in progress).
    None,
    /// A ✕ was pressed and released on itself: close this tab.
    Close(TabId),
    /// A drag ended and the tab was dropped. The order may be unchanged (the tab then
    /// snaps back), but the view changed either way.
    Reordered,
}

/// The tab widths `(pinned, regular)` for this mix, in whole px. Pinned tabs are
/// `PINNED_W`; regular ones share the rest between `TAB_MIN_W` and `TAB_MAX_W`. When even
/// that does not fit (more than ten tabs, or many pinned ones), every tab gets the same
/// share, so up to `MAX_TABS` tabs and "+" always fit `TABS`.
fn tab_widths(pinned: usize, regular: usize) -> (f32, f32) {
    let n = pinned + regular;
    if n == 0 {
        return (PINNED_W, TAB_MAX_W);
    }
    let plus = if n < MAX_TABS { PLUS_W + TAB_GAP } else { 0.0 };
    let avail = TABS.w - plus - TAB_GAP * (n - 1) as f32;
    let fixed = PINNED_W * pinned as f32;
    if fixed + TAB_MIN_W * regular as f32 > avail {
        let w = (avail / n as f32).floor();
        return (w, w);
    }
    let rw = if regular > 0 {
        ((avail - fixed) / regular as f32).min(TAB_MAX_W).floor()
    } else {
        TAB_MAX_W
    };
    (PINNED_W, rw)
}

/// The ✕ inside a regular tab's rect.
fn close_rect(tab: &Rect) -> Rect {
    Rect::new(
        tab.x + tab.w - CLOSE_INSET - CLOSE_SIZE,
        tab.y + (tab.h - CLOSE_SIZE) / 2.0,
        CLOSE_SIZE,
        CLOSE_SIZE,
    )
}

impl Tabs {
    /// No tabs.
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            active: None,
            next_id: 1,
            hover: None,
            held: None,
            menu: None,
        }
    }

    /// How many tabs there are (at most `MAX_TABS`).
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// No tabs at all.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// The tabs' ids in display order (pinned first).
    pub fn ids(&self) -> Vec<TabId> {
        self.tabs.iter().map(|t| t.id).collect()
    }

    /// The tab being shown; None only when there are no tabs.
    pub fn active(&self) -> Option<TabId> {
        self.active
    }

    /// False for an unknown id.
    pub fn is_pinned(&self, id: TabId) -> bool {
        self.tab(id).is_some_and(|t| t.pinned)
    }

    // --- editing ---

    /// A new tab, made active: a regular one at the end, a pinned one at the end of the
    /// pinned group. None at `MAX_TABS`.
    pub fn add(&mut self, title: String, pinned: bool) -> Option<TabId> {
        if self.tabs.len() >= MAX_TABS {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        let tab = Tab {
            id,
            title,
            pinned,
            activity: false,
            exited: false,
        };
        let at = if pinned {
            self.pinned_count()
        } else {
            self.tabs.len()
        };
        self.tabs.insert(at, tab);
        self.active = Some(id);
        Some(id)
    }

    /// Removes it; if it was active, the tab to its right becomes active, else the one to
    /// its left. False if unknown.
    pub fn close(&mut self, id: TabId) -> bool {
        let Some(i) = self.index_of(id) else {
            return false;
        };
        self.tabs.remove(i);
        if self.active == Some(id) {
            // The tab that slid into its place is the one on its right; past the end, the
            // last one is the one on its left.
            let next = self.tabs.get(i).or(self.tabs.last()).map(|t| t.id);
            self.active = next;
            self.show_active();
        }
        self.prune();
        true
    }

    /// Every regular tab except `id` (pinned ones stay); returns the closed ids. `id`
    /// becomes active.
    pub fn close_others(&mut self, id: TabId) -> Vec<TabId> {
        if self.tab(id).is_none() {
            return Vec::new();
        }
        let mut closed = Vec::new();
        self.tabs.retain(|t| {
            let keep = t.pinned || t.id == id;
            if !keep {
                closed.push(t.id);
            }
            keep
        });
        self.activate(id);
        self.prune();
        closed
    }

    /// Every regular tab to the right of `id`; returns the closed ids. The active tab stays
    /// if it survives, else `id` becomes active.
    pub fn close_right(&mut self, id: TabId) -> Vec<TabId> {
        let Some(at) = self.index_of(id) else {
            return Vec::new();
        };
        let mut closed = Vec::new();
        let mut i = 0;
        self.tabs.retain(|t| {
            let keep = i <= at || t.pinned;
            i += 1;
            if !keep {
                closed.push(t.id);
            }
            keep
        });
        if self.active.is_some_and(|a| closed.contains(&a)) {
            self.activate(id);
        }
        self.prune();
        closed
    }

    /// Pin: moves to the end of the pinned group. Unpin: to the start of the regular group.
    /// The active tab stays active; an unknown id or no change does nothing.
    pub fn set_pinned(&mut self, id: TabId, pinned: bool) {
        let Some(i) = self.index_of(id) else {
            return;
        };
        if self.tabs[i].pinned == pinned {
            return;
        }
        let mut tab = self.tabs.remove(i);
        tab.pinned = pinned;
        // Without this tab, the end of the pinned group is also the start of the regular
        // one.
        let at = self.pinned_count();
        self.tabs.insert(at, tab);
    }

    /// Makes it the active tab and clears its activity dot. An unknown id does nothing.
    pub fn activate(&mut self, id: TabId) {
        if self.tab(id).is_some() {
            self.active = Some(id);
            self.show_active();
        }
    }

    /// The next tab in display order, wrapping around.
    pub fn next(&mut self) {
        self.step(1);
    }

    /// The previous tab in display order, wrapping around.
    pub fn prev(&mut self) {
        self.step(-1);
    }

    /// Activates the tab at this display index; ignored when out of range.
    pub fn jump(&mut self, index: usize) {
        if let Some(id) = self.tabs.get(index).map(|t| t.id) {
            self.activate(id);
        }
    }

    /// The title the tab shows (the shell's window title). Unknown ids are ignored.
    pub fn set_title(&mut self, id: TabId, title: String) {
        if let Some(t) = self.tab_mut(id) {
            t.title = title;
        }
    }

    /// Whether a tab has output that has not been shown yet.
    pub fn has_activity(&self, id: TabId) -> bool {
        self.tab(id).is_some_and(|t| t.activity)
    }

    /// New output in a background tab. Ignored for the active tab (it is being shown).
    pub fn set_activity(&mut self, id: TabId) {
        if self.active == Some(id) {
            return;
        }
        if let Some(t) = self.tab_mut(id) {
            t.activity = true;
        }
    }

    /// Whether its shell has exited.
    pub fn set_exited(&mut self, id: TabId, exited: bool) {
        if let Some(t) = self.tab_mut(id) {
            t.exited = exited;
        }
    }

    // --- layout and hit testing ---

    /// Tab rects in display order (pinned `PINNED_W`, regular ones shrunk to fit between
    /// `TAB_MIN_W` and `TAB_MAX_W`, `TAB_GAP` apart, inside `TABS`), and "+" right after the
    /// last tab (None at `MAX_TABS`). Widths are whole px. With more tabs than fit at those
    /// widths, they all get the same, narrower, width (see `tab_widths`), so the tabs and
    /// "+" never reach past `TABS`. A dragged tab is not moved here (see `view`).
    pub fn layout(&self) -> (Vec<Rect>, Option<Rect>) {
        let pinned = self.pinned_count();
        let (pw, rw) = tab_widths(pinned, self.tabs.len() - pinned);
        let mut x = TABS.x;
        let rects: Vec<Rect> = self
            .tabs
            .iter()
            .map(|t| {
                let w = if t.pinned { pw } else { rw };
                let r = Rect::new(x, TABS.y, w, TABS.h);
                x += w + TAB_GAP;
                r
            })
            .collect();
        // `x` is already past the last gap; with no tabs it is where the first one would be.
        let plus = (self.tabs.len() < MAX_TABS).then(|| Rect::new(x, TABS.y, PLUS_W, TABS.h));
        (rects, plus)
    }

    /// What a panel-local point is over: a regular tab's ✕ (which is where a hovered or
    /// active tab draws it), a tab, or "+".
    pub fn hit(&self, x: f32, y: f32) -> Option<BarHit> {
        let (rects, plus) = self.layout();
        for (i, r) in rects.iter().enumerate() {
            if r.contains(x, y) {
                if !self.tabs[i].pinned && close_rect(r).contains(x, y) {
                    return Some(BarHit::Close(i));
                }
                return Some(BarHit::Tab(i));
            }
        }
        plus.filter(|p| p.contains(x, y)).map(|_| BarHit::Plus)
    }

    /// Pointer moved over the panel (panel-local). True if what is hovered changed (a tab,
    /// a ✕, "+", or a menu row). With the menu open only its enabled rows are hovered, and
    /// during a drag nothing is.
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        if let Some(menu) = self.menu {
            let row = match self.menu_row(&menu, x, y) {
                Some((k, _, true)) => Some(k),
                _ => None,
            };
            let changed = menu.hover != row || self.hover.is_some();
            self.hover = None;
            self.menu = Some(Menu { hover: row, ..menu });
            return changed;
        }
        if self.held.is_some_and(|h| h.dragging) {
            return false;
        }
        let over = self.over_at(x, y);
        if over == self.hover {
            return false;
        }
        self.hover = over;
        true
    }

    /// The pointer left the panel: nothing is hovered (an open menu stays, with no row
    /// lit). True if that changed anything.
    pub fn clear_hover(&mut self) -> bool {
        let mut changed = self.hover.take().is_some();
        if let Some(m) = self.menu.as_mut() {
            changed |= m.hover.take().is_some();
        }
        changed
    }

    // --- the mouse ---

    /// Left press on the header. A tab is activated at once and may become a drag; its ✕
    /// waits for the release (so a drag never closes). With the menu open, a press anywhere
    /// closes it, and acts only on an enabled menu row.
    pub fn press(&mut self, x: f32, y: f32) -> Press {
        self.held = None;
        if let Some(menu) = self.menu.take() {
            // Everything this press does is about the menu: nothing under it is touched.
            return match self.menu_row(&menu, x, y) {
                Some((_, action, true)) => {
                    let id = if menu.plus {
                        self.active.unwrap_or(0)
                    } else {
                        menu.id
                    };
                    Press::Menu(action, id)
                }
                _ => Press::MenuDismissed,
            };
        }
        let (rects, plus) = self.layout();
        match self.hit(x, y) {
            Some(BarHit::Tab(i)) => {
                let id = self.tabs[i].id;
                self.activate(id);
                self.hold(id, (x, y), rects[i].x, false);
                Press::Activated(id)
            }
            Some(BarHit::Close(i)) => {
                let id = self.tabs[i].id;
                self.hold(id, (x, y), rects[i].x, true);
                Press::None
            }
            Some(BarHit::Plus) => {
                // `hit` only reports "+" when it exists.
                let p = plus.unwrap_or(TABS);
                self.menu = Some(Menu {
                    id: 0,
                    plus: true,
                    x: p.x.clamp(MENU_MARGIN, TERM_W - MENU_MARGIN - MENU_W),
                    y: p.y + p.h + TAB_GAP,
                    hover: None,
                });
                self.hover = None;
                Press::PlusMenu
            }
            None => Press::None,
        }
    }

    /// Pointer moved with the button down: past `DRAG_SLOP` the pressed tab follows the
    /// pointer (sideways only, kept inside its own group), and the order changes when its
    /// centre crosses a neighbour's centre, never across the pinned / regular boundary.
    /// True if the view changed.
    pub fn drag(&mut self, x: f32, y: f32) -> bool {
        let Some(mut held) = self.held else {
            return false;
        };
        let Some(mut i) = self.index_of(held.id) else {
            self.held = None;
            return false;
        };
        if !x.is_finite() || !y.is_finite() {
            return false;
        }
        let mut changed = false;
        if !held.dragging {
            if (x - held.from.0).hypot(y - held.from.1) < DRAG_SLOP {
                return false;
            }
            // A press that moved is a drag and nothing else: not a ✕ click any more, and
            // the tab is lifted, so nothing is hovered.
            held.dragging = true;
            held.on_close = false;
            self.hover = None;
            changed = true;
        }

        // The group the tab belongs to; its tabs all have one width, so the slots stay put
        // while the order changes.
        let pinned = self.pinned_count();
        let (lo, hi) = if self.tabs[i].pinned {
            (0, pinned - 1)
        } else {
            (pinned, self.tabs.len() - 1)
        };
        let (rects, _) = self.layout();
        let w = rects[i].w;
        let left = (x - held.grab).clamp(rects[lo].x, rects[hi].x);
        let centre = left + w / 2.0;
        // Reaching a neighbour's centre swaps (so the slot at either end of the group can
        // be reached: there the two centres meet exactly).
        while i < hi && centre >= rects[i + 1].x + w / 2.0 {
            self.tabs.swap(i, i + 1);
            i += 1;
            changed = true;
        }
        while i > lo && centre <= rects[i - 1].x + w / 2.0 {
            self.tabs.swap(i, i - 1);
            i -= 1;
            changed = true;
        }
        changed |= held.x != left;
        held.x = left;
        self.held = Some(held);
        changed
    }

    /// Button up, wherever the pointer is: ends a drag, or completes a ✕ press released on
    /// the same ✕.
    pub fn release(&mut self, x: f32, y: f32) -> Release {
        let Some(held) = self.held.take() else {
            return Release::None;
        };
        if held.dragging {
            self.hover = self.over_at(x, y);
            return Release::Reordered;
        }
        let on_same_close = held.on_close
            && self
                .index_of(held.id)
                .is_some_and(|i| self.hit(x, y) == Some(BarHit::Close(i)));
        if on_same_close {
            Release::Close(held.id)
        } else {
            Release::None
        }
    }

    /// Drops a press or drag in progress (the panel was hidden, or the mouse capture was
    /// lost). The order stays as the drag left it. True if there was one.
    pub fn cancel(&mut self) -> bool {
        self.held.take().is_some()
    }

    /// Middle click: the regular tab under the pointer to close (pinned tabs need the
    /// menu). None while the menu is open, which a click elsewhere only closes.
    pub fn middle_click(&self, x: f32, y: f32) -> Option<TabId> {
        if self.menu.is_some() {
            return None;
        }
        match self.hit(x, y)? {
            BarHit::Tab(i) | BarHit::Close(i) if !self.tabs[i].pinned => Some(self.tabs[i].id),
            _ => None,
        }
    }

    // --- the menu ---

    /// Right click: on a tab (or its ✕) opens that tab's menu just below it, kept inside
    /// the panel, replacing any open one. Anywhere else it closes an open menu. True if the
    /// view changed (a menu opened, moved or closed).
    pub fn open_menu(&mut self, x: f32, y: f32) -> bool {
        let (rects, _) = self.layout();
        let tab = match self.hit(x, y) {
            Some(BarHit::Tab(i) | BarHit::Close(i)) => i,
            _ => return self.close_menu(),
        };
        let r = rects[tab];
        self.menu = Some(Menu {
            id: self.tabs[tab].id,
            plus: false,
            x: r.x.clamp(MENU_MARGIN, TERM_W - MENU_MARGIN - MENU_W),
            y: r.y + r.h + TAB_GAP,
            hover: None,
        });
        // A right click is not part of a left press, and the menu is what is hovered now.
        self.held = None;
        self.hover = None;
        true
    }

    /// Closes the menu. True if it was open.
    pub fn close_menu(&mut self) -> bool {
        self.menu.take().is_some()
    }

    /// Whether the tab menu is open.
    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    // --- the view ---

    /// Everything the header draws: the tabs (the dragged one following the pointer), "+",
    /// what is hovered and the menu.
    pub fn view(&self) -> TabBarView {
        let (rects, plus) = self.layout();
        let hover = self.bar_hover();
        let dragged = self.held.filter(|h| h.dragging);
        let tabs = self
            .tabs
            .iter()
            .zip(rects)
            .enumerate()
            .map(|(i, (t, mut rect))| {
                let lifted = dragged.filter(|h| h.id == t.id);
                if let Some(h) = lifted {
                    rect.x = h.x;
                }
                let active = self.active == Some(t.id);
                let hovered = matches!(hover, Some(BarHit::Tab(j) | BarHit::Close(j)) if j == i);
                TabSlot {
                    id: t.id,
                    rect,
                    title: t.title.clone(),
                    pinned: t.pinned,
                    active,
                    activity: t.activity,
                    exited: t.exited,
                    close: (!t.pinned && (active || hovered)).then(|| close_rect(&rect)),
                    dragging: lifted.is_some(),
                }
            })
            .collect();
        TabBarView {
            tabs,
            plus,
            hover,
            menu: self.menu.as_ref().and_then(|m| self.menu_view(m)),
            rename: None,
        }
    }

    // --- helpers ---

    fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    /// How many tabs are pinned (they are the first ones).
    fn pinned_count(&self) -> usize {
        self.tabs.partition_point(|t| t.pinned)
    }

    /// The active tab is being shown, so it has no news.
    fn show_active(&mut self) {
        if let Some(t) = self.active.and_then(|id| self.tab_mut(id)) {
            t.activity = false;
        }
    }

    /// Moves the active tab by `delta` places in display order, wrapping around.
    fn step(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        if n == 0 {
            return;
        }
        let to = match self.active.and_then(|id| self.index_of(id)) {
            Some(i) => (i as isize + delta).rem_euclid(n) as usize,
            None => 0,
        };
        let id = self.tabs[to].id;
        self.activate(id);
    }

    /// Starts tracking a press on a tab whose left edge is at `tab_x`.
    fn hold(&mut self, id: TabId, from: (f32, f32), tab_x: f32, on_close: bool) {
        self.held = Some(Held {
            id,
            from,
            grab: from.0 - tab_x,
            on_close,
            dragging: false,
            x: tab_x,
        });
    }

    fn over_at(&self, x: f32, y: f32) -> Option<Over> {
        Some(match self.hit(x, y)? {
            BarHit::Tab(i) => Over::Tab(self.tabs[i].id),
            BarHit::Close(i) => Over::Close(self.tabs[i].id),
            BarHit::Plus => Over::Plus,
        })
    }

    /// The hover as a `BarHit` for the current order (None if what it named is gone).
    fn bar_hover(&self) -> Option<BarHit> {
        match self.hover? {
            Over::Tab(id) => self.index_of(id).map(BarHit::Tab),
            Over::Close(id) => {
                let i = self.index_of(id)?;
                Some(if self.tabs[i].pinned {
                    BarHit::Tab(i)
                } else {
                    BarHit::Close(i)
                })
            }
            Over::Plus => (self.tabs.len() < MAX_TABS).then_some(BarHit::Plus),
        }
    }

    /// Drops hover, press and menu state that names a tab that is gone.
    fn prune(&mut self) {
        let gone = |s: &Self, id: TabId| s.index_of(id).is_none();
        if let Some(Over::Tab(id) | Over::Close(id)) = self.hover {
            if gone(self, id) {
                self.hover = None;
            }
        }
        if self.held.is_some_and(|h| gone(self, h.id)) {
            self.held = None;
        }
        if self.menu.is_some_and(|m| !m.plus && gone(self, m.id)) {
            self.menu = None;
        }
        // No "+" at the limit, so no menu under it.
        if self.menu.is_some_and(|m| m.plus) && self.tabs.len() >= MAX_TABS {
            self.menu = None;
        }
    }

    /// The menu's rows: what, the label, the hint and whether it can be used.
    fn menu_items(&self, menu: &Menu) -> Vec<(MenuAction, &'static str, &'static str, bool)> {
        if menu.plus {
            return vec![
                (MenuAction::NewShell, "Terminal", "", true),
                (MenuAction::NewClaude, "Claude", "yolo", true),
                (MenuAction::NewCodex, "Codex", "yolo", true),
            ];
        }
        let id = menu.id;
        let Some(i) = self.index_of(id) else {
            return Vec::new();
        };
        let pin = if self.tabs[i].pinned {
            (MenuAction::Unpin, "Unpin tab", "", true)
        } else {
            (MenuAction::Pin, "Pin tab", "", true)
        };
        let others = self.tabs.iter().any(|t| !t.pinned && t.id != id);
        let right = self.tabs[i + 1..].iter().any(|t| !t.pinned);
        vec![
            pin,
            (MenuAction::Close, "Close", "", true),
            (MenuAction::CloseOthers, "Close others", "", others),
            (MenuAction::CloseRight, "Close to the right", "", right),
            (MenuAction::Rename, "Rename tab...", "", true),
        ]
    }

    /// The row of `menu` under the point: its index, action and whether it is enabled.
    fn menu_row(&self, menu: &Menu, x: f32, y: f32) -> Option<(usize, MenuAction, bool)> {
        let items = self.menu_items(menu);
        if items.is_empty() {
            return None;
        }
        let area = Rect::new(menu.x, menu.y, MENU_W, items.len() as f32 * MENU_ROW_H);
        if !area.contains(x, y) {
            return None;
        }
        let k = (((y - menu.y) / MENU_ROW_H) as usize).min(items.len() - 1);
        let (action, _, _, enabled) = items[k];
        Some((k, action, enabled))
    }

    fn menu_view(&self, menu: &Menu) -> Option<MenuView> {
        let items: Vec<MenuItemView> = self
            .menu_items(menu)
            .into_iter()
            .enumerate()
            .map(|(k, (action, label, hint, enabled))| MenuItemView {
                action,
                label,
                hint,
                rect: Rect::new(menu.x, menu.y + k as f32 * MENU_ROW_H, MENU_W, MENU_ROW_H),
                enabled,
            })
            .collect();
        if items.is_empty() {
            return None;
        }
        Some(MenuView {
            rect: Rect::new(menu.x, menu.y, MENU_W, items.len() as f32 * MENU_ROW_H),
            items,
            hover: menu.hover,
        })
    }
}

// ---------------------------------------------------------------------------------------
// Shortcuts

/// Ctrl+Shift+T new, Ctrl+Shift+W close, Ctrl+Tab next, Ctrl+Shift+Tab previous and
/// Ctrl+Shift+1..9 jump (`vk` is the virtual-key code: `'T'`, `'W'`, `'1'..='9'`, `VK_TAB`).
/// None for anything else. With Alt held it is always None: AltGr reports as Ctrl+Alt, so
/// a character typed with AltGr must never be taken as a shortcut (Ctrl+Alt+1..9 are
/// Tile's anyway).
pub fn shortcut(vk: u16, ctrl: bool, shift: bool, alt: bool) -> Option<TabKey> {
    if !ctrl || alt {
        return None;
    }
    match (vk, shift) {
        (VK_TAB, false) => Some(TabKey::Next),
        (VK_TAB, true) => Some(TabKey::Prev),
        (VK_T, true) => Some(TabKey::New),
        (VK_W, true) => Some(TabKey::Close),
        (VK_1..=VK_9, true) => Some(TabKey::Jump((vk - VK_1) as usize)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// The pinned-tabs file

/// A pinned tab as saved in `~/.daijobu/terminal-tabs.toml` (`[[pinned]]` tables): the
/// name it shows, the program and arguments to start, and the folder it started in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedTab {
    /// The tab's name; may be empty (the app then names it after the shell).
    #[serde(default)]
    pub title: String,
    /// The program to start; empty (or left out) is the default shell, as `[terminal]
    /// shell = ""`.
    #[serde(default)]
    pub shell: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// The folder the tab started in; empty for the default (home).
    #[serde(default)]
    pub cwd: String,
    /// The coding agent the tab runs (a `term_agents::Agent::key`), or empty for a plain
    /// shell. Unknown values are kept as they are; the app decides what to do with them.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent: String,
    /// A user override, separate from the last title reported by the shell.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub custom_title: String,
}

/// Reads `terminal-tabs.toml`: the valid pinned tabs (at most `MAX_TABS`, in file order),
/// and one message per problem. A syntax error (or a `pinned` that is not an array of
/// tables) rejects the whole file with one message. A bad entry (not a table, a wrong
/// type) is skipped and never takes the others down. Every field may be left out; no
/// `shell` is the default shell.
///
/// ```toml
/// [[pinned]]
/// title = "pwsh"
/// shell = "pwsh.exe"
/// args = ["-NoLogo"]
/// cwd = "C:\\Users\\me"
/// ```
pub fn parse_pinned(src: &str) -> (Vec<PinnedTab>, Vec<String>) {
    let table = match toml::from_str::<toml::Table>(src) {
        Ok(t) => t,
        Err(e) => {
            let at = e.span().map_or(String::new(), |s| {
                let end = s.start.min(src.len());
                let line = src.as_bytes()[..end]
                    .iter()
                    .filter(|&&b| b == b'\n')
                    .count()
                    + 1;
                format!(" at line {line}")
            });
            return (
                Vec::new(),
                vec![format!(
                    "terminal-tabs.toml: syntax error{at}: {}",
                    e.message()
                )],
            );
        }
    };
    let entries = match table.get("pinned") {
        None => return (Vec::new(), Vec::new()),
        Some(Value::Array(a)) => a,
        Some(_) => {
            return (
                Vec::new(),
                vec!["terminal-tabs.toml: `pinned` must be an array of tables ([[pinned]])".into()],
            )
        }
    };
    let (mut ok, mut notes) = (Vec::new(), Vec::new());
    for (i, entry) in entries.iter().enumerate() {
        if ok.len() == MAX_TABS {
            notes.push(format!(
                "terminal-tabs.toml: only the first {MAX_TABS} pinned tabs are kept"
            ));
            break;
        }
        match read_pinned(i + 1, entry) {
            Ok(t) => ok.push(t),
            Err(e) => notes.push(e),
        }
    }
    (ok, notes)
}

/// One `[[pinned]]` entry (`n` is its 1-based position, for messages).
fn read_pinned(n: usize, entry: &Value) -> Result<PinnedTab, String> {
    let tab: PinnedTab = entry
        .clone()
        .try_into()
        .map_err(|e: toml::de::Error| format!("pinned tab {n}: {}", e.message()))?;
    Ok(tab)
}

/// The `terminal-tabs.toml` text for these pinned tabs, as `parse_pinned` reads it.
pub fn pinned_to_toml(tabs: &[PinnedTab]) -> String {
    #[derive(Serialize)]
    struct File<'a> {
        pinned: &'a [PinnedTab],
    }
    let body = toml::to_string_pretty(&File { pinned: tabs }).unwrap_or_default();
    format!(
        "# Daijobu Island: the terminal's pinned tabs. Rewritten when you pin or unpin.\n\n{body}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{TERM_CLOSE, TERM_H};

    /// `p` pinned and `r` regular tabs titled p0.., r0.. (the last one added is active).
    fn mixed(p: usize, r: usize) -> Tabs {
        let mut t = Tabs::new();
        for i in 0..p {
            t.add(format!("p{i}"), true).unwrap();
        }
        for i in 0..r {
            t.add(format!("r{i}"), false).unwrap();
        }
        t
    }

    fn tabs(n: usize) -> Tabs {
        mixed(0, n)
    }

    fn rect(t: &Tabs, i: usize) -> Rect {
        t.layout().0[i]
    }

    /// The middle of tab `i` (clear of its ✕).
    fn body(t: &Tabs, i: usize) -> (f32, f32) {
        let r = rect(t, i);
        (r.x + 10.0, r.y + r.h / 2.0)
    }

    /// The middle of tab `i`'s ✕ (a regular tab).
    fn cross(t: &Tabs, i: usize) -> (f32, f32) {
        let c = close_rect(&rect(t, i));
        (c.x + c.w / 2.0, c.y + c.h / 2.0)
    }

    fn middle(r: &Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    fn pinned_flags(t: &Tabs) -> Vec<bool> {
        t.tabs.iter().map(|t| t.pinned).collect()
    }

    /// Pinned tabs first, never a pinned one after a regular one.
    fn pinned_first(t: &Tabs) -> bool {
        pinned_flags(t).windows(2).all(|w| w[0] || !w[1])
    }

    fn titles(t: &Tabs) -> Vec<String> {
        t.tabs.iter().map(|t| t.title.clone()).collect()
    }

    fn activity(t: &Tabs, id: TabId) -> bool {
        t.view().tabs.iter().find(|s| s.id == id).unwrap().activity
    }

    fn slot(t: &Tabs, id: TabId) -> TabSlot {
        t.view().tabs.into_iter().find(|s| s.id == id).unwrap()
    }

    // --- adding and the limit ---

    #[test]
    fn a_new_model_is_empty() {
        let mut t = Tabs::new();
        assert!(t.is_empty());
        assert_eq!(t.len(), 0);
        assert_eq!(t.active(), None);
        assert!(t.ids().is_empty());
        let v = t.view();
        assert!(v.tabs.is_empty() && v.hover.is_none() && v.menu.is_none());
        assert!(v.plus.is_some());
        assert!(!t.menu_open() && !t.cancel());
    }

    #[test]
    fn add_makes_the_new_tab_active_and_orders_pinned_first() {
        let mut t = Tabs::new();
        let a = t.add("a".into(), false).unwrap();
        assert_eq!(t.active(), Some(a));
        let b = t.add("b".into(), false).unwrap();
        assert_eq!(t.active(), Some(b));
        // A pinned tab goes after the pinned ones, before every regular one.
        let p = t.add("p".into(), true).unwrap();
        let q = t.add("q".into(), true).unwrap();
        assert_eq!(t.ids(), [p, q, a, b]);
        assert_eq!(t.active(), Some(q));
        assert!(t.is_pinned(p) && t.is_pinned(q) && !t.is_pinned(a));
        // A regular one goes to the very end.
        let c = t.add("c".into(), false).unwrap();
        assert_eq!(t.ids(), [p, q, a, b, c]);
        assert!(!t.is_pinned(9999));
    }

    #[test]
    fn at_the_limit_nothing_can_be_added() {
        let mut t = tabs(MAX_TABS);
        assert_eq!(t.len(), MAX_TABS);
        assert_eq!(t.add("x".into(), false), None);
        assert_eq!(t.add("x".into(), true), None);
        assert_eq!(t.len(), MAX_TABS);
        // Closing one makes room again.
        let first = t.ids()[0];
        assert!(t.close(first));
        assert!(t.add("x".into(), false).is_some());
        assert_eq!(t.len(), MAX_TABS);
        let mut p = mixed(MAX_TABS, 0);
        assert_eq!(p.add("x".into(), true), None);
    }

    #[test]
    fn ids_are_not_reused() {
        let mut t = Tabs::new();
        let a = t.add("a".into(), false).unwrap();
        let b = t.add("b".into(), false).unwrap();
        assert_ne!(a, b);
        t.close(b);
        let c = t.add("c".into(), false).unwrap();
        assert!(c != a && c != b, "{c}");
    }

    // --- closing: the active tab (Review Focus 2) ---

    #[test]
    fn closing_the_active_tab_activates_its_right_neighbour() {
        let mut t = tabs(4);
        let ids = t.ids();
        t.activate(ids[1]);
        assert!(t.close(ids[1]));
        assert_eq!(t.ids(), [ids[0], ids[2], ids[3]]);
        assert_eq!(t.active(), Some(ids[2]));
    }

    #[test]
    fn closing_the_last_active_tab_activates_its_left_neighbour() {
        let mut t = tabs(3);
        let ids = t.ids();
        assert_eq!(t.active(), Some(ids[2]));
        t.close(ids[2]);
        assert_eq!(t.active(), Some(ids[1]));
        t.close(ids[1]);
        assert_eq!(t.active(), Some(ids[0]));
        t.close(ids[0]);
        assert_eq!(t.active(), None);
        assert!(t.is_empty());
    }

    #[test]
    fn closing_across_the_pinned_boundary_follows_display_order() {
        // [P, A, B]: the first regular tab has the pinned one on its left.
        let mut t = mixed(1, 2);
        let [p, a, b] = [t.ids()[0], t.ids()[1], t.ids()[2]];
        t.activate(a);
        t.close(a);
        assert_eq!(t.active(), Some(b));
        t.close(b);
        assert_eq!(t.active(), Some(p));
        // The last pinned tab with a regular one after it hands over to that one.
        let mut t = mixed(2, 1);
        let [p0, p1, r] = [t.ids()[0], t.ids()[1], t.ids()[2]];
        t.activate(p1);
        t.close(p1);
        assert_eq!(t.active(), Some(r));
        t.activate(p0);
        t.close(p0);
        assert_eq!(t.active(), Some(r));
    }

    #[test]
    fn closing_a_background_tab_keeps_the_active_one() {
        let mut t = tabs(3);
        let ids = t.ids();
        t.activate(ids[1]);
        t.close(ids[0]);
        assert_eq!(t.active(), Some(ids[1]));
        t.close(ids[2]);
        assert_eq!(t.active(), Some(ids[1]));
    }

    #[test]
    fn closing_an_unknown_tab_does_nothing() {
        let mut t = tabs(2);
        assert!(!t.close(9999));
        assert_eq!(t.len(), 2);
        assert!(t.close_others(9999).is_empty());
        assert!(t.close_right(9999).is_empty());
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn a_pinned_tab_can_be_closed_with_close() {
        let mut t = mixed(2, 1);
        let p0 = t.ids()[0];
        assert!(t.close(p0));
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn the_tab_that_becomes_active_shows_its_news() {
        let mut t = tabs(3);
        let ids = t.ids();
        t.set_activity(ids[0]);
        t.set_activity(ids[1]);
        t.close(ids[2]);
        assert_eq!(t.active(), Some(ids[1]));
        assert!(!activity(&t, ids[1]));
        assert!(activity(&t, ids[0]));
    }

    // --- close others / close to the right ---

    #[test]
    fn close_others_keeps_pinned_and_the_target() {
        let mut t = mixed(2, 3);
        let ids = t.ids();
        let closed = t.close_others(ids[3]);
        assert_eq!(closed, [ids[2], ids[4]]);
        assert_eq!(t.ids(), [ids[0], ids[1], ids[3]]);
        assert_eq!(t.active(), Some(ids[3]));
    }

    #[test]
    fn close_others_from_a_pinned_tab_closes_every_regular_one() {
        let mut t = mixed(2, 2);
        let ids = t.ids();
        let closed = t.close_others(ids[0]);
        assert_eq!(closed, [ids[2], ids[3]]);
        assert_eq!(t.ids(), [ids[0], ids[1]]);
        assert_eq!(t.active(), Some(ids[0]));
    }

    #[test]
    fn close_others_with_nothing_to_close_still_activates_the_target() {
        let mut t = mixed(1, 1);
        let ids = t.ids();
        assert!(t.close_others(ids[1]).is_empty());
        assert_eq!(t.active(), Some(ids[1]));
        t.set_activity(ids[0]);
        assert!(t.close_others(ids[0]).len() == 1);
        assert_eq!(t.active(), Some(ids[0]));
        assert!(!activity(&t, ids[0]));
    }

    #[test]
    fn close_right_closes_the_regular_tabs_after_the_target() {
        let mut t = mixed(1, 4);
        let ids = t.ids();
        t.activate(ids[1]);
        let closed = t.close_right(ids[2]);
        assert_eq!(closed, [ids[3], ids[4]]);
        assert_eq!(t.ids(), [ids[0], ids[1], ids[2]]);
        // The active tab was left of it and stays.
        assert_eq!(t.active(), Some(ids[1]));
    }

    #[test]
    fn close_right_moves_the_active_tab_to_the_target_when_it_goes() {
        let mut t = mixed(1, 4);
        let ids = t.ids();
        assert_eq!(t.active(), Some(ids[4]));
        t.close_right(ids[2]);
        assert_eq!(t.active(), Some(ids[2]));
        // Closing to the right of the last tab closes nothing and keeps the active one.
        assert!(t.close_right(ids[2]).is_empty());
        assert_eq!(t.active(), Some(ids[2]));
    }

    #[test]
    fn close_right_of_a_pinned_tab_closes_every_regular_tab_and_keeps_pinned_ones() {
        let mut t = mixed(3, 2);
        let ids = t.ids();
        let closed = t.close_right(ids[0]);
        assert_eq!(closed, [ids[3], ids[4]]);
        assert_eq!(t.ids(), [ids[0], ids[1], ids[2]]);
        // The active tab was a regular one that closed, so the target took over.
        assert_eq!(t.active(), Some(ids[0]));
    }

    #[test]
    fn a_background_pinned_tab_survives_close_right_as_active() {
        let mut t = mixed(2, 2);
        let ids = t.ids();
        t.activate(ids[1]);
        t.close_right(ids[0]);
        assert_eq!(t.active(), Some(ids[1]));
    }

    /// Review Focus 2, exhaustively: whatever is closed, the active tab is one that
    /// exists, and it is the neighbour the user expects.
    #[test]
    fn the_active_tab_after_any_close_exists_and_is_the_expected_one() {
        for p in 0..=3 {
            for r in 0..=4 {
                let n = p + r;
                for active in 0..n {
                    for target in 0..n {
                        let fresh = || {
                            let mut t = mixed(p, r);
                            let ids = t.ids();
                            t.activate(ids[active]);
                            t
                        };
                        let ids = fresh().ids();
                        let (a, x) = (ids[active], ids[target]);

                        // close
                        let mut t = fresh();
                        assert!(t.close(x));
                        let left: Vec<TabId> = ids.iter().copied().filter(|&i| i != x).collect();
                        assert_eq!(t.ids(), left);
                        let want = if a != x {
                            Some(a)
                        } else {
                            left.get(target).or(left.last()).copied()
                        };
                        assert_eq!(t.active(), want, "close p{p} r{r} a{active} t{target}");
                        assert!(pinned_first(&t));

                        // close_others
                        let mut t = fresh();
                        let closed = t.close_others(x);
                        // The pinned tabs (the first `p`) and the target live; the rest go.
                        let expect: Vec<TabId> = ids
                            .iter()
                            .enumerate()
                            .filter(|&(k, _)| k >= p && k != target)
                            .map(|(_, &i)| i)
                            .collect();
                        assert_eq!(closed, expect, "others p{p} r{r} t{target}");
                        for (k, i) in ids.iter().enumerate() {
                            assert_eq!(t.ids().contains(i), k < p || k == target);
                        }
                        assert_eq!(t.active(), Some(x), "others p{p} r{r} t{target}");
                        assert!(pinned_first(&t));

                        // close_right
                        let mut t = fresh();
                        let closed = t.close_right(x);
                        let alive = t.ids();
                        let expect: Vec<TabId> = ids
                            .iter()
                            .enumerate()
                            .filter(|&(k, _)| k <= target || k < p)
                            .map(|(_, &i)| i)
                            .collect();
                        assert_eq!(alive, expect, "right p{p} r{r} t{target}");
                        let gone: Vec<TabId> = ids
                            .iter()
                            .copied()
                            .filter(|i| !expect.contains(i))
                            .collect();
                        assert_eq!(closed, gone, "right p{p} r{r} t{target}");
                        let want = if alive.contains(&a) { a } else { x };
                        assert_eq!(
                            t.active(),
                            Some(want),
                            "right p{p} r{r} a{active} t{target}"
                        );
                        assert!(pinned_first(&t));
                    }
                }
            }
        }
    }

    // --- pin and unpin ---

    #[test]
    fn pinning_moves_a_tab_to_the_end_of_the_pinned_group() {
        let mut t = mixed(1, 3);
        let ids = t.ids();
        t.set_pinned(ids[3], true);
        assert_eq!(t.ids(), [ids[0], ids[3], ids[1], ids[2]]);
        assert!(t.is_pinned(ids[3]));
        // Pinning again, or an unknown id, changes nothing.
        t.set_pinned(ids[3], true);
        t.set_pinned(9999, true);
        assert_eq!(t.ids(), [ids[0], ids[3], ids[1], ids[2]]);
    }

    #[test]
    fn unpinning_moves_a_tab_to_the_start_of_the_regular_group() {
        let mut t = mixed(3, 2);
        let ids = t.ids();
        t.set_pinned(ids[0], false);
        assert_eq!(t.ids(), [ids[1], ids[2], ids[0], ids[3], ids[4]]);
        assert!(!t.is_pinned(ids[0]));
        assert!(pinned_first(&t));
        // The last pinned tab stays where it is, just regular now.
        t.set_pinned(ids[2], false);
        assert_eq!(t.ids(), [ids[1], ids[2], ids[0], ids[3], ids[4]]);
        t.set_pinned(ids[2], false);
        assert_eq!(t.ids(), [ids[1], ids[2], ids[0], ids[3], ids[4]]);
    }

    #[test]
    fn pinning_does_not_change_the_active_tab() {
        let mut t = mixed(0, 3);
        let ids = t.ids();
        t.activate(ids[1]);
        t.set_pinned(ids[2], true);
        assert_eq!(t.active(), Some(ids[1]));
        t.set_pinned(ids[1], true);
        assert_eq!(t.active(), Some(ids[1]));
    }

    // --- activate, next, prev, jump ---

    #[test]
    fn activate_clears_activity_and_ignores_unknown_ids() {
        let mut t = tabs(2);
        let ids = t.ids();
        t.set_activity(ids[0]);
        assert!(activity(&t, ids[0]));
        t.activate(ids[0]);
        assert_eq!(t.active(), Some(ids[0]));
        assert!(!activity(&t, ids[0]));
        t.activate(9999);
        assert_eq!(t.active(), Some(ids[0]));
    }

    #[test]
    fn next_and_prev_wrap_around_in_display_order() {
        let mut t = mixed(1, 2);
        let ids = t.ids();
        t.activate(ids[0]);
        t.next();
        assert_eq!(t.active(), Some(ids[1]));
        t.next();
        t.next();
        assert_eq!(t.active(), Some(ids[0]));
        t.prev();
        assert_eq!(t.active(), Some(ids[2]));
        t.prev();
        assert_eq!(t.active(), Some(ids[1]));
    }

    #[test]
    fn next_and_prev_with_one_or_no_tab() {
        let mut t = Tabs::new();
        t.next();
        t.prev();
        assert_eq!(t.active(), None);
        let a = t.add("a".into(), false).unwrap();
        t.next();
        assert_eq!(t.active(), Some(a));
        t.prev();
        assert_eq!(t.active(), Some(a));
    }

    #[test]
    fn next_and_prev_show_the_tabs_news() {
        let mut t = tabs(3);
        let ids = t.ids();
        t.activate(ids[0]);
        t.set_activity(ids[1]);
        t.next();
        assert!(!activity(&t, ids[1]));
    }

    #[test]
    fn jump_goes_to_a_display_index_and_ignores_the_rest() {
        let mut t = mixed(1, 3);
        let ids = t.ids();
        t.jump(0);
        assert_eq!(t.active(), Some(ids[0]));
        t.jump(2);
        assert_eq!(t.active(), Some(ids[2]));
        t.jump(4);
        t.jump(usize::MAX);
        assert_eq!(t.active(), Some(ids[2]));
        t.jump(3);
        assert_eq!(t.active(), Some(ids[3]));
    }

    // --- title, activity, exit ---

    #[test]
    fn title_activity_and_exit_show_in_the_view() {
        let mut t = tabs(2);
        let ids = t.ids();
        t.set_title(ids[0], "cargo".into());
        assert_eq!(slot(&t, ids[0]).title, "cargo");
        t.set_activity(ids[0]);
        assert!(slot(&t, ids[0]).activity);
        // The active tab is being shown: it cannot have news.
        t.set_activity(ids[1]);
        assert!(!slot(&t, ids[1]).activity);
        t.set_exited(ids[1], true);
        assert!(slot(&t, ids[1]).exited);
        t.set_exited(ids[1], false);
        assert!(!slot(&t, ids[1]).exited);
        // Unknown ids are ignored.
        t.set_title(9999, "x".into());
        t.set_activity(9999);
        t.set_exited(9999, true);
        assert_eq!(titles(&t), ["cargo", "r1"]);
    }

    // --- layout ---

    #[test]
    fn one_tab_is_as_wide_as_a_tab_gets_with_plus_after_it() {
        let t = tabs(1);
        let (rects, plus) = t.layout();
        assert_eq!(rects, [Rect::new(TABS.x, TABS.y, TAB_MAX_W, TABS.h)]);
        assert_eq!(
            plus,
            Some(Rect::new(
                TABS.x + TAB_MAX_W + TAB_GAP,
                TABS.y,
                PLUS_W,
                TABS.h
            ))
        );
    }

    #[test]
    fn four_tabs_shrink_to_leave_room_for_plus() {
        let t = tabs(4);
        let (rects, plus) = t.layout();
        // (684 - plus 24 - 2 - three gaps of 2) / 4.
        assert!(rects.iter().all(|r| r.w == 163.0), "{rects:?}");
        let xs: Vec<f32> = rects.iter().map(|r| r.x).collect();
        assert_eq!(xs, [30.0, 195.0, 360.0, 525.0]);
        // Exactly full: "+" ends at the strip's right edge.
        let plus = plus.unwrap();
        assert_eq!(plus.x, 690.0);
        assert_eq!(plus.x + plus.w, TABS.x + TABS.w);
    }

    #[test]
    fn ten_tabs_are_the_most_that_keep_the_minimum_width() {
        let (rects, plus) = tabs(10).layout();
        assert!(rects.iter().all(|r| r.w == TAB_MIN_W));
        let plus = plus.unwrap();
        assert_eq!(plus.x + plus.w, TABS.x + TABS.w);
        // An eleventh makes them all a little narrower than the minimum, still fitting.
        let (rects, plus) = tabs(11).layout();
        assert!(rects.iter().all(|r| r.w == 58.0), "{rects:?}");
        let plus = plus.unwrap();
        assert!(plus.x + plus.w <= TABS.x + TABS.w);
    }

    #[test]
    fn twelve_tabs_fit_without_plus() {
        let t = tabs(MAX_TABS);
        let (rects, plus) = t.layout();
        assert_eq!(plus, None);
        assert!(t.view().plus.is_none());
        assert!(rects.iter().all(|r| r.w == 55.0), "{rects:?}");
        let last = rects.last().unwrap();
        assert!(last.x + last.w <= TABS.x + TABS.w);
        assert!(rects[0].x == TABS.x);
    }

    #[test]
    fn pinned_tabs_are_compact_and_regular_ones_fill_the_rest() {
        let t = mixed(2, 2);
        let (rects, plus) = t.layout();
        assert_eq!((rects[0].w, rects[1].w), (PINNED_W, PINNED_W));
        // 684 - plus 26 - three gaps - two pinned = 500, for two tabs: 250, capped at 168.
        assert_eq!((rects[2].w, rects[3].w), (TAB_MAX_W, TAB_MAX_W));
        assert_eq!(rects[0].x, TABS.x);
        assert_eq!(rects[1].x, TABS.x + PINNED_W + TAB_GAP);
        assert_eq!(rects[2].x, rects[1].x + PINNED_W + TAB_GAP);
        assert_eq!(plus.unwrap().x, rects[3].x + rects[3].w + TAB_GAP);
        // With more regular tabs the pinned ones keep their width and the others shrink.
        let t = mixed(2, 6);
        let (rects, _) = t.layout();
        assert_eq!(rects[0].w, PINNED_W);
        assert!(rects[2].w < TAB_MAX_W && rects[2].w >= TAB_MIN_W);
    }

    #[test]
    fn many_pinned_tabs_shrink_together() {
        let (rects, _) = mixed(8, 0).layout();
        assert!(rects.iter().all(|r| r.w == PINNED_W));
        let (rects, plus) = mixed(9, 0).layout();
        assert!(rects.iter().all(|r| r.w < PINNED_W));
        assert!(plus.unwrap().x + PLUS_W <= TABS.x + TABS.w);
    }

    #[test]
    fn no_tabs_still_have_a_plus_at_the_start() {
        let (rects, plus) = Tabs::new().layout();
        assert!(rects.is_empty());
        assert_eq!(plus, Some(Rect::new(TABS.x, TABS.y, PLUS_W, TABS.h)));
    }

    /// Review Focus 4: every mix of up to 12 tabs fits the strip, in order and apart, with
    /// "+" after the last one and clear of the panel's ✕.
    #[test]
    fn every_mix_of_tabs_fits_the_strip_without_overlap() {
        for p in 0..=MAX_TABS {
            for r in 0..=(MAX_TABS - p) {
                let t = mixed(p, r);
                let (rects, plus) = t.layout();
                let n = p + r;
                assert_eq!(rects.len(), n);
                let limit = TABS.x + TABS.w;
                let mut prev_right = None;
                for (i, rc) in rects.iter().enumerate() {
                    assert_eq!((rc.y, rc.h), (TABS.y, TABS.h));
                    assert!(rc.x >= TABS.x && rc.x + rc.w <= limit, "p{p} r{r} {rc:?}");
                    assert!(rc.w >= 55.0, "p{p} r{r}: {} wide", rc.w);
                    assert!(rc.w <= if i < p { PINNED_W } else { TAB_MAX_W });
                    assert_eq!(rc.w, rc.w.floor());
                    if let Some(pr) = prev_right {
                        assert_eq!(rc.x - pr, TAB_GAP, "p{p} r{r}");
                    }
                    prev_right = Some(rc.x + rc.w);
                }
                match plus {
                    None => assert_eq!(n, MAX_TABS),
                    Some(pl) => {
                        assert!(n < MAX_TABS);
                        assert_eq!(pl.x, prev_right.map_or(TABS.x, |x| x + TAB_GAP));
                        assert!(pl.x + pl.w <= limit, "p{p} r{r}");
                        assert_eq!((pl.y, pl.w, pl.h), (TABS.y, PLUS_W, TABS.h));
                    }
                }
                // Nothing reaches the panel's close button.
                assert!(limit <= TERM_CLOSE.x);
                // Hit testing agrees: the last tab and "+" are different things.
                if let (Some(pl), Some(last)) = (plus, rects.last()) {
                    assert!(pl.x >= last.x + last.w);
                }
            }
        }
    }

    // --- hit testing and hover ---

    #[test]
    fn hit_tells_tabs_closes_plus_and_nothing_apart() {
        let t = tabs(3);
        let (rects, plus) = t.layout();
        for i in 0..3 {
            let (x, y) = body(&t, i);
            assert_eq!(t.hit(x, y), Some(BarHit::Tab(i)));
            let (x, y) = cross(&t, i);
            assert_eq!(t.hit(x, y), Some(BarHit::Close(i)));
        }
        let (x, y) = middle(&plus.unwrap());
        assert_eq!(t.hit(x, y), Some(BarHit::Plus));
        // The gap between two tabs, above and below the strip, past "+", and the panel ✕.
        let gap_x = rects[0].x + rects[0].w + 1.0;
        assert_eq!(t.hit(gap_x, TABS.y + 5.0), None);
        assert_eq!(t.hit(rects[0].x + 20.0, TABS.y - 1.0), None);
        assert_eq!(t.hit(rects[0].x + 20.0, TABS.y + TABS.h), None);
        assert_eq!(t.hit(TABS.x - 1.0, TABS.y + 5.0), None);
        let (x, y) = middle(&TERM_CLOSE);
        assert_eq!(t.hit(x, y), None);
        assert_eq!(t.hit(f32::NAN, 10.0), None);
    }

    #[test]
    fn a_pinned_tab_has_no_close_to_hit() {
        let t = mixed(1, 1);
        let r = rect(&t, 0);
        // Where a regular tab's ✕ would be.
        let c = close_rect(&r);
        assert_eq!(t.hit(c.x + 8.0, c.y + 8.0), Some(BarHit::Tab(0)));
        let (x, y) = cross(&t, 1);
        assert_eq!(t.hit(x, y), Some(BarHit::Close(1)));
    }

    #[test]
    fn hover_reports_changes_only() {
        let mut t = tabs(3);
        let (x, y) = body(&t, 1);
        assert!(t.hover(x, y));
        assert!(!t.hover(x + 3.0, y));
        assert_eq!(t.view().hover, Some(BarHit::Tab(1)));
        let (cx, cy) = cross(&t, 1);
        assert!(t.hover(cx, cy));
        assert_eq!(t.view().hover, Some(BarHit::Close(1)));
        let (px, py) = middle(&t.layout().1.unwrap());
        assert!(t.hover(px, py));
        assert_eq!(t.view().hover, Some(BarHit::Plus));
        assert!(t.hover(5.0, 5.0));
        assert_eq!(t.view().hover, None);
        assert!(!t.hover(5.0, 5.0));
    }

    #[test]
    fn clear_hover_reports_whether_anything_was_hovered() {
        let mut t = tabs(2);
        assert!(!t.clear_hover());
        let (x, y) = body(&t, 0);
        t.hover(x, y);
        assert!(t.clear_hover());
        assert!(!t.clear_hover());
        assert_eq!(t.view().hover, None);
    }

    #[test]
    fn hover_follows_the_tab_when_the_order_changes_and_goes_when_it_closes() {
        let mut t = tabs(3);
        let ids = t.ids();
        let (x, y) = body(&t, 2);
        t.hover(x, y);
        t.close(ids[0]);
        // The hovered tab moved to index 1.
        assert_eq!(t.view().hover, Some(BarHit::Tab(1)));
        t.close(ids[2]);
        assert_eq!(t.view().hover, None);
    }

    #[test]
    fn the_close_shows_on_the_active_or_hovered_regular_tab_only() {
        let mut t = mixed(1, 3);
        let ids = t.ids();
        let closes =
            |t: &Tabs| -> Vec<bool> { t.view().tabs.iter().map(|s| s.close.is_some()).collect() };
        // Only the active tab (the last one).
        assert_eq!(closes(&t), [false, false, false, true]);
        let (x, y) = body(&t, 1);
        t.hover(x, y);
        assert_eq!(closes(&t), [false, true, false, true]);
        // Hovering the ✕ itself keeps it.
        let (cx, cy) = cross(&t, 1);
        t.hover(cx, cy);
        assert_eq!(closes(&t), [false, true, false, true]);
        // A pinned tab never has one, active or hovered.
        t.activate(ids[0]);
        let (x, y) = body(&t, 0);
        t.hover(x, y);
        assert_eq!(closes(&t), [false, false, false, false]);
        // The ✕ lies inside its tab.
        t.hover(5.0, 5.0);
        t.activate(ids[2]);
        let s = slot(&t, ids[2]);
        let c = s.close.unwrap();
        assert!(c.x >= s.rect.x && c.x + c.w <= s.rect.x + s.rect.w);
        assert!(c.y >= s.rect.y && c.y + c.h <= s.rect.y + s.rect.h);
    }

    #[test]
    fn the_view_lists_the_tabs_in_display_order_with_their_state() {
        let mut t = mixed(1, 2);
        let ids = t.ids();
        t.set_activity(ids[0]);
        t.set_exited(ids[1], true);
        let v = t.view();
        assert_eq!(v.tabs.len(), 3);
        assert_eq!(
            v.tabs.iter().map(|s| s.id).collect::<Vec<_>>(),
            ids,
            "display order"
        );
        let (rects, plus) = t.layout();
        assert_eq!(v.tabs.iter().map(|s| s.rect).collect::<Vec<_>>(), rects);
        assert_eq!(v.plus, plus);
        assert_eq!(
            v.tabs.iter().map(|s| s.pinned).collect::<Vec<_>>(),
            [true, false, false]
        );
        assert_eq!(
            v.tabs.iter().map(|s| s.active).collect::<Vec<_>>(),
            [false, false, true]
        );
        assert!(v.tabs[0].activity && v.tabs[1].exited);
        assert!(v.tabs.iter().all(|s| !s.dragging));
        assert!(v.menu.is_none());
    }

    // --- press and release ---

    #[test]
    fn pressing_a_tab_activates_it_at_once() {
        let mut t = tabs(3);
        let ids = t.ids();
        t.set_activity(ids[0]);
        let (x, y) = body(&t, 0);
        assert_eq!(t.press(x, y), Press::Activated(ids[0]));
        assert_eq!(t.active(), Some(ids[0]));
        assert!(!activity(&t, ids[0]));
        // A click is a press and a release in the same place: nothing more happens.
        assert_eq!(t.release(x, y), Release::None);
        assert_eq!(t.ids(), ids);
        // Pressing the active tab reports it again.
        assert_eq!(t.press(x, y), Press::Activated(ids[0]));
        t.release(x, y);
    }

    #[test]
    fn pressing_plus_or_empty_space() {
        let mut t = tabs(2);
        let (x, y) = middle(&t.layout().1.unwrap());
        assert_eq!(t.press(x, y), Press::PlusMenu);
        assert_eq!(t.release(x, y), Release::None);
        assert_eq!(t.len(), 2, "the model does not add by itself");
        t.close_menu();
        assert_eq!(t.press(700.0, 10.0), Press::None);
        let (cx, cy) = middle(&TERM_CLOSE);
        assert_eq!(t.press(cx, cy), Press::None);
        assert_eq!(t.press(300.0, 200.0), Press::None);
        assert_eq!(t.release(300.0, 200.0), Release::None);
        // At the limit there is no "+" to press.
        let mut t = tabs(MAX_TABS);
        assert_eq!(t.press(716.0, 18.0), Press::None);
    }

    #[test]
    fn a_press_on_the_close_waits_for_the_release() {
        let mut t = tabs(3);
        let ids = t.ids();
        let (x, y) = cross(&t, 0);
        // The tab is not even activated by it, and nothing is closed yet.
        assert_eq!(t.press(x, y), Press::None);
        assert_eq!(t.active(), Some(ids[2]));
        assert_eq!(t.len(), 3);
        assert_eq!(t.release(x, y), Release::Close(ids[0]));
        // The model leaves closing to the caller (it ends the shell too).
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn a_close_released_elsewhere_does_not_close() {
        let mut t = tabs(3);
        let (x, y) = cross(&t, 0);
        t.press(x, y);
        let (bx, by) = body(&t, 0);
        assert_eq!(t.release(bx, by), Release::None);
        // Nor on another tab's ✕, or outside the bar.
        t.press(x, y);
        let (ox, oy) = cross(&t, 1);
        assert_eq!(t.release(ox, oy), Release::None);
        t.press(x, y);
        assert_eq!(t.release(300.0, 300.0), Release::None);
        // A tiny wobble under the slop is still a click.
        t.press(x, y);
        assert!(!t.drag(x + 2.0, y + 1.0));
        assert_eq!(t.release(x + 2.0, y + 1.0), Release::Close(t.ids()[0]));
    }

    #[test]
    fn a_release_without_a_press_does_nothing() {
        let mut t = tabs(2);
        let (x, y) = cross(&t, 0);
        assert_eq!(t.release(x, y), Release::None);
        assert!(!t.drag(x + 50.0, y));
        assert!(!t.cancel());
    }

    // --- drag ---

    #[test]
    fn a_press_must_move_the_slop_before_it_drags() {
        let mut t = tabs(3);
        let (x, y) = body(&t, 0);
        t.press(x, y);
        assert!(!t.drag(x + 2.0, y));
        assert!(!t.drag(x + 2.0, y + 3.0)); // 3.6 px
        assert!(t.view().tabs.iter().all(|s| !s.dragging));
        assert_eq!(t.view().tabs[0].rect, rect(&t, 0));
        assert!(t.drag(x + DRAG_SLOP, y));
        let s = &t.view().tabs[0];
        assert!(s.dragging);
        assert_eq!(s.rect.x, rect(&t, 0).x + DRAG_SLOP);
        // Vertical moves count towards the slop, but the tab only ever moves sideways.
        let mut t = tabs(3);
        t.press(x, y);
        assert!(t.drag(x, y + 6.0));
        assert_eq!(t.view().tabs[0].rect, rect(&t, 0));
        assert!(t.view().tabs[0].dragging);
        assert_eq!(t.release(x, y), Release::Reordered);
    }

    #[test]
    fn the_dragged_tab_follows_the_pointer_and_others_make_room_at_the_centre() {
        let mut t = tabs(3);
        let ids = t.ids();
        let (a, b, c) = (ids[0], ids[1], ids[2]);
        let r0 = rect(&t, 0);
        let (x, y) = (r0.x + 10.0, 18.0);
        t.press(x, y);
        assert!(t.drag(x + 10.0, y));
        assert_eq!(slot(&t, a).rect.x, r0.x + 10.0);
        // The neighbour's centre is 170 px on; reaching it swaps, a hair short does not.
        let centre_b = rect(&t, 1).x + rect(&t, 1).w / 2.0;
        let dx = centre_b - (r0.x + r0.w / 2.0);
        assert!(t.drag(x + dx - 1.0, y));
        assert_eq!(t.ids(), [a, b, c]);
        assert!(t.drag(x + dx, y));
        assert_eq!(t.ids(), [b, a, c]);
        // The dragged tab is drawn under the pointer, the others in their slots.
        let v = t.view();
        assert_eq!(v.tabs[1].id, a);
        assert!(v.tabs[1].dragging);
        assert_eq!(v.tabs[1].rect.x, r0.x + dx);
        assert_eq!(v.tabs[0].rect, rect(&t, 0));
        assert_eq!(v.tabs[0].id, b);
        assert!(!v.tabs[0].dragging);
        // Going back a little does not swap straight back (the slot centres are a tab
        // apart); going back past the centre does.
        assert!(t.drag(x + dx - 20.0, y));
        assert_eq!(t.ids(), [b, a, c]);
        assert!(t.drag(x, y));
        assert_eq!(t.ids(), [a, b, c]);
        // All the way right: past the last centre, in one move.
        assert!(t.drag(2000.0, y));
        assert_eq!(t.ids(), [b, c, a]);
        assert_eq!(slot(&t, a).rect.x, rect(&t, 2).x);
        assert_eq!(t.release(2000.0, y), Release::Reordered);
        assert_eq!(t.ids(), [b, c, a]);
        // The drop: everything is in its slot again.
        let v = t.view();
        assert_eq!(
            v.tabs.iter().map(|s| s.rect).collect::<Vec<_>>(),
            t.layout().0
        );
        assert!(v.tabs.iter().all(|s| !s.dragging));
    }

    #[test]
    fn a_tab_dragged_to_either_end_reaches_it() {
        for n in 2..=MAX_TABS {
            for from in 0..n {
                let mut t = tabs(n);
                let ids = t.ids();
                let (x, y) = body(&t, from);
                t.press(x, y);
                t.drag(-3000.0, y);
                assert_eq!(t.ids()[0], ids[from], "{n} tabs, from {from}");
                t.drag(3000.0, y);
                assert_eq!(t.ids()[n - 1], ids[from], "{n} tabs, from {from}");
                t.release(3000.0, y);
                let mut rest = ids.clone();
                rest.remove(from);
                let mut got = t.ids();
                got.pop();
                assert_eq!(got, rest, "the others keep their order");
            }
        }
    }

    #[test]
    fn dragging_does_not_activate_a_tab_pressed_on_its_close() {
        let mut t = tabs(3);
        let ids = t.ids();
        let (x, y) = cross(&t, 0);
        t.press(x, y);
        assert!(t.drag(x + 300.0, y));
        assert_eq!(t.active(), Some(ids[2]));
        assert_eq!(t.release(x + 300.0, y), Release::Reordered);
        assert_eq!(t.ids(), [ids[1], ids[0], ids[2]]);
    }

    /// Review Focus 1: a drag never mixes the groups, whatever the pointer does.
    #[test]
    fn a_drag_never_crosses_the_pinned_boundary() {
        for (p, r) in [(1, 1), (2, 3), (3, 1), (1, 5), (4, 4)] {
            for from in 0..p + r {
                let mut t = mixed(p, r);
                let ids = t.ids();
                let dragged = ids[from];
                let pinned = from < p;
                let (x, y) = body(&t, from);
                t.press(x, y);
                let (rects, _) = t.layout();
                let group_left = if pinned { rects[0].x } else { rects[p].x };
                let group_right = if pinned {
                    rects[p - 1].x + rects[p - 1].w
                } else {
                    rects[p + r - 1].x + rects[p + r - 1].w
                };
                let mut px = -300.0;
                while px < 1100.0 {
                    t.drag(px, y + ((px as i32 % 7) as f32));
                    assert!(pinned_first(&t), "p{p} r{r} from{from} px{px}");
                    let ids_now = t.ids();
                    assert_eq!(ids_now[..p].contains(&dragged), pinned);
                    let s = slot(&t, dragged);
                    // Drawn inside its own group, never over the other one.
                    assert!(s.rect.x >= group_left, "px{px}: {:?}", s.rect);
                    assert!(s.rect.x + s.rect.w <= group_right, "px{px}: {:?}", s.rect);
                    px += 7.0;
                }
                assert_eq!(t.release(px, y), Release::Reordered);
                assert!(pinned_first(&t));
                // The same tabs in the same groups.
                let mut a = t.ids();
                let mut b = ids.clone();
                a.sort_unstable();
                b.sort_unstable();
                assert_eq!(a, b);
                assert_eq!(t.is_pinned(dragged), pinned);
            }
        }
    }

    #[test]
    fn pinned_tabs_reorder_among_themselves_and_regular_ones_among_themselves() {
        let mut t = mixed(2, 2);
        let ids = t.ids();
        // A pinned tab to the right: past the other pinned one, no further.
        let (x, y) = body(&t, 0);
        t.press(x, y);
        t.drag(900.0, y);
        t.release(900.0, y);
        assert_eq!(t.ids(), [ids[1], ids[0], ids[2], ids[3]]);
        // A regular tab to the left: past the other regular one, no further.
        let (x, y) = body(&t, 3);
        t.press(x, y);
        t.drag(-900.0, y);
        t.release(-900.0, y);
        assert_eq!(t.ids(), [ids[1], ids[0], ids[3], ids[2]]);
    }

    /// Review Focus 3: a press that turns into a drag is not also a click.
    #[test]
    fn a_press_on_the_close_that_becomes_a_drag_does_not_close() {
        let mut t = tabs(3);
        let ids = t.ids();
        let (x, y) = cross(&t, 0);
        assert_eq!(t.press(x, y), Press::None);
        assert!(t.drag(x + DRAG_SLOP, y));
        // Even back on the very same ✕: it is a drag now.
        t.drag(x, y);
        assert_eq!(t.release(x, y), Release::Reordered);
        assert_eq!(t.len(), 3);
        assert_eq!(t.ids(), ids);
        assert!(!t.menu_open());
        // And a fresh click on it closes again.
        t.press(x, y);
        assert_eq!(t.release(x, y), Release::Close(ids[0]));
    }

    #[test]
    fn a_drag_neither_closes_a_tab_nor_opens_the_menu() {
        let mut t = tabs(3);
        let (x, y) = body(&t, 1);
        t.press(x, y);
        t.drag(x + 30.0, y);
        assert_eq!(t.release(x + 30.0, y), Release::Reordered);
        assert!(!t.menu_open());
        assert!(t.view().menu.is_none());
        assert_eq!(t.len(), 3);
    }

    /// Review Focus 3: a drag released outside the bar still ends cleanly.
    #[test]
    fn a_drag_released_outside_the_bar_ends_cleanly() {
        for (rx, ry) in [
            (-80.0, 400.0),
            (900.0, -50.0),
            (300.0, 200.0),
            (f32::MAX, 0.0),
        ] {
            let mut t = mixed(1, 3);
            let ids = t.ids();
            let (x, y) = body(&t, 2);
            t.press(x, y);
            assert!(t.drag(x + 20.0, y + 150.0));
            t.drag(rx, ry);
            assert_eq!(t.release(rx, ry), Release::Reordered, "({rx}, {ry})");
            assert!(pinned_first(&t));
            let v = t.view();
            assert!(v.tabs.iter().all(|s| !s.dragging));
            assert_eq!(
                v.tabs.iter().map(|s| s.rect).collect::<Vec<_>>(),
                t.layout().0
            );
            // Nothing is stuck: the next press/click works, and the ✕ is not armed.
            assert_eq!(t.release(x, y), Release::None);
            assert!(!t.drag(x + 50.0, y));
            let (bx, by) = body(&t, 0);
            assert_eq!(t.press(bx, by), Press::Activated(t.ids()[0]));
            t.release(bx, by);
            let mut a = t.ids();
            a.sort_unstable();
            let mut b = ids;
            b.sort_unstable();
            assert_eq!(a, b);
        }
    }

    #[test]
    fn a_dragged_tab_closed_meanwhile_ends_the_drag() {
        let mut t = tabs(3);
        let ids = t.ids();
        let (x, y) = body(&t, 0);
        t.press(x, y);
        t.drag(x + 10.0, y);
        t.close(ids[0]);
        assert!(!t.drag(x + 100.0, y));
        assert_eq!(t.release(x + 100.0, y), Release::None);
        assert!(t.view().tabs.iter().all(|s| !s.dragging));
    }

    #[test]
    fn cancel_drops_a_press_in_progress() {
        let mut t = tabs(3);
        let (x, y) = body(&t, 0);
        t.press(x, y);
        t.drag(x + 10.0, y);
        assert!(t.cancel());
        assert!(!t.cancel());
        assert!(t.view().tabs.iter().all(|s| !s.dragging));
        assert_eq!(t.release(x, y), Release::None);
        let (cx, cy) = cross(&t, 1);
        t.press(cx, cy);
        assert!(t.cancel());
        assert_eq!(t.release(cx, cy), Release::None);
    }

    #[test]
    fn nothing_is_hovered_during_a_drag_and_after_the_drop_the_pointer_decides() {
        let mut t = tabs(3);
        let (x, y) = body(&t, 0);
        t.hover(x, y);
        t.press(x, y);
        t.drag(x + 20.0, y);
        assert_eq!(t.view().hover, None);
        assert!(!t.hover(x + 25.0, y));
        t.release(x + 20.0, y);
        assert_eq!(t.view().hover, Some(BarHit::Tab(0)));
    }

    // --- middle click ---

    #[test]
    fn middle_click_names_a_regular_tab_only() {
        let t = mixed(1, 2);
        let ids = t.ids();
        let (x, y) = body(&t, 0);
        assert_eq!(t.middle_click(x, y), None, "pinned tabs need the menu");
        let (x, y) = body(&t, 1);
        assert_eq!(t.middle_click(x, y), Some(ids[1]));
        let (x, y) = cross(&t, 2);
        assert_eq!(t.middle_click(x, y), Some(ids[2]));
        let (x, y) = middle(&t.layout().1.unwrap());
        assert_eq!(t.middle_click(x, y), None);
        assert_eq!(t.middle_click(400.0, 300.0), None);
        // It only names: nothing changed.
        assert_eq!(t.len(), 3);
    }

    // --- the menu ---

    fn open(t: &mut Tabs, i: usize) -> MenuView {
        let (x, y) = body(t, i);
        assert!(t.open_menu(x, y));
        t.view().menu.unwrap()
    }

    fn row(m: &MenuView, k: usize) -> (f32, f32) {
        middle(&m.items[k].rect)
    }

    fn rows(m: &MenuView) -> Vec<(MenuAction, &'static str, bool)> {
        m.items
            .iter()
            .map(|i| (i.action, i.label, i.enabled))
            .collect()
    }

    #[test]
    fn the_menu_of_a_regular_tab() {
        let mut t = mixed(1, 3);
        let m = open(&mut t, 2);
        assert_eq!(
            rows(&m),
            [
                (MenuAction::Pin, "Pin tab", true),
                (MenuAction::Close, "Close", true),
                (MenuAction::CloseOthers, "Close others", true),
                (MenuAction::CloseRight, "Close to the right", true),
                (MenuAction::Rename, "Rename tab...", true),
            ]
        );
        assert!(t.menu_open());
        assert_eq!(m.hover, None);
    }

    #[test]
    fn the_menu_of_a_pinned_tab_unpins() {
        let mut t = mixed(2, 0);
        let m = open(&mut t, 0);
        assert_eq!(m.items[0].action, MenuAction::Unpin);
        assert_eq!(m.items[0].label, "Unpin tab");
        // No regular tab: nothing to close in bulk.
        assert!(!m.items[2].enabled && !m.items[3].enabled);
        assert!(
            m.items[1].enabled,
            "a pinned tab can be closed from the menu"
        );
        // With a regular tab, both bulk rows work for a pinned tab.
        let mut t = mixed(2, 1);
        let m = open(&mut t, 1);
        assert!(m.items[2].enabled && m.items[3].enabled);
    }

    #[test]
    fn close_others_and_close_right_are_enabled_only_when_they_would_close_something() {
        // Only tab: nothing else, nothing right.
        let mut t = tabs(1);
        let m = open(&mut t, 0);
        assert!(!m.items[2].enabled && !m.items[3].enabled);
        // The last of several: others yes, right no.
        let mut t = tabs(3);
        let m = open(&mut t, 2);
        assert!(m.items[2].enabled && !m.items[3].enabled);
        // The first of several: both.
        let m = open(&mut t, 0);
        assert!(m.items[2].enabled && m.items[3].enabled);
        // A regular tab whose only neighbours are pinned: others no, right no.
        let mut t = mixed(3, 1);
        let m = open(&mut t, 3);
        assert!(!m.items[2].enabled && !m.items[3].enabled);
    }

    #[test]
    fn the_menu_opens_below_the_tab_in_rows() {
        let mut t = tabs(3);
        let r = rect(&t, 1);
        let m = open(&mut t, 1);
        assert_eq!(m.rect.x, r.x);
        assert_eq!(m.rect.y, r.y + r.h + TAB_GAP);
        assert_eq!((m.rect.w, m.rect.h), (MENU_W, 5.0 * MENU_ROW_H));
        for (k, item) in m.items.iter().enumerate() {
            assert_eq!(
                item.rect,
                Rect::new(
                    m.rect.x,
                    m.rect.y + k as f32 * MENU_ROW_H,
                    MENU_W,
                    MENU_ROW_H
                )
            );
        }
        // It opens from the ✕ too, and from a pinned tab.
        t.close_menu();
        let (x, y) = cross(&t, 1);
        assert!(t.open_menu(x, y));
        assert_eq!(t.view().menu.unwrap().rect, m.rect);
    }

    /// Review Focus 5: next to the panel's right edge, the menu stays inside the panel.
    #[test]
    fn the_menu_near_the_right_edge_stays_inside_the_panel() {
        for n in 1..=MAX_TABS {
            for pinned in [0, n.min(3)] {
                let mut t = mixed(pinned, n - pinned);
                for i in 0..n {
                    let m = open(&mut t, i);
                    assert!(m.rect.x >= 0.0, "{n} tabs, tab {i}");
                    assert!(
                        m.rect.x + m.rect.w <= TERM_W,
                        "{n} tabs, tab {i}: {:?}",
                        m.rect
                    );
                    assert!(m.rect.y + m.rect.h <= TERM_H);
                    assert!(m.items.iter().all(|it| {
                        it.rect.x >= m.rect.x
                            && it.rect.x + it.rect.w <= m.rect.x + m.rect.w
                            && it.rect.y >= m.rect.y
                            && it.rect.y + it.rect.h <= m.rect.y + m.rect.h
                    }));
                    // Still below the tab and under it, horizontally overlapping.
                    let r = rect(&t, i);
                    assert!(m.rect.y >= r.y + r.h);
                    assert!(m.rect.x <= r.x + r.w && m.rect.x + m.rect.w >= r.x);
                }
            }
        }
        // The last of twelve is pushed left to fit.
        let mut t = tabs(MAX_TABS);
        let r = rect(&t, MAX_TABS - 1);
        let m = open(&mut t, MAX_TABS - 1);
        assert!(m.rect.x < r.x);
        assert!(m.rect.x + MENU_W <= TERM_W);
    }

    /// Review Focus 5: a click outside the menu closes it and nothing else happens.
    #[test]
    fn a_press_outside_the_menu_closes_it_without_acting_on_what_is_under_it() {
        let mut t = tabs(3);
        let ids = t.ids();
        let m = open(&mut t, 1);
        // On a tab.
        let (x, y) = body(&t, 0);
        assert_eq!(t.press(x, y), Press::MenuDismissed);
        assert!(!t.menu_open());
        assert_eq!(
            t.active(),
            Some(ids[2]),
            "the tab under the press was not activated"
        );
        // The matching release and a drag do nothing: no press was started.
        assert!(!t.drag(x + 50.0, y));
        assert_eq!(t.release(x, y), Release::None);
        assert_eq!(t.ids(), ids);
        // On a ✕, on "+", on empty space and on the panel ✕.
        for pt in [
            cross(&t, 0),
            middle(&t.layout().1.unwrap()),
            (300.0, 300.0),
            middle(&TERM_CLOSE),
        ] {
            open(&mut t, 1);
            assert_eq!(t.press(pt.0, pt.1), Press::MenuDismissed, "{pt:?}");
            assert_eq!(t.release(pt.0, pt.1), Release::None);
            assert!(!t.menu_open());
            assert_eq!(t.len(), 3);
        }
        // Just outside the menu's own edges.
        for pt in [
            (m.rect.x - 1.0, m.rect.y + 5.0),
            (m.rect.x + m.rect.w, m.rect.y + 5.0),
            (m.rect.x + 5.0, m.rect.y + m.rect.h),
            (m.rect.x + 5.0, m.rect.y - 1.0),
        ] {
            open(&mut t, 1);
            assert_eq!(t.press(pt.0, pt.1), Press::MenuDismissed, "{pt:?}");
        }
        // With the menu gone, the same press works as usual again.
        assert_eq!(t.press(x, y), Press::Activated(ids[0]));
    }

    #[test]
    fn pressing_a_menu_row_returns_its_action_for_that_tab_and_closes_the_menu() {
        let mut t = mixed(1, 3);
        let ids = t.ids();
        let m = open(&mut t, 2);
        for (k, action) in [
            MenuAction::Pin,
            MenuAction::Close,
            MenuAction::CloseOthers,
            MenuAction::CloseRight,
        ]
        .into_iter()
        .enumerate()
        {
            open(&mut t, 2);
            let (x, y) = row(&m, k);
            assert_eq!(t.press(x, y), Press::Menu(action, ids[2]));
            assert!(!t.menu_open());
            // Nothing was done to the tabs: the caller carries the action out.
            assert_eq!(t.ids(), ids);
            assert_eq!(t.active(), Some(ids[3]));
            assert_eq!(t.release(x, y), Release::None);
        }
    }

    #[test]
    fn pressing_a_disabled_row_does_nothing_but_closes_the_menu() {
        let mut t = tabs(1);
        let m = open(&mut t, 0);
        for k in [2, 3] {
            open(&mut t, 0);
            let (x, y) = row(&m, k);
            assert_eq!(t.press(x, y), Press::MenuDismissed);
            assert!(!t.menu_open());
            assert_eq!(t.len(), 1);
        }
    }

    #[test]
    fn hovering_menu_rows() {
        let mut t = tabs(1);
        let m = open(&mut t, 0);
        // Hover on a tab before the menu opens is gone once it does.
        let (bx, by) = body(&t, 0);
        t.close_menu();
        t.hover(bx, by);
        assert!(t.open_menu(bx, by));
        assert_eq!(t.view().hover, None);
        let (x, y) = row(&m, 1);
        assert!(t.hover(x, y));
        assert_eq!(t.view().menu.unwrap().hover, Some(1));
        assert!(!t.hover(x + 2.0, y));
        let (x, y) = row(&m, 0);
        assert!(t.hover(x, y));
        assert_eq!(t.view().menu.unwrap().hover, Some(0));
        // A disabled row is not lit.
        let (x, y) = row(&m, 2);
        assert!(t.hover(x, y));
        assert_eq!(t.view().menu.unwrap().hover, None);
        // Neither is anything outside, and the tabs under the menu's surroundings are not
        // hovered while it is open.
        let (x, y) = row(&m, 1);
        t.hover(x, y);
        assert!(t.hover(bx, by));
        assert_eq!(t.view().menu.unwrap().hover, None);
        assert_eq!(t.view().hover, None);
        assert!(!t.hover(bx, by));
        // clear_hover keeps the menu open, with no row lit.
        t.hover(x, y);
        assert!(t.clear_hover());
        assert!(t.menu_open());
        assert_eq!(t.view().menu.unwrap().hover, None);
        assert!(!t.clear_hover());
    }

    #[test]
    fn a_right_click_elsewhere_moves_or_closes_the_menu() {
        let mut t = tabs(3);
        let m0 = open(&mut t, 0);
        let ids = t.ids();
        let m1 = open(&mut t, 1);
        assert_ne!(m0.rect, m1.rect);
        assert!(t.menu_open());
        // Off the tabs: closes it, and says the view changed.
        assert!(t.open_menu(400.0, 300.0));
        assert!(!t.menu_open());
        // Nothing to close, nothing changed.
        assert!(!t.open_menu(400.0, 300.0));
        let (x, y) = middle(&t.layout().1.unwrap());
        assert!(!t.open_menu(x, y), "\"+\" has no menu");
        assert!(!t.close_menu());
        open(&mut t, 2);
        assert!(t.close_menu());
        assert_eq!(t.ids(), ids);
    }

    #[test]
    fn the_menu_follows_its_tabs_state_and_goes_when_the_tab_does() {
        let mut t = mixed(0, 3);
        let ids = t.ids();
        open(&mut t, 1);
        // Another tab closes (a shortcut, say): "Close to the right" has nothing left.
        t.close(ids[2]);
        let m = t.view().menu.unwrap();
        assert!(!m.items[3].enabled);
        assert!(m.items[2].enabled);
        // The tab gets pinned meanwhile: the row now says Unpin.
        t.set_pinned(ids[1], true);
        assert_eq!(t.view().menu.unwrap().items[0].label, "Unpin tab");
        // Its tab closes: so does the menu.
        t.close(ids[1]);
        assert!(!t.menu_open());
        assert!(t.view().menu.is_none());
    }

    #[test]
    fn middle_click_does_nothing_while_the_menu_is_open() {
        let mut t = tabs(2);
        let (x, y) = body(&t, 0);
        assert!(t.middle_click(x, y).is_some());
        open(&mut t, 1);
        assert_eq!(t.middle_click(x, y), None);
    }

    #[test]
    fn a_right_click_cancels_a_press_in_progress() {
        let mut t = tabs(3);
        let (x, y) = cross(&t, 0);
        t.press(x, y);
        assert!(t.open_menu(x, y));
        // The ✕ press is not armed any more; the release closes nothing.
        assert_eq!(t.release(x, y), Release::None);
        assert!(t.menu_open());
    }

    // --- shortcuts ---

    #[test]
    fn the_tab_shortcuts() {
        let key = |vk: u16, c: bool, s: bool, a: bool| shortcut(vk, c, s, a);
        assert_eq!(key(b'T' as u16, true, true, false), Some(TabKey::New));
        assert_eq!(key(b'W' as u16, true, true, false), Some(TabKey::Close));
        assert_eq!(key(0x09, true, false, false), Some(TabKey::Next));
        assert_eq!(key(0x09, true, true, false), Some(TabKey::Prev));
        for d in 1..=9u16 {
            assert_eq!(
                key(b'0' as u16 + d, true, true, false),
                Some(TabKey::Jump(d as usize - 1))
            );
        }
        assert_eq!(key(b'0' as u16, true, true, false), None);
        assert_eq!(key(b':' as u16, true, true, false), None);
    }

    #[test]
    fn everything_else_goes_to_the_shell() {
        // Without Ctrl, or without Shift where it is needed.
        assert_eq!(shortcut(b'T' as u16, false, true, false), None);
        assert_eq!(shortcut(b'T' as u16, true, false, false), None); // Ctrl+T is the shell's
        assert_eq!(shortcut(b'W' as u16, true, false, false), None); // Ctrl+W deletes a word
        assert_eq!(shortcut(b'1' as u16, true, false, false), None);
        assert_eq!(shortcut(0x09, false, false, false), None);
        assert_eq!(shortcut(0x09, false, true, false), None);
        // Other keys, with and without modifiers.
        for vk in [b'A' as u16, b'C' as u16, 0x0D, 0x1B, 0x25, 0x70] {
            for (c, s) in [(true, true), (true, false), (false, true), (false, false)] {
                assert_eq!(
                    shortcut(vk, c, s, false),
                    None,
                    "vk {vk:#x} ctrl {c} shift {s}"
                );
            }
        }
    }

    #[test]
    fn alt_never_makes_a_shortcut() {
        // AltGr arrives as Ctrl+Alt: the characters it types must reach the shell.
        for vk in [b'T' as u16, b'W' as u16, 0x09, b'1' as u16, b'9' as u16] {
            for s in [false, true] {
                assert_eq!(shortcut(vk, true, s, true), None, "vk {vk:#x} shift {s}");
            }
        }
        // Ctrl+Alt+1..9 are Tile's.
        assert_eq!(shortcut(b'2' as u16, true, false, true), None);
        assert_eq!(shortcut(b'T' as u16, false, true, true), None);
    }

    // --- the pinned-tabs file ---

    fn pin(title: &str, shell: &str, args: &[&str], cwd: &str) -> PinnedTab {
        PinnedTab {
            title: title.into(),
            shell: shell.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: cwd.into(),
            agent: String::new(),
            custom_title: String::new(),
        }
    }

    #[test]
    fn parse_pinned_reads_good_entries() {
        let (tabs, notes) = parse_pinned(
            r#"
[[pinned]]
title = "build"
shell = "pwsh.exe"
args = ["-NoLogo", "-NoProfile"]
cwd = 'C:\Users\me\code'

[[pinned]]
title = "wsl"
shell = "wsl.exe"
args = []
cwd = ""
"#,
        );
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            tabs,
            [
                pin(
                    "build",
                    "pwsh.exe",
                    &["-NoLogo", "-NoProfile"],
                    r"C:\Users\me\code"
                ),
                pin("wsl", "wsl.exe", &[], ""),
            ]
        );
    }

    #[test]
    fn parse_pinned_fills_in_what_is_left_out() {
        let (tabs, notes) = parse_pinned("[[pinned]]\nshell = \"cmd.exe\"\nextra = 1\n");
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(tabs, [pin("", "cmd.exe", &[], "")]);
    }

    #[test]
    fn parse_pinned_of_nothing_is_nothing() {
        assert_eq!(parse_pinned(""), (Vec::new(), Vec::new()));
        assert_eq!(parse_pinned("# only a comment\n"), (Vec::new(), Vec::new()));
        assert_eq!(parse_pinned("other = 1\n"), (Vec::new(), Vec::new()));
        assert_eq!(parse_pinned("pinned = []\n"), (Vec::new(), Vec::new()));
    }

    #[test]
    fn parse_pinned_skips_a_bad_entry_and_keeps_the_rest() {
        let (tabs, notes) = parse_pinned(
            r#"
[[pinned]]
title = "ok one"
shell = "pwsh.exe"

[[pinned]]
title = "no shell"

[[pinned]]
title = "blank shell"
shell = "   "

[[pinned]]
title = 5
shell = "pwsh.exe"

[[pinned]]
shell = "pwsh.exe"
args = "not a list"

[[pinned]]
title = "ok two"
shell = "cmd.exe"
args = ["/k"]
"#,
        );
        // No shell, or a blank one, is the default shell.
        assert_eq!(
            tabs.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            ["ok one", "no shell", "blank shell", "ok two"]
        );
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].starts_with("pinned tab 4:"), "{notes:?}");
        assert!(notes[1].starts_with("pinned tab 5:"), "{notes:?}");
    }

    #[test]
    fn parse_pinned_entries_that_are_not_tables() {
        let (tabs, notes) = parse_pinned("pinned = [1, \"two\", { shell = \"cmd.exe\" }]\n");
        assert_eq!(tabs, [pin("", "cmd.exe", &[], "")]);
        assert_eq!(notes.len(), 2, "{notes:?}");
    }

    #[test]
    fn parse_pinned_rejects_the_file_with_one_message() {
        let (tabs, notes) = parse_pinned("[[pinned]\nshell = \"x\"\n");
        assert!(tabs.is_empty());
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("syntax error"), "{notes:?}");
        assert!(notes[0].contains("line 1"), "{notes:?}");
        let (tabs, notes) = parse_pinned("pinned = \"pwsh\"\n");
        assert!(tabs.is_empty());
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("array of tables"), "{notes:?}");
        // A syntax error further down names its line.
        let (_, notes) = parse_pinned("# a\n\n[[pinned]]\nshell = \"x\"\nbad bad\n");
        assert!(notes[0].contains("line 5"), "{notes:?}");
    }

    #[test]
    fn parse_pinned_keeps_at_most_the_limit() {
        let src = |n: usize| -> String {
            (0..n)
                .map(|i| format!("[[pinned]]\ntitle = \"t{i}\"\nshell = \"pwsh.exe\"\n\n"))
                .collect()
        };
        let (tabs, notes) = parse_pinned(&src(MAX_TABS + 3));
        assert_eq!(tabs.len(), MAX_TABS);
        assert_eq!(tabs[0].title, "t0");
        assert_eq!(tabs[MAX_TABS - 1].title, format!("t{}", MAX_TABS - 1));
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("12"), "{notes:?}");
        // Exactly the limit is fine, without a message.
        let (tabs, notes) = parse_pinned(&src(MAX_TABS));
        assert_eq!(tabs.len(), MAX_TABS);
        assert!(notes.is_empty(), "{notes:?}");
        // Bad entries do not count against the limit.
        let mut text = src(MAX_TABS);
        text.push_str("[[pinned]]\ntitle = 5\n");
        let (tabs, notes) = parse_pinned(&text);
        assert_eq!(tabs.len(), MAX_TABS);
        assert_eq!(notes.len(), 1, "{notes:?}");
    }

    #[test]
    fn pinned_tabs_round_trip_through_the_file() {
        let tabs = vec![
            pin(
                "build",
                "pwsh.exe",
                &["-NoLogo", "-Command", "a \"quoted\" arg"],
                r"C:\Users\me\code",
            ),
            pin("wsl", "wsl.exe", &[], ""),
            pin(
                "日本語 \u{1f600} tab",
                "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
                &["-c", "x=1\ny=2"],
                "D:\\a b\\c",
            ),
            pin("", "cmd.exe", &["/k", "echo hi"], "~"),
        ];
        let text = pinned_to_toml(&tabs);
        let (back, notes) = parse_pinned(&text);
        assert!(notes.is_empty(), "{notes:?}\n{text}");
        assert_eq!(back, tabs, "{text}");
        // And again: the text is stable.
        assert_eq!(pinned_to_toml(&back), text);
        assert!(text.contains("[[pinned]]"), "{text}");
    }

    #[test]
    fn an_empty_pinned_list_round_trips() {
        let text = pinned_to_toml(&[]);
        assert_eq!(parse_pinned(&text), (Vec::new(), Vec::new()), "{text}");
    }

    #[test]
    fn custom_pinned_titles_round_trip_without_changing_older_files() {
        let mut tab = pin("shell title", "pwsh.exe", &[], "");
        let old_text = pinned_to_toml(&[tab.clone()]);
        assert!(!old_text.contains("custom_title"));
        assert!(parse_pinned(&old_text).0[0].custom_title.is_empty());
        tab.custom_title = "API 日".into();
        let (read, errors) = parse_pinned(&pinned_to_toml(&[tab.clone()]));
        assert!(errors.is_empty());
        assert_eq!(read, [tab]);
    }

    #[test]
    fn the_agent_round_trips_and_is_left_out_when_empty() {
        let mut a = pin("claude", "pwsh.exe", &[], "C:\\x");
        a.agent = "claude".into();
        let mut odd = pin("odd", "pwsh.exe", &[], "");
        odd.agent = "gemini".into();
        let plain = pin("plain", "pwsh.exe", &[], "");
        let tabs = vec![a, odd, plain];
        let text = pinned_to_toml(&tabs);
        assert_eq!(text.matches("agent").count(), 2, "{text}");
        let (back, notes) = parse_pinned(&text);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(back, tabs, "unknown agents are kept as they are");
        // Old files have no agent.
        let (old, notes) = parse_pinned("[[pinned]]\ntitle = \"x\"\n");
        assert!(notes.is_empty());
        assert_eq!(old[0].agent, "");
    }

    // --- the "+" menu ---

    fn plus_center(t: &Tabs) -> (f32, f32) {
        middle(&t.layout().1.unwrap())
    }

    fn open_plus(t: &mut Tabs) -> MenuView {
        let (x, y) = plus_center(t);
        assert_eq!(t.press(x, y), Press::PlusMenu);
        assert!(t.menu_open());
        t.view().menu.unwrap()
    }

    #[test]
    fn the_plus_menu_rows_labels_and_hints() {
        let mut t = tabs(2);
        let m = open_plus(&mut t);
        let got: Vec<_> = m
            .items
            .iter()
            .map(|i| (i.action, i.label, i.hint, i.enabled))
            .collect();
        assert_eq!(
            got,
            vec![
                (MenuAction::NewShell, "Terminal", "", true),
                (MenuAction::NewClaude, "Claude", "yolo", true),
                (MenuAction::NewCodex, "Codex", "yolo", true),
            ]
        );
        assert_eq!(t.len(), 2, "opening the menu adds nothing");
        // Existing tab menus have no hints.
        t.close_menu();
        assert!(open(&mut t, 0).items.iter().all(|i| i.hint.is_empty()));
    }

    #[test]
    fn the_plus_menu_opens_under_plus() {
        let mut t = tabs(2);
        let p = t.layout().1.unwrap();
        let m = open_plus(&mut t);
        assert_eq!(m.rect.x, p.x);
        assert_eq!(m.rect.y, p.y + p.h + TAB_GAP);
        assert_eq!((m.rect.w, m.rect.h), (MENU_W, 3.0 * MENU_ROW_H));
        // With no tabs "+" is first in the strip.
        let mut t = Tabs::new();
        let p = t.layout().1.unwrap();
        assert_eq!(open_plus(&mut t).rect.x, p.x);
    }

    #[test]
    fn the_plus_menu_stays_inside_the_panel_when_plus_is_far_right() {
        let mut t = tabs(MAX_TABS - 1);
        let p = t.layout().1.unwrap();
        let m = open_plus(&mut t);
        assert!(m.rect.x >= MENU_MARGIN, "{:?}", m.rect);
        assert!(m.rect.x + m.rect.w <= TERM_W - MENU_MARGIN, "{:?}", m.rect);
        assert!(m.rect.x < p.x, "plus at {p:?} pushed the menu left");
    }

    #[test]
    fn choosing_a_plus_row_returns_the_action_with_the_active_tab() {
        for (k, action) in [
            MenuAction::NewShell,
            MenuAction::NewClaude,
            MenuAction::NewCodex,
        ]
        .into_iter()
        .enumerate()
        {
            let mut t = tabs(2);
            let active = t.active().unwrap();
            let m = open_plus(&mut t);
            let (x, y) = row(&m, k);
            assert_eq!(t.press(x, y), Press::Menu(action, active));
            assert!(!t.menu_open());
            assert_eq!(t.len(), 2);
        }
        // No tabs: id 0.
        let mut t = Tabs::new();
        let m = open_plus(&mut t);
        let (x, y) = row(&m, 1);
        assert_eq!(t.press(x, y), Press::Menu(MenuAction::NewClaude, 0));
    }

    #[test]
    fn an_outside_press_dismisses_the_plus_menu_without_acting() {
        let mut t = tabs(2);
        let ids = t.ids();
        open_plus(&mut t);
        let (x, y) = body(&t, 0);
        assert_eq!(t.press(x, y), Press::MenuDismissed);
        assert!(!t.menu_open());
        assert_eq!(
            t.active(),
            Some(ids[1]),
            "the tab under the press is untouched"
        );
        open_plus(&mut t);
        assert_eq!(t.press(400.0, 300.0), Press::MenuDismissed);
        assert!(!t.menu_open());
    }

    #[test]
    fn pressing_plus_again_closes_its_menu() {
        let mut t = tabs(2);
        open_plus(&mut t);
        let (x, y) = plus_center(&t);
        assert_eq!(t.press(x, y), Press::MenuDismissed);
        assert!(!t.menu_open());
        assert_eq!(t.press(x, y), Press::PlusMenu);
    }

    #[test]
    fn no_plus_no_plus_menu_at_the_limit() {
        let mut t = tabs(MAX_TABS);
        assert!(t.layout().1.is_none());
        assert_eq!(
            t.press(TABS.x + TABS.w - 4.0, TABS.y + 4.0),
            Press::Activated(t.ids()[MAX_TABS - 1])
        );
        t.close_menu();
        // Right click on the empty spot does nothing.
        assert!(!t.open_menu(TABS.x + TABS.w + 2.0, TABS.y + 4.0));
        assert!(!t.menu_open());
    }

    #[test]
    fn right_click_on_plus_does_nothing() {
        let mut t = tabs(2);
        let (x, y) = plus_center(&t);
        assert!(!t.open_menu(x, y));
        assert!(!t.menu_open());
    }

    #[test]
    fn hovering_plus_menu_rows() {
        let mut t = tabs(2);
        let m = open_plus(&mut t);
        let (x, y) = row(&m, 2);
        assert!(t.hover(x, y));
        assert_eq!(t.view().menu.unwrap().hover, Some(2));
        let (x, y) = row(&m, 0);
        assert!(t.hover(x, y));
        assert_eq!(t.view().menu.unwrap().hover, Some(0));
        assert!(t.hover(400.0, 300.0));
        assert_eq!(t.view().menu.unwrap().hover, None);
        assert!(!t.hover(400.0, 300.0));
        assert!(t.menu_open());
    }

    #[test]
    fn the_plus_menu_survives_a_tab_closing_and_tab_presses_still_work() {
        let mut t = tabs(3);
        let ids = t.ids();
        open_plus(&mut t);
        t.close(ids[0]);
        assert!(t.menu_open());
        t.close_menu();
        // A tab press and a drag behave as before.
        let (x, y) = body(&t, 0);
        assert_eq!(t.press(x, y), Press::Activated(t.ids()[0]));
        assert!(t.drag(x + 60.0, y));
        assert_eq!(t.release(x + 60.0, y), Release::Reordered);
        assert!(!t.menu_open());
    }
}
