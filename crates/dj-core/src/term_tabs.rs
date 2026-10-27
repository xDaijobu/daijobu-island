//! The terminal's tabs: their order, pinning, the tab bar's layout and hit testing,
//! drag-to-reorder and the tab menu. Pure: the app keys its shell sessions by [`TabId`] and
//! `dj-render` draws a [`TabBarView`].

use crate::layout::{Rect, TERM_W};

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
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuItemView {
    pub action: MenuAction,
    pub label: &'static str,
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
