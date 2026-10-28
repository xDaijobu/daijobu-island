//! The Snap preview stack: finished shots kept as cards below the island for a while,
//! to copy, save, pin or dismiss. `dj_core::snap_stack` has the rules (countdowns,
//! selection, capacity, layout), `preview_store` the image bytes and the IO thread,
//! `dj_win::previews` the window and `dj_render::previews` the pixels; this module wires
//! them together.
//!
//! [`Model`] is the part that decides (actions, worker replies, expiry) and runs in unit
//! tests with a fake clipboard. [`Previews`] adds the window and its surface on top.
//!
//! Memory and idle: the window, its surface, the thumbnails and the countdown timer
//! exist only while there are cards (or a stack-full notice). A card's thumbnail is made
//! once, at the card's DPI; the full image stays shared with the thumb (`Payload`).
//! Rendering happens on input, while a spring moves, and on the countdown tick (10 Hz,
//! only while a visible countdown runs).

use crate::preview_store::{CopyStart, Payload, ReplyKind, RevealError, Store};
use crate::snap::Clipboard;
use crate::snap_drag_store::{Completion, DragItem, DragStore};
use dj_core::layout::Rect;
use dj_core::snap::{Bitmap, Stamp, ThumbStatus};
use dj_core::snap_drag::{DragState, Outcome, RequestId};
use dj_core::snap_stack::{
    self as stack, Admit, BarAction, CardAction, CardLayout, CardStatus, Focus, Hit, Item, ItemId,
    KeyCmd, Motion, MotionCard, Stack, StackKey, StackLayout,
};
use dj_core::theme::ResolvedTokens;
use dj_render::{
    PreviewBar, PreviewButton, PreviewCard, PreviewScene, PreviewSurface, RenderError,
};
use dj_win::drag_drop::{DragOutcome, DragPayload};
use dj_win::previews::{self as win, PreviewInput};
use dj_win::{window, HWND};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The countdown tick (and the notice's one-shot). App timer ids 1-8 are taken.
pub const TIMER_SNAP_STACK: usize = 9;
/// 10 Hz: the most the ring is redrawn while a countdown runs.
const TICK_MS: u32 = 100;
/// Input region around a card, logical px: the hairline and anti-aliasing. A selected or
/// focused card adds its outline (2 px wide, 2 px off the card).
const REGION_PAD: f32 = 2.0;
const REGION_PAD_OUTLINE: f32 = 5.0;
const NOTICE_PAD: f32 = 3.0;
/// The window or its GPU surface couldn't be made: try again this often, this many times.
const RETRY: Duration = Duration::from_millis(500);
const MAX_RETRIES: u8 = 3;
/// A one-shot already armed within this of the wanted time is left alone.
const TIMER_SLACK: Duration = Duration::from_millis(20);

const VK_TAB: u16 = 0x09;
const VK_RETURN: u16 = 0x0D;
const VK_ESCAPE: u16 = 0x1B;
const VK_SPACE: u16 = 0x20;
const VK_LEFT: u16 = 0x25;
const VK_RIGHT: u16 = 0x27;
const VK_DELETE: u16 = 0x2E;
const VK_A: u16 = 0x41;
const VK_C: u16 = 0x43;
const VK_P: u16 = 0x50;
const VK_S: u16 = 0x53;

/// The key a stack key press stands for. Letters work with or without Ctrl (Ctrl+C
/// copies like C), except A, which needs Ctrl.
pub fn stack_key(vk: u16, ctrl: bool, shift: bool) -> Option<StackKey> {
    Some(match vk {
        VK_LEFT => StackKey::Left,
        VK_RIGHT => StackKey::Right,
        VK_TAB if shift => StackKey::ShiftTab,
        VK_TAB => StackKey::Tab,
        VK_SPACE => StackKey::Space,
        VK_RETURN => StackKey::Enter,
        VK_ESCAPE => StackKey::Escape,
        VK_DELETE => StackKey::Delete,
        VK_C => StackKey::C,
        VK_S => StackKey::S,
        VK_P => StackKey::P,
        VK_A if ctrl => StackKey::CtrlA,
        _ => return None,
    })
}

/// What a new card says first: how the shot's own copy (and save) went.
pub fn first_status(thumb: ThumbStatus) -> CardStatus {
    match thumb {
        ThumbStatus::Copied => CardStatus::Copied,
        ThumbStatus::CopyFailed => CardStatus::CopyFailed,
        ThumbStatus::Saved => CardStatus::Saved,
        ThumbStatus::SaveFailed => CardStatus::SaveFailed,
    }
}

/// Clicks retain the hidden-button behaviour, while the entire action row (including
/// its gaps) is excluded from body dragging even before hover reveals its buttons.
fn pointer_hits(
    layout: &StackLayout,
    buttons_shown: &[ItemId],
    x: f32,
    y: f32,
) -> (Option<Hit>, Option<ItemId>) {
    let raw = stack::hit(layout, x, y);
    let click = match raw {
        Some(Hit::Action(id, _)) if !buttons_shown.contains(&id) => Some(Hit::Card(id)),
        hit => hit,
    };
    let origin = match raw {
        Some(Hit::Card(id))
            if layout
                .cards
                .iter()
                .any(|card| card.id == id && y < card.actions[0].1.y) =>
        {
            Some(id)
        }
        _ => None,
    };
    (click, origin)
}

/// The stack's decisions: cards, their payloads, the actions and the worker's replies.
/// No window: errors come back as text for the app's log.
pub struct Model {
    stack: Stack,
    store: Store,
    clipboard: Box<dyn Clipboard>,
    /// Shows a saved file in Explorer (`Store::reveal`; a fake in tests).
    reveal: fn(&Path) -> Result<(), RevealError>,
    /// `[snap] save_dir` and the time for a new file's name.
    save_dir: PathBuf,
    stamp: fn() -> Stamp,
    /// Copies waiting for the worker to encode their PNG, and saves waiting for it.
    copying: Vec<ItemId>,
    saving: Vec<ItemId>,
    drag: DragState,
    /// Set by Previews with the owner's wake callback; headless tests inject replies.
    drag_store: Option<DragStore>,
    drag_ready: Option<(RequestId, DragPayload)>,
    drag_failed: HashSet<ItemId>,
    pressed: Option<Hit>,
    held: bool,
    /// What went wrong since the last `take_errors`, for the log.
    errors: Vec<String>,
}

impl Model {
    pub fn new(
        stack: Stack,
        store: Store,
        clipboard: Box<dyn Clipboard>,
        reveal: fn(&Path) -> Result<(), RevealError>,
        save_dir: PathBuf,
        stamp: fn() -> Stamp,
    ) -> Self {
        Self {
            stack,
            store,
            clipboard,
            reveal,
            save_dir,
            stamp,
            copying: Vec::new(),
            saving: Vec::new(),
            drag: DragState::default(),
            drag_store: None,
            drag_ready: None,
            drag_failed: HashSet::new(),
            pressed: None,
            held: false,
            errors: Vec::new(),
        }
    }

    pub fn stack(&self) -> &Stack {
        &self.stack
    }

    pub fn stack_mut(&mut self) -> &mut Stack {
        &mut self.stack
    }

    pub fn payload(&self, id: ItemId) -> Option<&Payload> {
        self.store.get(id.0)
    }

    pub fn take_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.errors)
    }

    /// A finished shot becomes the newest card, unless the stack is full of protected
    /// cards (or the shot alone is over the budget): then nothing changes but the notice,
    /// and the shot's own clipboard/save/thumb result is untouched either way.
    pub fn add(
        &mut self,
        payload: Payload,
        status: CardStatus,
        saved: Option<PathBuf>,
        now: Instant,
    ) -> Option<ItemId> {
        let (w, h) = (payload.bitmap.width, payload.bitmap.height);
        match self.stack.admit(w, h, payload.bytes(), now) {
            Admit::Added { id, evicted } => {
                for gone in evicted {
                    self.forget(gone);
                }
                self.store.insert(id.0, payload);
                self.stack.set_status(id, status);
                self.stack.set_saved(id, saved);
                Some(id)
            }
            Admit::Full => {
                self.errors
                    .push(format!("snap stack: full, {w}x{h} shot not kept as a card"));
                None
            }
        }
    }

    /// A card left the stack: its payload goes (the thumbnail stays until it has faded).
    fn forget(&mut self, id: ItemId) {
        self.store.remove(id.0);
        self.copying.retain(|c| *c != id);
        self.saving.retain(|s| *s != id);
        self.drag_failed.remove(&id);
    }

    /// Removes the cards whose countdown ran out; returns them.
    pub fn tick(&mut self, now: Instant) -> Vec<ItemId> {
        let gone = self.stack.tick(now);
        for &id in &gone {
            self.forget(id);
        }
        gone
    }

    /// A click (press and release on the same thing).
    pub fn click(&mut self, hit: Hit, now: Instant) {
        if self.drag.request().is_some() {
            return;
        }
        match hit {
            Hit::Card(id) => self.stack.toggle_select(id, now),
            Hit::Action(id, action) => self.card(id, action, now),
            Hit::Bar(action) => self.bar(action, now),
        }
    }

    pub fn card(&mut self, id: ItemId, action: CardAction, now: Instant) {
        if self.drag.request().is_some() || self.stack.get(id).is_none() {
            return;
        }
        self.drag_failed.remove(&id);
        match action {
            CardAction::Copy => self.copy(id, now),
            CardAction::Save => self.save_or_open(id, now),
            CardAction::Pin => self.stack.toggle_pin(id, now),
            CardAction::Dismiss => {
                if self.stack.remove(id, now) {
                    self.forget(id);
                }
            }
        }
    }

    pub fn bar(&mut self, action: BarAction, now: Instant) {
        if self.drag.request().is_some() {
            return;
        }
        match action {
            BarAction::SelectAll => self.stack.select_all(now),
            BarAction::Clear => self.stack.clear_selection(now),
            // Cards and the selection stay, each card shows its own result.
            BarAction::SaveSelected => {
                for id in self.stack.selected() {
                    let kept = self.saved_path(id).is_some_and(|p| p.exists());
                    if !kept {
                        self.save(id, now);
                    }
                }
            }
            BarAction::DismissSelected => {
                for id in self.stack.remove_selected(now) {
                    self.forget(id);
                }
            }
        }
    }

    fn saved_path(&self, id: ItemId) -> Option<PathBuf> {
        self.stack.get(id).and_then(|it| it.saved.clone())
    }

    fn copy(&mut self, id: ItemId, now: Instant) {
        if self.copying.contains(&id) {
            return;
        }
        match self.store.copy(id.0, self.clipboard.as_mut()) {
            CopyStart::Done(Ok(())) => {
                self.stack.set_status(id, CardStatus::Copied);
            }
            CopyStart::Done(Err(e)) => {
                self.stack.set_status(id, CardStatus::CopyFailed);
                self.errors.push(format!("snap stack: copy failed: {e}"));
            }
            CopyStart::Pending => {
                self.copying.push(id);
                self.sync_busy(id, now);
            }
        }
    }

    /// Save, or once saved, show the file. A file that has gone since is reported and
    /// the card can be saved again.
    fn save_or_open(&mut self, id: ItemId, now: Instant) {
        let Some(path) = self.saved_path(id) else {
            self.save(id, now);
            return;
        };
        match (self.reveal)(&path) {
            Ok(()) => {}
            Err(RevealError::Missing) => {
                self.stack.set_saved(id, None);
                self.stack.set_status(id, CardStatus::Missing);
                self.errors
                    .push(format!("snap stack: {} is gone", path.display()));
            }
            Err(RevealError::Other(e)) => {
                self.errors
                    .push(format!("snap stack: cannot open explorer: {e}"));
            }
        }
    }

    fn save(&mut self, id: ItemId, now: Instant) {
        if self.saving.contains(&id) {
            return;
        }
        // A file that went missing is saved again under a new name.
        self.stack.set_saved(id, None);
        if self.store.save(id.0, self.save_dir.clone(), (self.stamp)()) {
            self.saving.push(id);
            self.stack.set_status(id, CardStatus::Saving);
            self.sync_busy(id, now);
        } else {
            self.stack.set_status(id, CardStatus::SaveFailed);
            self.errors
                .push("snap stack: save failed: the save queue is full".into());
        }
    }

    /// Applies the worker's replies; replies for cards that are gone are dropped. Returns
    /// the images that were saved and where, so the thumb (which may share one) can say
    /// so too.
    pub fn drain(&mut self, now: Instant) -> Vec<(Arc<Bitmap>, PathBuf)> {
        let mut saved = Vec::new();
        for reply in self.store.drain() {
            let id = ItemId(reply.id);
            if self.stack.get(id).is_none() {
                continue;
            }
            match reply.kind {
                ReplyKind::Saved(path) => {
                    self.saving.retain(|s| *s != id);
                    if let Some(p) = self.store.get(id.0) {
                        saved.push((Arc::clone(&p.bitmap), path.clone()));
                    }
                    self.stack.set_saved(id, Some(path));
                    self.stack.set_status(id, CardStatus::Saved);
                }
                ReplyKind::SaveFailed(e) => {
                    self.saving.retain(|s| *s != id);
                    self.stack.set_status(id, CardStatus::SaveFailed);
                    self.errors.push(format!("snap stack: save failed: {e}"));
                }
                ReplyKind::Encoded => {
                    if self.take_copying(id) {
                        let status = match self.store.finish_copy(id.0, self.clipboard.as_mut()) {
                            Ok(()) => CardStatus::Copied,
                            Err(e) => {
                                self.errors.push(format!("snap stack: copy failed: {e}"));
                                CardStatus::CopyFailed
                            }
                        };
                        self.stack.set_status(id, status);
                    }
                }
                ReplyKind::EncodeFailed(e) => {
                    if self.take_copying(id) {
                        self.stack.set_status(id, CardStatus::CopyFailed);
                        self.errors.push(format!("snap stack: copy failed: {e}"));
                    }
                }
            }
            self.sync_busy(id, now);
        }
        let replies = self
            .drag_store
            .as_mut()
            .map_or_else(Vec::new, DragStore::drain);
        for reply in replies {
            saved.extend(self.drag_completion(reply, now));
        }
        saved
    }

    fn press(&mut self, hit: Option<Hit>, screen: (i32, i32), origin: Option<ItemId>) {
        if self.drag.request().is_some() {
            return;
        }
        self.pressed = hit;
        self.held = true;
        self.drag.press(origin, screen);
    }

    /// The only point that snapshots images: at the threshold, using the exact IDs
    /// returned by the core. No later completion may look up a replacement card.
    fn drag_motion(
        &mut self,
        screen: (i32, i32),
        threshold: (i32, i32),
        now: Instant,
    ) -> Option<(RequestId, Vec<DragItem>)> {
        if !self.held {
            return None;
        }
        let request = self.drag.motion(&mut self.stack, screen, threshold, now)?;
        self.pressed = None;
        self.drag_ready = None;
        let mut items = Vec::with_capacity(request.items.len());
        for &id in &request.items {
            self.drag_failed.remove(&id);
            let Some(payload) = self.store.get(id.0).cloned() else {
                self.fail_drag(
                    &request.items,
                    "a preview image is no longer available",
                    now,
                );
                return None;
            };
            items.push(DragItem {
                id: id.0,
                payload,
                saved: self.saved_path(id),
            });
        }
        Some((request.id, items))
    }

    fn submit_drag(&mut self, id: RequestId, items: Vec<DragItem>, now: Instant) {
        let result = match self.drag_store.as_mut() {
            Some(store) => store.submit(id.0, items, self.save_dir.clone(), (self.stamp)()),
            None => Err("the drag file worker is unavailable".into()),
        };
        if let Err(error) = result {
            let ids = self
                .drag
                .request()
                .map(|r| r.items.clone())
                .unwrap_or_default();
            self.fail_drag(&ids, &error, now);
        }
    }

    fn release(&mut self, hit: Option<Hit>, now: Instant) {
        let preparing = self
            .drag
            .request()
            .filter(|_| !self.drag.is_dragging())
            .map(|r| r.id);
        self.held = false;
        let suppress = self.drag.release(&mut self.stack, now);
        if let Some(id) = preparing {
            if let Some(store) = self.drag_store.as_mut() {
                store.cancel(id.0);
            }
            self.drag_ready = None;
        }
        let pressed = self.pressed.take();
        if !suppress {
            if let Some(hit) = pressed.filter(|p| Some(*p) == hit) {
                self.click(hit, now);
            }
        }
    }

    fn cancel_drag(&mut self, now: Instant) {
        if let Some(request) = self.drag.request() {
            if let Some(store) = self.drag_store.as_mut() {
                store.cancel(request.id.0);
            }
        }
        self.drag.cancel(&mut self.stack, now);
        self.drag_ready = None;
        self.pressed = None;
        self.held = false;
    }

    fn cancel_drag_request(&mut self, request: u64, now: Instant) -> bool {
        if self.drag.request().is_none_or(|r| r.id.0 != request) {
            return false;
        }
        self.cancel_drag(now);
        true
    }

    fn fail_drag(&mut self, ids: &[ItemId], error: &str, now: Instant) {
        for &id in ids {
            if self.stack.get(id).is_some() {
                self.drag_failed.insert(id);
                self.errors.push(format!(
                    "snap stack: drag failed for card {}: {error}",
                    id.0
                ));
            }
        }
        self.cancel_drag(now);
    }

    fn drag_completion(&mut self, reply: Completion, now: Instant) -> Vec<(Arc<Bitmap>, PathBuf)> {
        // Materialized files survive every outcome, including a cancelled generation.
        // Cache only into the exact original image, never a replacement with its ID.
        let mut saved = Vec::new();
        for prepared in &reply.saved {
            let id = ItemId(prepared.id);
            if self.stack.get(id).is_some()
                && self
                    .store
                    .get(id.0)
                    .is_some_and(|p| Arc::ptr_eq(&p.bitmap, &prepared.payload.bitmap))
            {
                self.store.insert(id.0, prepared.payload.clone());
                self.stack.set_saved(id, Some(prepared.path.clone()));
                self.stack.set_status(id, CardStatus::Saved);
                saved.push((Arc::clone(&prepared.payload.bitmap), prepared.path.clone()));
            }
        }
        let Some(request) = self
            .drag
            .request()
            .filter(|r| r.id.0 == reply.request)
            .cloned()
        else {
            return saved;
        };
        // Duplicate replies cannot replace a ready payload or complete a modal drag.
        if self.drag_ready.is_some() || self.drag.is_dragging() {
            return saved;
        }
        if reply.cancelled || !self.held {
            self.cancel_drag(now);
            return saved;
        }
        if !reply.failed.is_empty() {
            for (id, error) in reply.failed {
                self.fail_drag(&[ItemId(id)], &error, now);
            }
            return saved;
        }
        let complete = reply.saved.len() == request.items.len()
            && reply.saved.iter().zip(&request.items).all(|(p, &id)| {
                p.id == id.0
                    && self.stack.get(id).is_some_and(|it| it.busy)
                    && self
                        .store
                        .get(id.0)
                        .is_some_and(|v| Arc::ptr_eq(&v.bitmap, &p.payload.bitmap))
            });
        match reply.transport {
            Some(transport)
                if complete
                    && transport
                        .paths
                        .iter()
                        .eq(reply.saved.iter().map(|p| &p.path)) =>
            {
                self.drag_ready = Some((request.id, transport));
            }
            _ => self.fail_drag(
                &request.items,
                "the prepared group is incomplete or stale",
                now,
            ),
        }
        saved
    }

    fn take_drag(&mut self, pointer_valid: bool, now: Instant) -> Option<(u64, DragPayload)> {
        if self.drag.is_dragging() || self.drag.request().is_none() {
            return None;
        }
        if !self.held
            || !pointer_valid
            || self.drag.request().is_some_and(|r| {
                r.items
                    .iter()
                    .any(|&id| self.stack.get(id).is_none() || self.store.get(id.0).is_none())
            })
        {
            self.cancel_drag(now);
            return None;
        }
        let (id, transport) = self.drag_ready.take()?;
        if !self.drag.ready(id) {
            return None;
        }
        self.pressed = None;
        Some((id.0, transport))
    }

    fn finish_drag(&mut self, request: u64, result: Result<DragOutcome, String>, now: Instant) {
        let id = RequestId(request);
        let Some(active) = self.drag.request().filter(|r| r.id == id).cloned() else {
            return;
        };
        let outcome = match result {
            Ok(DragOutcome::DroppedCopy) => Outcome::DroppedCopy,
            Ok(DragOutcome::Cancelled) => Outcome::Cancelled,
            Err(error) => {
                for &id in &active.items {
                    self.drag_failed.insert(id);
                }
                self.errors
                    .push(format!("snap stack: drag failed: {error}"));
                Outcome::Failed
            }
        };
        for id in self.drag.finish(id, outcome, &mut self.stack, now) {
            self.forget(id);
        }
        self.drag_ready = None;
        self.pressed = None;
        self.held = false;
        self.stack.set_hover(None, now);
    }

    fn key(&mut self, key: StackKey, now: Instant) {
        if self.drag.request().is_some() {
            if key == StackKey::Escape {
                self.cancel_drag(now);
            }
            return;
        }
        match self.stack.key(key, now) {
            KeyCmd::None | KeyCmd::Redraw | KeyCmd::Release => {}
            KeyCmd::Card(id, action) => self.card(id, action, now),
            KeyCmd::ToggleSelect(id) => self.stack.toggle_select(id, now),
            KeyCmd::Bar(action) => self.bar(action, now),
        }
    }

    fn drag_status(&self, id: ItemId) -> Option<(&'static str, bool)> {
        if self.drag.request().is_some_and(|r| r.items.contains(&id)) {
            Some((
                if self.drag.is_dragging() {
                    "Dragging"
                } else {
                    "Preparing drag"
                },
                false,
            ))
        } else if self.drag_failed.contains(&id) {
            Some(("drag failed", true))
        } else {
            None
        }
    }

    fn drag_feedback(&self, cards: &mut [PreviewCard<'_>]) {
        for card in cards {
            if self.stack.get(ItemId(card.id)).is_none() {
                continue;
            }
            if let Some(status) = self.drag_status(ItemId(card.id)) {
                card.status = Some(status);
                if !status.1 {
                    // Full readable feedback in the body, clear of the selection
                    // check and pin/countdown badge. The chip covers the busy dots.
                    card.status_rect =
                        Rect::new(6.0, stack::CARD_H / 2.0 - 9.0, stack::CARD_W - 12.0, 18.0);
                    card.show_actions = false;
                }
            }
        }
    }

    /// The thumb saved the image a card shows: the card reads "open" for that file
    /// instead of saving a second copy.
    pub fn mark_saved(&mut self, bitmap: &Arc<Bitmap>, path: &Path) {
        let ids: Vec<ItemId> = self
            .stack
            .items()
            .iter()
            .map(|it| it.id)
            .filter(|id| !self.saving.contains(id))
            .filter(|id| {
                self.store
                    .get(id.0)
                    .is_some_and(|p| Arc::ptr_eq(&p.bitmap, bitmap))
            })
            .collect();
        for id in ids {
            self.stack.set_saved(id, Some(path.to_path_buf()));
            self.stack.set_status(id, CardStatus::Saved);
        }
    }

    fn take_copying(&mut self, id: ItemId) -> bool {
        let before = self.copying.len();
        self.copying.retain(|c| *c != id);
        self.copying.len() != before
    }

    /// A card is busy (paused, never evicted) while a save or a copy's encode is out.
    fn sync_busy(&mut self, id: ItemId, now: Instant) {
        let busy = self.saving.contains(&id)
            || self.copying.contains(&id)
            || self.drag.request().is_some_and(|r| r.items.contains(&id));
        self.stack.set_busy(id, busy, now);
    }
}

/// `rect` grown by `by` on every side, as window client px `(l, t, r, b)` at `scale`.
fn region_rect(rect: Rect, by: f32, scale: f32) -> (i32, i32, i32, i32) {
    (
        ((rect.x - by) * scale).floor() as i32,
        ((rect.y - by) * scale).floor() as i32,
        ((rect.x + rect.w + by) * scale).ceil() as i32,
        ((rect.y + rect.h + by) * scale).ceil() as i32,
    )
}

/// The window's input (and visible) region: every card where it is drawn this frame
/// (ghosts and pop-ins included) and where it will settle, the bar and the notice. The
/// gaps between cards and the rest of the window stay click-through.
pub fn region(
    st: &Stack,
    layout: &StackLayout,
    drawn: &[MotionCard],
    scale: f32,
) -> Vec<(i32, i32, i32, i32)> {
    let pad = |id: ItemId| match st.get(id) {
        Some(it) if it.selected || it.focused => REGION_PAD_OUTLINE,
        _ => REGION_PAD,
    };
    let mut out: Vec<_> = layout
        .cards
        .iter()
        .map(|c| region_rect(c.rect, pad(c.id), scale))
        .collect();
    for m in drawn {
        // Popping past 1x grows the card about its centre.
        let grow = m.rect.w * (m.scale - 1.0).max(0.0) / 2.0;
        out.push(region_rect(m.rect, pad(m.id) + grow, scale));
    }
    out.extend(layout.bar.iter().map(|b| region_rect(b.rect, 0.0, scale)));
    // The region's corners are the cards' r 12; the notice is drawn at r 8, so it gets a
    // little room or its corners would be cut.
    out.extend(
        layout
            .notice
            .iter()
            .map(|&r| region_rect(r, NOTICE_PAD, scale)),
    );
    out
}

/// `r` relative to `origin`'s top-left.
fn relative(r: Rect, origin: Rect) -> Rect {
    Rect::new(r.x - origin.x, r.y - origin.y, r.w, r.h)
}

fn card_label(action: CardAction, saved: bool, pinned: bool) -> &'static str {
    match action {
        CardAction::Copy => "copy",
        CardAction::Save if saved => "open",
        CardAction::Save => "save",
        CardAction::Pin if pinned => "unpin",
        CardAction::Pin => "pin",
        CardAction::Dismiss => "✕",
    }
}

fn bar_label(action: BarAction) -> &'static str {
    match action {
        BarAction::SelectAll => "Select all",
        BarAction::Clear => "Clear selection",
        BarAction::SaveSelected => "Save selected",
        BarAction::DismissSelected => "Dismiss selected",
    }
}

/// The cards to draw this frame, back to front. Ghosts (cards that are gone and fading)
/// draw with their last layout and no state.
pub fn scene_cards<'a>(
    st: &Stack,
    drawn: &[MotionCard],
    parts: &HashMap<ItemId, CardLayout>,
    now: Instant,
) -> Vec<PreviewCard<'a>> {
    let any_selected = st.items().iter().any(|it| it.selected);
    let focus = st.focus();
    drawn
        .iter()
        .filter_map(|m| {
            let lay = parts.get(&m.id)?;
            let item = st.get(m.id).filter(|_| !m.exiting);
            let flag = |f: fn(&Item) -> bool| item.is_some_and(f);
            let (selected, pinned, hovered, busy) = (
                flag(|it| it.selected),
                flag(|it| it.pinned),
                flag(|it| it.hovered),
                flag(|it| it.busy),
            );
            let focused = item.is_some() && focus == Some(Focus::Card(m.id));
            let saved = flag(|it| it.saved.is_some());
            let status = item.and_then(|it| it.status.label().map(|l| (l, it.status.is_error())));
            let actions = std::array::from_fn(|i| {
                let (action, r) = lay.actions[i];
                PreviewButton {
                    rect: relative(r, lay.rect),
                    label: card_label(action, saved, pinned),
                    focused: false,
                    primary: false,
                }
            });
            Some(PreviewCard {
                id: m.id.0,
                rect: m.rect,
                alpha: m.alpha,
                scale: m.scale,
                remaining: match item {
                    Some(it) if !it.pinned => st.fraction(it.id, now),
                    _ => None,
                },
                selected,
                pinned,
                hovered,
                focused,
                busy,
                show_check: item.is_some() && (hovered || focused || any_selected),
                show_actions: item.is_some() && (hovered || focused),
                status,
                check: relative(lay.check, lay.rect),
                ring: relative(lay.ring, lay.rect),
                status_rect: relative(lay.status, lay.rect),
                actions,
            })
        })
        .collect()
}

/// The stack's window and its GPU surface, made with the first card and destroyed with
/// the last.
struct Surface {
    hwnd: HWND,
    /// Dropped before the window is destroyed.
    gpu: Option<PreviewSurface>,
    /// Device px per logical px, and the window's desktop rect `(x, y, w, h)`.
    scale: f32,
    rect: (i32, i32, i32, i32),
    shown: bool,
}

/// What `TIMER_SNAP_STACK` is armed for.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Armed {
    /// The 10 Hz ring tick (repeating).
    Tick,
    /// One shot at this time: the notice ends, or a retry.
    At(Instant),
}

pub struct Previews {
    model: Model,
    motion: Motion,
    surface: Option<Surface>,
    /// The island window: owns the timer.
    owner: HWND,
    /// The island's centre x and lower edge, desktop px. The stack sits under it.
    anchor: (i32, i32),
    /// The theme, as the island shows it this frame.
    tokens: ResolvedTokens,
    /// The app shows the island and no capture is up.
    allowed: bool,
    /// Each card's last layout, so a fading card keeps its parts.
    parts: HashMap<ItemId, CardLayout>,
    /// The cards with a thumbnail on the GPU.
    uploaded: HashSet<ItemId>,
    /// The cards whose buttons were on screen in the last frame: only those take a click
    /// on a button (a tap on a card that shows none selects it instead).
    buttons_shown: Vec<ItemId>,
    /// The last frame's region, kept for one more frame: the swap chain shows a frame
    /// or two late, and a moving card must not be clipped meanwhile.
    last_region: Vec<(i32, i32, i32, i32)>,
    /// Draw a frame even though nothing moves.
    dirty: bool,
    timer: Option<Armed>,
    /// The window or the GPU surface is gone (device lost) or couldn't be made: make it
    /// again, at most `MAX_RETRIES` times in a row.
    rebuild: bool,
    retries: u8,
}

impl Previews {
    pub fn new(mut model: Model, owner: HWND, tokens: &ResolvedTokens) -> Self {
        let raw = owner.0 as isize;
        model.drag_store = Some(DragStore::new(Box::new(move || window::wake(raw))));
        Self {
            model,
            motion: Motion::new(),
            surface: None,
            owner,
            anchor: (0, 0),
            tokens: tokens.clone(),
            allowed: true,
            parts: HashMap::new(),
            uploaded: HashSet::new(),
            buttons_shown: Vec::new(),
            last_region: Vec::new(),
            dirty: false,
            timer: None,
            rebuild: false,
            retries: 0,
        }
    }

    fn shown(&self) -> bool {
        self.surface.as_ref().is_some_and(|s| s.shown)
    }

    /// Cards, a notice, or a card that has yet to fade out.
    fn has_content(&self, now: Instant) -> bool {
        let stack = self.model.stack();
        !stack.is_empty() || stack.notice(now).is_some() || !self.motion.cards().is_empty()
    }

    fn can_retry(&self) -> bool {
        self.rebuild && self.allowed && self.retries < MAX_RETRIES
    }

    /// There are cards the tray can give the keyboard to.
    pub fn can_focus(&self) -> bool {
        self.shown() && !self.model.stack().is_empty()
    }

    pub fn wants_frame(&self) -> bool {
        self.shown() && (self.dirty || self.motion.animating())
    }

    /// The theme changed (or is crossfading).
    pub fn set_tokens(&mut self, tokens: &ResolvedTokens) {
        self.tokens = tokens.clone();
        self.dirty = true;
    }

    fn log_errors(&mut self) {
        for e in self.model.take_errors() {
            crate::storage::log(&e);
        }
    }

    /// A finished snap: a new card (with its thumbnail), or the stack-full notice.
    pub fn add(
        &mut self,
        payload: Payload,
        status: CardStatus,
        saved: Option<PathBuf>,
        now: Instant,
    ) {
        self.cancel_drag(now);
        let id = self.model.add(payload, status, saved, now);
        self.log_errors();
        if self.surface.is_none() {
            // Uploads every card's thumbnail, this one's included.
            self.retries = 0;
            self.ensure_surface();
        } else if let Some(id) = id {
            self.upload(id);
        }
        self.dirty = true;
        self.sync(now);
    }

    fn upload(&mut self, id: ItemId) {
        let Some(s) = self.surface.as_mut() else {
            return;
        };
        let (Some(gpu), Some(p)) = (s.gpu.as_mut(), self.model.payload(id)) else {
            return;
        };
        if gpu.set_image(id.0, &p.bitmap, s.scale) {
            self.uploaded.insert(id);
        } else {
            crate::storage::log("snap stack: cannot make a card's thumbnail");
        }
    }

    /// The thumb saved its shot: a card showing the same image says so too.
    pub fn mark_saved(&mut self, bitmap: &Arc<Bitmap>, path: &Path) {
        self.model.mark_saved(bitmap, path);
        self.dirty = true;
    }

    /// The island moved: the stack follows its lower edge.
    pub fn set_anchor(&mut self, anchor: (i32, i32), now: Instant) {
        if anchor != self.anchor {
            self.cancel_drag(now);
            self.anchor = anchor;
            self.place(now);
        }
    }

    /// The island was shown or hidden, or a capture ended.
    pub fn set_allowed(&mut self, allowed: bool, now: Instant) {
        if !allowed {
            self.cancel_drag(now);
        }
        self.allowed = allowed;
        self.sync(now);
    }

    /// Before a freeze: hidden, and gone from the composed desktop when this returns.
    pub fn hide_for_capture(&mut self, now: Instant) {
        self.cancel_drag(now);
        let was_shown = self.shown();
        self.allowed = false;
        self.sync(now);
        if was_shown {
            win::flush_composition();
        }
    }

    /// DPI or displays changed: the window and the thumbnails are made again at the
    /// scale of the island's monitor.
    pub fn relayout(&mut self, anchor: (i32, i32), now: Instant) {
        self.cancel_drag(now);
        self.anchor = anchor;
        self.rebuild_surface(now);
    }

    fn rebuild_surface(&mut self, now: Instant) {
        self.destroy_surface(now);
        self.retries = 0;
        if self.has_content(now) {
            self.ensure_surface();
            self.dirty = true;
        }
        self.sync(now);
    }

    pub fn shutdown(&mut self) {
        self.cancel_drag(Instant::now());
        if self.timer.take().is_some() {
            window::kill_timer(self.owner, TIMER_SNAP_STACK);
        }
        self.destroy_surface(Instant::now());
    }

    /// Makes the window and its surface (hidden) if there are none, with every card's
    /// thumbnail. On failure the next `sync` tries again (`RETRY`, `MAX_RETRIES`).
    fn ensure_surface(&mut self) {
        if self.surface.is_some() {
            return;
        }
        let (ax, ay) = self.anchor;
        let scale = win::dpi_at(ax, ay) as f32 / 96.0;
        let rect = stack::place(ax, ay, scale, win::work_area_at(ax, ay));
        let hwnd = match win::create(rect.0, rect.1, rect.2, rect.3) {
            Ok(h) => h,
            Err(e) => {
                crate::storage::log(&format!("snap stack: cannot create the window: {e}"));
                self.failed();
                return;
            }
        };
        let mut gpu = match PreviewSurface::new(hwnd.0 as isize, rect.2 as u32, rect.3 as u32) {
            Ok(g) => g,
            Err(e) => {
                crate::storage::log(&format!("snap stack: renderer: {e}"));
                win::destroy(hwnd);
                self.failed();
                return;
            }
        };
        self.uploaded.clear();
        for it in self.model.stack().items() {
            if let Some(p) = self.model.payload(it.id) {
                if gpu.set_image(it.id.0, &p.bitmap, scale) {
                    self.uploaded.insert(it.id);
                } else {
                    crate::storage::log("snap stack: cannot make a card's thumbnail");
                }
            }
        }
        dj_win::system::set_topmost_companion(Some(hwnd));
        self.surface = Some(Surface {
            hwnd,
            gpu: Some(gpu),
            scale,
            rect,
            shown: false,
        });
        self.rebuild = false;
    }

    fn failed(&mut self) {
        self.rebuild = true;
        self.retries = self.retries.saturating_add(1);
        if self.retries == MAX_RETRIES {
            crate::storage::log("snap stack: giving up on the window until the next snap");
        }
    }

    /// The window goes (its GPU side first). Nothing it showed can be hovered, pressed
    /// or focused any more: a destroyed window sends no `WM_MOUSELEAVE`.
    fn destroy_surface(&mut self, now: Instant) {
        self.cancel_drag(now);
        if let Some(mut s) = self.surface.take() {
            dj_win::system::set_topmost_companion(None);
            s.gpu = None;
            win::destroy(s.hwnd);
        }
        self.uploaded.clear();
        self.buttons_shown.clear();
        self.last_region.clear();
        let stack = self.model.stack_mut();
        stack.set_hover(None, now);
        stack.set_focus(None, now);
    }

    fn place(&mut self, now: Instant) {
        let Some((old_scale, old_rect)) = self.surface.as_ref().map(|s| (s.scale, s.rect)) else {
            return;
        };
        let (ax, ay) = self.anchor;
        let scale = win::dpi_at(ax, ay) as f32 / 96.0;
        if (scale - old_scale).abs() > f32::EPSILON {
            // The island is on a monitor with another DPI now.
            self.rebuild_surface(now);
            return;
        }
        let rect = stack::place(ax, ay, scale, win::work_area_at(ax, ay));
        if let Some(s) = self.surface.as_mut().filter(|_| rect != old_rect) {
            s.rect = rect;
            win::place(s.hwnd, rect.0, rect.1, rect.2, rect.3);
        }
    }

    /// Shows or hides the window for the current state (drawing before it shows, so a
    /// stale frame never flashes), frees everything once nothing is left to show, keeps
    /// the countdowns paused exactly while nothing is on screen, and arms the timer.
    fn sync(&mut self, now: Instant) {
        self.sync_drag_escape(now);
        if self.surface.is_none() && self.can_retry() && self.has_content(now) {
            self.ensure_surface();
            self.dirty = true;
        }
        if !self.has_content(now) {
            self.destroy_surface(now);
            self.rebuild = false;
        }
        let want = self.allowed && self.surface.is_some() && self.has_content(now);
        if want && !self.shown() {
            // Drawn while still hidden; a lost device leaves nothing to show (and the
            // countdowns paused) until the rebuild.
            self.draw(now, 0.0);
            if let Some(s) = self.surface.as_mut() {
                win::show(s.hwnd);
                s.shown = true;
            }
        } else if !want && self.shown() {
            self.cancel_drag(now);
            if let Some(s) = self.surface.as_mut() {
                win::hide(s.hwnd);
                s.shown = false;
            }
            let stack = self.model.stack_mut();
            stack.set_hover(None, now);
            stack.set_focus(None, now);
            // A card still fading out doesn't keep a hidden window alive.
            if self.model.stack().is_empty() {
                self.motion = Motion::new();
                self.parts.clear();
            }
            if !self.has_content(now) {
                self.destroy_surface(now);
            }
        }
        let hidden = !self.shown();
        self.model.stack_mut().set_hidden(hidden, now);
        self.sync_timer(now);
    }

    fn sync_drag_escape(&mut self, now: Instant) {
        let preparing = self
            .model
            .drag
            .request()
            .filter(|_| !self.model.drag.is_dragging());
        if !window::watch_drag_escape(self.owner, preparing.map(|r| r.id.0)) {
            let ids = preparing.map(|r| r.items.clone()).unwrap_or_default();
            self.model
                .fail_drag(&ids, "Escape cancellation could not be enabled", now);
            if let Some(s) = self.surface.as_ref() {
                win::release_capture(s.hwnd);
            }
            self.dirty = true;
            self.log_errors();
        }
    }

    /// 10 Hz while a visible countdown runs; otherwise a one-shot to the next change
    /// (the notice going away, shown or not) or to a retry; otherwise nothing.
    fn wanted_timer(&self, now: Instant) -> Option<Armed> {
        let stack = self.model.stack();
        if self.shown()
            && (stack.any_running()
                || (self.model.drag.request().is_some() && !self.model.drag.is_dragging()))
        {
            return Some(Armed::Tick);
        }
        let change = self
            .surface
            .as_ref()
            .and(stack.next_change(now))
            .map(|d| now + d);
        let retry = (self.surface.is_none() && self.can_retry()).then(|| now + RETRY);
        match (change, retry) {
            (Some(a), Some(b)) => Some(Armed::At(a.min(b))),
            (a, b) => a.or(b).map(Armed::At),
        }
    }

    /// Arms `TIMER_SNAP_STACK` for `wanted_timer`, leaving a timer that is already right
    /// alone: re-arming on every input would keep the tick from ever firing.
    fn sync_timer(&mut self, now: Instant) {
        let want = self.wanted_timer(now);
        let same = match (self.timer, want) {
            (Some(Armed::Tick), Some(Armed::Tick)) | (None, None) => true,
            (Some(Armed::At(a)), Some(Armed::At(b))) => a.max(b) - a.min(b) < TIMER_SLACK,
            _ => false,
        };
        if same {
            return;
        }
        match want {
            Some(Armed::Tick) => window::set_timer(self.owner, TIMER_SNAP_STACK, TICK_MS),
            Some(Armed::At(t)) => {
                let ms = (t.saturating_duration_since(now).as_millis() as u32).saturating_add(5);
                window::set_timer(self.owner, TIMER_SNAP_STACK, ms.max(1));
            }
            None => window::kill_timer(self.owner, TIMER_SNAP_STACK),
        }
        self.timer = want;
    }

    /// `TIMER_SNAP_STACK`: expire cards, redraw the rings, retry a lost window.
    pub fn timer(&mut self, now: Instant) {
        window::kill_timer(self.owner, TIMER_SNAP_STACK);
        self.timer = None;
        if self.model.drag.request().is_some()
            && !self.model.drag.is_dragging()
            && !self
                .surface
                .as_ref()
                .is_some_and(|s| s.shown && win::drag_pointer_valid(s.hwnd))
        {
            self.cancel_drag(now);
        }
        self.model.tick(now);
        self.dirty = true;
        self.sync(now);
    }

    /// `Event::Wake`: the worker may have results. Returns the images it saved and where,
    /// for the thumb.
    pub fn drain(&mut self, now: Instant) -> Vec<(Arc<Bitmap>, PathBuf)> {
        let saved = self.model.drain(now);
        self.log_errors();
        self.dirty = true;
        self.sync(now);
        saved
    }

    /// Take an owned native payload for the outer dispatcher. No app/controller borrow
    /// may survive the OLE loop; this method only validates and hands off its inputs.
    pub fn take_drag(&mut self, now: Instant) -> Option<(u64, DragPayload)> {
        let hwnd = self
            .surface
            .as_ref()
            .filter(|s| self.allowed && s.shown)
            .map(|s| s.hwnd);
        let valid = hwnd.is_some_and(win::drag_pointer_valid);
        let had_request = self.model.drag.request().is_some();
        let payload = self.model.take_drag(valid, now);
        if payload.is_some() {
            // Keep this request's Escape latch armed while OLE/helper setup may
            // pump messages. The native transport disarms it at modal entry;
            // the outer dispatcher also clears it on every early return.
            // Ignore the synchronous WM_CAPTURECHANGED caused by this deliberate
            // handoff. All other losses remain cancellation events.
            if let Some(hwnd) = hwnd {
                win::release_capture(hwnd);
            }
            self.dirty = true;
            self.sync_timer(now);
        } else if had_request && self.model.drag.request().is_none() {
            window::watch_drag_escape(self.owner, None);
            if let Some(hwnd) = hwnd {
                win::release_capture(hwnd);
            }
            self.dirty = true;
            self.sync_timer(now);
        }
        payload
    }

    pub fn finish_drag(&mut self, request: u64, result: Result<DragOutcome, String>, now: Instant) {
        self.model.finish_drag(request, result, now);
        self.log_errors();
        self.dirty = true;
        self.sync(now);
    }

    /// Invalidate preparation before hide, Snap, shutdown, display/placement changes
    /// or capture loss. The outer dispatcher separately cancels a running OLE loop.
    pub fn cancel_drag(&mut self, now: Instant) {
        window::watch_drag_escape(self.owner, None);
        self.model.cancel_drag(now);
        if let Some(s) = self.surface.as_ref() {
            win::release_capture(s.hwnd);
        }
        self.dirty = true;
    }

    /// A latched Escape addresses the request that was preparing on key-down. A late
    /// event from an earlier gesture must never cancel a newer preparation.
    pub fn cancel_drag_request(&mut self, request: u64, now: Instant) {
        if self.model.cancel_drag_request(request, now) {
            self.cancel_drag(now);
            self.sync(now);
        }
    }

    /// The tray's "Focus Snap previews": the keyboard goes to the newest card, if the
    /// window really got the foreground.
    pub fn focus_keyboard(&mut self, now: Instant) {
        let Some(hwnd) = self.surface.as_ref().filter(|s| s.shown).map(|s| s.hwnd) else {
            return;
        };
        let Some(first) = self.model.stack().items().first().map(|it| it.id) else {
            return;
        };
        win::focus(hwnd);
        if win::has_foreground(hwnd) {
            self.model
                .stack_mut()
                .set_focus(Some(Focus::Card(first)), now);
        } else {
            crate::storage::log("snap stack: could not take the keyboard");
        }
        self.dirty = true;
        self.sync(now);
    }

    /// What a client point lands on. A button that isn't on screen counts as its card.
    fn hit_at(&self, x: i32, y: i32, now: Instant) -> Option<Hit> {
        let scale = self.surface.as_ref().map_or(1.0, |s| s.scale);
        let layout = stack::layout(self.model.stack(), now);
        pointer_hits(
            &layout,
            &self.buttons_shown,
            x as f32 / scale,
            y as f32 / scale,
        )
        .0
    }

    pub fn input(&mut self, input: PreviewInput, now: Instant) {
        let Some(hwnd) = self.surface.as_ref().filter(|s| s.shown).map(|s| s.hwnd) else {
            return;
        };
        match input {
            PreviewInput::Move { x, y } => {
                let hover = match self.hit_at(x, y, now) {
                    Some(Hit::Card(id) | Hit::Action(id, _)) => Some(id),
                    _ => None,
                };
                self.model.stack_mut().set_hover(hover, now);
                if let Some(screen) = win::screen_point(hwnd, x, y) {
                    if let Some((id, items)) =
                        self.model
                            .drag_motion(screen, win::drag_threshold(hwnd), now)
                    {
                        self.sync_drag_escape(now);
                        if self.model.drag.request().is_some_and(|r| r.id == id) {
                            self.model.submit_drag(id, items, now);
                        }
                    }
                } else {
                    self.cancel_drag(now);
                }
            }
            PreviewInput::Press { x, y } => {
                if let Some(screen) = win::screen_point(hwnd, x, y) {
                    let scale = self.surface.as_ref().map_or(1.0, |s| s.scale);
                    let layout = stack::layout(self.model.stack(), now);
                    let (hit, origin) = pointer_hits(
                        &layout,
                        &self.buttons_shown,
                        x as f32 / scale,
                        y as f32 / scale,
                    );
                    self.model.press(hit, screen, origin);
                } else {
                    self.cancel_drag(now);
                }
            }
            PreviewInput::Release { x, y } => {
                let hit = self.hit_at(x, y, now);
                self.model.release(hit, now);
            }
            PreviewInput::Leave => {
                self.model.stack_mut().set_hover(None, now);
            }
            PreviewInput::CancelCapture => {
                self.cancel_drag(now);
                self.model.stack_mut().set_hover(None, now);
            }
            PreviewInput::Key { vk, ctrl, shift } => {
                let Some(key) = stack_key(vk, ctrl, shift) else {
                    return;
                };
                let preparing = self.model.drag.request().is_some();
                self.model.key(key, now);
                if key == StackKey::Escape && preparing {
                    win::release_capture(hwnd);
                }
            }
            // A loss that was queued before the tray handed the keyboard back is stale.
            PreviewInput::FocusLost if win::has_foreground(hwnd) => {}
            PreviewInput::FocusLost => {
                self.cancel_drag(now);
                self.model.stack_mut().set_focus(None, now);
            }
        }
        self.log_errors();
        self.dirty = true;
        self.sync(now);
    }

    /// One frame: springs, input region, pixels. `dt` in seconds.
    pub fn frame(&mut self, dt: f32, now: Instant) {
        if !self.wants_frame() {
            return;
        }
        self.dirty = false;
        self.draw(now, dt);
        // Thumbnails and parts of cards that are gone and done fading (or that were
        // evicted before they were ever drawn) are freed.
        self.motion.take_finished();
        let live: HashSet<ItemId> = self
            .model
            .stack()
            .items()
            .iter()
            .map(|it| it.id)
            .chain(self.motion.cards().iter().map(|m| m.id))
            .collect();
        if let Some(gpu) = self.surface.as_mut().and_then(|s| s.gpu.as_mut()) {
            for id in self.uploaded.difference(&live) {
                gpu.drop_image(id.0);
            }
        }
        self.uploaded.retain(|id| live.contains(id));
        self.parts.retain(|id, _| live.contains(id));
        self.sync(now);
    }

    /// Steps the springs by `dt` and draws the stack as it is at `now`.
    fn draw(&mut self, now: Instant, dt: f32) {
        let layout = stack::layout(self.model.stack(), now);
        self.motion.sync(&layout);
        self.motion.step(dt, self.tokens.expand);
        for c in &layout.cards {
            self.parts.insert(c.id, c.clone());
        }
        let drawn = self.motion.cards();
        let Some(s) = self.surface.as_mut() else {
            return;
        };
        let st = self.model.stack();
        let rects = region(st, &layout, &drawn, s.scale);
        let mut both = rects.clone();
        both.extend_from_slice(&self.last_region);
        win::set_regions(s.hwnd, &both, (stack::CARD_R * s.scale).round() as i32);
        self.last_region = rects;
        let mut cards = scene_cards(st, &drawn, &self.parts, now);
        self.model.drag_feedback(&mut cards);
        self.buttons_shown = cards
            .iter()
            .filter(|c| c.show_actions)
            .map(|c| ItemId(c.id))
            .collect();
        let count = format!("{} selected", st.selected().len());
        let focus = st.focus();
        let buttons: Vec<PreviewButton> = layout
            .bar
            .iter()
            .flat_map(|b| b.buttons.iter())
            .map(|&(a, r)| PreviewButton {
                rect: r,
                label: bar_label(a),
                focused: focus == Some(Focus::Bar(a)),
                primary: a == BarAction::SaveSelected,
            })
            .collect();
        let scene = PreviewScene {
            tokens: &self.tokens,
            scale: s.scale,
            cards: &cards,
            bar: layout.bar.as_ref().map(|b| PreviewBar {
                rect: b.rect,
                count: (b.count, count.as_str()),
                buttons: &buttons,
            }),
            notice: layout.notice.zip(st.notice(now)),
        };
        let result = match s.gpu.as_mut() {
            Some(gpu) => gpu.render(&scene),
            None => Ok(()),
        };
        match result {
            Ok(()) => self.retries = 0,
            Err(RenderError::DeviceLost) => {
                crate::storage::log("snap stack: GPU device lost; rebuilding");
                self.destroy_surface(now);
                self.failed();
            }
            Err(e) => crate::storage::log(&format!("snap stack: render: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::snap::Bitmap;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Duration;

    type Puts = Rc<RefCell<Vec<(Vec<u8>, Vec<u8>)>>>;

    struct FakeClipboard {
        puts: Puts,
        fail: Rc<RefCell<bool>>,
    }

    impl Clipboard for FakeClipboard {
        fn put(&mut self, png: &[u8], dib: &[u8]) -> Result<(), String> {
            if *self.fail.borrow() {
                return Err("busy".into());
            }
            self.puts.borrow_mut().push((png.to_vec(), dib.to_vec()));
            Ok(())
        }
    }

    thread_local! {
        static REVEALED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    }

    /// Records what Explorer would have been asked to show; a missing file is `Missing`,
    /// like `Store::reveal`.
    fn fake_reveal(p: &Path) -> Result<(), RevealError> {
        if !p.exists() {
            return Err(RevealError::Missing);
        }
        REVEALED.with(|r| r.borrow_mut().push(p.to_path_buf()));
        Ok(())
    }

    fn revealed() -> Vec<PathBuf> {
        REVEALED.with(|r| r.borrow().clone())
    }

    fn stamp() -> Stamp {
        Stamp {
            year: 2026,
            month: 10,
            day: 26,
            hour: 14,
            minute: 5,
            second: 9,
        }
    }

    const LIFE: Duration = Duration::from_secs(10);

    struct Rig {
        m: Model,
        puts: Puts,
        fail: Rc<RefCell<bool>>,
        dir: tempfile::TempDir,
        t0: Instant,
    }

    fn rig() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let puts: Puts = Rc::default();
        let fail = Rc::new(RefCell::new(false));
        let m = Model::new(
            Stack::new(LIFE),
            Store::new(Box::new(|| {})),
            Box::new(FakeClipboard {
                puts: puts.clone(),
                fail: fail.clone(),
            }),
            fake_reveal,
            dir.path().join("shots"),
            stamp,
        );
        Rig {
            m,
            puts,
            fail,
            dir,
            t0: Instant::now(),
        }
    }

    /// A 4x2 shot whose "PNG" is `tag` repeated (the store writes it as it is).
    fn payload(tag: u8) -> Payload {
        Payload {
            bitmap: Arc::new(Bitmap::black(4, 2)),
            png: Some(Arc::from(vec![tag; 16])),
        }
    }

    impl Rig {
        fn add(&mut self, tag: u8) -> ItemId {
            self.m
                .add(payload(tag), CardStatus::Copied, None, self.t0)
                .unwrap()
        }

        fn status(&self, id: ItemId) -> CardStatus {
            self.m.stack().get(id).unwrap().status
        }

        fn saved(&self, id: ItemId) -> Option<PathBuf> {
            self.m.stack().get(id).unwrap().saved.clone()
        }

        /// Drains the worker until `done` holds (5 s at most).
        fn settle(&mut self, done: impl Fn(&Model) -> bool) {
            for _ in 0..500 {
                self.m.drain(self.t0);
                if done(&self.m) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("the worker never answered");
        }

        fn files(&self) -> Vec<PathBuf> {
            let mut v: Vec<_> = std::fs::read_dir(self.dir.path().join("shots"))
                .map(|d| d.map(|e| e.unwrap().path()).collect())
                .unwrap_or_default();
            v.sort();
            v
        }

        fn drag(&mut self, origin: ItemId) -> (RequestId, Vec<DragItem>) {
            self.m
                .press(Some(Hit::Card(origin)), (-1920, -500), Some(origin));
            self.m.drag_motion((-1907, -500), (6, 6), self.t0).unwrap()
        }

        /// Synthetic file replies, without OLE, a window or the system clipboard.
        fn prepared(&self, request: RequestId, items: &[DragItem]) -> Completion {
            let saved: Vec<_> = items
                .iter()
                .map(|item| {
                    let path = self
                        .dir
                        .path()
                        .join(format!("drag-{}-{}.png", request.0, item.id));
                    std::fs::write(
                        &path,
                        item.payload.png.as_deref().unwrap_or(b"synthetic png"),
                    )
                    .unwrap();
                    crate::snap_drag_store::PreparedItem {
                        id: item.id,
                        payload: item.payload.clone(),
                        path,
                    }
                })
                .collect();
            let transport = DragPayload {
                paths: saved.iter().map(|p| p.path.clone()).collect(),
                image: None,
                ghost: None,
            };
            Completion {
                request: request.0,
                saved,
                failed: vec![],
                cancelled: false,
                transport: Some(transport),
            }
        }
    }

    #[test]
    fn drag_short_press_still_selects_and_action_row_never_drags() {
        let mut r = rig();
        let id = r.add(1);
        r.m.press(Some(Hit::Card(id)), (-100, 50), Some(id));
        assert!(r.m.drag_motion((-94, 56), (6, 6), r.t0).is_none());
        r.m.release(Some(Hit::Card(id)), r.t0);
        assert_eq!(r.m.stack.selected(), vec![id]);
        r.m.press(Some(Hit::Action(id, CardAction::Copy)), (-100, 50), None);
        assert!(r.m.drag_motion((500, 50), (6, 6), r.t0).is_none());
        r.m.release(Some(Hit::Action(id, CardAction::Copy)), r.t0);
        assert_eq!(r.puts.borrow().len(), 1, "ordinary Copy still works");
        assert!(r.m.drag.request().is_none());
        r.m.press(Some(Hit::Bar(BarAction::Clear)), (0, 0), None);
        assert!(r.m.drag_motion((100, 0), (4, 4), r.t0).is_none());
        r.m.release(Some(Hit::Bar(BarAction::Clear)), r.t0);
        assert!(r.m.stack.selected().is_empty());
    }

    #[test]
    fn drag_excludes_hidden_action_rows_and_gaps_at_all_render_scales() {
        let mut r = rig();
        let id = r.add(1);
        let layout = stack::layout(r.m.stack(), r.t0);
        let card = &layout.cards[0];
        for scale in [1.0, 1.5, 2.0] {
            let hit = |x: f32, y: f32, shown: &[ItemId]| {
                let physical = ((x * scale).round() as i32, (y * scale).round() as i32);
                pointer_hits(
                    &layout,
                    shown,
                    physical.0 as f32 / scale,
                    physical.1 as f32 / scale,
                )
            };
            let body = (card.rect.x + 50.0, card.rect.y + 40.0);
            assert_eq!(hit(body.0, body.1, &[]), (Some(Hit::Card(id)), Some(id)));
            let button = card.actions[0].1;
            assert_eq!(
                hit(button.x + 10.0, button.y + 8.0, &[]),
                (Some(Hit::Card(id)), None)
            );
            assert_eq!(
                hit(button.x + 10.0, button.y + 8.0, &[id]),
                (Some(Hit::Action(id, CardAction::Copy)), None)
            );
            assert_eq!(hit(button.x + button.w + 1.0, button.y + 8.0, &[]).1, None);
        }
        let button = card.actions[0].1;
        let (click, origin) = pointer_hits(&layout, &[], button.x + 10.0, button.y + 8.0);
        r.m.press(click, (0, 0), origin);
        assert!(r.m.drag_motion((200, 0), (4, 4), r.t0).is_none());
        r.m.release(click, r.t0);
        assert_eq!(
            r.m.stack.selected(),
            vec![id],
            "hidden row still selects on click"
        );
    }

    #[test]
    fn drag_snapshots_selected_group_in_display_order_including_pins() {
        let mut r = rig();
        let old = r.add(1);
        let new = r.add(2);
        r.m.stack.select_all(r.t0);
        r.m.stack.toggle_pin(old, r.t0);
        let (_, items) = r.drag(old);
        assert_eq!(
            items.iter().map(|it| it.id).collect::<Vec<_>>(),
            vec![new.0, old.0]
        );
        assert!(Arc::ptr_eq(
            &items[0].payload.bitmap,
            &r.m.payload(new).unwrap().bitmap
        ));
        assert!(r.m.stack.items().iter().all(|it| it.busy));
        assert_eq!(r.m.drag_status(old), Some(("Preparing drag", false)));
        assert!(r.puts.borrow().is_empty(), "drag preparation never copies");
    }

    #[test]
    fn drag_unselected_origin_replaces_selection_and_busy_group_is_ineligible() {
        let mut r = rig();
        let a = r.add(1);
        let b = r.add(2);
        r.m.stack.toggle_select(a, r.t0);
        let (_, items) = r.drag(b);
        assert_eq!(items.iter().map(|i| i.id).collect::<Vec<_>>(), vec![b.0]);
        assert_eq!(r.m.stack.selected(), vec![b]);
        r.m.cancel_drag(r.t0);
        r.m.stack.select_all(r.t0);
        r.m.saving.push(a);
        r.m.sync_busy(a, r.t0);
        r.m.press(Some(Hit::Card(b)), (0, 0), Some(b));
        assert!(r.m.drag_motion((20, 0), (4, 4), r.t0).is_none());
        r.m.release(Some(Hit::Card(b)), r.t0);
        assert_eq!(r.m.stack.selected(), vec![b, a]);
        assert!(r.m.stack.get(a).unwrap().busy);
        assert!(!r.m.stack.get(b).unwrap().busy);
    }

    #[test]
    fn drag_release_before_ready_keeps_files_but_never_starts_late() {
        let mut r = rig();
        let id = r.add(3);
        let (request, items) = r.drag(id);
        let reply = r.prepared(request, &items);
        let path = reply.saved[0].path.clone();
        r.m.release(Some(Hit::Card(id)), r.t0);
        let saved = r.m.drag_completion(reply, r.t0);
        assert_eq!(saved.len(), 1);
        assert_eq!(r.saved(id), Some(path.clone()));
        assert_eq!(std::fs::read(path).unwrap(), vec![3; 16]);
        assert!(!r.m.stack.get(id).unwrap().busy);
        assert_eq!(r.m.stack.selected(), vec![id], "no trailing toggle");
        assert!(r.m.take_drag(true, r.t0).is_none());
        assert!(r.puts.borrow().is_empty());
    }

    #[test]
    fn drag_ready_requires_current_pointer_and_is_taken_once() {
        for pointer_valid in [false, true] {
            let mut r = rig();
            let id = r.add(1);
            let (request, items) = r.drag(id);
            r.m.drag_completion(r.prepared(request, &items), r.t0);
            r.m.drag_completion(r.prepared(request, &items), r.t0);
            let taken = r.m.take_drag(pointer_valid, r.t0);
            assert_eq!(taken.is_some(), pointer_valid);
            assert!(
                r.m.take_drag(true, r.t0).is_none(),
                "one dispatch per request"
            );
            assert_eq!(r.m.drag.is_dragging(), pointer_valid);
            assert_eq!(r.m.stack.get(id).unwrap().busy, pointer_valid);
            if pointer_valid {
                r.m.drag_completion(r.prepared(request, &items), r.t0);
                assert!(
                    r.m.take_drag(true, r.t0).is_none(),
                    "modal duplicate ignored"
                );
            }
        }
    }

    #[test]
    fn drag_cancel_at_every_phase_preserves_cards_selection_and_remainder() {
        for phase in 0..3 {
            let mut r = rig();
            let id = r.add(1);
            let now = r.t0 + Duration::from_secs(3);
            r.m.press(Some(Hit::Card(id)), (0, 0), Some(id));
            let (request, items) = r.m.drag_motion((20, 0), (4, 4), now).unwrap();
            let remaining = r.m.stack.remaining(id, now).unwrap();
            if phase > 0 {
                r.m.drag_completion(r.prepared(request, &items), now);
            }
            if phase > 1 {
                assert!(r.m.take_drag(true, now).is_some());
            }
            let later = now + Duration::from_secs(30);
            r.m.cancel_drag(later);
            r.m.drag_completion(r.prepared(request, &items), later);
            r.m.finish_drag(request.0, Ok(DragOutcome::DroppedCopy), later);
            r.m.release(Some(Hit::Card(id)), later);
            assert!(r.m.take_drag(true, later).is_none());
            assert!(!r.m.stack.get(id).unwrap().busy);
            assert_eq!(r.m.stack.selected(), vec![id]);
            assert_eq!(r.m.stack.remaining(id, later), Some(remaining));
            r.m.stack.clear_selection(later);
            assert_eq!(
                r.m.stack.remaining(id, later + Duration::from_secs(1)),
                Some(remaining - Duration::from_secs(1))
            );
        }
    }

    #[test]
    fn drag_result_dismisses_only_confirmed_unpinned_copies() {
        for outcome in [
            Ok(DragOutcome::DroppedCopy),
            Ok(DragOutcome::Cancelled),
            Err("target failed".to_owned()),
        ] {
            let mut r = rig();
            let pinned = r.add(1);
            let unpinned = r.add(2);
            r.m.stack.toggle_pin(pinned, r.t0);
            r.m.stack.select_all(r.t0);
            let (request, items) = r.drag(unpinned);
            let reply = r.prepared(request, &items);
            let paths: Vec<_> = reply.saved.iter().map(|p| p.path.clone()).collect();
            r.m.drag_completion(reply, r.t0);
            assert!(r.m.take_drag(true, r.t0).is_some());
            let success = matches!(outcome, Ok(DragOutcome::DroppedCopy));
            r.m.finish_drag(request.0, outcome, r.t0);
            r.m.release(Some(Hit::Card(pinned)), r.t0);
            assert!(r.m.stack.get(pinned).unwrap().selected);
            assert_eq!(r.m.stack.get(unpinned).is_none(), success);
            assert_eq!(r.m.payload(unpinned).is_none(), success);
            assert!(r.m.stack.items().iter().all(|it| !it.busy));
            assert!(paths.iter().all(|p| p.exists()));
            assert!(r.puts.borrow().is_empty());
        }
    }

    #[test]
    fn drag_partial_failure_records_successful_paths_and_failed_card() {
        let mut r = rig();
        let a = r.add(1);
        let b = r.add(2);
        r.m.stack.select_all(r.t0);
        let (request, items) = r.drag(a);
        let mut reply = r.prepared(request, &items);
        let kept_path = reply.saved[0].path.clone();
        reply.saved.pop();
        reply.failed.push((a.0, "disk full".into()));
        reply.transport = None;
        r.m.drag_completion(reply, r.t0);
        assert!(r.m.take_drag(true, r.t0).is_none());
        assert_eq!(r.saved(b), Some(kept_path.clone()));
        assert!(kept_path.exists());
        assert_eq!(r.m.drag_status(a), Some(("drag failed", true)));
        assert!(r
            .m
            .take_errors()
            .iter()
            .any(|e| e.contains(&format!("card {}", a.0)) && e.contains("disk full")));
        assert_eq!(r.m.stack.selected(), vec![b, a]);
        assert!(r.m.stack.items().iter().all(|it| !it.busy));
    }

    #[test]
    fn drag_stale_member_rejects_the_whole_prepared_group() {
        let mut r = rig();
        let a = r.add(1);
        let b = r.add(2);
        r.m.stack.select_all(r.t0);
        let (request, items) = r.drag(a);
        r.m.stack.remove(a, r.t0);
        r.m.forget(a);
        let replacement = r.add(3);
        r.m.drag_completion(r.prepared(request, &items), r.t0);
        assert!(r.m.take_drag(true, r.t0).is_none());
        assert!(!r.m.stack.get(b).unwrap().busy);
        assert!(!r.m.stack.get(replacement).unwrap().busy);
        assert!(r.saved(replacement).is_none());
        assert_eq!(r.m.stack.selected(), vec![b]);
    }

    #[test]
    fn drag_old_reply_cannot_cancel_or_replace_a_new_generation() {
        let mut r = rig();
        let id = r.add(1);
        let (old, old_items) = r.drag(id);
        r.m.cancel_drag(r.t0);
        let (new, new_items) = r.drag(id);
        assert_ne!(old, new);
        r.m.drag_completion(r.prepared(old, &old_items), r.t0);
        r.m.finish_drag(old.0, Err("old failure".into()), r.t0);
        assert_eq!(r.m.drag.request().unwrap().id, new);
        assert!(r.m.stack.get(id).unwrap().busy);
        assert!(r.m.take_drag(true, r.t0).is_none());
        r.m.drag_completion(r.prepared(new, &new_items), r.t0);
        assert_eq!(r.m.take_drag(true, r.t0).unwrap().0, new.0);
    }

    #[test]
    fn drag_escape_preserves_selection_and_blocks_other_shortcuts_until_cancelled() {
        let mut r = rig();
        let id = r.add(1);
        let (_, _) = r.drag(id);
        r.m.stack.set_focus(Some(Focus::Card(id)), r.t0);
        for key in [StackKey::C, StackKey::Delete, StackKey::Space, StackKey::P] {
            r.m.key(key, r.t0);
        }
        assert!(r.puts.borrow().is_empty());
        assert!(r
            .m
            .stack
            .get(id)
            .is_some_and(|it| it.busy && !it.pinned && it.selected));
        r.m.key(StackKey::Escape, r.t0);
        assert_eq!(r.m.stack.selected(), vec![id]);
        assert!(!r.m.stack.get(id).unwrap().busy);
        r.m.key(StackKey::C, r.t0);
        assert_eq!(r.puts.borrow().len(), 1);
    }

    #[test]
    fn drag_quick_escape_cancels_exact_request_even_after_key_up_and_late_reply() {
        let mut r = rig();
        let id = r.add(1);
        let (old, items) = r.drag(id);
        // The hook latches key-down with this generation. By app dispatch time the
        // key can already be up; the event still cancels without a key-state poll.
        assert!(r.m.cancel_drag_request(old.0, r.t0));
        r.m.drag_completion(r.prepared(old, &items), r.t0);
        assert!(r.m.take_drag(true, r.t0).is_none());
        assert!(!r.m.stack.get(id).unwrap().busy);
        assert_eq!(r.m.stack.selected(), vec![id]);
        let (new, items) = r.drag(id);
        assert!(!r.m.cancel_drag_request(old.0, r.t0));
        r.m.drag_completion(r.prepared(new, &items), r.t0);
        assert_eq!(r.m.take_drag(true, r.t0).unwrap().0, new.0);
    }

    #[test]
    fn drag_request_cancellation_covers_ready_and_modal_without_harming_new_requests() {
        for modal in [false, true] {
            let mut r = rig();
            let id = r.add(1);
            let (old, items) = r.drag(id);
            r.m.drag_completion(r.prepared(old, &items), r.t0);
            if modal {
                assert!(r.m.take_drag(true, r.t0).is_some());
            }
            assert!(!r.m.cancel_drag_request(old.0 + 1, r.t0));
            assert!(r.m.stack.get(id).unwrap().busy);
            assert!(r.m.cancel_drag_request(old.0, r.t0));
            assert!(!r.m.cancel_drag_request(old.0, r.t0));
            assert!(!r.m.stack.get(id).unwrap().busy);
            assert!(r.m.drag_ready.is_none());
            assert_eq!(r.m.stack.selected(), vec![id]);

            let (new, items) = r.drag(id);
            r.m.finish_drag(old.0, Ok(DragOutcome::DroppedCopy), r.t0);
            assert!(!r.m.cancel_drag_request(old.0, r.t0));
            assert!(r.m.stack.get(id).unwrap().busy);
            r.m.drag_completion(r.prepared(new, &items), r.t0);
            assert_eq!(r.m.take_drag(true, r.t0).unwrap().0, new.0);
        }
    }

    #[test]
    fn drag_feedback_hides_focused_actions_without_changing_stack_focus_or_badges() {
        let mut r = rig();
        let id = r.add(1);
        r.m.stack.toggle_pin(id, r.t0);
        let focus = Focus::Card(id);
        r.m.stack.set_focus(Some(focus), r.t0);
        let _ = r.drag(id);
        let layout = stack::layout(r.m.stack(), r.t0);
        let parts = layout.cards.iter().map(|c| (c.id, c.clone())).collect();
        let drawn = [MotionCard {
            id,
            rect: layout.cards[0].rect,
            alpha: 1.0,
            scale: 1.0,
            exiting: false,
        }];
        let mut cards = scene_cards(r.m.stack(), &drawn, &parts, r.t0);
        assert!(cards[0].show_actions);
        r.m.drag_feedback(&mut cards);
        assert_eq!(cards[0].status, Some(("Preparing drag", false)));
        assert!(cards[0].busy && cards[0].selected && cards[0].pinned);
        assert!(!cards[0].show_actions);
        assert!(cards[0].focused);
        assert_eq!(r.m.stack.focus(), Some(focus));
        r.m.cancel_drag(r.t0);
        let cards = scene_cards(r.m.stack(), &drawn, &parts, r.t0);
        assert!(cards[0].show_actions);
    }

    #[test]
    fn drag_submission_failure_unsets_busy_and_reports_the_group() {
        let mut r = rig();
        let id = r.add(1);
        let (request, items) = r.drag(id);
        r.m.submit_drag(request, items, r.t0); // No worker connected in this fake model.
        assert!(r.m.drag.request().is_none());
        assert!(!r.m.stack.get(id).unwrap().busy);
        assert_eq!(r.m.drag_status(id), Some(("drag failed", true)));
        assert!(r.m.take_errors()[0].contains("worker is unavailable"));
    }

    #[test]
    fn drag_async_worker_can_prepare_without_touching_the_clipboard() {
        let mut r = rig();
        let id = r.add(4);
        r.m.drag_store = Some(DragStore::new(Box::new(|| {})));
        let (request, items) = r.drag(id);
        r.m.submit_drag(request, items, r.t0);
        r.settle(|m| m.drag_ready.is_some() || m.drag.request().is_none());
        let (actual, transport) = r.m.take_drag(true, r.t0).unwrap();
        assert_eq!(actual, request.0);
        assert_eq!(transport.paths, vec![r.saved(id).unwrap()]);
        assert!(transport.image.is_some());
        assert_eq!(std::fs::read(&transport.paths[0]).unwrap(), vec![4; 16]);
        assert!(r.puts.borrow().is_empty());
        r.m.finish_drag(actual, Ok(DragOutcome::Cancelled), r.t0);
        assert!(transport.paths[0].exists());
    }

    #[test]
    fn keys_map_to_stack_keys() {
        assert_eq!(stack_key(VK_LEFT, false, false), Some(StackKey::Left));
        assert_eq!(stack_key(VK_RIGHT, false, false), Some(StackKey::Right));
        assert_eq!(stack_key(VK_TAB, false, false), Some(StackKey::Tab));
        assert_eq!(stack_key(VK_TAB, false, true), Some(StackKey::ShiftTab));
        assert_eq!(stack_key(VK_SPACE, false, false), Some(StackKey::Space));
        assert_eq!(stack_key(VK_RETURN, false, false), Some(StackKey::Enter));
        assert_eq!(stack_key(VK_ESCAPE, false, false), Some(StackKey::Escape));
        assert_eq!(stack_key(VK_DELETE, false, false), Some(StackKey::Delete));
        // Letters with or without Ctrl; A only with it.
        assert_eq!(stack_key(VK_C, true, false), Some(StackKey::C));
        assert_eq!(stack_key(VK_C, false, false), Some(StackKey::C));
        assert_eq!(stack_key(VK_S, false, false), Some(StackKey::S));
        assert_eq!(stack_key(VK_P, false, false), Some(StackKey::P));
        assert_eq!(stack_key(VK_A, true, false), Some(StackKey::CtrlA));
        assert_eq!(stack_key(VK_A, false, false), None);
        assert_eq!(stack_key(0x5A, true, false), None, "Ctrl+Z is nothing here");
    }

    #[test]
    fn a_new_card_says_how_the_shots_own_copy_and_save_went() {
        assert_eq!(first_status(ThumbStatus::Copied), CardStatus::Copied);
        assert_eq!(
            first_status(ThumbStatus::CopyFailed),
            CardStatus::CopyFailed
        );
        assert_eq!(first_status(ThumbStatus::Saved), CardStatus::Saved);
        assert_eq!(
            first_status(ThumbStatus::SaveFailed),
            CardStatus::SaveFailed
        );
    }

    #[test]
    fn a_finished_shot_becomes_the_newest_card_sharing_its_payload() {
        let mut r = rig();
        let p = payload(1);
        let saved = r.dir.path().join("x.png");
        let id =
            r.m.add(p.clone(), CardStatus::Saved, Some(saved.clone()), r.t0)
                .unwrap();
        assert!(Arc::ptr_eq(&r.m.payload(id).unwrap().bitmap, &p.bitmap));
        assert_eq!(r.status(id), CardStatus::Saved);
        assert_eq!(r.saved(id), Some(saved));
        assert_eq!(r.m.stack().bytes(), p.bytes());
        let newer = r.add(2);
        assert_eq!(r.m.stack().items()[0].id, newer, "newest first");
    }

    #[test]
    fn a_full_stack_of_pinned_cards_keeps_them_and_reports_the_new_shot() {
        let mut r = rig();
        let ids: Vec<_> = (0..5).map(|i| r.add(i)).collect();
        for &id in &ids {
            r.m.card(id, CardAction::Pin, r.t0);
        }
        assert_eq!(r.m.add(payload(9), CardStatus::Copied, None, r.t0), None);
        assert_eq!(r.m.stack().len(), 5);
        assert!(ids.iter().all(|&id| r.m.payload(id).is_some()));
        assert_eq!(r.m.stack().notice(r.t0), Some(stack::NOTICE_TEXT));
        assert_eq!(r.m.take_errors().len(), 1);
        // Unpinned, the oldest makes room and its payload goes.
        r.m.card(ids[0], CardAction::Pin, r.t0);
        let id = r.m.add(payload(9), CardStatus::Copied, None, r.t0).unwrap();
        assert!(r.m.payload(ids[0]).is_none());
        assert!(r.m.payload(id).is_some());
    }

    #[test]
    fn copy_puts_the_cards_png_and_dib_on_the_clipboard() {
        let mut r = rig();
        let id = r.add(7);
        r.m.stack_mut().set_status(id, CardStatus::Ready);
        r.m.click(Hit::Action(id, CardAction::Copy), r.t0);
        assert_eq!(r.status(id), CardStatus::Copied);
        let puts = r.puts.borrow();
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0].0, vec![7; 16]);
        assert!(!puts[0].1.is_empty(), "with a DIB");
        drop(puts);
        // A failure stays on the card and goes to the log.
        *r.fail.borrow_mut() = true;
        r.m.click(Hit::Action(id, CardAction::Copy), r.t0);
        assert_eq!(r.status(id), CardStatus::CopyFailed);
        assert_eq!(r.m.take_errors().len(), 1);
    }

    #[test]
    fn a_card_without_a_png_copies_once_the_worker_encoded_it() {
        let mut r = rig();
        let p = Payload {
            bitmap: Arc::new(Bitmap::black(3, 3)),
            png: None,
        };
        let id = r.m.add(p, CardStatus::CopyFailed, None, r.t0).unwrap();
        r.m.card(id, CardAction::Copy, r.t0);
        assert!(r.m.stack().get(id).unwrap().busy, "busy while encoding");
        assert!(r.puts.borrow().is_empty());
        r.settle(|m| !m.stack().get(id).unwrap().busy);
        assert_eq!(r.status(id), CardStatus::Copied);
        assert_eq!(r.puts.borrow().len(), 1);
        assert!(r.puts.borrow()[0].0.starts_with(b"\x89PNG"));
    }

    #[test]
    fn save_writes_a_new_file_then_the_button_opens_it() {
        let mut r = rig();
        let id = r.add(3);
        r.m.click(Hit::Action(id, CardAction::Save), r.t0);
        assert_eq!(r.status(id), CardStatus::Saving);
        assert!(r.m.stack().get(id).unwrap().busy);
        r.settle(|m| !m.stack().get(id).unwrap().busy);
        assert_eq!(r.status(id), CardStatus::Saved);
        let path = r.saved(id).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), vec![3; 16]);
        assert!(path.ends_with("snap-2026-10-26_140509.png"));
        // Now it reads "open": Explorer shows the same file, nothing new is written.
        r.m.click(Hit::Action(id, CardAction::Save), r.t0);
        assert_eq!(revealed(), vec![path.clone()]);
        assert_eq!(r.files(), vec![path]);
    }

    #[test]
    fn a_saved_file_that_went_missing_is_reported_and_can_be_saved_again() {
        let mut r = rig();
        let id = r.add(4);
        r.m.card(id, CardAction::Save, r.t0);
        r.settle(|m| !m.stack().get(id).unwrap().busy);
        let path = r.saved(id).unwrap();
        std::fs::remove_file(&path).unwrap();
        r.m.card(id, CardAction::Save, r.t0);
        assert_eq!(r.status(id), CardStatus::Missing);
        assert_eq!(r.saved(id), None);
        assert_eq!(r.m.take_errors().len(), 1);
        r.m.card(id, CardAction::Save, r.t0);
        r.settle(|m| !m.stack().get(id).unwrap().busy);
        assert_eq!(r.status(id), CardStatus::Saved);
        assert!(r.saved(id).unwrap().exists());
    }

    #[test]
    fn save_selected_keeps_the_cards_and_skips_ones_already_saved() {
        let mut r = rig();
        let a = r.add(1);
        let b = r.add(2);
        let c = r.add(3);
        r.m.card(a, CardAction::Save, r.t0);
        r.settle(|m| !m.stack().get(a).unwrap().busy);
        let first = r.saved(a).unwrap();
        for id in [a, b] {
            r.m.click(Hit::Card(id), r.t0);
        }
        r.m.click(Hit::Bar(BarAction::SaveSelected), r.t0);
        r.settle(|m| !m.stack().get(b).unwrap().busy);
        assert_eq!(r.saved(a), Some(first), "already saved: kept as it was");
        assert_eq!(r.status(b), CardStatus::Saved);
        assert_eq!(r.saved(c), None, "not selected");
        assert_eq!(r.files().len(), 2, "no-overwrite names, one file each");
        assert_eq!(r.m.stack().selected(), vec![b, a], "the selection stays");
        assert_eq!(r.m.stack().len(), 3);
    }

    #[test]
    fn a_failed_save_stays_on_its_card_and_the_others_still_save() {
        let mut r = rig();
        // A file where the folder should be: every save fails.
        std::fs::write(r.dir.path().join("shots"), b"in the way").unwrap();
        let a = r.add(1);
        r.m.card(a, CardAction::Save, r.t0);
        r.settle(|m| !m.stack().get(a).unwrap().busy);
        assert_eq!(r.status(a), CardStatus::SaveFailed);
        assert_eq!(r.saved(a), None);
        assert_eq!(r.m.take_errors().len(), 1);
        assert_eq!(r.m.stack().len(), 1, "never dismissed by a failure");
    }

    #[test]
    fn dismiss_and_expiry_free_the_payload_but_leave_files_and_clipboard_alone() {
        let mut r = rig();
        let a = r.add(1);
        let b = r.add(2);
        r.m.card(a, CardAction::Save, r.t0);
        r.settle(|m| !m.stack().get(a).unwrap().busy);
        r.m.card(a, CardAction::Copy, r.t0);
        let (files, puts) = (r.files(), r.puts.borrow().len());
        r.m.click(Hit::Action(a, CardAction::Dismiss), r.t0);
        assert!(r.m.stack().get(a).is_none());
        assert!(r.m.payload(a).is_none());
        // b runs out (a pinned card would not).
        let gone = r.m.tick(r.t0 + LIFE + Duration::from_millis(1));
        assert_eq!(gone, vec![b]);
        assert!(r.m.payload(b).is_none());
        assert!(r.m.stack().is_empty());
        assert_eq!(r.files(), files);
        assert_eq!(r.puts.borrow().len(), puts);
    }

    #[test]
    fn dismiss_selected_releases_only_the_selection() {
        let mut r = rig();
        let a = r.add(1);
        let b = r.add(2);
        let c = r.add(3);
        r.m.card(c, CardAction::Pin, r.t0);
        r.m.bar(BarAction::SelectAll, r.t0);
        r.m.click(Hit::Card(b), r.t0);
        r.m.bar(BarAction::DismissSelected, r.t0);
        let left: Vec<_> = r.m.stack().items().iter().map(|it| it.id).collect();
        assert_eq!(left, vec![b]);
        assert!(r.m.payload(a).is_none() && r.m.payload(c).is_none());
    }

    #[test]
    fn a_reply_for_a_dismissed_card_is_dropped() {
        let mut r = rig();
        let a = r.add(1);
        r.m.card(a, CardAction::Save, r.t0);
        r.m.card(a, CardAction::Dismiss, r.t0);
        let b = r.add(2);
        // The explicit save still lands on disk; nothing lands on another card.
        for _ in 0..500 {
            r.m.drain(r.t0);
            if !r.files().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        r.m.drain(r.t0);
        assert_eq!(r.files().len(), 1);
        assert_eq!(r.status(b), CardStatus::Copied);
        assert_eq!(r.saved(b), None);
        assert!(r.m.take_errors().is_empty());
    }

    #[test]
    fn the_region_hugs_the_cards_and_keeps_the_gaps_click_through() {
        let mut r = rig();
        let a = r.add(1);
        let b = r.add(2);
        let st = r.m.stack();
        let layout = stack::layout(st, r.t0);
        let rects = region(st, &layout, &[], 1.0);
        assert_eq!(rects.len(), 2);
        let (left, right) = (layout.cards[0].rect, layout.cards[1].rect);
        assert_eq!(
            rects[0],
            (
                left.x as i32 - 2,
                left.y as i32 - 2,
                (left.x + left.w) as i32 + 2,
                (left.y + left.h) as i32 + 2
            )
        );
        // The middle of the 10 px gap is in no rect.
        let gap = ((left.x + left.w + right.x) / 2.0) as i32;
        assert!(rects.iter().all(|&(l, _, r, _)| gap < l || gap >= r));
        // A selected card adds its outline, and the bar comes in.
        r.m.click(Hit::Card(b), r.t0);
        let st = r.m.stack();
        let layout = stack::layout(st, r.t0);
        let rects = region(st, &layout, &[], 1.5);
        let sel = layout.cards.iter().find(|c| c.id == b).unwrap().rect;
        assert!(rects.contains(&(
            ((sel.x - 5.0) * 1.5).floor() as i32,
            ((sel.y - 5.0) * 1.5).floor() as i32,
            ((sel.x + sel.w + 5.0) * 1.5).ceil() as i32,
            ((sel.y + sel.h + 5.0) * 1.5).ceil() as i32
        )));
        assert_eq!(rects.len(), 3, "two cards and the bar");
        // Where a card is drawn while it moves counts too.
        let moving = MotionCard {
            id: a,
            rect: Rect::new(10.0, -14.0, 124.0, 80.0),
            alpha: 0.5,
            scale: 0.8,
            exiting: true,
        };
        let rects = region(st, &layout, &[moving], 1.0);
        assert!(rects.contains(&(8, -16, 136, 68)));
    }

    #[test]
    fn scene_cards_carry_state_labels_and_card_relative_parts() {
        let mut r = rig();
        let a = r.add(1);
        let b = r.add(2);
        r.m.card(a, CardAction::Pin, r.t0);
        r.m.stack_mut()
            .set_saved(b, Some(r.dir.path().join("b.png")));
        r.m.stack_mut().set_hover(Some(b), r.t0);
        let layout = stack::layout(r.m.stack(), r.t0);
        let parts: HashMap<_, _> = layout.cards.iter().map(|c| (c.id, c.clone())).collect();
        let drawn: Vec<_> = layout
            .cards
            .iter()
            .map(|c| MotionCard {
                id: c.id,
                rect: c.rect,
                alpha: 1.0,
                scale: 1.0,
                exiting: false,
            })
            .collect();
        let cards = scene_cards(r.m.stack(), &drawn, &parts, r.t0);
        let card = |id: ItemId| cards.iter().find(|c| c.id == id.0).unwrap();
        let (pa, pb) = (card(a), card(b));
        assert!(pa.pinned && pa.remaining.is_none() && pa.actions[2].label == "unpin");
        assert!(pb.hovered && pb.show_actions && pb.show_check);
        assert_eq!(pb.actions[1].label, "open");
        assert_eq!(pb.status, Some(("copied", false)));
        assert!(!pa.show_actions, "only the hovered card shows its buttons");
        assert_eq!((pb.check.x, pb.check.y), (6.0, 6.0), "relative to the card");
        assert_eq!(
            pb.remaining,
            Some(1.0),
            "hovered: paused, the ring still shows"
        );
        // A ghost is drawn plain.
        let ghost = MotionCard {
            exiting: true,
            ..drawn[0]
        };
        let cards = scene_cards(r.m.stack(), &[ghost], &parts, r.t0);
        assert!(cards[0].status.is_none() && !cards[0].show_actions && !cards[0].pinned);
    }

    #[test]
    fn the_thumbs_save_marks_the_card_showing_the_same_image() {
        let mut r = rig();
        let shot = payload(1);
        let same =
            r.m.add(shot.clone(), CardStatus::Copied, None, r.t0)
                .unwrap();
        let other = r.add(2);
        let path = r.dir.path().join("thumb.png");
        r.m.mark_saved(&shot.bitmap, &path);
        assert_eq!(r.saved(same), Some(path));
        assert_eq!(r.status(same), CardStatus::Saved);
        assert_eq!(r.saved(other), None);
        assert_eq!(r.status(other), CardStatus::Copied);
    }

    #[test]
    fn a_cards_save_is_reported_for_the_thumb() {
        let mut r = rig();
        let shot = payload(5);
        let id =
            r.m.add(shot.clone(), CardStatus::Copied, None, r.t0)
                .unwrap();
        r.m.card(id, CardAction::Save, r.t0);
        let mut reported = Vec::new();
        for _ in 0..500 {
            reported.extend(r.m.drain(r.t0));
            if !reported.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(reported.len(), 1);
        assert!(Arc::ptr_eq(&reported[0].0, &shot.bitmap));
        assert_eq!(Some(reported[0].1.clone()), r.saved(id));
    }

    #[test]
    fn a_card_stays_busy_until_its_save_and_its_copy_are_both_done() {
        let mut r = rig();
        let id = r.add(1);
        r.m.saving.push(id);
        r.m.copying.push(id);
        r.m.sync_busy(id, r.t0);
        assert!(r.m.stack().get(id).unwrap().busy);
        r.m.saving.clear();
        r.m.sync_busy(id, r.t0);
        assert!(r.m.stack().get(id).unwrap().busy, "the copy is still out");
        r.m.copying.clear();
        r.m.sync_busy(id, r.t0);
        assert!(!r.m.stack().get(id).unwrap().busy);
    }

    #[test]
    fn the_notice_gets_room_for_its_smaller_corners() {
        let mut r = rig();
        for i in 0..5 {
            let id = r.add(i);
            r.m.card(id, CardAction::Pin, r.t0);
        }
        assert_eq!(r.m.add(payload(9), CardStatus::Copied, None, r.t0), None);
        let layout = stack::layout(r.m.stack(), r.t0);
        let n = layout.notice.unwrap();
        let rects = region(r.m.stack(), &layout, &[], 1.0);
        assert!(rects.contains(&(
            n.x as i32 - 3,
            n.y as i32 - 3,
            (n.x + n.w) as i32 + 3,
            (n.y + n.h) as i32 + 3
        )));
    }
}
