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
use dj_core::layout::Rect;
use dj_core::snap::{Stamp, ThumbStatus};
use dj_core::snap_stack::{
    self as stack, Admit, BarAction, CardAction, CardLayout, CardStatus, Focus, Hit, Item, ItemId,
    KeyCmd, Motion, MotionCard, Stack, StackKey, StackLayout,
};
use dj_core::theme::ResolvedTokens;
use dj_render::{
    PreviewBar, PreviewButton, PreviewCard, PreviewScene, PreviewSurface, RenderError,
};
use dj_win::previews::{self as win, PreviewInput};
use dj_win::{window, HWND};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The countdown tick (and the notice's one-shot). App timer ids 1-8 are taken.
pub const TIMER_SNAP_STACK: usize = 9;
/// 10 Hz: the most the ring is redrawn while a countdown runs.
const TICK_MS: u32 = 100;
/// Input region around a card, logical px: the hairline and anti-aliasing. A selected or
/// focused card adds its outline (2 px wide, 2 px off the card).
const REGION_PAD: f32 = 2.0;
const REGION_PAD_OUTLINE: f32 = 5.0;

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
    /// Copies waiting for the worker to encode their PNG.
    copying: Vec<ItemId>,
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
        match hit {
            Hit::Card(id) => self.stack.toggle_select(id, now),
            Hit::Action(id, action) => self.card(id, action, now),
            Hit::Bar(action) => self.bar(action, now),
        }
    }

    pub fn card(&mut self, id: ItemId, action: CardAction, now: Instant) {
        if self.stack.get(id).is_none() {
            return;
        }
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
                self.stack.set_busy(id, true, now);
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
        if self.stack.get(id).is_some_and(|it| it.busy) {
            return;
        }
        // A file that went missing is saved again under a new name.
        self.stack.set_saved(id, None);
        if self.store.save(id.0, self.save_dir.clone(), (self.stamp)()) {
            self.stack.set_status(id, CardStatus::Saving);
            self.stack.set_busy(id, true, now);
        } else {
            self.stack.set_status(id, CardStatus::SaveFailed);
            self.errors
                .push("snap stack: save failed: the save queue is full".into());
        }
    }

    /// Applies the worker's replies. Replies for cards that are gone are dropped.
    pub fn drain(&mut self, now: Instant) {
        for reply in self.store.drain() {
            let id = ItemId(reply.id);
            if self.stack.get(id).is_none() {
                continue;
            }
            match reply.kind {
                ReplyKind::Saved(path) => {
                    self.stack.set_busy(id, false, now);
                    self.stack.set_saved(id, Some(path));
                    self.stack.set_status(id, CardStatus::Saved);
                }
                ReplyKind::SaveFailed(e) => {
                    self.stack.set_busy(id, false, now);
                    self.stack.set_status(id, CardStatus::SaveFailed);
                    self.errors.push(format!("snap stack: save failed: {e}"));
                }
                ReplyKind::Encoded => {
                    if self.take_copying(id) {
                        self.stack.set_busy(id, false, now);
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
                        self.stack.set_busy(id, false, now);
                        self.stack.set_status(id, CardStatus::CopyFailed);
                        self.errors.push(format!("snap stack: copy failed: {e}"));
                    }
                }
            }
        }
    }

    fn take_copying(&mut self, id: ItemId) -> bool {
        let before = self.copying.len();
        self.copying.retain(|c| *c != id);
        self.copying.len() != before
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
    out.extend(layout.notice.iter().map(|&r| region_rect(r, 0.0, scale)));
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
    /// Where the left button went down: a click is a press and a release on the same.
    pressed: Option<Hit>,
    /// Each card's last layout, so a fading card keeps its parts.
    parts: HashMap<ItemId, CardLayout>,
    /// Draw a frame even though nothing moves.
    dirty: bool,
    timer_armed: bool,
    /// The GPU device was lost: make the surface again on the next sync.
    rebuild: bool,
}

impl Previews {
    pub fn new(model: Model, owner: HWND, tokens: &ResolvedTokens) -> Self {
        Self {
            model,
            motion: Motion::new(),
            surface: None,
            owner,
            anchor: (0, 0),
            tokens: tokens.clone(),
            allowed: true,
            pressed: None,
            parts: HashMap::new(),
            dirty: false,
            timer_armed: false,
            rebuild: false,
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
        let id = self.model.add(payload, status, saved, now);
        self.log_errors();
        if self.surface.is_none() {
            // Uploads every card's thumbnail, this one's included.
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
        if !gpu.set_image(id.0, &p.bitmap, s.scale) {
            crate::storage::log("snap stack: cannot make a card's thumbnail");
        }
    }

    /// The island moved: the stack follows its lower edge.
    pub fn set_anchor(&mut self, anchor: (i32, i32), now: Instant) {
        if anchor != self.anchor {
            self.anchor = anchor;
            self.place(now);
        }
    }

    /// The island was shown or hidden, or a capture ended.
    pub fn set_allowed(&mut self, allowed: bool, now: Instant) {
        self.allowed = allowed;
        self.sync(now);
    }

    /// Before a freeze: hidden, and gone from the composed desktop when this returns.
    pub fn hide_for_capture(&mut self, now: Instant) {
        let was_shown = self.shown();
        self.allowed = false;
        self.sync(now);
        if was_shown {
            win::flush_composition();
        }
    }

    /// DPI or displays changed: the window and the thumbnails are made again at the new
    /// scale.
    pub fn relayout(&mut self, now: Instant) {
        self.destroy_surface();
        if self.has_content(now) {
            self.ensure_surface();
            self.dirty = true;
        }
        self.sync(now);
    }

    pub fn shutdown(&mut self) {
        if self.timer_armed {
            window::kill_timer(self.owner, TIMER_SNAP_STACK);
            self.timer_armed = false;
        }
        self.destroy_surface();
    }

    /// Makes the window and its surface (hidden) if there are none, with every card's
    /// thumbnail.
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
                return;
            }
        };
        let mut gpu = match PreviewSurface::new(hwnd.0 as isize, rect.2 as u32, rect.3 as u32) {
            Ok(g) => g,
            Err(e) => {
                crate::storage::log(&format!("snap stack: renderer: {e}"));
                win::destroy(hwnd);
                return;
            }
        };
        for it in self.model.stack().items() {
            if let Some(p) = self.model.payload(it.id) {
                if !gpu.set_image(it.id.0, &p.bitmap, scale) {
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
    }

    fn destroy_surface(&mut self) {
        if let Some(mut s) = self.surface.take() {
            dj_win::system::set_topmost_companion(None);
            s.gpu = None;
            win::destroy(s.hwnd);
        }
        self.pressed = None;
    }

    fn place(&mut self, now: Instant) {
        let Some((old_scale, old_rect)) = self.surface.as_ref().map(|s| (s.scale, s.rect)) else {
            return;
        };
        let (ax, ay) = self.anchor;
        let scale = win::dpi_at(ax, ay) as f32 / 96.0;
        if (scale - old_scale).abs() > f32::EPSILON {
            // The island is on a monitor with another DPI now.
            self.relayout(now);
            return;
        }
        let rect = stack::place(ax, ay, scale, win::work_area_at(ax, ay));
        if let Some(s) = self.surface.as_mut().filter(|_| rect != old_rect) {
            s.rect = rect;
            win::place(s.hwnd, rect.0, rect.1, rect.2, rect.3);
        }
    }

    /// Shows or hides the window for the current state (drawing before it shows, so a
    /// stale frame never flashes), keeps the countdowns paused while it is hidden, frees
    /// everything once nothing is left to show, and arms the timer.
    fn sync(&mut self, now: Instant) {
        let content = self.has_content(now);
        if !content {
            self.destroy_surface();
        } else if self.rebuild && self.allowed {
            self.rebuild = false;
            self.ensure_surface();
            self.dirty = true;
        }
        let want = self.allowed && content && self.surface.is_some();
        let shown = self.shown();
        if want && !shown {
            self.model.stack_mut().set_hidden(false, now);
            self.draw(now, 0.0);
            if let Some(s) = self.surface.as_mut() {
                win::show(s.hwnd);
                s.shown = true;
            }
        } else if !want && shown {
            if let Some(s) = self.surface.as_mut() {
                win::hide(s.hwnd);
                s.shown = false;
            }
            self.pressed = None;
            let stack = self.model.stack_mut();
            stack.set_hover(None, now);
            stack.set_focus(None, now);
        }
        if !want {
            self.model.stack_mut().set_hidden(true, now);
        }
        self.sync_timer(now);
    }

    /// 10 Hz while a visible countdown runs; otherwise a one-shot to the next change
    /// (the notice going away); otherwise nothing.
    fn sync_timer(&mut self, now: Instant) {
        let stack = self.model.stack();
        let ms = if !self.shown() {
            None
        } else if stack.any_running() {
            Some(TICK_MS)
        } else {
            stack
                .next_change(now)
                .map(|d| (d.as_millis() as u32).saturating_add(5).max(1))
        };
        match ms {
            Some(ms) => {
                window::set_timer(self.owner, TIMER_SNAP_STACK, ms);
                self.timer_armed = true;
            }
            None if self.timer_armed => {
                window::kill_timer(self.owner, TIMER_SNAP_STACK);
                self.timer_armed = false;
            }
            None => {}
        }
    }

    /// `TIMER_SNAP_STACK`: expire cards and redraw the rings.
    pub fn timer(&mut self, now: Instant) {
        window::kill_timer(self.owner, TIMER_SNAP_STACK);
        self.timer_armed = false;
        self.model.tick(now);
        self.dirty = true;
        self.sync(now);
    }

    /// `Event::Wake`: the worker may have results.
    pub fn drain(&mut self, now: Instant) {
        self.model.drain(now);
        self.log_errors();
        self.dirty = true;
        self.sync(now);
    }

    /// The tray's "Focus Snap previews": the keyboard goes to the newest card.
    pub fn focus_keyboard(&mut self, now: Instant) {
        let Some(hwnd) = self.surface.as_ref().filter(|s| s.shown).map(|s| s.hwnd) else {
            return;
        };
        let Some(first) = self.model.stack().items().first().map(|it| it.id) else {
            return;
        };
        self.model
            .stack_mut()
            .set_focus(Some(Focus::Card(first)), now);
        win::focus(hwnd);
        self.dirty = true;
        self.sync(now);
    }

    fn hit_at(&self, x: i32, y: i32, now: Instant) -> Option<Hit> {
        let scale = self.surface.as_ref().map_or(1.0, |s| s.scale);
        let layout = stack::layout(self.model.stack(), now);
        stack::hit(&layout, x as f32 / scale, y as f32 / scale)
    }

    pub fn input(&mut self, input: PreviewInput, now: Instant) {
        if !self.shown() {
            return;
        }
        match input {
            PreviewInput::Move { x, y } => {
                let hover = match self.hit_at(x, y, now) {
                    Some(Hit::Card(id) | Hit::Action(id, _)) => Some(id),
                    _ => None,
                };
                self.model.stack_mut().set_hover(hover, now);
            }
            PreviewInput::Press { x, y } => self.pressed = self.hit_at(x, y, now),
            PreviewInput::Release { x, y } => {
                let hit = self.hit_at(x, y, now);
                if let Some(h) = self.pressed.take().filter(|p| Some(*p) == hit) {
                    self.model.click(h, now);
                }
            }
            PreviewInput::Leave => {
                self.pressed = None;
                self.model.stack_mut().set_hover(None, now);
            }
            PreviewInput::Key { vk, ctrl, shift } => {
                let Some(key) = stack_key(vk, ctrl, shift) else {
                    return;
                };
                match self.model.stack_mut().key(key, now) {
                    // Release: the stack's focus is gone; the window keeps the keyboard
                    // until the user clicks elsewhere.
                    KeyCmd::None | KeyCmd::Redraw | KeyCmd::Release => {}
                    KeyCmd::Card(id, action) => self.model.card(id, action, now),
                    KeyCmd::ToggleSelect(id) => self.model.stack_mut().toggle_select(id, now),
                    KeyCmd::Bar(action) => self.model.bar(action, now),
                }
            }
            PreviewInput::FocusLost => self.model.stack_mut().set_focus(None, now),
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
        let finished = self.motion.take_finished();
        if let Some(gpu) = self.surface.as_mut().and_then(|s| s.gpu.as_mut()) {
            for id in &finished {
                gpu.drop_image(id.0);
            }
        }
        for id in &finished {
            self.parts.remove(id);
        }
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
        win::set_regions(
            s.hwnd,
            &region(st, &layout, &drawn, s.scale),
            (stack::CARD_R * s.scale).round() as i32,
        );
        let cards = scene_cards(st, &drawn, &self.parts, now);
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
            Ok(()) => {}
            Err(RenderError::DeviceLost) => {
                crate::storage::log("snap stack: GPU device lost; rebuilding");
                self.destroy_surface();
                self.rebuild = true;
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
}
