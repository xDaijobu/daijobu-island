//! Snap preview stack: the finished snaps kept as cards below the island. Pure model:
//! time enters only as an `Instant` argument, nothing ticks inside, and nothing here
//! touches a window, a bitmap or the clipboard (the app owns those and addresses them by
//! `ItemId`).
//!
//! One predicate decides whether a card's countdown runs, and it is re-evaluated after
//! every mutation, so nothing drifts:
//! `!hidden && no card selected && !hovered && !focused && !pinned && !busy`.
//! Pausing stores the remainder and resuming continues it, never restarts it.
//!
//! Layout is in logical px (96 DPI units) in the stack window's client space, origin at
//! its top-left; `place` maps the window onto the desktop in physical px.

use crate::layout::Rect;
use crate::snap::Countdown;
use crate::spring::{Spring, SpringSpec};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Most cards at once.
pub const MAX_ITEMS: usize = 5;
/// Most retained full-resolution payload bytes (BGRA + PNG) at once.
pub const MAX_BYTES: u64 = 256 * 1024 * 1024;
pub const CARD_W: f32 = 124.0;
pub const CARD_H: f32 = 80.0;
pub const CARD_R: f32 = 12.0;
pub const GAP: f32 = 10.0;
/// The countdown ring's size.
pub const RING: f32 = 16.0;
/// Gap between the island's lower edge and the stack window's content top.
pub const BELOW_ISLAND: f32 = 12.0;
/// Transparent margin around the content inside the window (shadow + lift room).
pub const PAD: f32 = 16.0;
/// "Preview stack full" stays this long.
pub const NOTICE: Duration = Duration::from_secs(3);
/// The text of the stack-full notice.
pub const NOTICE_TEXT: &str = "Preview stack full";

/// The selected mark: a `CHECK` square `CHECK_INSET` in from the card's top-left.
pub const CHECK: f32 = 18.0;
pub const CHECK_INSET: f32 = 6.0;
/// The ring sits this far in from the card's top-right.
pub const RING_INSET: f32 = 5.0;
/// Space between the check, the status chip and the ring.
pub const CHIP_GAP: f32 = 4.0;
/// The action row: `ACTION_H` high, `ACTION_INSET` in from the card's left, right and bottom.
pub const ACTION_H: f32 = 16.0;
pub const ACTION_INSET: f32 = 5.0;
pub const ACTION_GAP: f32 = 3.0;
/// Widths of Copy, Save/Open, Pin/Unpin and Dismiss (they fill the 114 px row with the gaps).
pub const ACTION_W: [f32; 4] = [27.0, 27.0, 33.0, 18.0];
/// The selection bar: its height, side padding and the gap between its parts.
pub const BAR_H: f32 = 32.0;
pub const BAR_PAD: f32 = 8.0;
pub const BAR_GAP: f32 = 6.0;
pub const BAR_BTN_H: f32 = 24.0;
/// The "N selected" text area, then the widths of Select all, Clear, Save selected and
/// Dismiss selected.
pub const BAR_COUNT_W: f32 = 72.0;
pub const BAR_BTN_W: [f32; 4] = [76.0, 52.0, 100.0, 120.0];
/// The stack-full notice's size.
pub const NOTICE_W: f32 = 168.0;
pub const NOTICE_H: f32 = 24.0;

/// A card's identity: monotonic from 1, never reused. Async results address cards by it,
/// never by their position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u64);

/// What a card says about its last copy/save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardStatus {
    Ready,
    Copied,
    CopyFailed,
    Saving,
    Saved,
    SaveFailed,
    /// The saved file is gone.
    Missing,
}

impl CardStatus {
    /// The chip text. `Ready` has none.
    pub fn label(self) -> Option<&'static str> {
        match self {
            CardStatus::Ready => None,
            CardStatus::Copied => Some("copied"),
            CardStatus::CopyFailed => Some("copy failed"),
            CardStatus::Saving => Some("saving…"),
            CardStatus::Saved => Some("saved"),
            CardStatus::SaveFailed => Some("save failed"),
            CardStatus::Missing => Some("file missing"),
        }
    }

    pub fn is_error(self) -> bool {
        matches!(
            self,
            CardStatus::CopyFailed | CardStatus::SaveFailed | CardStatus::Missing
        )
    }
}

/// A card's four buttons, in row order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardAction {
    Copy,
    /// Reads "open" once saved.
    Save,
    /// Reads "unpin" when pinned.
    Pin,
    Dismiss,
}

impl CardAction {
    pub const ALL: [CardAction; 4] = [
        CardAction::Copy,
        CardAction::Save,
        CardAction::Pin,
        CardAction::Dismiss,
    ];
}

/// The selection bar's four buttons, in row order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarAction {
    SelectAll,
    Clear,
    SaveSelected,
    DismissSelected,
}

impl BarAction {
    pub const ALL: [BarAction; 4] = [
        BarAction::SelectAll,
        BarAction::Clear,
        BarAction::SaveSelected,
        BarAction::DismissSelected,
    ];
}

/// One card. Read through `Stack::items`; the stack owns every field, and the flags are
/// changed through its mutators so the countdowns stay in step with them.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: ItemId,
    /// Image size in px.
    pub width: u32,
    pub height: u32,
    /// Retained BGRA + PNG bytes for this payload.
    pub bytes: u64,
    pub pinned: bool,
    pub selected: bool,
    pub hovered: bool,
    /// Has the keyboard focus.
    pub focused: bool,
    /// A copy/save/encode for this card is in flight.
    pub busy: bool,
    pub status: CardStatus,
    /// Where it was saved, once it was.
    pub saved: Option<PathBuf>,
    countdown: Countdown,
}

impl Item {
    /// Pinned, selected, hovered, focused or busy: eviction never takes it.
    pub fn is_protected(&self) -> bool {
        self.pinned || self.selected || self.hovered || self.focused || self.busy
    }
}

/// What `admit` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admit {
    /// The card is in; `evicted` were dropped (oldest first) to make room.
    Added { id: ItemId, evicted: Vec<ItemId> },
    /// No room even after evicting every unprotected card, or the shot alone is over the
    /// byte budget. Nothing changed except the notice.
    Full,
}

/// Where the keyboard focus is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Card(ItemId),
    Bar(BarAction),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackKey {
    Left,
    Right,
    Tab,
    ShiftTab,
    Space,
    Enter,
    Escape,
    Delete,
    C,
    S,
    P,
    CtrlA,
}

/// What the caller does after `Stack::key`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCmd {
    /// Nothing changed.
    None,
    /// Only the focus moved: redraw.
    Redraw,
    /// Run this action on the card.
    Card(ItemId, CardAction),
    /// Toggle the card's selection (`Stack::toggle_select`).
    ToggleSelect(ItemId),
    /// Run this bar action.
    Bar(BarAction),
    /// Give the keyboard focus back. The stack's own focus is already cleared.
    Release,
}

/// `Stack::next_id` before the first card.
const FIRST_ID: u64 = 1;

/// The stack's cards and the rules that keep them.
#[derive(Clone, Debug)]
pub struct Stack {
    lifetime: Duration,
    max_items: usize,
    max_bytes: u64,
    /// Newest first.
    items: Vec<Item>,
    next_id: u64,
    /// The stack window is hidden or a capture is up: no time counts.
    hidden: bool,
    focus: Option<Focus>,
    /// The stack-full notice shows until this instant (wall time, hidden or not).
    notice_until: Option<Instant>,
}

/// The one countdown predicate.
fn runs(hidden: bool, any_selected: bool, it: &Item) -> bool {
    !hidden && !any_selected && !it.hovered && !it.focused && !it.pinned && !it.busy
}

impl Stack {
    /// `MAX_ITEMS` cards and `MAX_BYTES`.
    pub fn new(lifetime: Duration) -> Self {
        Self::with_limits(lifetime, MAX_ITEMS, MAX_BYTES)
    }

    /// Custom limits. `layout` and `window_size` assume at most `MAX_ITEMS` cards.
    pub fn with_limits(lifetime: Duration, max_items: usize, max_bytes: u64) -> Self {
        Self {
            lifetime,
            max_items,
            max_bytes,
            items: Vec::new(),
            next_id: FIRST_ID,
            hidden: false,
            focus: None,
            notice_until: None,
        }
    }

    /// Adds a card with the full lifetime and `CardStatus::Ready`, newest first. Makes room
    /// by evicting the oldest unprotected cards until there are fewer than `max_items` and
    /// the new payload fits the byte budget. If that can't be done (or `bytes` alone is
    /// over the budget) nothing is evicted or added: `Full`, and the notice starts.
    pub fn admit(&mut self, width: u32, height: u32, bytes: u64, now: Instant) -> Admit {
        let Some(evict) = self.plan_evictions(bytes) else {
            self.notice_until = Some(now + NOTICE);
            return Admit::Full;
        };
        self.drop_where(now, |it| evict.contains(&it.id));
        let id = ItemId(self.next_id);
        self.next_id += 1;
        self.items.insert(
            0,
            Item {
                id,
                width,
                height,
                bytes,
                pinned: false,
                selected: false,
                hovered: false,
                focused: false,
                busy: false,
                status: CardStatus::Ready,
                saved: None,
                countdown: Countdown::start(self.lifetime, now),
            },
        );
        self.refresh(now);
        Admit::Added { id, evicted: evict }
    }

    /// The ids to evict (oldest first) so that `bytes` more fit, or None if it can't be done.
    fn plan_evictions(&self, bytes: u64) -> Option<Vec<ItemId>> {
        if bytes > self.max_bytes {
            return None;
        }
        let mut len = self.items.len();
        let mut total = self.bytes();
        let mut plan = Vec::new();
        let mut oldest_first = self.items.iter().rev().filter(|it| !it.is_protected());
        while len >= self.max_items || total.saturating_add(bytes) > self.max_bytes {
            let it = oldest_first.next()?;
            plan.push(it.id);
            len -= 1;
            total -= it.bytes;
        }
        Some(plan)
    }

    /// Dismisses a card, pinned or not. False for an id that is not in the stack.
    pub fn remove(&mut self, id: ItemId, now: Instant) -> bool {
        if self.index_of(id).is_none() {
            return false;
        }
        self.drop_where(now, |it| it.id == id);
        true
    }

    /// Dismisses every selected card (pinned ones too). Returns them, newest first.
    pub fn remove_selected(&mut self, now: Instant) -> Vec<ItemId> {
        self.drop_where(now, |it| it.selected)
    }

    /// Removes the cards whose countdown ran out and returns them, newest first. Also ends
    /// a finished notice.
    pub fn tick(&mut self, now: Instant) -> Vec<ItemId> {
        if self.notice_until.is_some_and(|u| now >= u) {
            self.notice_until = None;
        }
        let due: Vec<ItemId> = self
            .items
            .iter()
            .filter(|it| it.countdown.expired(now))
            .map(|it| it.id)
            .collect();
        if due.is_empty() {
            return due;
        }
        self.drop_where(now, |it| due.contains(&it.id))
    }

    /// The cards, newest first.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn get(&self, id: ItemId) -> Option<&Item> {
        self.items.iter().find(|it| it.id == id)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Retained payload bytes of all cards.
    pub fn bytes(&self) -> u64 {
        self.items.iter().map(|it| it.bytes).sum()
    }

    /// The stack window is hidden, or a capture is up. Nothing counts down meanwhile; the
    /// other flags are left alone.
    pub fn set_hidden(&mut self, hidden: bool, now: Instant) {
        self.hidden = hidden;
        self.refresh(now);
    }

    /// The pointer is over this card (`None`: over no card). An id that is not in the stack
    /// changes nothing.
    pub fn set_hover(&mut self, id: Option<ItemId>, now: Instant) {
        if id.is_some_and(|id| self.index_of(id).is_none()) {
            return;
        }
        for it in &mut self.items {
            it.hovered = Some(it.id) == id;
        }
        self.refresh(now);
    }

    /// Adds the card to the selection or takes it out. Any selection pauses the whole stack.
    pub fn toggle_select(&mut self, id: ItemId, now: Instant) {
        if let Some(it) = self.item_mut(id) {
            it.selected = !it.selected;
            self.refresh(now);
        }
    }

    pub fn select_all(&mut self, now: Instant) {
        for it in &mut self.items {
            it.selected = true;
        }
        self.refresh(now);
    }

    pub fn clear_selection(&mut self, now: Instant) {
        for it in &mut self.items {
            it.selected = false;
        }
        self.refresh(now);
    }

    /// The selected cards, newest first.
    pub fn selected(&self) -> Vec<ItemId> {
        self.items
            .iter()
            .filter(|it| it.selected)
            .map(|it| it.id)
            .collect()
    }

    /// Pins the card (keeps it until dismissed) or unpins it.
    pub fn toggle_pin(&mut self, id: ItemId, now: Instant) {
        if let Some(it) = self.item_mut(id) {
            it.pinned = !it.pinned;
            self.refresh(now);
        }
    }

    /// An action on the card is in flight (or done). False for a stale id.
    pub fn set_busy(&mut self, id: ItemId, busy: bool, now: Instant) -> bool {
        let Some(it) = self.item_mut(id) else {
            return false;
        };
        it.busy = busy;
        self.refresh(now);
        true
    }

    /// False for a stale id.
    pub fn set_status(&mut self, id: ItemId, status: CardStatus) -> bool {
        self.item_mut(id).map(|it| it.status = status).is_some()
    }

    /// Where the card was saved (None forgets it). False for a stale id.
    pub fn set_saved(&mut self, id: ItemId, path: Option<PathBuf>) -> bool {
        self.item_mut(id).map(|it| it.saved = path).is_some()
    }

    /// Time left / lifetime, 0..=1 (the ring). None for a stale id.
    pub fn fraction(&self, id: ItemId, now: Instant) -> Option<f32> {
        let left = self.remaining(id, now)?;
        if self.lifetime.is_zero() {
            return Some(0.0);
        }
        Some((left.as_secs_f32() / self.lifetime.as_secs_f32()).clamp(0.0, 1.0))
    }

    /// Active time the card has left. None for a stale id.
    pub fn remaining(&self, id: ItemId, now: Instant) -> Option<Duration> {
        self.get(id).map(|it| it.countdown.left(now))
    }

    /// The card's countdown is running right now.
    pub fn is_running(&self, id: ItemId) -> bool {
        let any_selected = self.any_selected();
        self.get(id)
            .is_some_and(|it| runs(self.hidden, any_selected, it))
    }

    /// Some card's countdown is running (so the ring needs redrawing).
    pub fn any_running(&self) -> bool {
        let any_selected = self.any_selected();
        self.items
            .iter()
            .any(|it| runs(self.hidden, any_selected, it))
    }

    /// Time until something changes by itself: the soonest expiry of a running card or the
    /// end of the notice. None when nothing will. Zero when one is already due.
    pub fn next_change(&self, now: Instant) -> Option<Duration> {
        let any_selected = self.any_selected();
        let expiries = self
            .items
            .iter()
            .filter(|it| runs(self.hidden, any_selected, it))
            .map(|it| it.countdown.left(now));
        let notice = self
            .notice_until
            .filter(|&until| now < until)
            .map(|until| until.saturating_duration_since(now));
        expiries.chain(notice).min()
    }

    /// "Preview stack full" for `NOTICE` after a refused `admit`.
    pub fn notice(&self, now: Instant) -> Option<&'static str> {
        self.notice_until
            .is_some_and(|until| now < until)
            .then_some(NOTICE_TEXT)
    }

    /// The keyboard focus. A focused card is paused (and protected from eviction).
    pub fn focus(&self) -> Option<Focus> {
        self.focus
    }

    /// Moves the focus. A card that is not in the stack, or a bar button while nothing is
    /// selected (no bar), changes nothing.
    pub fn set_focus(&mut self, f: Option<Focus>, now: Instant) {
        let valid = match f {
            None => true,
            Some(Focus::Card(id)) => self.index_of(id).is_some(),
            Some(Focus::Bar(_)) => self.any_selected(),
        };
        if valid {
            self.focus = f;
            self.refresh(now);
        }
    }

    /// A key press while the stack has the keyboard.
    ///
    /// - Left/Right move between the cards (newest is leftmost) and **clamp** at the ends;
    ///   on a bar button they move between the bar's buttons, also clamped. With no focus
    ///   both go to the first (newest) card.
    /// - Tab/Shift+Tab walk the cards, then the bar's buttons (while there is a bar), and
    ///   **wrap** around. With no focus they go to the first / last of them.
    /// - Space/Enter on a card give `ToggleSelect`; on a bar button, `Bar`.
    /// - C/S/P/Delete give `Card(focused, Copy/Save/Pin/Dismiss)`; with no card focused,
    ///   nothing. Ctrl+A gives `Bar(SelectAll)` (while there are cards).
    /// - Escape clears the focus and gives `Release`.
    ///
    /// The stack changes only its focus here; the caller applies the returned command.
    pub fn key(&mut self, key: StackKey, now: Instant) -> KeyCmd {
        let card_cmd = |focus: Option<Focus>, action| match focus {
            Some(Focus::Card(id)) => KeyCmd::Card(id, action),
            _ => KeyCmd::None,
        };
        match key {
            StackKey::Escape => {
                self.set_focus(None, now);
                KeyCmd::Release
            }
            StackKey::CtrlA if self.items.is_empty() => KeyCmd::None,
            StackKey::CtrlA => KeyCmd::Bar(BarAction::SelectAll),
            StackKey::Left => self.step_focus(-1, false, now),
            StackKey::Right => self.step_focus(1, false, now),
            StackKey::ShiftTab => self.step_focus(-1, true, now),
            StackKey::Tab => self.step_focus(1, true, now),
            StackKey::Space | StackKey::Enter => match self.focus {
                Some(Focus::Card(id)) => KeyCmd::ToggleSelect(id),
                Some(Focus::Bar(a)) => KeyCmd::Bar(a),
                None => KeyCmd::None,
            },
            StackKey::C => card_cmd(self.focus, CardAction::Copy),
            StackKey::S => card_cmd(self.focus, CardAction::Save),
            StackKey::P => card_cmd(self.focus, CardAction::Pin),
            StackKey::Delete => card_cmd(self.focus, CardAction::Dismiss),
        }
    }

    /// Everything the focus can land on, in Tab order: the cards, then the bar's buttons.
    fn focus_order(&self) -> Vec<Focus> {
        let cards = self.items.iter().map(|it| Focus::Card(it.id));
        let bar = BarAction::ALL
            .into_iter()
            .filter(|_| self.any_selected())
            .map(Focus::Bar);
        cards.chain(bar).collect()
    }

    /// Moves the focus by one: along the whole Tab order and wrapping with `tab`, else
    /// within the cards (or the bar's buttons) and clamping.
    fn step_focus(&mut self, dir: i32, tab: bool, now: Instant) -> KeyCmd {
        let order = self.focus_order();
        if order.is_empty() {
            return KeyCmd::None;
        }
        let at = self.focus.and_then(|f| order.iter().position(|o| *o == f));
        let n = order.len() as i32;
        let next = match (at, tab) {
            (None, false) => 0,
            (None, true) if dir > 0 => 0,
            (None, true) => n - 1,
            (Some(i), true) => (i as i32 + dir).rem_euclid(n),
            (Some(i), false) => {
                // Arrows stay in their own row: the cards or the bar.
                let cards = self.items.len() as i32;
                let (lo, hi) = if (i as i32) < cards {
                    (0, cards - 1)
                } else {
                    (cards, n - 1)
                };
                (i as i32 + dir).clamp(lo, hi)
            }
        };
        let target = order[next as usize];
        if Some(target) == self.focus {
            return KeyCmd::None;
        }
        self.set_focus(Some(target), now);
        KeyCmd::Redraw
    }

    fn any_selected(&self) -> bool {
        self.items.iter().any(|it| it.selected)
    }

    fn index_of(&self, id: ItemId) -> Option<usize> {
        self.items.iter().position(|it| it.id == id)
    }

    fn item_mut(&mut self, id: ItemId) -> Option<&mut Item> {
        self.items.iter_mut().find(|it| it.id == id)
    }

    /// Removes the cards `gone` picks and returns their ids (newest first). A removed
    /// focused card hands the focus to its older neighbour, else the newer one.
    fn drop_where(&mut self, now: Instant, gone: impl Fn(&Item) -> bool) -> Vec<ItemId> {
        let focus_at = match self.focus {
            Some(Focus::Card(id)) => self.index_of(id),
            _ => None,
        };
        let mut removed = Vec::new();
        let mut kept_before_focus = 0;
        for (i, it) in std::mem::take(&mut self.items).into_iter().enumerate() {
            if gone(&it) {
                removed.push(it.id);
            } else {
                kept_before_focus += usize::from(focus_at.is_some_and(|f| i < f));
                self.items.push(it);
            }
        }
        if let Some(Focus::Card(id)) = self.focus {
            if removed.contains(&id) {
                self.focus = self
                    .items
                    .get(kept_before_focus)
                    .or(self.items.last())
                    .map(|it| Focus::Card(it.id));
            }
        }
        self.refresh(now);
        removed
    }

    /// Re-establishes the invariants after any mutation: the focus points at something that
    /// exists, every card's `focused` flag matches it, and every countdown runs exactly
    /// when `runs` says.
    fn refresh(&mut self, now: Instant) {
        let any_selected = self.any_selected();
        let lost = match self.focus {
            Some(Focus::Card(id)) => self.index_of(id).is_none(),
            Some(Focus::Bar(_)) => !any_selected,
            None => false,
        };
        if lost {
            self.focus = self.items.first().map(|it| Focus::Card(it.id));
        }
        let (hidden, focus) = (self.hidden, self.focus);
        for it in &mut self.items {
            it.focused = focus == Some(Focus::Card(it.id));
            if runs(hidden, any_selected, it) {
                it.countdown.resume(now);
            } else {
                it.countdown.pause(now);
            }
        }
    }
}

/// A card's rect and its parts, in window client space (absolute, logical px).
#[derive(Clone, Debug, PartialEq)]
pub struct CardLayout {
    pub id: ItemId,
    pub rect: Rect,
    /// The selected mark: 18x18, 6 px in from the top-left.
    pub check: Rect,
    /// The countdown ring (or PIN badge): 16x16, 5 px in from the top-right.
    pub ring: Rect,
    /// The status chip, between the check and the ring.
    pub status: Rect,
    /// Copy, Save/Open, Pin/Unpin, Dismiss: the bottom row, 5 px in, 16 px high.
    pub actions: [(CardAction, Rect); 4],
}

/// The selection bar: its rect, the "N selected" text area and its four buttons.
#[derive(Clone, Debug, PartialEq)]
pub struct BarLayout {
    pub rect: Rect,
    pub count: Rect,
    pub buttons: Vec<(BarAction, Rect)>,
}

/// Everything drawn and hit-tested in the stack window.
#[derive(Clone, Debug, PartialEq)]
pub struct StackLayout {
    /// Newest first (leftmost first), like `Stack::items`.
    pub cards: Vec<CardLayout>,
    /// Only while at least one card is selected.
    pub bar: Option<BarLayout>,
    /// The stack-full notice: where the bar would be, under the bar if both show.
    pub notice: Option<Rect>,
}

/// What a point on the stack lands on. A card's buttons win over its body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Card(ItemId),
    Action(ItemId, CardAction),
    Bar(BarAction),
}

fn right(r: &Rect) -> f32 {
    r.x + r.w
}

fn bottom(r: &Rect) -> f32 {
    r.y + r.h
}

/// The selection bar's width.
fn bar_width() -> f32 {
    2.0 * BAR_PAD + BAR_COUNT_W + BAR_BTN_W.iter().sum::<f32>() + BAR_BTN_W.len() as f32 * BAR_GAP
}

/// The window's size in logical px, constant: `MAX_ITEMS` cards in a row, the bar row and
/// the notice row under them, and `PAD` all round.
pub fn window_size() -> (f32, f32) {
    let row = MAX_ITEMS as f32 * CARD_W + (MAX_ITEMS - 1) as f32 * GAP;
    (
        row.max(bar_width()).max(NOTICE_W) + 2.0 * PAD,
        PAD + CARD_H + GAP + BAR_H + GAP + NOTICE_H + PAD,
    )
}

fn card_layout(id: ItemId, rect: Rect) -> CardLayout {
    let check = Rect::new(rect.x + CHECK_INSET, rect.y + CHECK_INSET, CHECK, CHECK);
    let ring = Rect::new(
        right(&rect) - RING_INSET - RING,
        rect.y + RING_INSET,
        RING,
        RING,
    );
    let status = Rect::new(
        right(&check) + CHIP_GAP,
        check.y,
        ring.x - CHIP_GAP - (right(&check) + CHIP_GAP),
        CHECK,
    );
    let mut x = rect.x + ACTION_INSET;
    let y = bottom(&rect) - ACTION_INSET - ACTION_H;
    let actions = std::array::from_fn(|i| {
        let r = Rect::new(x, y, ACTION_W[i], ACTION_H);
        x += ACTION_W[i] + ACTION_GAP;
        (CardAction::ALL[i], r)
    });
    CardLayout {
        id,
        rect,
        check,
        ring,
        status,
        actions,
    }
}

fn bar_layout(window_w: f32, y: f32) -> BarLayout {
    let w = bar_width();
    let rect = Rect::new((window_w - w) / 2.0, y, w, BAR_H);
    let count = Rect::new(rect.x + BAR_PAD, rect.y, BAR_COUNT_W, BAR_H);
    let mut x = right(&count) + BAR_GAP;
    let by = rect.y + (BAR_H - BAR_BTN_H) / 2.0;
    let buttons = BarAction::ALL
        .into_iter()
        .zip(BAR_BTN_W)
        .map(|(a, bw)| {
            let r = Rect::new(x, by, bw, BAR_BTN_H);
            x += bw + BAR_GAP;
            (a, r)
        })
        .collect();
    BarLayout {
        rect,
        count,
        buttons,
    }
}

/// Lays the stack out: the cards in one row (newest leftmost, centred, top at `PAD`), the
/// bar under them while something is selected, the notice in the next free row.
pub fn layout(stack: &Stack, now: Instant) -> StackLayout {
    let (w, _) = window_size();
    let n = stack.items().len();
    let row = if n == 0 {
        0.0
    } else {
        n as f32 * CARD_W + (n - 1) as f32 * GAP
    };
    let x0 = (w - row) / 2.0;
    let cards: Vec<CardLayout> = stack
        .items()
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let x = x0 + i as f32 * (CARD_W + GAP);
            card_layout(it.id, Rect::new(x, PAD, CARD_W, CARD_H))
        })
        .collect();
    // The row under the cards (the top, when there are none).
    let below = if cards.is_empty() {
        PAD
    } else {
        PAD + CARD_H + GAP
    };
    let bar = (n > 0 && stack.items().iter().any(|it| it.selected)).then(|| bar_layout(w, below));
    let notice_y = match &bar {
        Some(b) => bottom(&b.rect) + GAP,
        None => below,
    };
    let notice = stack
        .notice(now)
        .map(|_| Rect::new((w - NOTICE_W) / 2.0, notice_y, NOTICE_W, NOTICE_H));
    StackLayout { cards, bar, notice }
}

/// What the point `(x, y)` (window client space) lands on. Bar buttons and a card's
/// buttons win over the card body; gaps, the bar's background and the notice hit nothing.
pub fn hit(layout: &StackLayout, x: f32, y: f32) -> Option<Hit> {
    if let Some(bar) = &layout.bar {
        if let Some((a, _)) = bar.buttons.iter().find(|(_, r)| r.contains(x, y)) {
            return Some(Hit::Bar(*a));
        }
    }
    let card = layout.cards.iter().find(|c| c.rect.contains(x, y))?;
    match card.actions.iter().find(|(_, r)| r.contains(x, y)) {
        Some((a, _)) => Some(Hit::Action(card.id, *a)),
        None => Some(Hit::Card(card.id)),
    }
}

/// The rects that take input: each card and the bar. The gaps, the shadow margin and the
/// notice stay click-through.
pub fn input_rects(layout: &StackLayout) -> Vec<Rect> {
    layout
        .cards
        .iter()
        .map(|c| c.rect)
        .chain(layout.bar.as_ref().map(|b| b.rect))
        .collect()
}

/// Where the stack window goes, in physical desktop px: centred under the island and
/// `BELOW_ISLAND` logical px below its lower edge (the window's content starts at PAD inside),
/// clamped into `work` = (left, top, right, bottom) of the island's monitor. Returns (x, y, w, h).
///
/// The size is `window_size()` x `scale`, rounded up. A window wider (taller) than the work
/// area keeps its left (top) edge on the work area's. Desktop coordinates may be negative.
pub fn place(
    island_center_x: i32,
    island_bottom: i32,
    scale: f32,
    work: (i32, i32, i32, i32),
) -> (i32, i32, i32, i32) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let (ww, wh) = window_size();
    let (w, h) = ((ww * scale).ceil() as i32, (wh * scale).ceil() as i32);
    let gap = (BELOW_ISLAND * scale).round() as i32;
    let pad = (PAD * scale).round() as i32;
    let (left, top, right, bottom) = work;
    let x = island_center_x - w / 2;
    let y = island_bottom + gap - pad;
    let clamp = |v: i32, lo: i32, hi: i32, size: i32| {
        if size >= hi - lo {
            lo
        } else {
            v.clamp(lo, hi - size)
        }
    };
    (clamp(x, left, right, w), clamp(y, top, bottom, h), w, h)
}

/// A card as the renderer draws it this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionCard {
    pub id: ItemId,
    /// Animated, logical px, window client space.
    pub rect: Rect,
    /// 0..=1.
    pub alpha: f32,
    /// About the card's centre (pops past 1 before it settles).
    pub scale: f32,
    /// Its card is gone and it is fading out.
    pub exiting: bool,
}

/// A new card starts this far above its slot, at `POP_SCALE` and transparent.
const POP_LIFT: f32 = 30.0;
const POP_SCALE: f32 = 0.6;
/// A removed card lifts this much and shrinks to `EXIT_SCALE` while it fades.
const EXIT_LIFT: f32 = 12.0;
const EXIT_SCALE: f32 = 0.9;
/// Rest thresholds: px for positions, 0..1 units for alpha and scale.
const POS_EPS: f32 = 0.05;
const UNIT_EPS: f32 = 0.002;
/// Until `step` sets the caller's spec.
const SYNC_SPEC: SpringSpec = SpringSpec::new(0.35, 0.95);

#[derive(Clone, Debug)]
struct Anim {
    id: ItemId,
    w: f32,
    h: f32,
    x: Spring,
    y: Spring,
    alpha: Spring,
    scale: Spring,
    exiting: bool,
}

impl Anim {
    fn springs(&mut self) -> [&mut Spring; 4] {
        [&mut self.x, &mut self.y, &mut self.alpha, &mut self.scale]
    }

    fn settled(&self) -> bool {
        [&self.x, &self.y, &self.alpha, &self.scale]
            .iter()
            .all(|s| s.settled())
    }
}

fn aim(s: &mut Spring, target: f32) {
    s.retarget(target, s.spec);
}

/// The cards' springs: new ones pop in, the others slide to their slot, removed ones fade
/// out as ghosts until they finish. Stepped by the app's frame loop.
#[derive(Clone, Debug, Default)]
pub struct Motion {
    /// Newest first, ghosts included.
    cards: Vec<Anim>,
    finished: Vec<ItemId>,
}

impl Motion {
    pub fn new() -> Self {
        Self::default()
    }

    /// Follows a new layout: ids not seen before pop in (from 30 px above, scale .6, alpha 0),
    /// moved ones slide, ids that left the layout become ghosts that fade and lift out.
    pub fn sync(&mut self, layout: &StackLayout) {
        for a in &mut self.cards {
            if !a.exiting && !layout.cards.iter().any(|c| c.id == a.id) {
                a.exiting = true;
                let lifted = a.y.value - EXIT_LIFT;
                aim(&mut a.y, lifted);
                aim(&mut a.alpha, 0.0);
                aim(&mut a.scale, EXIT_SCALE);
            }
        }
        for c in &layout.cards {
            let r = c.rect;
            if let Some(a) = self.cards.iter_mut().find(|a| a.id == c.id) {
                (a.w, a.h) = (r.w, r.h);
                if a.exiting {
                    a.exiting = false;
                    aim(&mut a.alpha, 1.0);
                    aim(&mut a.scale, 1.0);
                }
                aim(&mut a.x, r.x);
                aim(&mut a.y, r.y);
            } else {
                let mut a = Anim {
                    id: c.id,
                    w: r.w,
                    h: r.h,
                    x: Spring::new(r.x, SYNC_SPEC, POS_EPS),
                    y: Spring::new(r.y - POP_LIFT, SYNC_SPEC, POS_EPS),
                    alpha: Spring::new(0.0, SYNC_SPEC, UNIT_EPS),
                    scale: Spring::new(POP_SCALE, SYNC_SPEC, UNIT_EPS),
                    exiting: false,
                };
                aim(&mut a.y, r.y);
                aim(&mut a.alpha, 1.0);
                aim(&mut a.scale, 1.0);
                // Ids grow, so newest first is descending id.
                let at = self
                    .cards
                    .iter()
                    .position(|o| o.id < a.id)
                    .unwrap_or(self.cards.len());
                self.cards.insert(at, a);
            }
        }
    }

    /// Advances every spring by `dt` seconds with `spec`; ghosts that came to rest are
    /// dropped and reported by `take_finished`.
    pub fn step(&mut self, dt: f32, spec: SpringSpec) {
        let Self { cards, finished } = self;
        for a in cards.iter_mut() {
            for s in a.springs() {
                s.spec = spec;
                s.step(dt);
            }
        }
        cards.retain(|a| {
            let done = a.exiting && a.settled();
            if done {
                finished.push(a.id);
            }
            !done
        });
    }

    /// Something moves or fades, or a ghost waits for its last step.
    pub fn animating(&self) -> bool {
        self.cards.iter().any(|a| a.exiting || !a.settled())
    }

    /// What to draw, back to front: oldest first, ghosts included.
    pub fn cards(&self) -> Vec<MotionCard> {
        self.cards
            .iter()
            .rev()
            .map(|a| MotionCard {
                id: a.id,
                rect: Rect::new(a.x.value, a.y.value, a.w, a.h),
                alpha: a.alpha.value.clamp(0.0, 1.0),
                scale: a.scale.value.max(0.0),
                exiting: a.exiting,
            })
            .collect()
    }

    /// The ghosts that finished fading since the last call (their images can be freed).
    pub fn take_finished(&mut self) -> Vec<ItemId> {
        std::mem::take(&mut self.finished)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIFE: Duration = Duration::from_secs(10);

    /// What `tick` and `remove_selected` return when nothing happened.
    const NONE: [ItemId; 0] = [];

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn stack() -> Stack {
        Stack::new(LIFE)
    }

    /// Admits a 1-byte card and returns its id.
    fn add(st: &mut Stack, now: Instant) -> ItemId {
        add_bytes(st, 1, now)
    }

    fn add_bytes(st: &mut Stack, bytes: u64, now: Instant) -> ItemId {
        match st.admit(200, 120, bytes, now) {
            Admit::Added { id, .. } => id,
            Admit::Full => panic!("stack unexpectedly full"),
        }
    }

    fn ids(st: &Stack) -> Vec<u64> {
        st.items().iter().map(|it| it.id.0).collect()
    }

    /// Every countdown is paused exactly when the predicate says so, every focused flag
    /// matches the focus.
    fn assert_in_step(st: &Stack) {
        for it in st.items() {
            assert_eq!(
                !it.countdown.is_paused(),
                st.is_running(it.id),
                "countdown state of {:?}",
                it.id
            );
            assert_eq!(
                it.focused,
                st.focus() == Some(Focus::Card(it.id)),
                "focused flag of {:?}",
                it.id
            );
        }
        match st.focus() {
            Some(Focus::Card(id)) => assert!(st.get(id).is_some()),
            Some(Focus::Bar(_)) => assert!(!st.selected().is_empty()),
            None => {}
        }
    }

    // ----- ids, status, expiry ------------------------------------------------------

    #[test]
    fn ids_start_at_one_grow_and_are_never_reused() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        let b = add(&mut st, t0);
        let c = add(&mut st, t0);
        assert_eq!((a, b, c), (ItemId(1), ItemId(2), ItemId(3)));
        assert_eq!(ids(&st), [3, 2, 1], "newest first");
        assert!(st.remove(c, t0));
        assert!(st.remove(a, t0));
        assert_eq!(
            add(&mut st, t0),
            ItemId(4),
            "the newest id is not handed out again"
        );
        // Nor are the ids of evicted or expired cards.
        let mut small = Stack::with_limits(LIFE, 2, MAX_BYTES);
        let x = add(&mut small, t0);
        let y = add(&mut small, t0);
        let Admit::Added { id: z, evicted } = small.admit(1, 1, 1, t0) else {
            panic!("full")
        };
        assert_eq!(
            (x, y, z, evicted),
            (ItemId(1), ItemId(2), ItemId(3), vec![x])
        );
        assert_eq!(small.tick(t0 + secs(10)), [z, y]);
        assert_eq!(add(&mut small, t0 + secs(10)), ItemId(4));
        // A refused admission takes no id either way, but never reuses one.
        assert_eq!(small.admit(1, 1, MAX_BYTES + 1, t0 + secs(10)), Admit::Full);
        assert_eq!(add(&mut small, t0 + secs(10)), ItemId(5));
    }

    #[test]
    fn a_new_card_is_ready_with_the_full_lifetime() {
        let t0 = Instant::now();
        let mut st = stack();
        let Admit::Added { id, evicted } = st.admit(612, 388, 4096, t0) else {
            panic!("full")
        };
        assert!(evicted.is_empty());
        let it = st.get(id).unwrap();
        assert_eq!((it.width, it.height, it.bytes), (612, 388, 4096));
        assert_eq!(it.status, CardStatus::Ready);
        assert!(it.saved.is_none());
        assert!(!(it.pinned || it.selected || it.hovered || it.focused || it.busy));
        assert_eq!(st.remaining(id, t0), Some(LIFE));
        assert_eq!(st.fraction(id, t0), Some(1.0));
        assert!(st.is_running(id) && st.any_running());
        assert_in_step(&st);
    }

    #[test]
    fn status_labels_and_errors() {
        use CardStatus::*;
        let table = [
            (Ready, None, false),
            (Copied, Some("copied"), false),
            (CopyFailed, Some("copy failed"), true),
            (Saving, Some("saving…"), false),
            (Saved, Some("saved"), false),
            (SaveFailed, Some("save failed"), true),
            (Missing, Some("file missing"), true),
        ];
        for (s, label, error) in table {
            assert_eq!(s.label(), label, "{s:?}");
            assert_eq!(s.is_error(), error, "{s:?}");
        }
    }

    #[test]
    fn a_card_expires_after_exactly_its_lifetime_of_active_time() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        assert!(st.tick(t0 + ms(9_999)).is_empty());
        assert_eq!(st.remaining(a, t0 + ms(9_999)), Some(ms(1)));
        assert_eq!(st.tick(t0 + secs(10)), [a]);
        assert!(st.is_empty());
        assert_eq!(st.get(a), None);
        assert!(st.tick(t0 + secs(99)).is_empty());
    }

    #[test]
    fn tick_removes_every_due_card_newest_first_and_only_those() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        let b = add(&mut st, t0 + secs(4));
        let c = add(&mut st, t0 + secs(4));
        assert_eq!(st.tick(t0 + secs(10)), [a]);
        assert_eq!(ids(&st), [3, 2]);
        assert_eq!(st.tick(t0 + secs(14)), [c, b]);
        assert_in_step(&st);
    }

    #[test]
    fn the_fraction_is_remaining_over_lifetime() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        assert_eq!(st.fraction(a, t0 + secs(5)), Some(0.5));
        assert_eq!(st.fraction(a, t0 + secs(10)), Some(0.0));
        assert_eq!(st.fraction(a, t0 + secs(99)), Some(0.0), "never negative");
        assert_eq!(st.fraction(ItemId(9), t0), None);
        assert_eq!(st.remaining(ItemId(9), t0), None);
        // A clock that seems to go backwards adds no time.
        assert_eq!(st.fraction(a, t0 - ms(500)), Some(1.0));
        let mut zero = Stack::new(Duration::ZERO);
        let z = add(&mut zero, t0);
        assert_eq!(zero.fraction(z, t0), Some(0.0));
    }

    // ----- pause reasons ------------------------------------------------------------

    #[test]
    fn hover_pauses_the_card_and_resume_continues_the_remainder() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        st.set_hover(Some(a), t0 + secs(4));
        assert!(!st.is_running(a) && !st.any_running());
        assert_eq!(st.remaining(a, t0 + secs(4)), Some(secs(6)));
        assert_eq!(st.remaining(a, t0 + secs(400)), Some(secs(6)));
        assert!(st.tick(t0 + secs(400)).is_empty());
        st.set_hover(None, t0 + secs(30));
        assert!(st.is_running(a));
        assert_eq!(
            st.remaining(a, t0 + secs(30)),
            Some(secs(6)),
            "not restarted"
        );
        assert!(st.tick(t0 + ms(35_999)).is_empty());
        assert_eq!(st.tick(t0 + secs(36)), [a]);
    }

    #[test]
    fn hover_pauses_only_the_hovered_card_and_moves_with_the_pointer() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        let b = add(&mut st, t0);
        st.set_hover(Some(a), t0 + secs(2));
        assert!(!st.is_running(a) && st.is_running(b));
        st.set_hover(Some(b), t0 + secs(5));
        assert!(st.is_running(a) && !st.is_running(b));
        assert!(!st.get(a).unwrap().hovered && st.get(b).unwrap().hovered);
        assert_eq!(st.remaining(a, t0 + secs(5)), Some(secs(8)));
        assert_eq!(st.remaining(b, t0 + secs(5)), Some(secs(5)));
        // A stale id changes nothing, not even the hover that is there.
        st.set_hover(Some(ItemId(77)), t0 + secs(6));
        assert!(st.get(b).unwrap().hovered);
        assert_in_step(&st);
    }

    #[test]
    fn any_selection_pauses_the_whole_stack() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        let b = add(&mut st, t0 + secs(1));
        st.toggle_select(a, t0 + secs(2));
        assert!(!st.is_running(a) && !st.is_running(b));
        assert_eq!(st.remaining(a, t0 + secs(50)), Some(secs(8)));
        assert_eq!(st.remaining(b, t0 + secs(50)), Some(secs(9)));
        assert!(st.tick(t0 + secs(500)).is_empty());
        st.toggle_select(a, t0 + secs(50));
        assert!(st.is_running(a) && st.is_running(b));
        assert_eq!(st.tick(t0 + secs(58)), [a]);
        assert_eq!(st.tick(t0 + secs(59)), [b]);
    }

    #[test]
    fn a_card_admitted_while_something_is_selected_waits_with_the_full_lifetime() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        st.toggle_select(a, t0 + secs(1));
        let b = add(&mut st, t0 + secs(2));
        assert!(!st.is_running(b));
        assert_eq!(st.remaining(b, t0 + secs(30)), Some(LIFE));
        st.clear_selection(t0 + secs(30));
        assert_eq!(st.tick(t0 + secs(39)), [a]);
        assert_eq!(st.tick(t0 + secs(40)), [b]);
    }

    #[test]
    fn pin_pauses_only_that_card_and_it_survives_the_others_expiry() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        let b = add(&mut st, t0);
        st.toggle_pin(a, t0 + secs(1));
        assert!(!st.is_running(a) && st.is_running(b));
        assert_eq!(st.tick(t0 + secs(10)), [b]);
        assert_eq!(ids(&st), [1]);
        assert_eq!(st.tick(t0 + secs(1_000_000)), NONE);
        assert_eq!(st.remaining(a, t0 + secs(1_000_000)), Some(secs(9)));
        st.toggle_pin(a, t0 + secs(20));
        assert!(!st.get(a).unwrap().pinned);
        assert_eq!(st.tick(t0 + secs(28)), NONE);
        assert_eq!(st.tick(t0 + secs(29)), [a]);
    }

    #[test]
    fn pinned_cards_can_still_be_dismissed() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        st.toggle_pin(a, t0);
        assert!(st.remove(a, t0));
        assert!(st.is_empty());
        assert!(!st.remove(a, t0), "already gone");
    }

    #[test]
    fn focus_and_busy_pause_too() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        st.set_focus(Some(Focus::Card(a)), t0 + secs(1));
        assert!(!st.is_running(a) && st.get(a).unwrap().focused);
        st.set_focus(None, t0 + secs(20));
        assert!(st.is_running(a));
        assert_eq!(st.remaining(a, t0 + secs(20)), Some(secs(9)));
        assert!(st.set_busy(a, true, t0 + secs(22)));
        assert!(!st.is_running(a));
        assert!(st.set_busy(a, false, t0 + secs(60)));
        assert_eq!(st.remaining(a, t0 + secs(60)), Some(secs(7)));
        assert_eq!(st.tick(t0 + secs(67)), [a]);
    }

    #[derive(Clone, Copy, Debug)]
    enum Reason {
        Hover,
        Select,
        Pin,
        Focus,
        Busy,
        Hidden,
    }

    const REASONS: [Reason; 6] = [
        Reason::Hover,
        Reason::Select,
        Reason::Pin,
        Reason::Focus,
        Reason::Busy,
        Reason::Hidden,
    ];

    /// Switches one pause reason on `id` on or off.
    fn set(st: &mut Stack, id: ItemId, r: Reason, on: bool, now: Instant) {
        match r {
            Reason::Hover => st.set_hover(on.then_some(id), now),
            Reason::Select => {
                if st.get(id).unwrap().selected != on {
                    st.toggle_select(id, now);
                }
            }
            Reason::Pin => {
                if st.get(id).unwrap().pinned != on {
                    st.toggle_pin(id, now);
                }
            }
            Reason::Focus => st.set_focus(on.then_some(Focus::Card(id)), now),
            Reason::Busy => assert!(st.set_busy(id, on, now)),
            Reason::Hidden => st.set_hidden(on, now),
        }
    }

    #[test]
    fn every_combination_of_pause_reasons_resumes_from_the_right_remainder() {
        let t0 = Instant::now();
        for mask in 1u32..64 {
            let on: Vec<Reason> = REASONS
                .into_iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, r)| r)
                .collect();
            for reverse_release in [false, true] {
                let mut st = stack();
                let a = add(&mut st, t0);
                let other = add(&mut st, t0);
                // 2 s of active time pass, then the reasons pile up one per second.
                let mut now = t0 + secs(2);
                for (k, r) in on.iter().enumerate() {
                    set(&mut st, a, *r, true, now);
                    assert_in_step(&st);
                    assert!(!st.is_running(a), "{on:?} after {k} reasons");
                    now += secs(1);
                }
                let select = on.iter().any(|r| matches!(r, Reason::Select));
                let hidden = on.iter().any(|r| matches!(r, Reason::Hidden));
                assert_eq!(
                    st.is_running(other),
                    !(select || hidden),
                    "the other card follows only select and hidden: {on:?}"
                );
                // Lots of paused time, then the reasons go in either order.
                now += secs(500);
                let mut left = on.clone();
                while !left.is_empty() {
                    let r = if reverse_release {
                        left.pop().unwrap()
                    } else {
                        left.remove(0)
                    };
                    assert!(!st.is_running(a), "still held by {left:?} + {r:?}");
                    set(&mut st, a, r, false, now);
                    assert_in_step(&st);
                    assert_eq!(st.is_running(a), left.is_empty(), "{on:?} left {left:?}");
                    now += secs(1);
                }
                let released = now - secs(1);
                assert_eq!(
                    st.remaining(a, released),
                    Some(secs(8)),
                    "{on:?}: the 8 s left when it paused, nothing more or less"
                );
                assert_eq!(st.remaining(a, released + secs(3)), Some(secs(5)));
                assert!(st.tick(released + ms(7_999)).iter().all(|i| *i != a));
                assert!(st.tick(released + secs(8)).contains(&a), "{on:?}");
            }
        }
    }

    #[test]
    fn hidden_time_never_counts() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        st.set_hidden(true, t0 + secs(3));
        assert!(!st.any_running());
        assert_eq!(st.tick(t0 + secs(1000)), NONE);
        st.set_hidden(false, t0 + secs(1000));
        assert_eq!(st.remaining(a, t0 + secs(1000)), Some(secs(7)));
        assert_eq!(st.tick(t0 + ms(1_006_999)), NONE);
        assert_eq!(st.tick(t0 + secs(1007)), [a]);
    }

    #[test]
    fn a_card_admitted_while_hidden_starts_paused_with_the_full_lifetime() {
        let t0 = Instant::now();
        let mut st = stack();
        st.set_hidden(true, t0);
        let a = add(&mut st, t0 + secs(1));
        assert!(!st.is_running(a));
        assert_eq!(st.remaining(a, t0 + secs(50)), Some(LIFE));
        st.set_hidden(false, t0 + secs(50));
        assert_eq!(st.tick(t0 + secs(60)), [a]);
    }

    #[test]
    fn a_hidden_stack_keeps_the_other_flags() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        st.set_hover(Some(a), t0);
        st.set_hidden(true, t0 + secs(1));
        st.set_hidden(false, t0 + secs(2));
        assert!(st.get(a).unwrap().hovered && !st.is_running(a));
    }

    #[test]
    fn a_due_card_that_is_paused_before_the_tick_waits_instead_of_expiring() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        // It ran out at 10 s, but the app had not ticked yet when the pointer arrived.
        st.set_hover(Some(a), t0 + secs(12));
        assert_eq!(st.tick(t0 + secs(50)), NONE);
        assert_eq!(st.remaining(a, t0 + secs(50)), Some(Duration::ZERO));
        assert_eq!(st.next_change(t0 + secs(50)), None);
        st.set_hover(None, t0 + secs(60));
        assert_eq!(st.next_change(t0 + secs(60)), Some(Duration::ZERO));
        assert_eq!(st.tick(t0 + secs(60)), [a]);
    }

    // ----- selection ----------------------------------------------------------------

    #[test]
    fn select_all_clear_and_toggle() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        let b = add(&mut st, t0);
        let c = add(&mut st, t0);
        assert!(st.selected().is_empty());
        st.toggle_select(b, t0);
        assert_eq!(st.selected(), [b]);
        st.toggle_select(a, t0);
        assert_eq!(st.selected(), [b, a], "newest first");
        st.toggle_select(b, t0);
        assert_eq!(st.selected(), [a]);
        st.select_all(t0);
        assert_eq!(st.selected(), [c, b, a]);
        st.clear_selection(t0);
        assert!(st.selected().is_empty());
        assert!(st.any_running());
        // Nothing to select in an empty stack.
        let mut empty = stack();
        empty.select_all(t0);
        assert!(empty.selected().is_empty());
        assert_in_step(&st);
    }

    #[test]
    fn remove_selected_dismisses_them_all_pinned_ones_too() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        let b = add(&mut st, t0);
        let c = add(&mut st, t0);
        let d = add(&mut st, t0);
        st.toggle_pin(b, t0);
        st.toggle_select(d, t0);
        st.toggle_select(b, t0);
        assert_eq!(st.remove_selected(t0 + secs(1)), [d, b]);
        assert_eq!(ids(&st), [c.0, a.0]);
        assert!(st.selected().is_empty());
        assert!(
            st.any_running(),
            "nothing selected any more: the rest resume"
        );
        assert_eq!(st.remove_selected(t0 + secs(1)), NONE, "nothing selected");
        assert_eq!(st.len(), 2);
        assert_in_step(&st);
    }

    #[test]
    fn selection_resumes_the_stack_from_the_remainders() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        let b = add(&mut st, t0 + secs(3));
        st.select_all(t0 + secs(5));
        st.clear_selection(t0 + secs(100));
        assert_eq!(st.remaining(a, t0 + secs(100)), Some(secs(5)));
        assert_eq!(st.remaining(b, t0 + secs(100)), Some(secs(8)));
    }

    // ----- capacity -----------------------------------------------------------------

    #[test]
    fn the_sixth_card_evicts_the_oldest_unprotected_one() {
        let t0 = Instant::now();
        let mut st = stack();
        let first: Vec<ItemId> = (0..5).map(|_| add(&mut st, t0)).collect();
        assert_eq!(st.len(), MAX_ITEMS);
        let Admit::Added { id, evicted } = st.admit(1, 1, 1, t0) else {
            panic!("full")
        };
        assert_eq!((id, evicted), (ItemId(6), vec![first[0]]));
        assert_eq!(ids(&st), [6, 5, 4, 3, 2]);
        // The oldest pinned / hovered: the next one goes.
        st.toggle_pin(ItemId(2), t0);
        st.set_hover(Some(ItemId(3)), t0);
        let Admit::Added { evicted, .. } = st.admit(1, 1, 1, t0) else {
            panic!("full")
        };
        assert_eq!(evicted, [ItemId(4)]);
        assert_eq!(ids(&st), [7, 6, 5, 3, 2]);
        assert_in_step(&st);
    }

    #[test]
    fn the_byte_budget_evicts_as_many_old_cards_as_it_takes() {
        let t0 = Instant::now();
        let mut st = Stack::with_limits(LIFE, 5, 1000);
        let a = add_bytes(&mut st, 300, t0);
        let b = add_bytes(&mut st, 300, t0);
        let c = add_bytes(&mut st, 300, t0);
        assert_eq!(st.bytes(), 900);
        let Admit::Added { id, evicted } = st.admit(1, 1, 700, t0) else {
            panic!("full")
        };
        assert_eq!(evicted, [a, b], "oldest first, no more than needed");
        assert_eq!(st.bytes(), 1000);
        assert_eq!(ids(&st), [id.0, c.0]);
        // Exactly the budget is fine; one byte over is not.
        let mut st = Stack::with_limits(LIFE, 5, 1000);
        add_bytes(&mut st, 1000, t0);
        let Admit::Added { evicted, .. } = st.admit(1, 1, 1000, t0) else {
            panic!("full")
        };
        assert_eq!(evicted.len(), 1);
        assert_eq!(st.admit(1, 1, 1001, t0), Admit::Full);
        assert_eq!(st.bytes(), 1000);
    }

    #[test]
    fn the_byte_budget_skips_protected_cards() {
        let t0 = Instant::now();
        let mut st = Stack::with_limits(LIFE, 5, 1000);
        let a = add_bytes(&mut st, 300, t0);
        let b = add_bytes(&mut st, 300, t0);
        let c = add_bytes(&mut st, 300, t0);
        st.toggle_pin(a, t0);
        let Admit::Added { evicted, .. } = st.admit(1, 1, 700, t0) else {
            panic!("full")
        };
        assert_eq!(evicted, [b, c], "the pinned oldest stays");
        assert!(st.get(a).is_some());
    }

    #[test]
    fn a_full_stack_of_protected_cards_refuses_and_changes_nothing() {
        let t0 = Instant::now();
        let mut st = stack();
        let v: Vec<ItemId> = (0..5).map(|_| add(&mut st, t0)).collect();
        // One protection reason each.
        st.toggle_pin(v[0], t0);
        st.set_hover(Some(v[1]), t0);
        st.set_focus(Some(Focus::Card(v[2])), t0);
        assert!(st.set_busy(v[3], true, t0));
        st.toggle_select(v[4], t0);
        let before = (ids(&st), st.bytes(), st.selected(), st.focus());
        assert_eq!(st.admit(1, 1, 1, t0 + secs(1)), Admit::Full);
        assert_eq!((ids(&st), st.bytes(), st.selected(), st.focus()), before);
        assert_eq!(st.notice(t0 + secs(1)), Some("Preview stack full"));
        for id in &v {
            assert!(st.get(*id).is_some_and(|it| it.is_protected()));
        }
        // The refusal took no id: the next card gets the next one in line.
        st.clear_selection(t0 + secs(2));
        st.set_busy(v[3], false, t0 + secs(2));
        let Admit::Added { id, evicted } = st.admit(1, 1, 1, t0 + secs(2)) else {
            panic!("full")
        };
        assert_eq!((id, evicted), (ItemId(6), vec![v[3]]));
    }

    #[test]
    fn a_refusal_is_all_or_nothing() {
        let t0 = Instant::now();
        // Needs two evictions but only one card is unprotected: nothing is evicted.
        let mut st = Stack::with_limits(LIFE, 5, 1000);
        let a = add_bytes(&mut st, 400, t0);
        let b = add_bytes(&mut st, 400, t0);
        st.toggle_pin(b, t0);
        assert_eq!(st.admit(1, 1, 900, t0), Admit::Full);
        assert_eq!(ids(&st), [b.0, a.0]);
        assert_eq!(st.bytes(), 800);
        // The same for the item limit.
        let mut one = Stack::with_limits(LIFE, 1, MAX_BYTES);
        let p = add(&mut one, t0);
        one.toggle_pin(p, t0);
        assert_eq!(one.admit(1, 1, 1, t0), Admit::Full);
        assert_eq!(ids(&one), [1]);
        // No room at all.
        let mut none = Stack::with_limits(LIFE, 0, MAX_BYTES);
        assert_eq!(none.admit(1, 1, 1, t0), Admit::Full);
    }

    #[test]
    fn an_oversized_shot_is_refused_even_by_an_empty_stack() {
        let t0 = Instant::now();
        let mut st = Stack::with_limits(LIFE, 5, 1000);
        assert_eq!(st.admit(1, 1, 1001, t0), Admit::Full);
        assert!(st.is_empty());
        assert!(st.notice(t0).is_some());
        // Nor are small cards evicted to "make room" for it.
        let a = add_bytes(&mut st, 10, t0);
        assert_eq!(st.admit(1, 1, 1001, t0), Admit::Full);
        assert_eq!(ids(&st), [a.0]);
        assert_eq!(st.admit(1, 1, u64::MAX, t0), Admit::Full);
        // The default budget is 256 MiB.
        let mut big = stack();
        assert_eq!(big.admit(1, 1, MAX_BYTES + 1, t0), Admit::Full);
        assert!(matches!(
            big.admit(1, 1, MAX_BYTES, t0),
            Admit::Added { .. }
        ));
    }

    #[test]
    fn evicting_never_touches_the_flags_or_focus_of_the_survivors() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        let b = add(&mut st, t0);
        for _ in 0..3 {
            add(&mut st, t0);
        }
        st.set_focus(Some(Focus::Card(b)), t0);
        let Admit::Added { evicted, .. } = st.admit(1, 1, 1, t0) else {
            panic!("full")
        };
        assert_eq!(evicted, [a]);
        assert_eq!(st.focus(), Some(Focus::Card(b)));
        assert_in_step(&st);
    }

    // ----- notice, next_change ------------------------------------------------------

    #[test]
    fn the_full_notice_shows_for_three_seconds() {
        let t0 = Instant::now();
        let mut st = Stack::with_limits(LIFE, 5, 10);
        assert_eq!(st.notice(t0), None);
        assert_eq!(st.admit(1, 1, 11, t0), Admit::Full);
        assert_eq!(st.notice(t0), Some("Preview stack full"));
        assert_eq!(st.notice(t0 + ms(2_999)), Some("Preview stack full"));
        assert_eq!(st.notice(t0 + secs(3)), None);
        // A second refusal restarts it.
        assert_eq!(st.admit(1, 1, 11, t0 + secs(2)), Admit::Full);
        assert_eq!(st.notice(t0 + ms(4_999)), Some(NOTICE_TEXT));
        assert_eq!(st.notice(t0 + secs(5)), None);
        // `tick` forgets a finished one; a running one stays.
        st.tick(t0 + secs(4));
        assert!(st.notice(t0 + secs(4)).is_some());
        st.tick(t0 + secs(5));
        assert_eq!(st.notice_until, None);
        // It counts wall time, hidden or not.
        st.set_hidden(true, t0 + secs(6));
        assert_eq!(st.admit(1, 1, 11, t0 + secs(6)), Admit::Full);
        assert!(st.notice(t0 + secs(8)).is_some());
        assert_eq!(st.notice(t0 + secs(9)), None);
    }

    #[test]
    fn next_change_is_the_soonest_expiry_or_the_end_of_the_notice() {
        let t0 = Instant::now();
        let mut st = Stack::with_limits(LIFE, 5, 1000);
        assert_eq!(st.next_change(t0), None, "empty");
        let a = add(&mut st, t0);
        assert_eq!(st.next_change(t0), Some(secs(10)));
        let b = add(&mut st, t0 + secs(4));
        assert_eq!(st.next_change(t0 + secs(5)), Some(secs(5)), "a, not b");
        // Paused cards don't count.
        st.set_hover(Some(a), t0 + secs(6));
        assert_eq!(st.next_change(t0 + secs(6)), Some(secs(8)), "b: 4 + 10 - 6");
        st.toggle_pin(b, t0 + secs(6));
        assert_eq!(st.next_change(t0 + secs(6)), None, "all paused");
        assert!(!st.any_running());
        // The notice is a change of its own, even with everything paused.
        assert_eq!(st.admit(1, 1, 1001, t0 + secs(7)), Admit::Full);
        assert_eq!(st.next_change(t0 + secs(7)), Some(secs(3)));
        assert_eq!(st.next_change(t0 + secs(9)), Some(secs(1)));
        assert_eq!(st.next_change(t0 + secs(10)), None);
        // The earlier of the two wins: b has 8 s left from 10 s on (a is still hovered).
        st.toggle_pin(b, t0 + secs(10));
        assert_eq!(st.next_change(t0 + secs(10)), Some(secs(8)));
        assert_eq!(st.admit(1, 1, 1001, t0 + secs(11)), Admit::Full);
        assert_eq!(
            st.next_change(t0 + secs(11)),
            Some(secs(3)),
            "the notice, not b's 7 s"
        );
        assert_eq!(st.next_change(t0 + secs(14)), Some(secs(4)), "then b");
        // Hidden: nothing runs, only the notice is left.
        st.set_hidden(true, t0 + secs(12));
        assert_eq!(st.next_change(t0 + secs(12)), Some(secs(2)));
        assert_eq!(st.next_change(t0 + secs(14)), None);
    }

    #[test]
    fn next_change_is_zero_when_a_card_is_due() {
        let t0 = Instant::now();
        let mut st = stack();
        add(&mut st, t0);
        assert_eq!(st.next_change(t0 + secs(10)), Some(Duration::ZERO));
        assert_eq!(st.next_change(t0 + secs(30)), Some(Duration::ZERO));
    }

    // ----- stale ids ----------------------------------------------------------------

    #[test]
    fn stale_ids_return_false_and_change_nothing() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        let gone = add(&mut st, t0);
        st.remove(gone, t0);
        let before = (ids(&st), st.get(a).unwrap().clone().status);
        assert!(!st.remove(gone, t0));
        assert!(!st.set_busy(gone, true, t0));
        assert!(!st.set_status(gone, CardStatus::Saved));
        assert!(!st.set_saved(gone, Some(PathBuf::from("C:/x.png"))));
        st.toggle_select(gone, t0);
        st.toggle_pin(gone, t0);
        st.set_hover(Some(gone), t0);
        st.set_focus(Some(Focus::Card(gone)), t0);
        assert_eq!(st.get(gone).map(|i| i.id), None);
        assert_eq!(st.fraction(gone, t0), None);
        assert_eq!(st.remaining(gone, t0), None);
        assert!(!st.is_running(gone));
        assert_eq!((ids(&st), st.get(a).unwrap().status), before);
        assert!(st.selected().is_empty() && st.focus().is_none());
        assert!(!st.get(a).unwrap().pinned && !st.get(a).unwrap().hovered);
        // Ids that never existed behave the same.
        assert!(!st.set_busy(ItemId(0), true, t0));
        assert!(!st.remove(ItemId(u64::MAX), t0));
    }

    #[test]
    fn status_and_saved_path_are_kept_per_card() {
        let t0 = Instant::now();
        let mut st = stack();
        let a = add(&mut st, t0);
        let b = add(&mut st, t0);
        assert!(st.set_status(a, CardStatus::Saved));
        assert!(st.set_saved(a, Some(PathBuf::from("C:/shots/a.png"))));
        assert_eq!(st.get(a).unwrap().status, CardStatus::Saved);
        assert_eq!(
            st.get(a).unwrap().saved.as_deref(),
            Some(std::path::Path::new("C:/shots/a.png"))
        );
        assert_eq!(st.get(b).unwrap().status, CardStatus::Ready);
        assert!(st.set_saved(a, None));
        assert!(st.get(a).unwrap().saved.is_none());
        // Status and path don't touch the countdown.
        assert!(st.is_running(a));
    }

    // ----- layout -------------------------------------------------------------------

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn inside(outer: &Rect, inner: &Rect) -> bool {
        inner.x >= outer.x - 1e-3
            && inner.y >= outer.y - 1e-3
            && right(inner) <= right(outer) + 1e-3
            && bottom(inner) <= bottom(outer) + 1e-3
    }

    fn overlap(a: &Rect, b: &Rect) -> bool {
        a.x < right(b) && b.x < right(a) && a.y < bottom(b) && b.y < bottom(a)
    }

    fn filled(n: usize, t0: Instant) -> Stack {
        let mut st = stack();
        for _ in 0..n {
            add(&mut st, t0);
        }
        st
    }

    #[test]
    fn the_window_fits_five_cards_the_bar_the_notice_and_the_padding() {
        let (w, h) = window_size();
        assert!(w >= 5.0 * CARD_W + 4.0 * GAP + 2.0 * PAD);
        assert!(w >= bar_width() + 2.0 * PAD && w >= NOTICE_W + 2.0 * PAD);
        assert!(h >= PAD + CARD_H + GAP + BAR_H + GAP + NOTICE_H + PAD);
        const { assert!(BAR_BTN_H >= 24.0 && BAR_BTN_H <= BAR_H) };
        assert_eq!(window_size(), (w, h), "constant");
    }

    #[test]
    fn cards_sit_in_one_centred_row_newest_leftmost() {
        let t0 = Instant::now();
        let (w, h) = window_size();
        let win = Rect::new(0.0, 0.0, w, h);
        for n in 1..=5 {
            let st = filled(n, t0);
            let l = layout(&st, t0);
            assert_eq!(l.cards.len(), n);
            assert!(l.bar.is_none() && l.notice.is_none());
            for (i, c) in l.cards.iter().enumerate() {
                assert_eq!(c.id, ItemId((n - i) as u64), "newest first, n = {n}");
                assert_eq!((c.rect.w, c.rect.h, c.rect.y), (CARD_W, CARD_H, PAD));
                assert!(inside(&win, &c.rect), "n = {n} card {i}");
                if i > 0 {
                    let prev = &l.cards[i - 1].rect;
                    assert!(near(c.rect.x - right(prev), GAP), "gap, n = {n}");
                }
            }
            let (first, last) = (&l.cards[0].rect, &l.cards[n - 1].rect);
            assert!(
                near(first.x, w - right(last)),
                "centred: {} left, {} right (n = {n})",
                first.x,
                w - right(last)
            );
        }
        assert!(near(layout(&filled(5, t0), t0).cards[0].rect.x, PAD));
        // No cards, no layout.
        let empty = layout(&stack(), t0);
        assert!(empty.cards.is_empty() && empty.bar.is_none() && empty.notice.is_none());
    }

    #[test]
    fn a_cards_parts_sit_where_the_contract_says() {
        let t0 = Instant::now();
        let l = layout(&filled(1, t0), t0);
        let c = &l.cards[0];
        let r = c.rect;
        assert_eq!(c.check, Rect::new(r.x + 6.0, r.y + 6.0, 18.0, 18.0));
        assert_eq!(
            c.ring,
            Rect::new(right(&r) - 5.0 - 16.0, r.y + 5.0, 16.0, 16.0)
        );
        // The status chip fills the space between the check and the ring.
        assert!(near(c.status.x - right(&c.check), CHIP_GAP));
        assert!(near(c.ring.x - right(&c.status), CHIP_GAP));
        assert!(
            c.status.w > 40.0,
            "room for \"file missing\" in a small font"
        );
        assert!(!overlap(&c.check, &c.status) && !overlap(&c.status, &c.ring));
        assert!(!overlap(&c.check, &c.ring));
        // Four buttons along the bottom, 5 px in, 16 high, 3 px apart, filling the 114 px.
        assert_eq!(
            c.actions.map(|(a, _)| a),
            [
                CardAction::Copy,
                CardAction::Save,
                CardAction::Pin,
                CardAction::Dismiss
            ]
        );
        let widths = c.actions.map(|(_, a)| a.w);
        assert_eq!(widths, [27.0, 27.0, 33.0, 18.0]);
        for (i, (_, a)) in c.actions.iter().enumerate() {
            assert_eq!((a.y, a.h), (bottom(&r) - 5.0 - 16.0, 16.0));
            assert!(inside(&r, a));
            if i > 0 {
                assert!(near(a.x - right(&c.actions[i - 1].1), ACTION_GAP));
            }
            assert!(!overlap(a, &c.check) && !overlap(a, &c.ring) && !overlap(a, &c.status));
        }
        assert!(near(c.actions[0].1.x, r.x + 5.0));
        assert!(near(right(&c.actions[3].1), right(&r) - 5.0));
        assert!(inside(&r, &c.check) && inside(&r, &c.ring) && inside(&r, &c.status));
    }

    #[test]
    fn the_bar_shows_only_with_a_selection_under_the_cards() {
        let t0 = Instant::now();
        let (w, h) = window_size();
        let win = Rect::new(0.0, 0.0, w, h);
        let mut st = filled(3, t0);
        assert!(layout(&st, t0).bar.is_none());
        st.toggle_select(ItemId(2), t0);
        let l = layout(&st, t0);
        let bar = l.bar.as_ref().expect("a selection shows the bar");
        assert!(
            near(bar.rect.y, PAD + CARD_H + GAP),
            "under the cards with GAP"
        );
        assert!(near(bar.rect.x, w - right(&bar.rect)), "centred");
        assert!(inside(&win, &bar.rect));
        assert!(inside(&bar.rect, &bar.count));
        assert_eq!(
            bar.buttons.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
            BarAction::ALL
        );
        let mut prev = right(&bar.count);
        for (_, b) in &bar.buttons {
            assert!(inside(&bar.rect, b) && b.h >= 24.0);
            assert!(b.x >= prev, "in order, not overlapping");
            prev = right(b);
        }
        assert!(near(bar.buttons[0].1.x - right(&bar.count), BAR_GAP));
        // The last card deselected: gone again. Deselecting everything too.
        st.toggle_select(ItemId(2), t0);
        assert!(layout(&st, t0).bar.is_none());
        st.select_all(t0);
        assert!(layout(&st, t0).bar.is_some());
        st.clear_selection(t0);
        assert!(layout(&st, t0).bar.is_none());
    }

    #[test]
    fn the_notice_takes_the_bars_row_or_goes_under_the_bar() {
        let t0 = Instant::now();
        let (w, h) = window_size();
        let win = Rect::new(0.0, 0.0, w, h);
        let mut st = Stack::with_limits(LIFE, 5, 1000);
        // Alone in an empty stack: at the top of the content.
        assert_eq!(st.admit(1, 1, 1001, t0), Admit::Full);
        let n = layout(&st, t0).notice.expect("notice");
        assert!(near(n.y, PAD) && near(n.x, w - right(&n)) && inside(&win, &n));
        // Under the cards, where the bar would be.
        add(&mut st, t0);
        assert_eq!(st.admit(1, 1, 1001, t0), Admit::Full);
        let l = layout(&st, t0);
        let n = l.notice.expect("notice");
        assert!(near(n.y, PAD + CARD_H + GAP) && inside(&win, &n));
        assert_eq!((n.w, n.h), (NOTICE_W, NOTICE_H));
        // With a bar: under the bar.
        st.toggle_select(ItemId(1), t0);
        let l = layout(&st, t0);
        let (bar, n) = (l.bar.unwrap(), l.notice.unwrap());
        assert!(near(n.y, bottom(&bar.rect) + GAP) && inside(&win, &n));
        assert!(!overlap(&n, &bar.rect));
        // It ends with the notice.
        assert!(layout(&st, t0 + secs(3)).notice.is_none());
    }

    #[test]
    fn nothing_in_the_layout_overlaps_with_a_full_house() {
        let t0 = Instant::now();
        let (w, h) = window_size();
        let win = Rect::new(0.0, 0.0, w, h);
        let mut st = Stack::with_limits(LIFE, 5, 100);
        for _ in 0..5 {
            add_bytes(&mut st, 1, t0);
        }
        st.select_all(t0);
        st.toggle_pin(ItemId(5), t0);
        // Everything is protected: admission is refused and the notice shows.
        assert_eq!(st.admit(1, 1, 1, t0), Admit::Full);
        let l = layout(&st, t0);
        let (bar, n) = (l.bar.as_ref().unwrap(), l.notice.unwrap());
        let mut all: Vec<Rect> = l.cards.iter().map(|c| c.rect).collect();
        all.push(bar.rect);
        all.push(n);
        for (i, a) in all.iter().enumerate() {
            assert!(inside(&win, a), "{i}");
            for b in &all[i + 1..] {
                assert!(!overlap(a, b), "{a:?} {b:?}");
            }
        }
    }

    // ----- hit testing --------------------------------------------------------------

    #[test]
    fn hit_testing_actions_win_over_the_card_body() {
        let t0 = Instant::now();
        let st = filled(3, t0);
        let l = layout(&st, t0);
        for c in &l.cards {
            let r = c.rect;
            // The body: the middle of the top edge, the check and the ring.
            for p in [
                (r.x + r.w / 2.0, r.y + 2.0),
                (c.check.x + 2.0, c.check.y + 2.0),
                (c.ring.x + 2.0, c.ring.y + 2.0),
                (r.x + 1.0, r.y + 1.0),
            ] {
                assert_eq!(hit(&l, p.0, p.1), Some(Hit::Card(c.id)), "{p:?}");
            }
            // Every action's four corners, and its centre.
            for (a, ar) in &c.actions {
                for p in [
                    (ar.x, ar.y),
                    (ar.x + ar.w - 0.01, ar.y + ar.h - 0.01),
                    (ar.x + ar.w / 2.0, ar.y + ar.h / 2.0),
                ] {
                    assert_eq!(hit(&l, p.0, p.1), Some(Hit::Action(c.id, *a)), "{a:?}");
                }
            }
            // The 3 px between two buttons is body.
            let gap_x = right(&c.actions[0].1) + 1.0;
            assert_eq!(
                hit(&l, gap_x, c.actions[0].1.y + 4.0),
                Some(Hit::Card(c.id))
            );
        }
    }

    #[test]
    fn hit_testing_gaps_the_margin_and_the_outside_hit_nothing() {
        let t0 = Instant::now();
        let st = filled(3, t0);
        let l = layout(&st, t0);
        let (w, h) = window_size();
        let (a, b) = (&l.cards[0].rect, &l.cards[1].rect);
        // Inside the gap between two cards, at card height.
        assert_eq!(hit(&l, right(a) + GAP / 2.0, a.y + a.h / 2.0), None);
        assert_eq!(hit(&l, right(a) + 0.01, a.y + 5.0), None);
        assert_eq!(hit(&l, b.x - 0.01, a.y + 5.0), None);
        // The shadow margin, above and below the row, and beyond the window.
        assert_eq!(hit(&l, a.x + 10.0, PAD - 1.0), None);
        assert_eq!(hit(&l, a.x + 10.0, bottom(a) + 1.0), None);
        assert_eq!(hit(&l, 0.0, 0.0), None);
        assert_eq!(hit(&l, w - 1.0, h - 1.0), None);
        assert_eq!(hit(&l, -5.0, -5.0), None);
        assert_eq!(hit(&l, f32::NAN, 10.0), None);
        // The right edge of a card is outside it.
        assert_eq!(hit(&l, right(a), a.y + 5.0), None);
        // An empty stack hits nothing.
        let e = layout(&stack(), t0);
        assert_eq!(hit(&e, w / 2.0, PAD + 10.0), None);
    }

    #[test]
    fn hit_testing_the_bar_buttons_and_not_its_background_or_the_notice() {
        let t0 = Instant::now();
        let mut st = Stack::with_limits(LIFE, 5, 1000);
        add(&mut st, t0);
        add(&mut st, t0);
        st.toggle_select(ItemId(1), t0);
        assert_eq!(st.admit(1, 1, 1001, t0), Admit::Full);
        let l = layout(&st, t0);
        let bar = l.bar.as_ref().unwrap();
        for (a, r) in &bar.buttons {
            assert_eq!(
                hit(&l, r.x + r.w / 2.0, r.y + r.h / 2.0),
                Some(Hit::Bar(*a))
            );
            assert_eq!(hit(&l, r.x, r.y), Some(Hit::Bar(*a)));
        }
        // The count text, the padding and the gap between buttons: the bar, but no button.
        assert_eq!(hit(&l, bar.count.x + 3.0, bar.count.y + 10.0), None);
        assert_eq!(hit(&l, bar.rect.x + 1.0, bar.rect.y + 1.0), None);
        let (b0, b1) = (&bar.buttons[0].1, &bar.buttons[1].1);
        assert_eq!(hit(&l, (right(b0) + b1.x) / 2.0, b0.y + 5.0), None);
        // The notice is for reading only.
        let n = l.notice.unwrap();
        assert_eq!(hit(&l, n.x + n.w / 2.0, n.y + n.h / 2.0), None);
    }

    #[test]
    fn input_rects_are_the_cards_and_the_bar_without_the_gaps() {
        let t0 = Instant::now();
        let mut st = Stack::with_limits(LIFE, 5, 1000);
        for _ in 0..3 {
            add(&mut st, t0);
        }
        let l = layout(&st, t0);
        let rects = input_rects(&l);
        assert_eq!(rects.len(), 3, "one rect per card, not their union");
        for (r, c) in rects.iter().zip(&l.cards) {
            assert_eq!(*r, c.rect);
        }
        let gap = (right(&rects[0]) + rects[1].x) / 2.0;
        assert!(!rects.iter().any(|r| r.contains(gap, PAD + 10.0)));
        st.toggle_select(ItemId(1), t0);
        assert_eq!(st.admit(1, 1, 1001, t0), Admit::Full);
        let l = layout(&st, t0);
        let rects = input_rects(&l);
        assert_eq!(rects.len(), 4);
        assert_eq!(rects[3], l.bar.as_ref().unwrap().rect);
        let n = l.notice.unwrap();
        assert!(
            !rects.iter().any(|r| r.contains(n.x + 1.0, n.y + 1.0)),
            "the notice is click-through"
        );
        assert!(input_rects(&layout(&stack(), t0)).is_empty());
        // Whatever hit() finds is inside an input rect.
        for c in &l.cards {
            for (_, a) in &c.actions {
                assert!(rects.iter().any(|r| r.contains(a.x + 1.0, a.y + 1.0)));
            }
        }
    }

    // ----- keyboard -----------------------------------------------------------------

    fn focus_of(st: &Stack) -> Option<Focus> {
        st.focus()
    }

    const ID1: ItemId = ItemId(1);
    const ID2: ItemId = ItemId(2);
    const ID3: ItemId = ItemId(3);

    #[test]
    fn arrows_move_between_cards_newest_leftmost_and_clamp_at_the_ends() {
        let t0 = Instant::now();
        let mut st = filled(3, t0);
        assert_eq!(st.key(StackKey::Right, t0), KeyCmd::Redraw);
        assert_eq!(
            focus_of(&st),
            Some(Focus::Card(ID3)),
            "no focus: the newest"
        );
        assert_eq!(st.key(StackKey::Right, t0), KeyCmd::Redraw);
        assert_eq!(focus_of(&st), Some(Focus::Card(ID2)));
        assert_eq!(st.key(StackKey::Right, t0), KeyCmd::Redraw);
        assert_eq!(focus_of(&st), Some(Focus::Card(ID1)));
        assert_eq!(
            st.key(StackKey::Right, t0),
            KeyCmd::None,
            "clamped, no wrap"
        );
        assert_eq!(focus_of(&st), Some(Focus::Card(ID1)));
        assert_eq!(st.key(StackKey::Left, t0), KeyCmd::Redraw);
        assert_eq!(focus_of(&st), Some(Focus::Card(ID2)));
        st.key(StackKey::Left, t0);
        assert_eq!(st.key(StackKey::Left, t0), KeyCmd::None);
        assert_eq!(focus_of(&st), Some(Focus::Card(ID3)));
        // Left with no focus lands on the first card as well.
        st.set_focus(None, t0);
        assert_eq!(st.key(StackKey::Left, t0), KeyCmd::Redraw);
        assert_eq!(focus_of(&st), Some(Focus::Card(ID3)));
        assert_in_step(&st);
    }

    #[test]
    fn tab_walks_the_cards_then_the_bar_buttons_and_wraps() {
        let t0 = Instant::now();
        let mut st = filled(3, t0);
        // No bar: cards only.
        let mut seen = Vec::new();
        for _ in 0..4 {
            assert_eq!(st.key(StackKey::Tab, t0), KeyCmd::Redraw);
            seen.push(focus_of(&st).unwrap());
        }
        assert_eq!(
            seen,
            [
                Focus::Card(ID3),
                Focus::Card(ID2),
                Focus::Card(ID1),
                Focus::Card(ID3)
            ],
            "wraps from the last card to the first"
        );
        // With a selection the four bar buttons follow the cards.
        st.toggle_select(ID2, t0);
        st.set_focus(Some(Focus::Card(ID1)), t0);
        let mut seen = Vec::new();
        for _ in 0..6 {
            st.key(StackKey::Tab, t0);
            seen.push(focus_of(&st).unwrap());
        }
        assert_eq!(
            seen,
            [
                Focus::Bar(BarAction::SelectAll),
                Focus::Bar(BarAction::Clear),
                Focus::Bar(BarAction::SaveSelected),
                Focus::Bar(BarAction::DismissSelected),
                Focus::Card(ID3),
                Focus::Card(ID2),
            ]
        );
        // And backwards, wrapping from the first card to the last button.
        st.set_focus(Some(Focus::Card(ID3)), t0);
        st.key(StackKey::ShiftTab, t0);
        assert_eq!(focus_of(&st), Some(Focus::Bar(BarAction::DismissSelected)));
        st.key(StackKey::ShiftTab, t0);
        assert_eq!(focus_of(&st), Some(Focus::Bar(BarAction::SaveSelected)));
        // From nothing: Tab starts at the first, Shift+Tab at the last.
        st.set_focus(None, t0);
        st.key(StackKey::Tab, t0);
        assert_eq!(focus_of(&st), Some(Focus::Card(ID3)));
        st.set_focus(None, t0);
        st.key(StackKey::ShiftTab, t0);
        assert_eq!(focus_of(&st), Some(Focus::Bar(BarAction::DismissSelected)));
        st.clear_selection(t0);
        st.set_focus(None, t0);
        st.key(StackKey::ShiftTab, t0);
        assert_eq!(
            focus_of(&st),
            Some(Focus::Card(ID1)),
            "no bar: the last card"
        );
        assert_in_step(&st);
    }

    #[test]
    fn a_single_focus_target_has_nowhere_to_tab_to() {
        let t0 = Instant::now();
        let mut st = filled(1, t0);
        assert_eq!(st.key(StackKey::Tab, t0), KeyCmd::Redraw);
        assert_eq!(st.key(StackKey::Tab, t0), KeyCmd::None);
        assert_eq!(st.key(StackKey::ShiftTab, t0), KeyCmd::None);
        assert_eq!(focus_of(&st), Some(Focus::Card(ID1)));
        // An empty stack has no focus to give.
        let mut empty = stack();
        for k in [
            StackKey::Tab,
            StackKey::ShiftTab,
            StackKey::Left,
            StackKey::Right,
        ] {
            assert_eq!(empty.key(k, t0), KeyCmd::None, "{k:?}");
        }
        assert_eq!(empty.focus(), None);
    }

    #[test]
    fn arrows_on_a_bar_button_move_along_the_bar_and_clamp() {
        let t0 = Instant::now();
        let mut st = filled(2, t0);
        st.toggle_select(ID1, t0);
        st.set_focus(Some(Focus::Bar(BarAction::SelectAll)), t0);
        assert_eq!(
            st.key(StackKey::Left, t0),
            KeyCmd::None,
            "never back to the cards"
        );
        assert_eq!(st.key(StackKey::Right, t0), KeyCmd::Redraw);
        assert_eq!(focus_of(&st), Some(Focus::Bar(BarAction::Clear)));
        st.key(StackKey::Right, t0);
        st.key(StackKey::Right, t0);
        assert_eq!(focus_of(&st), Some(Focus::Bar(BarAction::DismissSelected)));
        assert_eq!(st.key(StackKey::Right, t0), KeyCmd::None);
        assert_eq!(st.key(StackKey::Left, t0), KeyCmd::Redraw);
        assert_eq!(focus_of(&st), Some(Focus::Bar(BarAction::SaveSelected)));
    }

    #[test]
    fn space_and_enter_toggle_a_card_or_press_a_bar_button() {
        let t0 = Instant::now();
        let mut st = filled(2, t0);
        for k in [StackKey::Space, StackKey::Enter] {
            assert_eq!(st.key(k, t0), KeyCmd::None, "nothing focused");
        }
        st.set_focus(Some(Focus::Card(ID2)), t0);
        assert_eq!(st.key(StackKey::Space, t0), KeyCmd::ToggleSelect(ID2));
        assert_eq!(st.key(StackKey::Enter, t0), KeyCmd::ToggleSelect(ID2));
        // The stack only reports it: the caller toggles.
        assert!(st.selected().is_empty());
        st.toggle_select(ID2, t0);
        st.set_focus(Some(Focus::Bar(BarAction::SaveSelected)), t0);
        assert_eq!(
            st.key(StackKey::Enter, t0),
            KeyCmd::Bar(BarAction::SaveSelected)
        );
        assert_eq!(
            st.key(StackKey::Space, t0),
            KeyCmd::Bar(BarAction::SaveSelected)
        );
    }

    #[test]
    fn c_s_p_and_delete_act_on_the_focused_card() {
        let t0 = Instant::now();
        let mut st = filled(3, t0);
        let keys = [
            (StackKey::C, CardAction::Copy),
            (StackKey::S, CardAction::Save),
            (StackKey::P, CardAction::Pin),
            (StackKey::Delete, CardAction::Dismiss),
        ];
        for (k, _) in keys {
            assert_eq!(st.key(k, t0), KeyCmd::None, "{k:?}: no focus");
        }
        st.set_focus(Some(Focus::Card(ID2)), t0);
        for (k, a) in keys {
            assert_eq!(st.key(k, t0), KeyCmd::Card(ID2, a), "{k:?}");
        }
        // Reported, not applied: the card is still there and unpinned.
        assert_eq!(st.len(), 3);
        assert!(!st.get(ID2).unwrap().pinned);
        // A bar button has no card to act on.
        st.toggle_select(ID1, t0);
        st.set_focus(Some(Focus::Bar(BarAction::Clear)), t0);
        for (k, _) in keys {
            assert_eq!(st.key(k, t0), KeyCmd::None, "{k:?}: bar focus");
        }
    }

    #[test]
    fn ctrl_a_selects_all_while_there_are_cards() {
        let t0 = Instant::now();
        let mut st = filled(2, t0);
        assert_eq!(
            st.key(StackKey::CtrlA, t0),
            KeyCmd::Bar(BarAction::SelectAll)
        );
        st.set_focus(Some(Focus::Card(ID1)), t0);
        assert_eq!(
            st.key(StackKey::CtrlA, t0),
            KeyCmd::Bar(BarAction::SelectAll)
        );
        assert_eq!(stack().key(StackKey::CtrlA, t0), KeyCmd::None);
    }

    #[test]
    fn escape_releases_and_drops_the_focus_so_the_countdown_resumes() {
        let t0 = Instant::now();
        let mut st = filled(2, t0);
        st.set_focus(Some(Focus::Card(ID1)), t0 + secs(2));
        assert!(!st.is_running(ID1));
        assert_eq!(st.key(StackKey::Escape, t0 + secs(30)), KeyCmd::Release);
        assert_eq!(st.focus(), None);
        assert!(st.is_running(ID1));
        assert_eq!(st.remaining(ID1, t0 + secs(30)), Some(secs(8)));
        // Escape with no focus still releases.
        assert_eq!(st.key(StackKey::Escape, t0 + secs(31)), KeyCmd::Release);
        assert_eq!(stack().key(StackKey::Escape, t0), KeyCmd::Release);
        assert_in_step(&st);
    }

    #[test]
    fn focus_moves_to_a_neighbour_when_its_card_is_removed() {
        let t0 = Instant::now();
        // [4, 3, 2, 1]: focus the middle one and remove it: the older neighbour.
        let mut st = filled(4, t0);
        st.set_focus(Some(Focus::Card(ID3)), t0);
        assert!(st.remove(ID3, t0));
        assert_eq!(st.focus(), Some(Focus::Card(ID2)));
        assert_in_step(&st);
        // The oldest: the newer neighbour.
        st.set_focus(Some(Focus::Card(ID1)), t0);
        st.remove(ID1, t0);
        assert_eq!(st.focus(), Some(Focus::Card(ID2)));
        // The newest goes: the next one takes over.
        st.set_focus(Some(Focus::Card(ItemId(4))), t0);
        st.remove(ItemId(4), t0);
        assert_eq!(st.focus(), Some(Focus::Card(ID2)));
        // The last card: nothing to focus.
        st.remove(ID2, t0);
        assert_eq!(st.focus(), None);
        assert!(st.is_empty());
        // Removing another card leaves the focus alone.
        let mut st = filled(3, t0);
        st.set_focus(Some(Focus::Card(ID2)), t0);
        st.remove(ID3, t0);
        assert_eq!(st.focus(), Some(Focus::Card(ID2)));
        assert_in_step(&st);
    }

    #[test]
    fn dismissing_the_selection_moves_the_focus_to_a_survivor() {
        let t0 = Instant::now();
        // [5, 4, 3, 2, 1], focus 3, select 4, 3 and 2: the survivors are 5 and 1, and the
        // focus goes to the first one at or after its old place, so the older card 1.
        let mut st = filled(5, t0);
        for id in [4, 3, 2] {
            st.toggle_select(ItemId(id), t0);
        }
        st.set_focus(Some(Focus::Card(ID3)), t0);
        let gone = st.remove_selected(t0);
        assert_eq!(gone, [ItemId(4), ID3, ID2]);
        assert_eq!(st.focus(), Some(Focus::Card(ID1)));
        assert_in_step(&st);
        // Everything selected and dismissed: no focus left.
        st.select_all(t0);
        st.set_focus(Some(Focus::Bar(BarAction::DismissSelected)), t0);
        st.remove_selected(t0);
        assert_eq!(st.focus(), None);
        // A focused card that survives keeps the focus.
        let mut st = filled(3, t0);
        st.toggle_select(ID3, t0);
        st.set_focus(Some(Focus::Card(ID1)), t0);
        st.remove_selected(t0);
        assert_eq!(st.focus(), Some(Focus::Card(ID1)));
    }

    #[test]
    fn a_bar_focus_falls_back_to_the_first_card_when_the_bar_goes() {
        let t0 = Instant::now();
        let mut st = filled(3, t0);
        st.toggle_select(ID2, t0);
        st.set_focus(Some(Focus::Bar(BarAction::Clear)), t0);
        assert_eq!(st.focus(), Some(Focus::Bar(BarAction::Clear)));
        st.clear_selection(t0);
        assert_eq!(st.focus(), Some(Focus::Card(ID3)));
        assert_in_step(&st);
        // Not selectable without a selection: refused.
        st.set_focus(Some(Focus::Bar(BarAction::SelectAll)), t0);
        assert_eq!(st.focus(), Some(Focus::Card(ID3)));
    }

    #[test]
    fn a_focused_card_does_not_expire_nor_get_evicted() {
        let t0 = Instant::now();
        let mut st = Stack::with_limits(LIFE, 2, MAX_BYTES);
        let a = add(&mut st, t0);
        let b = add(&mut st, t0);
        st.set_focus(Some(Focus::Card(a)), t0 + secs(1));
        assert_eq!(st.tick(t0 + secs(10)), [b]);
        let c = add(&mut st, t0 + secs(10));
        let Admit::Added { evicted, .. } = st.admit(1, 1, 1, t0 + secs(10)) else {
            panic!("full")
        };
        assert_eq!(
            evicted,
            [c],
            "the focused oldest stays, the next oldest goes"
        );
        assert!(st.get(a).is_some());
    }

    // ----- motion -------------------------------------------------------------------

    const SPEC: SpringSpec = SpringSpec::new(0.45, 0.75);

    fn settle(m: &mut Motion) -> usize {
        let mut steps = 0;
        while m.animating() {
            m.step(1.0 / 60.0, SPEC);
            steps += 1;
            assert!(steps < 1200, "still animating after 20 s");
        }
        steps
    }

    fn rect_of(m: &Motion, id: ItemId) -> Rect {
        m.cards().iter().find(|c| c.id == id).unwrap().rect
    }

    #[test]
    fn a_new_card_pops_in_from_above_and_settles_in_its_slot() {
        let t0 = Instant::now();
        let st = filled(1, t0);
        let l = layout(&st, t0);
        let slot = l.cards[0].rect;
        let mut m = Motion::new();
        assert!(!m.animating() && m.cards().is_empty());
        m.sync(&l);
        assert!(m.animating());
        let c = m.cards()[0];
        assert_eq!(c.id, ID1);
        assert!(near(c.rect.y, slot.y - 30.0) && near(c.rect.x, slot.x));
        assert_eq!((c.rect.w, c.rect.h), (CARD_W, CARD_H));
        assert!(near(c.scale, 0.6) && c.alpha == 0.0 && !c.exiting);
        m.step(1.0 / 60.0, SPEC);
        let c = m.cards()[0];
        assert!(
            c.rect.y > slot.y - 30.0 && c.rect.y < slot.y + 1.0,
            "moving down"
        );
        assert!(c.alpha > 0.0 && c.scale > 0.6, "fading and growing in");
        settle(&mut m);
        let c = m.cards()[0];
        assert_eq!(c.rect, slot);
        assert_eq!((c.alpha, c.scale), (1.0, 1.0));
        assert!(!m.animating());
        assert!(m.take_finished().is_empty(), "nothing exited");
        // Settled: stepping again changes nothing.
        m.step(1.0, SPEC);
        assert_eq!(m.cards()[0], c);
    }

    #[test]
    fn syncing_the_same_layout_twice_does_not_restart_anything() {
        let t0 = Instant::now();
        let l = layout(&filled(2, t0), t0);
        let mut m = Motion::new();
        m.sync(&l);
        settle(&mut m);
        let before = m.cards();
        m.sync(&l);
        assert!(!m.animating());
        assert_eq!(m.cards(), before);
    }

    #[test]
    fn the_cards_are_drawn_oldest_first_and_new_ones_join_at_the_front() {
        let t0 = Instant::now();
        let mut st = filled(2, t0);
        let mut m = Motion::new();
        m.sync(&layout(&st, t0));
        settle(&mut m);
        assert_eq!(
            m.cards().iter().map(|c| c.id.0).collect::<Vec<_>>(),
            [1, 2],
            "back to front"
        );
        add(&mut st, t0);
        m.sync(&layout(&st, t0));
        assert_eq!(
            m.cards().iter().map(|c| c.id.0).collect::<Vec<_>>(),
            [1, 2, 3]
        );
    }

    #[test]
    fn existing_cards_slide_to_their_new_slots() {
        let t0 = Instant::now();
        let mut st = filled(1, t0);
        let mut m = Motion::new();
        m.sync(&layout(&st, t0));
        settle(&mut m);
        let old = rect_of(&m, ID1);
        add(&mut st, t0);
        let l = layout(&st, t0);
        m.sync(&l);
        // Not there yet: the old card still sits where it was.
        assert_eq!(rect_of(&m, ID1), old);
        m.step(1.0 / 60.0, SPEC);
        let mid = rect_of(&m, ID1);
        let target = l.cards[1].rect;
        assert!(
            target.x > old.x && mid.x > old.x && mid.x < target.x + 1.0,
            "sliding right"
        );
        settle(&mut m);
        assert_eq!(rect_of(&m, ID1), target);
        assert_eq!(rect_of(&m, ID2), l.cards[0].rect);
    }

    #[test]
    fn a_removed_card_fades_out_as_a_ghost_and_is_reported_once_done() {
        let t0 = Instant::now();
        let mut st = filled(3, t0);
        let mut m = Motion::new();
        m.sync(&layout(&st, t0));
        settle(&mut m);
        let at = rect_of(&m, ID2);
        st.remove(ID2, t0);
        let l = layout(&st, t0);
        m.sync(&l);
        assert!(m.animating());
        let ghost = m.cards().into_iter().find(|c| c.id == ID2).unwrap();
        assert!(
            ghost.exiting && ghost.alpha == 1.0,
            "still fully there at first"
        );
        assert!(m.take_finished().is_empty());
        m.step(1.0 / 60.0, SPEC);
        let ghost = m.cards().into_iter().find(|c| c.id == ID2).unwrap();
        assert!(
            ghost.exiting && ghost.alpha < 1.0 && ghost.rect.y < at.y,
            "fading and lifting"
        );
        // The survivors are not ghosts.
        assert!(m.cards().iter().filter(|c| c.id != ID2).all(|c| !c.exiting));
        settle(&mut m);
        assert_eq!(m.take_finished(), [ID2]);
        assert!(m.take_finished().is_empty(), "reported once");
        assert_eq!(m.cards().iter().map(|c| c.id.0).collect::<Vec<_>>(), [1, 3]);
        assert!(!m.animating());
    }

    #[test]
    fn the_last_card_leaves_an_empty_motion_once_it_finished() {
        let t0 = Instant::now();
        let mut st = filled(1, t0);
        let mut m = Motion::new();
        m.sync(&layout(&st, t0));
        settle(&mut m);
        st.remove(ID1, t0);
        m.sync(&layout(&st, t0));
        assert_eq!(m.cards().len(), 1, "the ghost");
        assert!(m.animating());
        settle(&mut m);
        assert!(m.cards().is_empty());
        assert_eq!(m.take_finished(), [ID1]);
    }

    #[test]
    fn a_card_removed_before_it_ever_stepped_still_finishes_and_is_reported() {
        let t0 = Instant::now();
        let mut st = filled(1, t0);
        let mut m = Motion::new();
        // Pops in, then is removed before the first step: its alpha is still 0 (already at
        // rest), but it must not hang around forever or go without a report.
        m.sync(&layout(&st, t0));
        st.remove(ID1, t0);
        m.sync(&layout(&st, t0));
        assert!(m.animating());
        settle(&mut m);
        assert_eq!(m.take_finished(), [ID1]);
        assert!(m.cards().is_empty());
    }

    #[test]
    fn a_removed_card_that_comes_back_stops_exiting() {
        let t0 = Instant::now();
        let st = filled(1, t0);
        let l = layout(&st, t0);
        let mut m = Motion::new();
        m.sync(&l);
        settle(&mut m);
        m.sync(&layout(&stack(), t0));
        m.step(1.0 / 60.0, SPEC);
        m.sync(&l);
        assert!(!m.cards()[0].exiting);
        settle(&mut m);
        assert!(m.take_finished().is_empty());
        assert_eq!(m.cards()[0].rect, l.cards[0].rect);
        assert_eq!(m.cards()[0].alpha, 1.0);
    }

    #[test]
    fn alpha_stays_in_range_even_when_the_spring_overshoots() {
        let t0 = Instant::now();
        let mut st = filled(2, t0);
        let mut m = Motion::new();
        m.sync(&layout(&st, t0));
        let bouncy = SpringSpec::new(0.3, 0.2);
        for _ in 0..120 {
            m.step(1.0 / 60.0, bouncy);
            assert!(m.cards().iter().all(|c| (0.0..=1.0).contains(&c.alpha)));
        }
        st.remove(ID1, t0);
        m.sync(&layout(&st, t0));
        for _ in 0..600 {
            m.step(1.0 / 60.0, bouncy);
            assert!(m.cards().iter().all(|c| (0.0..=1.0).contains(&c.alpha)));
        }
    }

    // ----- placement ----------------------------------------------------------------

    /// The window at 100 %: `window_size()` rounded up.
    fn win100() -> (i32, i32) {
        let (w, h) = window_size();
        (w.ceil() as i32, h.ceil() as i32)
    }

    #[test]
    fn place_centres_under_the_island_with_the_content_below_its_edge() {
        let (w, h) = win100();
        let work = (0, 0, 1920, 1040);
        let (x, y, pw, ph) = place(960, 40, 1.0, work);
        assert_eq!((pw, ph), (w, h));
        assert_eq!(x, 960 - w / 2);
        // The content starts PAD inside the window, BELOW_ISLAND under the island.
        assert_eq!(y + PAD as i32, 40 + BELOW_ISLAND as i32);
        assert!(x >= 0 && x + pw <= 1920 && y >= 0 && y + ph <= 1040);
    }

    #[test]
    fn place_scales_the_window_and_the_offsets_for_dpi() {
        let work = (0, 0, 2880, 1560);
        let (x, y, w, h) = place(1440, 60, 1.5, work);
        let (ww, wh) = window_size();
        assert_eq!((w, h), ((ww * 1.5).ceil() as i32, (wh * 1.5).ceil() as i32));
        assert_eq!(x, 1440 - w / 2);
        assert_eq!(y + (PAD * 1.5) as i32, 60 + (BELOW_ISLAND * 1.5) as i32);
        // Sizes round up, so the window never clips the content.
        let (_, _, w, h) = place(1440, 60, 1.1, work);
        assert!(w as f32 >= ww * 1.1 && h as f32 >= wh * 1.1);
        assert!((w as f32) < ww * 1.1 + 1.0 && (h as f32) < wh * 1.1 + 1.0);
        // Nonsense scales act as 100 %.
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                place(960, 40, bad, (0, 0, 1920, 1040)),
                place(960, 40, 1.0, (0, 0, 1920, 1040))
            );
        }
    }

    #[test]
    fn place_handles_monitors_at_negative_coordinates() {
        let (w, h) = win100();
        // A monitor left of the primary one.
        let left = (-1920, 0, 0, 1040);
        let (x, y, pw, ph) = place(-960, 40, 1.0, left);
        assert_eq!((x, pw, ph), (-960 - w / 2, w, h));
        assert!(x >= -1920 && x + pw <= 0);
        assert_eq!(y, 40 + 12 - 16);
        // A monitor above the primary one.
        let above = (0, -1080, 1920, 0);
        let (x, y, ..) = place(960, -1040, 1.0, above);
        assert_eq!((x, y), (960 - w / 2, -1040 + 12 - 16));
        // Both negative.
        let both = (-2560, -1440, 0, 0);
        let (x, y, pw, ph) = place(-1280, -1400, 1.0, both);
        assert_eq!((x, y), (-1280 - w / 2, -1400 + 12 - 16));
        assert!(x >= -2560 && x + pw <= 0 && y >= -1440 && y + ph <= 0);
    }

    #[test]
    fn place_clamps_into_the_work_area_on_every_side() {
        let (w, h) = win100();
        let work = (-1920, 0, 0, 1040);
        // Island near the left edge: the window's left edge stops at the work area's.
        assert_eq!(place(-1900, 40, 1.0, work).0, -1920);
        // Near the right edge: its right edge stops at the work area's.
        let (x, _, pw, _) = place(-10, 40, 1.0, work);
        assert_eq!((x, x + pw), (-w, 0));
        // Island low on the screen: pushed up so the whole window is on it.
        let (_, y, _, ph) = place(-960, 1030, 1.0, work);
        assert_eq!((y, y + ph), (1040 - h, 1040));
        // Island above the work area (a reserved strip): pushed down to its top.
        assert_eq!(place(-960, -100, 1.0, work).1, 0);
        // A reserved top strip moves `top` down: the window never starts above it.
        assert_eq!(place(960, 10, 1.0, (0, 40, 1920, 1040)).1, 40);
    }

    #[test]
    fn place_keeps_the_left_and_top_edges_when_the_window_is_bigger_than_the_work_area() {
        let (w, h) = win100();
        assert!(w > 500 && h > 100);
        let (x, y, pw, ph) = place(250, 20, 1.0, (100, 50, 600, 150));
        assert_eq!((x, y), (100, 50), "clamped to the left and top");
        assert_eq!((pw, ph), (w, h));
        // Exactly as wide as the area still fits.
        let (x, ..) = place(500, 20, 1.0, (100, 0, 100 + w, 1000));
        assert_eq!(x, 100);
        // An empty or inverted work area does not panic.
        let _ = place(0, 0, 1.0, (0, 0, 0, 0));
        let _ = place(0, 0, 1.0, (10, 10, -10, -10));
    }
}
