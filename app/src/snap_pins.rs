//! Session-owned screenshot references. The store retains original pixels independently
//! of the preview stack; each entry owns its drawing resources before its native HWND.
//! Native calls are behind a small adapter so controller tests never open windows.

use crate::preview_store::{CopyStart, Payload, Reply, ReplyKind, RevealError, Store};
use crate::snap::{Clipboard, SystemClipboard};
use dj_core::snap::{Bitmap, IRect, Stamp};
use dj_core::snap_pin::{
    self as pin, Admission, Allocation, Hit, HoldId, Layout, PinAction, PinId, SourceId,
};
use dj_core::theme::ResolvedTokens;
use dj_render::{PinScene, PinSurface};
use dj_win::snap_pins::{self as win, PinInput};
use dj_win::{window, HWND};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_RETRIES: u8 = 3;
const ACTIONS: [PinAction; 3] = [PinAction::Copy, PinAction::Save, PinAction::Close];

/// All native/GPU effects live in the production adapter, outside controller policy.
trait Platform {
    type Window;
    type Surface;
    type Focus: Copy;

    fn create_window(&mut self, id: PinId, bounds: IRect) -> Result<Self::Window, String>;
    fn create_surface(
        &mut self,
        window: &Self::Window,
        size: (u32, u32),
        bitmap: &Bitmap,
        scale: f32,
    ) -> Result<Self::Surface, String>;
    fn render(surface: &mut Self::Surface, scene: &PinScene<'_>) -> Result<(), String>;
    fn hwnd(window: &Self::Window) -> isize;
    fn show(window: &Self::Window);
    fn hide(window: &Self::Window);
    fn place(window: &Self::Window, bounds: IRect);
    fn capture(window: &Self::Window) -> bool;
    fn release(window: &Self::Window);
    fn foreground(window: &Self::Window) -> bool;
    fn focus(window: &Self::Window) -> Option<Self::Focus>;
    fn target_is_window(target: Self::Focus, window: &Self::Window) -> bool;
    fn restore(target: Self::Focus);
    fn work_at(&self, point: (i32, i32)) -> (IRect, u32);
    fn work_for(&self, window: &Self::Window) -> (IRect, u32);
}

struct Native;

fn tuple(r: IRect) -> (i32, i32, i32, i32) {
    (r.x, r.y, r.w, r.h)
}

fn rect(r: (i32, i32, i32, i32)) -> IRect {
    IRect::new(r.0, r.1, r.2, r.3)
}

impl Platform for Native {
    type Window = win::Window;
    type Surface = PinSurface;
    type Focus = win::FocusTarget;

    fn create_window(&mut self, id: PinId, bounds: IRect) -> Result<Self::Window, String> {
        win::Window::new(id.0, tuple(bounds))
    }

    fn create_surface(
        &mut self,
        window: &Self::Window,
        size: (u32, u32),
        bitmap: &Bitmap,
        scale: f32,
    ) -> Result<Self::Surface, String> {
        let mut surface =
            PinSurface::new(Self::hwnd(window), size.0, size.1).map_err(|e| e.to_string())?;
        if !surface.set_image(bitmap, scale) {
            return Err("could not prepare the pinned image".into());
        }
        Ok(surface)
    }

    fn render(surface: &mut Self::Surface, scene: &PinScene<'_>) -> Result<(), String> {
        surface.render(scene).map_err(|e| e.to_string())
    }

    fn hwnd(window: &Self::Window) -> isize {
        window.hwnd().0 as isize
    }
    fn show(window: &Self::Window) {
        window.show();
    }
    fn hide(window: &Self::Window) {
        window.hide();
    }
    fn place(window: &Self::Window, bounds: IRect) {
        window.place(tuple(bounds));
    }
    fn capture(window: &Self::Window) -> bool {
        window.capture()
    }
    fn release(window: &Self::Window) {
        window.release_capture();
    }
    fn foreground(window: &Self::Window) -> bool {
        window.has_foreground()
    }
    fn focus(window: &Self::Window) -> Option<Self::Focus> {
        window.focus()
    }
    fn restore(target: Self::Focus) {
        win::restore_focus(target);
    }
    fn target_is_window(target: Self::Focus, window: &Self::Window) -> bool {
        target.is_window(window.hwnd())
    }
    fn work_at(&self, point: (i32, i32)) -> (IRect, u32) {
        let (bounds, dpi) = win::work_area_at(point);
        (rect(bounds), dpi)
    }
    fn work_for(&self, window: &Self::Window) -> (IRect, u32) {
        let (bounds, dpi) = win::work_area_for(window.hwnd());
        (rect(bounds), dpi)
    }
}

#[derive(Clone, Copy)]
struct Drag {
    screen: (i32, i32),
    origin: IRect,
}

struct Entry<P: Platform> {
    // Rust drops fields in declaration order: never destroy the HWND before the GPU.
    surface: Option<P::Surface>,
    window: P::Window,
    layout: Layout,
    scale: f32,
    bounds: IRect,
    shown: bool,
    dirty: bool,
    retries: u8,
    hover: Option<PinAction>,
    focus: Option<PinAction>,
    previous: Option<P::Focus>,
    pressed: Option<Hit>,
    /// `Platform::capture` acknowledged the live native press behind `pressed`/`drag`.
    /// Only then may the controller end that press itself.
    captured: bool,
    drag: Option<Drag>,
    copying: bool,
    saving: bool,
    saved: Option<PathBuf>,
    status: Option<(String, bool)>,
}

impl<P: Platform> Entry<P> {
    fn drag_to(&mut self, screen: (i32, i32)) {
        if let Some(drag) = self.drag {
            self.bounds.x = drag
                .origin
                .x
                .saturating_add(screen.0.saturating_sub(drag.screen.0));
            self.bounds.y = drag
                .origin
                .y
                .saturating_add(screen.1.saturating_sub(drag.screen.1));
            self.drag = Some(Drag {
                screen,
                origin: self.bounds,
            });
            P::place(&self.window, self.bounds);
        }
    }

    /// Forget pointer state without touching the native press. The native layer ends
    /// its own press before it emits Up/CancelCapture/FocusLost, so those paths must
    /// not release: a newer press may already be live behind a still-queued Down.
    fn reset_pointer(&mut self) {
        self.pressed = None;
        self.captured = false;
        self.drag = None;
        self.hover = None;
        self.dirty = true;
    }

    /// Controller-initiated cancel (hide, relayout, close). It ends the native press
    /// only when this controller acknowledged it, so a newer press it has not seen
    /// yet survives.
    fn cancel_pointer(&mut self) {
        if self.captured {
            P::release(&self.window);
        }
        self.reset_pointer();
    }

    fn retry(&mut self) {
        self.retries = 0;
        self.dirty = true;
    }
}

/// A queued Store job that keeps its payload Arcs alive until its reply is drained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Op {
    /// An `Encode` job started by Copy.
    Copy,
    Save,
}

struct Controller<P: Platform> {
    platform: P,
    state: pin::Pins,
    entries: HashMap<PinId, Entry<P>>,
    /// Retained IO ownership, kept outside `entries` so closing a pin keeps its holds
    /// until the worker's reply has dropped the job's Arcs. One worker replies in
    /// submission order, so each key releases its oldest hold first.
    holds: HashMap<(PinId, Op), Vec<HoldId>>,
    store: Store,
    clipboard: Box<dyn Clipboard>,
    reveal: fn(&Path) -> Result<(), RevealError>,
    stamp: fn() -> Stamp,
    tokens: ResolvedTokens,
    save_dir: PathBuf,
    allowed: bool,
    capturing: bool,
}

/// App-facing controller. Pin IDs and queued operations never depend on stack IDs.
pub struct Pins {
    inner: Controller<Native>,
}

impl Pins {
    pub fn memory_counters(&self) -> dj_core::dev_memory::Row {
        dj_core::dev_memory::Row::resource("Pinned screenshots", format!("{} exact data", dj_core::dev_memory::bytes(Some(self.inner.state.bytes()))), format!("{} floating image references; unique payload ledger includes operation holds and shares data with stack.", self.inner.entries.len()))
    }

    pub fn new(owner: HWND, tokens: &ResolvedTokens, save_dir: PathBuf) -> Self {
        let raw = owner.0 as isize;
        Self {
            inner: Controller::new(
                Native,
                Store::new(Box::new(move || window::wake(raw))),
                Box::new(SystemClipboard { owner: raw }),
                tokens,
                save_dir,
            ),
        }
    }

    pub fn open(
        &mut self,
        payload: Payload,
        saved: Option<PathBuf>,
        anchor: (i32, i32),
    ) -> Result<PinId, String> {
        self.inner.open(payload, saved, anchor)
    }
    pub fn input(&mut self, id: u64, input: PinInput) {
        self.inner.input(PinId(id), input);
    }
    pub fn set_tokens(&mut self, tokens: &ResolvedTokens) {
        self.inner.tokens = tokens.clone();
        for entry in self.inner.entries.values_mut() {
            entry.retry();
        }
    }
    pub fn set_save_dir(&mut self, dir: PathBuf) {
        self.inner.save_dir = dir;
    }
    pub fn set_allowed(&mut self, allowed: bool) {
        self.inner.allowed = allowed;
        self.inner.state.set_allowed(allowed);
        self.inner.sync_visibility();
    }
    pub fn begin_capture(&mut self) {
        self.inner.capturing = true;
        self.inner.state.begin_capture();
        self.inner.sync_visibility();
    }
    pub fn end_capture(&mut self) {
        self.inner.capturing = false;
        self.inner.state.end_capture();
        self.inner.sync_visibility();
    }
    pub fn display_changed(&mut self) {
        self.inner.display_changed();
    }
    pub fn show_keyboard(&mut self, previous: Option<win::FocusTarget>) {
        self.inner.show_keyboard(previous);
    }
    pub fn can_focus(&self) -> bool {
        self.inner.can_focus()
    }
    pub fn keyboard_owner(&self) -> Option<PinId> {
        self.inner.keyboard_owner()
    }
    pub fn refocus(&mut self, id: PinId) -> bool {
        self.inner.refocus(id)
    }
    pub fn mark_saved(&mut self, bitmap: &Arc<Bitmap>, path: &Path) {
        self.inner.mark_saved(bitmap, path);
    }
    pub fn wants_frame(&self) -> bool {
        self.inner.wants_frame()
    }
    pub fn frame(&mut self) {
        self.inner.frame();
    }
    pub fn drain(&mut self) -> Vec<(Arc<Bitmap>, PathBuf)> {
        self.inner.drain()
    }
    pub fn shutdown(&mut self) {
        self.inner.shutdown();
    }
}

fn local_stamp() -> Stamp {
    let (year, month, day, hour, minute, second) = dj_win::system::local_datetime();
    Stamp {
        year,
        month,
        day,
        hour,
        minute,
        second,
    }
}

/// The Arc object stays live as long as its source ID or allocation key does. Distinct
/// live Bitmap objects cannot share an address, even when their pixels are identical.
fn allocations(payload: &Payload) -> Vec<Allocation> {
    let mut out = vec![Allocation {
        key: Arc::as_ptr(&payload.bitmap) as usize as u64,
        bytes: payload.bitmap.pixels.len() as u64,
    }];
    if let Some(png) = &payload.png {
        out.push(Allocation {
            key: Arc::as_ptr(png) as *const u8 as usize as u64,
            bytes: png.len() as u64,
        });
    }
    out
}

fn scale(dpi: u32) -> f32 {
    (dpi.clamp(48, 768) as f32) / 96.0
}

impl<P: Platform> Controller<P> {
    fn new(
        platform: P,
        store: Store,
        clipboard: Box<dyn Clipboard>,
        tokens: &ResolvedTokens,
        save_dir: PathBuf,
    ) -> Self {
        Self {
            platform,
            state: pin::Pins::default(),
            entries: HashMap::new(),
            holds: HashMap::new(),
            store,
            clipboard,
            reveal: Store::reveal,
            stamp: local_stamp,
            tokens: tokens.clone(),
            save_dir,
            allowed: true,
            capturing: false,
        }
    }

    fn open(
        &mut self,
        payload: Payload,
        saved: Option<PathBuf>,
        anchor: (i32, i32),
    ) -> Result<PinId, String> {
        let source = SourceId(Arc::as_ptr(&payload.bitmap) as usize as u64);
        let admitted = self
            .state
            .admit(source, allocations(&payload))
            .map_err(|error| {
                if self.state.holds() > 0 && error.contains("256 MiB") {
                    format!("{error} Finish pending copies or saves first.")
                } else {
                    error
                }
            })?;
        let id = match admitted {
            Admission::Existing(id) => {
                if let Some(entry) = self.entries.get_mut(&id) {
                    entry.retry();
                    if saved.is_some() {
                        entry.saved = saved;
                        entry.status = Some(("Saved".into(), false));
                    }
                }
                self.sync_visibility();
                if self.state.is_visible(id) {
                    if let Some(entry) = self.entries.get(&id) {
                        P::show(&entry.window);
                    }
                }
                return Ok(id);
            }
            Admission::New(id) => id,
        };
        // Keep all fallible preparation local. Neither existing entries nor the store
        // change until the new HWND, GPU and first frame have all succeeded.
        let prepared = (|| {
            let (work, dpi) = self.platform.work_at(anchor);
            let scale = scale(dpi);
            let layout = pin::layout((payload.bitmap.width, payload.bitmap.height), scale, work);
            let bounds = pin::place(&layout, scale, work, anchor);
            let window = self.platform.create_window(id, bounds)?;
            let mut surface = self.platform.create_surface(
                &window,
                pin::allocation_size(scale),
                &payload.bitmap,
                scale,
            )?;
            P::render(
                &mut surface,
                &PinScene {
                    tokens: &self.tokens,
                    scale,
                    layout: &layout,
                    hover: None,
                    focus: None,
                    status: None,
                    saved: saved.is_some(),
                    busy: false,
                },
            )?;
            Ok(Entry {
                surface: Some(surface),
                window,
                layout,
                scale,
                bounds,
                shown: false,
                dirty: false,
                retries: 0,
                hover: None,
                focus: None,
                previous: None,
                pressed: None,
                captured: false,
                drag: None,
                copying: false,
                saving: false,
                saved,
                status: None,
            })
        })();
        match prepared {
            Ok(entry) => {
                self.entries.insert(id, entry);
                self.store.insert(id.0, payload);
                self.sync_visibility();
                Ok(id)
            }
            Err(error) => {
                self.state.close(id);
                Err(error)
            }
        }
    }

    fn sync_visibility(&mut self) {
        for id in self.state.ids() {
            let visible = self.state.is_visible(id);
            let Some(entry) = self.entries.get(&id) else {
                continue;
            };
            if visible == entry.shown {
                continue;
            }
            let dragged = entry.drag.is_some();
            // Restore against the *current* work area, including monitor changes
            // that occurred while hidden. A cancelled offscreen drag is also clamped.
            if visible {
                self.relayout(id, None);
            }
            let entry = self.entries.get_mut(&id).unwrap();
            entry.shown = visible;
            if visible {
                entry.retry();
                P::show(&entry.window);
            } else {
                entry.cancel_pointer();
                P::hide(&entry.window);
                if dragged {
                    self.relayout(id, None);
                }
            }
        }
    }

    fn can_focus(&self) -> bool {
        !self.capturing && self.state.count() > 0
    }

    fn mark_saved(&mut self, bitmap: &Arc<Bitmap>, path: &Path) {
        for (&id, entry) in &mut self.entries {
            if self
                .store
                .get(id.0)
                .is_some_and(|payload| Arc::ptr_eq(&payload.bitmap, bitmap))
            {
                entry.saved = Some(path.to_path_buf());
                entry.status = Some(("Saved".into(), false));
                entry.dirty = true;
            }
        }
    }

    fn close(&mut self, id: PinId, restore: bool) {
        self.state.close(id);
        self.store.remove(id.0);
        if let Some(mut entry) = self.entries.remove(&id) {
            entry.cancel_pointer();
            let previous = if restore && P::foreground(&entry.window) {
                entry.previous
            } else {
                None
            };
            // Release GPU references while the HWND is still alive, then hide it and
            // restore focus before the HWND dies, so the system's auto-activation of
            // the next window cannot race the restore.
            entry.surface.take();
            if entry.shown {
                P::hide(&entry.window);
            }
            if let Some(target) = previous {
                P::restore(target);
            }
            drop(entry);
        }
    }

    fn action(&mut self, id: PinId, action: PinAction) {
        if action == PinAction::Close {
            self.close(id, true);
            return;
        }
        let Some(entry) = self.entries.get_mut(&id) else {
            return;
        };
        entry.retry();
        match action {
            PinAction::Copy if !entry.copying => {
                // A cached PNG copies on the spot; only an encode job needs a hold.
                let needs_job = self.store.get(id.0).is_some_and(|p| p.png.is_none());
                let hold = if needs_job {
                    match hold_for(&mut self.state, &self.store, id) {
                        Ok(hold) => Some(hold),
                        Err(error) => {
                            entry.status = Some((format!("Copy failed: {error}"), true));
                            return;
                        }
                    }
                } else {
                    None
                };
                match self.store.copy(id.0, self.clipboard.as_mut()) {
                    CopyStart::Pending => {
                        if let Some(hold) = hold {
                            self.holds.entry((id, Op::Copy)).or_default().push(hold);
                        }
                        entry.copying = true;
                        entry.status = Some(("Preparing copy".into(), false));
                    }
                    CopyStart::Done(result) => {
                        if let Some(hold) = hold {
                            self.state.release(hold);
                        }
                        entry.status = Some(status(result, "Copied", "Copy failed"));
                    }
                }
            }
            PinAction::Save if !entry.saving => {
                if let Some(path) = &entry.saved {
                    match (self.reveal)(path) {
                        Ok(()) => entry.status = Some(("Opened".into(), false)),
                        Err(RevealError::Missing) => {
                            entry.saved = None;
                            entry.status =
                                Some(("File missing; Save to create a new PNG".into(), true));
                        }
                        Err(e) => entry.status = Some((format!("Open failed: {e}"), true)),
                    }
                } else {
                    match hold_for(&mut self.state, &self.store, id) {
                        Err(error) => {
                            entry.status = Some((format!("Save failed: {error}"), true));
                        }
                        Ok(hold) => {
                            if self.store.save(id.0, self.save_dir.clone(), (self.stamp)()) {
                                self.holds.entry((id, Op::Save)).or_default().push(hold);
                                entry.saving = true;
                                entry.status = Some(("Saving".into(), false));
                            } else {
                                self.state.release(hold);
                                entry.status = Some(("Save failed: busy, try again".into(), true));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn drain(&mut self) -> Vec<(Arc<Bitmap>, PathBuf)> {
        // Store::drain has dropped every Done's Arcs (or cached its PNG into a live
        // payload), so each reply, even one for a closed or unknown pin, ends its hold.
        let replies = self.store.drain();
        for reply in &replies {
            self.release_hold(PinId(reply.id), reply_op(&reply.kind));
        }
        let saved = self.apply_replies(replies);
        // Store::drain installs encoder caches. Finish pending clipboard writes first:
        // over-budget PNGs may be used transiently but must not stay in this manager.
        self.reaccount();
        saved
    }

    fn release_hold(&mut self, id: PinId, op: Op) {
        let Some(queue) = self.holds.get_mut(&(id, op)) else {
            return;
        };
        let hold = queue.remove(0);
        if queue.is_empty() {
            self.holds.remove(&(id, op));
        }
        self.state.release(hold);
    }

    fn apply_replies(&mut self, replies: Vec<Reply>) -> Vec<(Arc<Bitmap>, PathBuf)> {
        let mut saved = Vec::new();
        for reply in replies {
            let id = PinId(reply.id);
            let Some(entry) = self.entries.get_mut(&id) else {
                continue;
            };
            match reply.kind {
                ReplyKind::Saved(path) if entry.saving => {
                    entry.saving = false;
                    entry.saved = Some(path.clone());
                    entry.status = Some(("Saved".into(), false));
                    if let Some(payload) = self.store.get(id.0) {
                        saved.push((Arc::clone(&payload.bitmap), path));
                    }
                }
                ReplyKind::SaveFailed(error) if entry.saving => {
                    entry.saving = false;
                    entry.status = Some((format!("Save failed: {error}"), true));
                }
                ReplyKind::Encoded if entry.copying => {
                    entry.copying = false;
                    entry.status = Some(status(
                        self.store.finish_copy(id.0, self.clipboard.as_mut()),
                        "Copied",
                        "Copy failed",
                    ));
                }
                ReplyKind::EncodeFailed(error) if entry.copying => {
                    entry.copying = false;
                    entry.status = Some((format!("Copy failed: {error}"), true));
                }
                _ => continue,
            }
            entry.dirty = true;
        }
        saved
    }

    fn reaccount(&mut self) {
        for id in self.state.ids() {
            let Some(payload) = self.store.get(id.0) else {
                continue;
            };
            if self
                .state
                .replace_allocations(id, allocations(payload))
                .is_err()
            {
                let mut uncached = payload.clone();
                uncached.png = None;
                // The bitmap was already admitted, so removing an optional cache
                // cannot grow the previous committed allocation set.
                let result = self.state.replace_allocations(id, allocations(&uncached));
                debug_assert!(result.is_ok());
                self.store.insert(id.0, uncached);
            }
        }
    }

    fn input(&mut self, id: PinId, input: PinInput) {
        if !self.entries.contains_key(&id) {
            return;
        }
        match input {
            PinInput::Close => return self.close(id, true),
            PinInput::Destroyed => return self.close(id, false),
            PinInput::DisplayChanged => return self.display_changed(),
            PinInput::Dpi { dpi, suggested } => {
                return self.relayout(id, Some((dpi, rect(suggested))));
            }
            PinInput::CancelCapture | PinInput::FocusLost => {
                let entry = self.entries.get_mut(&id).unwrap();
                // A queued loss from tray/capture activation can arrive after
                // deliberate recovery has already restored this HWND's focus.
                if matches!(input, PinInput::FocusLost) && P::foreground(&entry.window) {
                    return;
                }
                let dragged = entry.drag.is_some();
                // The native layer already ended its press before emitting this.
                entry.reset_pointer();
                if matches!(input, PinInput::FocusLost) {
                    entry.focus = None;
                }
                if dragged {
                    self.relayout(id, None);
                }
                return;
            }
            PinInput::Paint => {
                let entry = self.entries.get_mut(&id).unwrap();
                // Paint cannot restart an exhausted device-loss retry sequence.
                if entry.retries < MAX_RETRIES {
                    entry.dirty = true;
                }
                return;
            }
            _ => {}
        }
        if !self.state.is_visible(id) {
            return;
        }
        let entry = self.entries.get_mut(&id).unwrap();
        match input {
            PinInput::Move {
                x,
                y,
                screen_x,
                screen_y,
            } => {
                if entry.drag.is_some() {
                    entry.drag_to((screen_x, screen_y));
                } else {
                    entry.hover = action_hit(&entry.layout, entry.scale, x, y);
                    entry.dirty = true;
                }
            }
            PinInput::Down {
                x,
                y,
                screen_x,
                screen_y,
            } => {
                entry.retry();
                // The native layer started this press itself. Reset only our own
                // fields: releasing here would kill the press we are about to ack.
                entry.reset_pointer();
                let hit = entry
                    .layout
                    .hit(x as f32 / entry.scale, y as f32 / entry.scale);
                match hit {
                    Some(Hit::Body) => {
                        // A drag needs the live press; a finished one is refused.
                        if P::capture(&entry.window) {
                            entry.captured = true;
                            entry.pressed = hit;
                            entry.drag = Some(Drag {
                                screen: (screen_x, screen_y),
                                origin: entry.bounds,
                            });
                        }
                    }
                    Some(Hit::Action(_)) => {
                        // A quick click may have finished natively before this Down
                        // drained. Its FIFO-queued Up still runs the action, and a
                        // CancelCapture in between clears it.
                        entry.captured = P::capture(&entry.window);
                        entry.pressed = hit;
                    }
                    None => {}
                }
            }
            PinInput::Up {
                x,
                y,
                screen_x,
                screen_y,
            } => {
                let hit = entry
                    .layout
                    .hit(x as f32 / entry.scale, y as f32 / entry.scale);
                let action = match (entry.pressed, hit) {
                    (Some(Hit::Action(a)), Some(Hit::Action(b))) if a == b => Some(a),
                    _ => None,
                };
                let dragged = entry.drag.is_some();
                entry.drag_to((screen_x, screen_y));
                // The native Up already ended the press and released capture.
                entry.reset_pointer();
                if dragged {
                    self.relayout(id, None);
                }
                if let Some(action) = action {
                    self.action(id, action);
                }
            }
            PinInput::Leave => {
                entry.hover = None;
                entry.dirty = true;
            }
            PinInput::Key {
                vk,
                ctrl,
                shift,
                alt,
            } if !alt && P::foreground(&entry.window) => {
                entry.retry();
                let action = match vk {
                    0x1B => Some(PinAction::Close),
                    0x43 if ctrl => Some(PinAction::Copy),
                    0x53 if ctrl => Some(PinAction::Save),
                    0x0D if !ctrl => entry.focus,
                    0x09 if !ctrl => {
                        let current = entry
                            .focus
                            .and_then(|a| ACTIONS.iter().position(|v| *v == a));
                        let next = match current {
                            Some(i) => (i + if shift { 2 } else { 1 }) % 3,
                            None => {
                                if shift {
                                    2
                                } else {
                                    0
                                }
                            }
                        };
                        entry.focus = Some(ACTIONS[next]);
                        None
                    }
                    _ => None,
                };
                if let Some(action) = action {
                    self.action(id, action);
                }
            }
            _ => {}
        }
    }

    fn relayout(&mut self, id: PinId, suggested: Option<(u32, IRect)>) {
        let Some(entry) = self.entries.get_mut(&id) else {
            return;
        };
        // A normal DPI crossing can happen during our captured body drag. Keep
        // that gesture alive and rebase it after applying the suggested bounds.
        // Display/work-area disruptions and action presses still cancel safely.
        let keep_drag = suggested.is_some() && entry.drag.is_some();
        if !keep_drag {
            entry.cancel_pointer();
        }
        // Windows switches DPI once most of the window is on the new monitor, so the
        // suggested top-left can still be on the old one: pick the monitor by centre.
        let (work, current_dpi) = if let Some((_, suggested)) = suggested {
            self.platform.work_at((
                suggested.x.saturating_add(suggested.w / 2),
                suggested.y.saturating_add(suggested.h / 2),
            ))
        } else {
            self.platform.work_for(&entry.window)
        };
        let dpi = suggested.map_or(current_dpi, |s| s.0);
        let new_scale = scale(dpi);
        let Some(payload) = self.store.get(id.0) else {
            return;
        };
        let layout = pin::layout(
            (payload.bitmap.width, payload.bitmap.height),
            new_scale,
            work,
        );
        let anchor = suggested.map_or((entry.bounds.x, entry.bounds.y), |s| (s.1.x, s.1.y));
        let mut bounds = pin::place(&layout, new_scale, work, anchor);
        if let (true, Some(drag)) = (keep_drag, entry.drag) {
            // Mid-drag the pin may straddle monitors: do not clamp (Up/Cancel do).
            // Rescale about the cursor so the grabbed point stays under it.
            let ratio = f64::from(new_scale) / f64::from(entry.scale);
            let from_cursor = |cursor: i32, origin: i32| {
                let grab = (i64::from(cursor) - i64::from(origin)) as f64 * ratio;
                (f64::from(cursor) - grab).round() as i32
            };
            bounds.x = from_cursor(drag.screen.0, drag.origin.x);
            bounds.y = from_cursor(drag.screen.1, drag.origin.y);
        }
        if entry.scale != new_scale {
            entry.surface.take();
        }
        entry.scale = new_scale;
        entry.layout = layout;
        entry.bounds = bounds;
        if let Some(drag) = entry.drag.as_mut() {
            drag.origin = bounds;
        }
        entry.retry();
        P::place(&entry.window, bounds);
    }

    fn display_changed(&mut self) {
        for id in self.state.ids() {
            self.relayout(id, None);
        }
    }

    fn show_keyboard(&mut self, previous: Option<P::Focus>) {
        let ids = self.state.ids();
        for &id in &ids {
            self.state.reveal(id);
            if let Some(entry) = self.entries.get_mut(&id) {
                entry.retry();
            }
        }
        self.sync_visibility();
        if !self.allowed || self.capturing {
            return;
        }
        // A repeated tray invocation cycles references, giving every nonactivating
        // window a keyboard route without requiring a foreground-stealing click.
        let current = ids.iter().position(|id| {
            self.entries.get(id).is_some_and(|entry| match previous {
                Some(target) => P::target_is_window(target, &entry.window),
                None => P::foreground(&entry.window),
            })
        });
        let remembered = current.and_then(|i| self.entries[&ids[i]].previous);
        if let Some(id) = ids.get(current.map_or(0, |i| (i + 1) % ids.len())).copied() {
            let entry = self.entries.get_mut(&id).unwrap();
            // Repeated tray recovery must not replace the external focus target with
            // the pin itself. FocusTarget is validated by dj-win before restoration.
            if !P::foreground(&entry.window) {
                let target = P::focus(&entry.window);
                if !P::foreground(&entry.window) {
                    entry.focus = None;
                    entry.retry();
                    return;
                }
                entry.previous = if current.is_some() {
                    remembered.or(entry.previous)
                } else {
                    previous.or(target)
                };
            }
            entry.focus = Some(PinAction::Copy);
            entry.retry();
        }
    }

    /// The live pin whose native window holds the foreground, if any.
    fn keyboard_owner(&self) -> Option<PinId> {
        self.state.ids().into_iter().find(|id| {
            self.entries
                .get(id)
                .is_some_and(|entry| P::foreground(&entry.window))
        })
    }

    /// Give the keyboard back to a pin after capture suppression ended. It never shows
    /// a hidden pin and keeps `previous`, the external target Escape restores.
    fn refocus(&mut self, id: PinId) -> bool {
        if !self.state.is_visible(id) {
            return false;
        }
        let Some(entry) = self.entries.get_mut(&id) else {
            return false;
        };
        if !entry.shown {
            return false;
        }
        if !P::foreground(&entry.window) {
            // The returned target is whatever lost the foreground, not the destination
            // Escape restores, so it must not replace `previous`.
            P::focus(&entry.window);
            if !P::foreground(&entry.window) {
                entry.focus = None;
                entry.retry();
                return false;
            }
        }
        entry.focus.get_or_insert(PinAction::Copy);
        entry.retry();
        true
    }

    fn wants_frame(&self) -> bool {
        self.entries
            .values()
            .any(|entry| entry.shown && entry.dirty && entry.retries < MAX_RETRIES)
    }

    fn frame(&mut self) {
        for (&id, entry) in &mut self.entries {
            if !entry.shown || !entry.dirty || entry.retries >= MAX_RETRIES {
                continue;
            }
            let result = (|| {
                if entry.surface.is_none() {
                    let payload = self.store.get(id.0).ok_or("no pinned image")?;
                    entry.surface = Some(self.platform.create_surface(
                        &entry.window,
                        pin::allocation_size(entry.scale),
                        &payload.bitmap,
                        entry.scale,
                    )?);
                }
                if entry
                    .status
                    .as_ref()
                    .is_some_and(|(text, _)| text.starts_with("Drawing failed: "))
                {
                    entry.status = None;
                }
                P::render(
                    entry.surface.as_mut().unwrap(),
                    &PinScene {
                        tokens: &self.tokens,
                        scale: entry.scale,
                        layout: &entry.layout,
                        hover: entry.hover,
                        focus: entry.focus,
                        status: entry
                            .status
                            .as_ref()
                            .map(|(text, error)| (text.as_str(), *error)),
                        saved: entry.saved.is_some(),
                        busy: entry.copying || entry.saving,
                    },
                )
            })();
            match result {
                Ok(()) => {
                    entry.dirty = false;
                    entry.retries = 0;
                }
                Err(error) => {
                    entry.surface.take();
                    entry.retries += 1;
                    entry.dirty = entry.retries < MAX_RETRIES;
                    entry.status = Some((format!("Drawing failed: {error}"), true));
                }
            }
        }
    }

    fn shutdown(&mut self) {
        for id in self.state.ids() {
            self.close(id, false);
        }
        for hold in self.holds.drain().flat_map(|(_, queue)| queue) {
            self.state.release(hold);
        }
    }
}

impl<P: Platform> Drop for Controller<P> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The hold a reply ends: Copy's encode job or a Save job.
fn reply_op(kind: &ReplyKind) -> Op {
    match kind {
        ReplyKind::Saved(_) | ReplyKind::SaveFailed(_) => Op::Save,
        ReplyKind::Encoded | ReplyKind::EncodeFailed(_) => Op::Copy,
    }
}

/// Retain the pin's current payload for a job about to be queued. Allocations are keyed
/// by Arc identity, so a payload already admitted for this pin is not counted twice.
fn hold_for(state: &mut pin::Pins, store: &Store, id: PinId) -> Result<HoldId, String> {
    let held = store.get(id.0).map(allocations).unwrap_or_default();
    state.hold(held).map_err(|_| "memory limit".to_string())
}

fn action_hit(layout: &Layout, scale: f32, x: i32, y: i32) -> Option<PinAction> {
    match layout.hit(x as f32 / scale, y as f32 / scale) {
        Some(Hit::Action(action)) => Some(action),
        _ => None,
    }
}

fn status(result: Result<(), String>, ok: &str, failed: &str) -> (String, bool) {
    match result {
        Ok(()) => (ok.into(), false),
        Err(error) => (format!("{failed}: {error}"), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview_store::Encoder;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::{mpsc, Mutex};
    use std::time::{Duration, Instant};

    thread_local! {
        static RESTORED: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
        static REVEALED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
        /// Hide/restore/window-drop effects in the order they happened.
        static ORDER: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    struct Effects {
        events: Vec<String>,
        fail_window: bool,
        fail_surface: bool,
        fail_render: bool,
        deny_focus: bool,
        foreground: u64,
        work: IRect,
        dpi: u32,
        captured: Option<u64>,
        /// The window whose native press (WM_LBUTTONDOWN + SetCapture) is still live.
        native_press: Option<u64>,
        /// Monitors for `work_at`; empty means the single `work`/`dpi` pair.
        monitors: Vec<(IRect, u32)>,
        placed: Option<IRect>,
    }

    impl Default for Effects {
        fn default() -> Self {
            Self {
                events: Vec::new(),
                fail_window: false,
                fail_surface: false,
                fail_render: false,
                deny_focus: false,
                foreground: 999,
                work: IRect::new(-1600, 32, 1600, 900),
                dpi: 96,
                captured: None,
                native_press: None,
                monitors: Vec::new(),
                placed: None,
            }
        }
    }

    struct FakeWindow {
        id: u64,
        effects: Rc<RefCell<Effects>>,
    }

    impl Drop for FakeWindow {
        fn drop(&mut self) {
            ORDER.with(|order| order.borrow_mut().push(format!("drop window {}", self.id)));
            self.effects
                .borrow_mut()
                .events
                .push(format!("drop window {}", self.id));
        }
    }

    struct FakeSurface {
        id: u64,
        effects: Rc<RefCell<Effects>>,
    }

    impl Drop for FakeSurface {
        fn drop(&mut self) {
            self.effects
                .borrow_mut()
                .events
                .push(format!("drop surface {}", self.id));
        }
    }

    struct Fake(Rc<RefCell<Effects>>);

    impl Platform for Fake {
        type Window = FakeWindow;
        type Surface = FakeSurface;
        type Focus = u64;

        fn create_window(&mut self, id: PinId, _: IRect) -> Result<FakeWindow, String> {
            let mut effects = self.0.borrow_mut();
            effects.events.push(format!("new window {}", id.0));
            if effects.fail_window {
                return Err("window failed".into());
            }
            Ok(FakeWindow {
                id: id.0,
                effects: Rc::clone(&self.0),
            })
        }
        fn create_surface(
            &mut self,
            window: &FakeWindow,
            _: (u32, u32),
            _: &Bitmap,
            _: f32,
        ) -> Result<FakeSurface, String> {
            let mut effects = self.0.borrow_mut();
            effects.events.push(format!("new surface {}", window.id));
            if effects.fail_surface {
                return Err("surface failed".into());
            }
            Ok(FakeSurface {
                id: window.id,
                effects: Rc::clone(&self.0),
            })
        }
        fn render(surface: &mut FakeSurface, _: &PinScene<'_>) -> Result<(), String> {
            let mut effects = surface.effects.borrow_mut();
            effects.events.push(format!("render {}", surface.id));
            if effects.fail_render {
                Err("device lost".into())
            } else {
                Ok(())
            }
        }
        fn hwnd(window: &FakeWindow) -> isize {
            window.id as isize
        }
        fn show(window: &FakeWindow) {
            window
                .effects
                .borrow_mut()
                .events
                .push(format!("show {}", window.id));
        }
        fn hide(window: &FakeWindow) {
            ORDER.with(|order| order.borrow_mut().push(format!("hide {}", window.id)));
            window
                .effects
                .borrow_mut()
                .events
                .push(format!("hide {}", window.id));
        }
        fn place(window: &FakeWindow, bounds: IRect) {
            let mut effects = window.effects.borrow_mut();
            effects.placed = Some(bounds);
            effects.events.push(format!("place {}", window.id));
        }
        /// Like the native layer: acknowledges a still-live press, never acquires one.
        fn capture(window: &FakeWindow) -> bool {
            let mut effects = window.effects.borrow_mut();
            let live = effects.native_press == Some(window.id);
            if live {
                effects.captured = Some(window.id);
            }
            live
        }
        /// Like `Window::release_capture`: ends the live native press.
        fn release(window: &FakeWindow) {
            let mut effects = window.effects.borrow_mut();
            if effects.native_press == Some(window.id) {
                effects.native_press = None;
            }
            if effects.captured == Some(window.id) {
                effects.captured = None;
            }
        }
        fn foreground(window: &FakeWindow) -> bool {
            window.effects.borrow().foreground == window.id
        }
        fn focus(window: &FakeWindow) -> Option<u64> {
            let mut effects = window.effects.borrow_mut();
            let old = effects.foreground;
            if !effects.deny_focus {
                effects.foreground = window.id;
            }
            effects.events.push(format!("focus {}", window.id));
            Some(old)
        }
        fn restore(target: u64) {
            ORDER.with(|order| order.borrow_mut().push(format!("restore {target}")));
            RESTORED.with(|restored| restored.borrow_mut().push(target));
        }
        fn target_is_window(target: u64, window: &FakeWindow) -> bool {
            target == window.id
        }
        fn work_at(&self, point: (i32, i32)) -> (IRect, u32) {
            let effects = self.0.borrow();
            effects
                .monitors
                .iter()
                .find(|(work, _)| {
                    point.0 >= work.x
                        && point.0 < work.right()
                        && point.1 >= work.y
                        && point.1 < work.bottom()
                })
                .copied()
                .unwrap_or((effects.work, effects.dpi))
        }
        fn work_for(&self, _: &FakeWindow) -> (IRect, u32) {
            let placed = self.0.borrow().placed;
            self.work_at(placed.map_or((0, 0), |r| (r.x + r.w / 2, r.y + r.h / 2)))
        }
    }

    struct FakeClipboard(Rc<RefCell<Vec<Vec<u8>>>>);
    impl Clipboard for FakeClipboard {
        fn put(&mut self, png: &[u8], dib: &[u8]) -> Result<(), String> {
            assert!(dib.len() >= 40);
            self.0.borrow_mut().push(png.to_vec());
            Ok(())
        }
    }

    fn reveal(path: &Path) -> Result<(), RevealError> {
        if path == Path::new("missing.png") {
            return Err(RevealError::Missing);
        }
        REVEALED.with(|paths| paths.borrow_mut().push(path.to_path_buf()));
        Ok(())
    }

    fn stamp() -> Stamp {
        Stamp {
            year: 2026,
            month: 10,
            day: 28,
            hour: 15,
            minute: 4,
            second: 2,
        }
    }

    struct Rig {
        controller: Controller<Fake>,
        effects: Rc<RefCell<Effects>>,
        copies: Rc<RefCell<Vec<Vec<u8>>>>,
        dir: tempfile::TempDir,
    }

    fn rig() -> Rig {
        rig_with(Store::new(Box::new(|| {})))
    }

    /// An encoder that reports it started, then blocks until the test sends one release
    /// (or 5 s pass), so a job stays queued or running for as long as the test needs.
    fn gated_store(queue: usize) -> (Store, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_tx, entered) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let (entered_tx, release_rx) = (Mutex::new(entered_tx), Mutex::new(release_rx));
        let encoder: Encoder = Arc::new(move |_| {
            let _ = entered_tx.lock().unwrap().send(());
            match release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
            {
                Ok(()) => Ok(b"PNG".to_vec()),
                Err(_) => Err("the gate was never opened".into()),
            }
        });
        let store = Store::with_parts(Box::new(|| {}), encoder, queue);
        (store, entered, release)
    }

    fn rig_with(store: Store) -> Rig {
        RESTORED.with(|r| r.borrow_mut().clear());
        ORDER.with(|r| r.borrow_mut().clear());
        REVEALED.with(|r| r.borrow_mut().clear());
        let effects = Rc::new(RefCell::new(Effects::default()));
        let copies = Rc::new(RefCell::new(Vec::new()));
        let registry = dj_core::theme::ThemeRegistry::with_builtins();
        let tokens = registry.get(&registry.ids()[0]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut controller = Controller::new(
            Fake(Rc::clone(&effects)),
            store,
            Box::new(FakeClipboard(Rc::clone(&copies))),
            tokens,
            dir.path().into(),
        );
        controller.stamp = stamp;
        controller.reveal = reveal;
        Rig {
            controller,
            effects,
            copies,
            dir,
        }
    }

    fn payload(png: bool) -> Payload {
        Payload {
            bitmap: Arc::new(Bitmap::black(4, 3)),
            png: png.then(|| Arc::from(&b"synthetic png"[..])),
        }
    }

    impl Rig {
        fn open(&mut self, payload: Payload) -> PinId {
            self.controller.open(payload, None, (-1200, 100)).unwrap()
        }
        fn settle(&mut self, id: PinId) -> Vec<(Arc<Bitmap>, PathBuf)> {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut saved = Vec::new();
            loop {
                saved.extend(self.controller.drain());
                let entry = &self.controller.entries[&id];
                if !entry.copying && !entry.saving {
                    return saved;
                }
                assert!(Instant::now() < deadline, "IO worker did not finish");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        /// Drain until every hold has ended (the replies for closed pins included).
        fn settle_holds(&mut self) -> Vec<(Arc<Bitmap>, PathBuf)> {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut saved = Vec::new();
            loop {
                saved.extend(self.controller.drain());
                if self.controller.state.holds() == 0 {
                    return saved;
                }
                assert!(Instant::now() < deadline, "IO worker did not finish");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        /// Mirrors `Pins::begin_capture` / `Pins::end_capture`.
        fn capture(&mut self, on: bool) {
            self.controller.capturing = on;
            if on {
                self.controller.state.begin_capture();
            } else {
                self.controller.state.end_capture();
            }
            self.controller.sync_visibility();
        }
        fn focus_events(&self, id: PinId) -> usize {
            let focus = format!("focus {}", id.0);
            let effects = self.effects.borrow();
            effects.events.iter().filter(|e| **e == focus).count()
        }
        fn key(&mut self, id: PinId, vk: u16, ctrl: bool, shift: bool) {
            self.controller.input(
                id,
                PinInput::Key {
                    vk,
                    ctrl,
                    shift,
                    alt: false,
                },
            );
        }
        /// WM_LBUTTONDOWN: the native layer starts the press (SetCapture + sampler).
        fn native_down(&self, id: PinId) {
            self.effects.borrow_mut().native_press = Some(id.0);
        }
        /// The native press ends (WM_LBUTTONUP, cancel, focus loss) before its input
        /// is queued for the controller.
        fn native_end(&self) {
            let mut effects = self.effects.borrow_mut();
            effects.native_press = None;
            effects.captured = None;
        }
        fn down(&mut self, id: PinId, point: (i32, i32)) {
            self.native_down(id);
            self.down_at(id, point, (-1000, 200));
        }
        fn down_at(&mut self, id: PinId, point: (i32, i32), screen: (i32, i32)) {
            self.controller.input(
                id,
                PinInput::Down {
                    x: point.0,
                    y: point.1,
                    screen_x: screen.0,
                    screen_y: screen.1,
                },
            );
        }
        fn move_to(&mut self, id: PinId, screen: (i32, i32)) {
            self.controller.input(
                id,
                PinInput::Move {
                    x: 1,
                    y: 1,
                    screen_x: screen.0,
                    screen_y: screen.1,
                },
            );
        }
        fn cancel(&mut self, id: PinId) {
            self.native_end();
            self.controller.input(id, PinInput::CancelCapture);
        }
        fn focus_lost(&mut self, id: PinId) {
            self.native_end();
            self.controller.input(id, PinInput::FocusLost);
        }
        fn up(&mut self, id: PinId, point: (i32, i32)) {
            self.native_end();
            self.controller.input(
                id,
                PinInput::Up {
                    x: point.0,
                    y: point.1,
                    screen_x: -900,
                    screen_y: 300,
                },
            );
        }
        fn button(&self, id: PinId, action: PinAction) -> (i32, i32) {
            let entry = &self.controller.entries[&id];
            let rect = entry
                .layout
                .actions
                .iter()
                .find(|(a, _)| *a == action)
                .unwrap()
                .1;
            (
                ((rect.x + rect.w / 2.0) * entry.scale) as i32,
                ((rect.y + rect.h / 2.0) * entry.scale) as i32,
            )
        }
    }

    #[test]
    fn admission_rolls_back_window_surface_and_first_frame_failures() {
        let mut r = rig();
        let first = r.open(payload(true));
        let bytes = r.controller.state.bytes();
        for failure in 0..3 {
            {
                let mut effects = r.effects.borrow_mut();
                effects.fail_window = failure == 0;
                effects.fail_surface = failure == 1;
                effects.fail_render = failure == 2;
            }
            let new = payload(true);
            let weak = Arc::downgrade(&new.bitmap);
            assert!(r.controller.open(new, None, (0, 0)).is_err());
            assert!(weak.upgrade().is_none());
            assert_eq!(r.controller.state.ids(), vec![first]);
            assert_eq!(r.controller.state.bytes(), bytes);
            assert_eq!(r.controller.entries.len(), 1);
        }
        assert!(r.copies.borrow().is_empty());
        assert!(std::fs::read_dir(r.dir.path()).unwrap().next().is_none());
        let events = &r.effects.borrow().events;
        let gpu = events.iter().position(|e| e == "drop surface 4").unwrap();
        let hwnd = events.iter().position(|e| e == "drop window 4").unwrap();
        assert!(gpu < hwnd);
    }

    #[test]
    fn shared_identity_reveals_and_independent_images_share_only_png_accounting() {
        let mut r = rig();
        let p = payload(true);
        let weak = Arc::downgrade(&p.bitmap);
        let first = r.open(p.clone());
        r.controller.state.set_visible(first, false);
        r.controller.sync_visibility();
        assert_eq!(r.open(p.clone()), first);
        assert!(r.controller.entries[&first].shown);
        let mut other = payload(false);
        other.png = p.png.clone();
        let second = r.open(other);
        assert_ne!(first, second);
        assert_eq!(r.controller.state.bytes(), 2 * 48 + 13);
        drop(p); // Simulates the originating stack card/thumb expiring.
        assert!(weak.upgrade().is_some());
        r.controller.close(first, true);
        assert!(weak.upgrade().is_none());
        assert!(r.controller.state.contains(second));
        assert_eq!(r.controller.state.bytes(), 48 + 13);
    }

    #[test]
    fn duplicate_reasserts_topmost_even_when_already_visible_without_showing_during_capture() {
        let mut r = rig();
        let source = payload(true);
        let id = r.open(source.clone());
        r.open(payload(true)); // Covers the older topmost pin.
        let shows = |effects: &Effects| {
            effects
                .events
                .iter()
                .filter(|e| **e == format!("show {}", id.0))
                .count()
        };
        let before = shows(&r.effects.borrow());
        assert_eq!(r.open(source.clone()), id);
        assert!(shows(&r.effects.borrow()) > before);
        r.controller.capturing = true;
        r.controller.state.begin_capture();
        r.controller.sync_visibility();
        let before = shows(&r.effects.borrow());
        assert_eq!(r.open(source), id);
        assert_eq!(shows(&r.effects.borrow()), before);
        assert!(!r.controller.entries[&id].shown);
    }

    #[test]
    fn count_limit_never_evicts_and_close_does_not_recycle_id() {
        let mut r = rig();
        let ids: Vec<_> = (0..8).map(|_| r.open(payload(true))).collect();
        assert!(r.controller.open(payload(true), None, (0, 0)).is_err());
        assert_eq!(r.controller.state.ids(), ids);
        r.controller.close(ids[3], true);
        let later = r.open(payload(true));
        assert!(later.0 > ids[7].0);
    }

    #[test]
    fn capture_restores_only_live_desired_visible_pins_after_later_global_changes() {
        let mut r = rig();
        let a = r.open(payload(true));
        let b = r.open(payload(true));
        r.controller.state.set_visible(b, false);
        r.controller.sync_visibility();
        r.controller.state.begin_capture();
        r.controller.capturing = true;
        r.controller.sync_visibility();
        assert!(r.controller.entries.values().all(|e| !e.shown));
        r.controller.close(a, false);
        let c = r.open(payload(true));
        assert!(!r.controller.entries[&c].shown);
        r.controller.state.set_allowed(false);
        r.controller.allowed = false;
        r.controller.state.end_capture();
        r.controller.capturing = false;
        r.controller.sync_visibility();
        assert!(r.controller.entries.values().all(|e| !e.shown));
        r.controller.state.set_allowed(true);
        r.controller.allowed = true;
        r.controller.sync_visibility();
        assert!(!r.controller.entries[&b].shown);
        assert!(r.controller.entries[&c].shown);
        assert!(!r.controller.state.contains(a));
        r.controller.state.begin_capture();
        r.controller.capturing = true;
        r.controller.sync_visibility();
        r.controller.show_keyboard(None); // Recovery requests visibility, never foreground during freeze.
        assert_eq!(r.effects.borrow().foreground, 999);
        assert!(r.controller.entries.values().all(|e| !e.shown));
        r.controller.state.end_capture();
        r.controller.capturing = false;
        r.controller.sync_visibility();
        assert!(r.controller.entries.values().all(|e| e.shown));
    }

    #[test]
    fn close_drops_gpu_before_hwnd_shutdown_is_idempotent_and_late_events_are_ignored() {
        let mut r = rig();
        let a = r.open(payload(true));
        let b = r.open(payload(true));
        r.down(a, (1, 1));
        r.controller.input(a, PinInput::Destroyed);
        assert_eq!(r.effects.borrow().captured, None);
        r.controller.input(a, PinInput::Paint);
        r.controller.input(a, PinInput::Close);
        assert!(r.controller.state.contains(b));
        r.controller.shutdown();
        r.controller.shutdown();
        assert_eq!(r.controller.state.count(), 0);
        assert_eq!(r.controller.state.bytes(), 0);
        let effects = r.effects.borrow();
        for id in [a, b] {
            let surface = effects
                .events
                .iter()
                .position(|e| *e == format!("drop surface {}", id.0))
                .unwrap();
            let window = effects
                .events
                .iter()
                .position(|e| *e == format!("drop window {}", id.0))
                .unwrap();
            assert!(surface < window);
            assert_eq!(
                effects
                    .events
                    .iter()
                    .filter(|e| **e == format!("drop window {}", id.0))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn copy_save_open_are_independent_and_keep_original_pixels_after_source_drop() {
        let mut r = rig();
        let p = payload(true);
        let bitmap = Arc::clone(&p.bitmap);
        let id = r.open(p);
        let other = r.open(payload(true));
        r.controller.action(id, PinAction::Copy);
        assert_eq!(*r.copies.borrow(), vec![b"synthetic png".to_vec()]);
        r.controller.action(id, PinAction::Save);
        r.controller.action(id, PinAction::Save); // Coalesced until its reply.
        let saved = r.settle(id);
        assert_eq!(saved.len(), 1);
        assert!(Arc::ptr_eq(&saved[0].0, &bitmap));
        let path = &saved[0].1;
        assert_eq!(std::fs::read(path).unwrap(), b"synthetic png");
        assert!(r.controller.entries[&other].saved.is_none());
        r.controller.action(id, PinAction::Save);
        REVEALED.with(|paths| assert_eq!(*paths.borrow(), vec![path.clone()]));
        r.controller.close(id, true);
        assert!(path.exists());
        assert_eq!(std::fs::read_dir(r.dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn hidden_pins_remain_available_for_tray_recovery_but_capture_blocks_it() {
        let mut r = rig();
        assert!(!r.controller.can_focus());
        let id = r.open(payload(true));
        r.controller.allowed = false;
        r.controller.state.set_allowed(false);
        r.controller.sync_visibility();
        assert!(r.controller.can_focus());
        r.controller.show_keyboard(None);
        assert_eq!(r.effects.borrow().foreground, 999);
        assert!(!r.controller.entries[&id].shown);
        r.controller.capturing = true;
        assert!(!r.controller.can_focus());
    }

    #[test]
    fn source_save_updates_an_open_pin_by_arc_identity_without_duplicate_disk_writes() {
        let mut r = rig();
        let p = payload(true);
        let id = r.open(p.clone());
        let other = r.open(payload(true));
        let path = r.dir.path().join("saved-by-thumb.png");
        std::fs::write(&path, b"synthetic png").unwrap();
        r.controller.entries.get_mut(&id).unwrap().copying = true;
        r.controller.mark_saved(&p.bitmap, &path);
        assert!(r.controller.entries[&id].copying);
        assert!(r.controller.entries[&other].saved.is_none());
        assert_eq!(r.controller.entries[&id].saved.as_ref(), Some(&path));
        r.controller.action(id, PinAction::Save);
        assert!(!r.controller.entries[&id].saving);
        REVEALED.with(|paths| assert_eq!(*paths.borrow(), vec![path.clone()]));
        assert_eq!(std::fs::read_dir(r.dir.path()).unwrap().count(), 1);
        let updated = r.dir.path().join("new-source-save.png");
        assert_eq!(r.controller.open(p, Some(updated.clone()), (0, 0)), Ok(id));
        assert_eq!(r.controller.entries[&id].saved.as_ref(), Some(&updated));
        assert!(r.controller.entries[&id].copying);
    }

    #[test]
    fn missing_open_returns_to_save_and_errors_stay_on_their_pin() {
        let mut r = rig();
        let a = r.open(payload(true));
        let b = r.open(payload(true));
        r.controller.entries.get_mut(&a).unwrap().saved = Some("missing.png".into());
        r.controller.action(a, PinAction::Save);
        assert!(r.controller.entries[&a].saved.is_none());
        assert!(r.controller.entries[&a].status.as_ref().unwrap().1);
        assert!(r.controller.entries[&b].status.is_none());
        r.controller.action(a, PinAction::Save);
        r.settle(a);
        assert!(r.controller.entries[&a].saved.as_ref().unwrap().exists());
    }

    #[test]
    fn encoded_copy_reaccounts_png_and_late_or_unsolicited_replies_do_not_change_another_pin() {
        let mut r = rig();
        let a = r.open(payload(false));
        r.controller.action(a, PinAction::Copy);
        r.controller.action(a, PinAction::Save);
        r.settle(a);
        assert_eq!(r.copies.borrow().len(), 1);
        let cached = r.controller.store.get(a.0).unwrap();
        assert!(cached.png.is_some());
        assert_eq!(r.controller.state.bytes(), cached.bytes());
        r.controller.close(a, false);
        let b = r.open(payload(true));
        r.controller.apply_replies(vec![
            Reply {
                id: a.0,
                kind: ReplyKind::Saved("late.png".into()),
            },
            Reply {
                id: a.0,
                kind: ReplyKind::Encoded,
            },
            Reply {
                id: b.0,
                kind: ReplyKind::SaveFailed("unsolicited".into()),
            },
        ]);
        assert!(r.controller.entries[&b].saved.is_none());
        assert!(r.controller.entries[&b].status.is_none());
        assert_eq!(r.copies.borrow().len(), 1);
    }

    #[test]
    fn concurrent_io_failures_clear_only_the_matching_operation_and_pin() {
        let mut r = rig();
        let a = r.open(payload(false));
        let b = r.open(payload(true));
        let entry = r.controller.entries.get_mut(&a).unwrap();
        entry.copying = true;
        entry.saving = true;
        r.controller.apply_replies(vec![Reply {
            id: a.0,
            kind: ReplyKind::SaveFailed("disk full".into()),
        }]);
        assert!(r.controller.entries[&a].copying);
        assert!(!r.controller.entries[&a].saving);
        assert!(r.controller.entries[&a].status.as_ref().unwrap().1);
        assert!(r.controller.entries[&b].status.is_none());
        r.controller.apply_replies(vec![Reply {
            id: a.0,
            kind: ReplyKind::EncodeFailed("encoder failed".into()),
        }]);
        assert!(!r.controller.entries[&a].copying);
        assert!(r.controller.entries[&a]
            .status
            .as_ref()
            .unwrap()
            .0
            .contains("Copy failed"));
        assert!(r.copies.borrow().is_empty());
    }

    #[test]
    fn over_budget_encoder_cache_is_discarded_after_completing_copy() {
        let mut r = rig();
        let id = r.open(payload(false));
        // Reserve the rest of the domain budget without allocating 256 MiB in a
        // policy unit test. This stands for other pins' already-accounted payloads.
        r.controller
            .state
            .admit(
                SourceId(u64::MAX),
                vec![Allocation {
                    key: u64::MAX,
                    bytes: 256 * 1024 * 1024 - 48,
                }],
            )
            .unwrap();
        let effects_before = r.effects.borrow().events.len();
        assert!(r.controller.open(payload(false), None, (0, 0)).is_err());
        assert_eq!(r.effects.borrow().events.len(), effects_before);
        let mut p = r.controller.store.get(id.0).unwrap().clone();
        p.png = Some(Arc::from(&b"new cache"[..]));
        r.controller.store.insert(id.0, p);
        r.controller.entries.get_mut(&id).unwrap().copying = true;
        r.controller.apply_replies(vec![Reply {
            id: id.0,
            kind: ReplyKind::Encoded,
        }]);
        r.controller.reaccount();
        assert_eq!(*r.copies.borrow(), vec![b"new cache".to_vec()]);
        assert!(r.controller.store.get(id.0).unwrap().png.is_none());
        assert_eq!(r.controller.state.bytes(), 256 * 1024 * 1024);
        assert!(r.controller.state.contains(id));
    }

    #[test]
    fn pointer_events_are_isolated_buttons_never_drag_and_capture_loss_cancels_clicks() {
        let mut r = rig();
        let a = r.open(payload(true));
        let b = r.open(payload(true));
        let original = r.controller.entries[&a].bounds;
        r.down(a, (1, 1));
        r.controller.input(
            b,
            PinInput::Move {
                x: 20,
                y: 20,
                screen_x: -500,
                screen_y: 500,
            },
        );
        assert_eq!(r.controller.entries[&a].bounds, original);
        r.controller.input(
            a,
            PinInput::Move {
                x: 20,
                y: 20,
                screen_x: -900,
                screen_y: 300,
            },
        );
        assert_eq!(r.controller.entries[&a].bounds.x, original.x + 100);
        r.cancel(a);
        let stopped = r.controller.entries[&a].bounds;
        r.controller.input(
            a,
            PinInput::Move {
                x: 20,
                y: 20,
                screen_x: -800,
                screen_y: 400,
            },
        );
        assert_eq!(r.controller.entries[&a].bounds, stopped);
        let copy = r.button(a, PinAction::Copy);
        r.down(a, copy);
        assert!(r.controller.entries[&a].drag.is_none());
        r.focus_lost(a);
        r.up(a, copy);
        assert!(r.copies.borrow().is_empty());
        r.down(a, copy);
        r.up(a, copy);
        assert_eq!(r.copies.borrow().len(), 1);
        assert_eq!(r.effects.borrow().captured, None);
    }

    #[test]
    fn interrupted_offscreen_drags_and_visibility_recovery_keep_close_reachable() {
        for interruption in 0..4 {
            let mut r = rig();
            let id = r.open(payload(true));
            r.down(id, (1, 1));
            r.controller.input(
                id,
                PinInput::Move {
                    x: 1,
                    y: 1,
                    screen_x: 9000,
                    screen_y: 9000,
                },
            );
            match interruption {
                0 => r.cancel(id),
                1 => r.focus_lost(id),
                2 => {
                    r.controller.state.set_allowed(false);
                    r.controller.sync_visibility();
                    r.effects.borrow_mut().work = IRect::new(0, 48, 800, 600);
                    r.controller.state.set_allowed(true);
                    r.controller.sync_visibility();
                }
                _ => {
                    r.controller.state.begin_capture();
                    r.controller.sync_visibility();
                    r.controller.state.end_capture();
                    r.controller.sync_visibility();
                }
            }
            let entry = &r.controller.entries[&id];
            let work = r.effects.borrow().work;
            assert_eq!(pin::clamp(entry.bounds, work), entry.bounds);
            let close = entry.layout.actions[2].1;
            assert!(
                entry.bounds.x + ((close.x + close.w) * entry.scale).ceil() as i32 <= work.right()
            );
            assert!(
                entry.bounds.y + ((close.y + close.h) * entry.scale).ceil() as i32 <= work.bottom()
            );
            assert!(entry.drag.is_none());
            assert!(entry.pressed.is_none());
            assert_eq!(r.effects.borrow().captured, None);
        }
    }

    #[test]
    fn dpi_crossing_rebases_the_active_body_drag_and_keeps_capture() {
        let mut r = rig();
        let id = r.open(payload(true));
        r.down(id, (1, 1));
        r.controller.input(
            id,
            PinInput::Move {
                x: 1,
                y: 1,
                screen_x: -950,
                screen_y: 250,
            },
        );
        r.effects.borrow_mut().dpi = 144;
        r.controller.input(
            id,
            PinInput::Dpi {
                dpi: 144,
                suggested: (-1000, 100, 400, 200),
            },
        );
        assert_eq!(r.effects.borrow().captured, Some(id.0));
        let bounds = r.controller.entries[&id].bounds;
        assert!(r.controller.entries[&id].drag.is_some());
        r.controller.input(
            id,
            PinInput::Move {
                x: 1,
                y: 1,
                screen_x: -900,
                screen_y: 300,
            },
        );
        assert_eq!(r.controller.entries[&id].bounds.x, bounds.x + 50);
        assert_eq!(r.controller.entries[&id].bounds.y, bounds.y + 50);
        r.up(id, (1, 1));
        assert_eq!(r.effects.borrow().captured, None);
    }

    #[test]
    fn monitor_and_dpi_changes_clamp_dragged_pins_without_reallocating_on_motion() {
        let mut r = rig();
        let id = r.open(payload(true));
        let creates = r
            .effects
            .borrow()
            .events
            .iter()
            .filter(|e| e.starts_with("new surface"))
            .count();
        r.down(id, (1, 1));
        r.controller.input(
            id,
            PinInput::Move {
                x: 1,
                y: 1,
                screen_x: -9000,
                screen_y: -9000,
            },
        );
        r.controller.input(
            id,
            PinInput::Up {
                x: 1,
                y: 1,
                screen_x: -9000,
                screen_y: -9000,
            },
        );
        let bounds = r.controller.entries[&id].bounds;
        assert_eq!(pin::clamp(bounds, r.effects.borrow().work), bounds);
        assert_eq!(
            r.effects
                .borrow()
                .events
                .iter()
                .filter(|e| e.starts_with("new surface"))
                .count(),
            creates
        );
        {
            let mut effects = r.effects.borrow_mut();
            effects.work = IRect::new(0, 48, 800, 600);
            effects.dpi = 192;
        }
        r.controller.input(
            id,
            PinInput::Dpi {
                dpi: 192,
                suggested: (-900, -600, 600, 500),
            },
        );
        r.controller.frame();
        let bounds = r.controller.entries[&id].bounds;
        assert_eq!(r.controller.entries[&id].scale, 2.0);
        assert_eq!(pin::clamp(bounds, r.effects.borrow().work), bounds);
        assert_eq!(
            r.effects
                .borrow()
                .events
                .iter()
                .filter(|e| e.starts_with("new surface"))
                .count(),
            creates + 1
        );
    }

    #[test]
    fn deliberate_keyboard_recovery_cycles_actions_and_restores_only_owned_foreground() {
        let mut r = rig();
        let a = r.open(payload(true));
        r.key(a, 0x43, true, false); // Not deliberately focused.
        assert!(r.copies.borrow().is_empty());
        r.controller.show_keyboard(None);
        r.controller.show_keyboard(None); // Must keep target 999.
        assert_eq!(r.controller.entries[&a].focus, Some(PinAction::Copy));
        r.key(a, 0x09, false, true);
        assert_eq!(r.controller.entries[&a].focus, Some(PinAction::Close));
        r.key(a, 0x09, false, false);
        r.key(a, 0x0D, false, false);
        assert_eq!(r.copies.borrow().len(), 1);
        r.key(a, 0x53, true, false);
        r.settle(a);
        r.key(a, 0x1B, false, false);
        RESTORED.with(|restored| assert_eq!(*restored.borrow(), vec![999]));
        let b = r.open(payload(true));
        r.effects.borrow_mut().foreground = 888;
        r.controller.show_keyboard(None);
        r.effects.borrow_mut().foreground = 777; // User switched to another app.
        r.controller.close(b, true);
        RESTORED.with(|restored| assert_eq!(*restored.borrow(), vec![999]));
    }

    #[test]
    fn queued_focus_loss_after_recovery_preserves_keyboard_focus_and_enter() {
        let mut r = rig();
        let id = r.open(payload(true));
        r.controller.show_keyboard(Some(4242));
        r.controller.input(id, PinInput::FocusLost);
        assert_eq!(r.controller.entries[&id].focus, Some(PinAction::Copy));
        r.key(id, 0x0D, false, false);
        assert_eq!(r.copies.borrow().len(), 1);
    }

    #[test]
    fn pre_tray_target_overrides_native_fallback_and_survives_repeated_tray_focus() {
        let mut r = rig();
        let id = r.open(payload(true));
        r.effects.borrow_mut().foreground = 123; // Tray menu owner, not the external app.
        r.controller.show_keyboard(Some(4242));
        assert_eq!(r.controller.entries[&id].previous, Some(4242));
        r.effects.borrow_mut().foreground = 123;
        r.controller.show_keyboard(Some(id.0)); // Tray invoked while the pin had focus.
        assert_eq!(r.controller.entries[&id].previous, Some(4242));
        r.key(id, 0x1B, false, false);
        RESTORED.with(|restored| assert_eq!(*restored.borrow(), vec![4242]));
    }

    #[test]
    fn recovery_from_a_new_external_window_replaces_the_old_focus_destination() {
        let mut r = rig();
        let id = r.open(payload(true));
        r.controller.show_keyboard(Some(4242));
        r.effects.borrow_mut().foreground = 123;
        r.controller.show_keyboard(Some(8484));
        assert_eq!(r.controller.entries[&id].previous, Some(8484));
        r.key(id, 0x1B, false, false);
        RESTORED.with(|restored| assert_eq!(*restored.borrow(), vec![8484]));
    }

    #[test]
    fn repeated_tray_recovery_reaches_other_pins_and_preserves_external_focus_target() {
        let mut r = rig();
        let a = r.open(payload(true));
        let b = r.open(payload(true));
        r.controller.show_keyboard(None);
        assert_eq!(r.effects.borrow().foreground, a.0);
        r.effects.borrow_mut().foreground = 123;
        r.controller.show_keyboard(Some(a.0));
        assert_eq!(r.effects.borrow().foreground, b.0);
        r.key(b, 0x1B, false, false);
        assert!(r.controller.state.contains(a));
        RESTORED.with(|restored| assert_eq!(*restored.borrow(), vec![999]));
    }

    #[test]
    fn device_loss_has_bounded_retries_retains_original_and_can_recover_on_input() {
        let mut r = rig();
        let p = payload(true);
        let weak = Arc::downgrade(&p.bitmap);
        let id = r.open(p);
        r.effects.borrow_mut().fail_render = true;
        r.controller.entries.get_mut(&id).unwrap().dirty = true;
        for _ in 0..10 {
            r.controller.frame();
        }
        assert_eq!(r.controller.entries[&id].retries, MAX_RETRIES);
        assert!(!r.controller.wants_frame());
        assert!(weak.upgrade().is_some());
        r.controller.input(id, PinInput::Paint);
        assert!(!r.controller.wants_frame());
        r.effects.borrow_mut().fail_render = false;
        r.controller.show_keyboard(None);
        assert!(r.controller.wants_frame());
        r.controller.frame();
        assert!(!r.controller.wants_frame());
        assert!(r.controller.entries[&id].surface.is_some());
        assert!(r.controller.entries[&id].status.is_none());
    }

    const LIMIT: u64 = 256 * 1024 * 1024;

    /// A filler admission standing for other pins' accounted payloads (no real memory).
    fn fill(r: &mut Rig, bytes: u64) -> Result<pin::Admission, String> {
        r.controller.state.admit(
            SourceId(u64::MAX - bytes),
            vec![Allocation {
                key: u64::MAX - bytes,
                bytes,
            }],
        )
    }

    #[test]
    fn closing_a_pin_keeps_its_stalled_copy_in_the_budget_until_the_reply_drains() {
        let (store, entered, release) = gated_store(16);
        let mut r = rig_with(store);
        let p = payload(false);
        let weak = Arc::downgrade(&p.bitmap);
        let a = r.open(p);
        r.controller.action(a, PinAction::Copy);
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(r.controller.entries[&a].copying);
        assert_eq!(r.controller.state.holds(), 1);
        r.controller.close(a, true);
        // The encode job still owns the pixels, so admission must keep counting them.
        assert!(weak.upgrade().is_some());
        assert_eq!(r.controller.state.count(), 0);
        assert_eq!(r.controller.state.holds(), 1);
        assert_eq!(r.controller.state.bytes(), 48);
        assert!(fill(&mut r, LIMIT - 47).is_err());
        assert_eq!(r.controller.state.bytes(), 48);
        r.controller.drain();
        assert_eq!(r.controller.state.holds(), 1);
        release.send(()).unwrap();
        r.settle_holds();
        assert!(weak.upgrade().is_none());
        assert_eq!(r.controller.state.holds(), 0);
        assert_eq!(r.controller.state.bytes(), 0);
        assert!(fill(&mut r, LIMIT - 47).is_ok());
        assert!(r.copies.borrow().is_empty());
    }

    #[test]
    fn closing_a_pin_keeps_a_queued_save_with_a_cached_png_held() {
        let (store, entered, release) = gated_store(16);
        let mut r = rig_with(store);
        let b = r.open(payload(false));
        r.controller.action(b, PinAction::Copy);
        entered.recv_timeout(Duration::from_secs(5)).unwrap(); // The worker is now stalled.
        let p = payload(true);
        let png = Arc::downgrade(p.png.as_ref().unwrap());
        let a = r.open(p);
        r.controller.action(a, PinAction::Save);
        assert!(r.controller.entries[&a].saving);
        assert_eq!(r.controller.state.holds(), 2);
        r.controller.close(a, true);
        assert!(png.upgrade().is_some());
        assert_eq!(r.controller.state.bytes(), 48 + (48 + 13));
        assert_eq!(r.controller.state.holds(), 2);
        release.send(()).unwrap();
        let saved = r.settle_holds();
        // The reply is for a closed pin: nothing is reported, but the hold ended.
        assert!(saved.is_empty());
        assert!(png.upgrade().is_none());
        assert_eq!(r.controller.state.holds(), 0);
        // The surviving pin now owns its freshly cached PNG; the closed pin owns nothing.
        assert_eq!(r.controller.state.bytes(), 48 + 3);
        let files: Vec<_> = std::fs::read_dir(r.dir.path()).unwrap().collect();
        assert_eq!(files.len(), 1);
        assert_eq!(
            std::fs::read(files[0].as_ref().unwrap().path()).unwrap(),
            b"synthetic png"
        );
    }

    #[test]
    fn reopening_the_same_arc_while_a_hold_exists_does_not_double_count() {
        let (store, entered, release) = gated_store(16);
        let mut r = rig_with(store);
        let p = payload(false);
        let first = r.open(p.clone());
        r.controller.action(first, PinAction::Copy);
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        r.controller.close(first, true);
        assert_eq!(r.controller.state.bytes(), 48);
        let second = r.open(p.clone());
        assert_ne!(first, second);
        assert_eq!(r.controller.state.bytes(), 48);
        release.send(()).unwrap();
        r.settle_holds();
        assert_eq!(r.controller.state.holds(), 0);
        assert_eq!(r.controller.state.bytes(), 48);
        assert!(r.controller.state.contains(second));
        drop(p);
        r.controller.close(second, true);
        assert_eq!(r.controller.state.bytes(), 0);
    }

    #[test]
    fn a_full_queue_refuses_the_job_and_leaks_no_hold() {
        let (store, entered, release) = gated_store(1);
        let mut r = rig_with(store);
        let ids: Vec<_> = (0..4).map(|_| r.open(payload(false))).collect();
        r.controller.action(ids[0], PinAction::Copy);
        entered.recv_timeout(Duration::from_secs(5)).unwrap(); // Running.
        r.controller.action(ids[1], PinAction::Copy); // Fills the one-slot queue.
        assert_eq!(r.controller.state.holds(), 2);
        r.controller.action(ids[2], PinAction::Copy);
        r.controller.action(ids[3], PinAction::Save);
        assert_eq!(r.controller.state.holds(), 2);
        assert!(!r.controller.entries[&ids[2]].copying);
        assert!(!r.controller.entries[&ids[3]].saving);
        for refused in [ids[2], ids[3]] {
            let (text, error) = r.controller.entries[&refused].status.clone().unwrap();
            assert!(error && text.contains("busy"), "{text}");
        }
        assert!(!r.controller.holds.contains_key(&(ids[2], Op::Copy)));
        assert!(!r.controller.holds.contains_key(&(ids[3], Op::Save)));
        release.send(()).unwrap();
        release.send(()).unwrap();
        r.settle_holds();
        assert_eq!(r.controller.state.holds(), 0);
        assert!(r.controller.holds.is_empty());
    }

    #[test]
    fn replies_for_closed_pins_release_their_holds_and_unknown_keys_are_ignored() {
        let (store, entered, release) = gated_store(16);
        let mut r = rig_with(store);
        let a = r.open(payload(false));
        let b = r.open(payload(false));
        r.controller.action(a, PinAction::Copy);
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        r.controller.action(b, PinAction::Copy);
        r.controller.action(b, PinAction::Copy); // Coalesced: no second hold.
        assert_eq!(r.controller.state.holds(), 2);
        r.controller.close(a, true);
        r.controller.close(b, true);
        assert_eq!(r.controller.state.count(), 0);
        assert_eq!(r.controller.state.bytes(), 96);
        r.controller.release_hold(PinId(999), Op::Save);
        r.controller.release_hold(a, Op::Save); // Wrong operation for this pin.
        assert_eq!(r.controller.state.holds(), 2);
        release.send(()).unwrap();
        release.send(()).unwrap();
        r.settle_holds();
        assert_eq!(r.controller.state.bytes(), 0);
        assert!(r.controller.holds.is_empty());
    }

    #[test]
    fn encoder_panics_release_matching_holds_for_live_and_closed_pins_and_allow_retry() {
        for closed_action in [PinAction::Copy, PinAction::Save] {
            let (entered_tx, entered) = mpsc::channel();
            let (release, release_rx) = mpsc::channel::<bool>();
            let (entered_tx, release_rx) = (Mutex::new(entered_tx), Mutex::new(release_rx));
            let encoder: Encoder = Arc::new(move |_| {
                entered_tx.lock().unwrap().send(()).unwrap();
                let should_panic = release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .expect("the encoder gate was never opened");
                // Panic outside the mutex guard so subsequent jobs can use the gate.
                assert!(!should_panic, "synthetic encoder panic");
                Ok(b"PNG".to_vec())
            });
            let mut r = rig_with(Store::with_parts(Box::new(|| {}), encoder, 16));
            let live = r.open(payload(false));
            let p = payload(false);
            let closed_bitmap = Arc::downgrade(&p.bitmap);
            let closed = r.open(p);
            let other = r.open(payload(true));
            r.controller.action(live, PinAction::Copy);
            entered.recv_timeout(Duration::from_secs(5)).unwrap();
            r.controller.action(live, PinAction::Save);
            r.controller.action(closed, closed_action);
            r.controller.close(closed, true);
            assert_eq!(r.controller.state.holds(), 3);
            assert_eq!(r.controller.state.bytes(), 48 + 48 + 61);
            assert!(closed_bitmap.upgrade().is_some());

            release.send(true).unwrap(); // Copy panics; Save then blocks at the gate.
            entered.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(r.controller.drain().is_empty());
            assert_eq!(r.controller.state.holds(), 2);
            assert!(!r.controller.holds.contains_key(&(live, Op::Copy)));
            assert!(r.controller.holds.contains_key(&(live, Op::Save)));
            let entry = &r.controller.entries[&live];
            assert!(!entry.copying && entry.saving);
            assert_eq!(
                entry.status,
                Some(("Copy failed: the worker crashed".into(), true))
            );

            release.send(true).unwrap(); // Save panics; the closed pin's job blocks.
            entered.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(r.controller.drain().is_empty());
            assert_eq!(r.controller.state.holds(), 1);
            assert!(!r.controller.holds.contains_key(&(live, Op::Save)));
            let entry = &r.controller.entries[&live];
            assert!(!entry.copying && !entry.saving);
            let failed_save = Some(("Save failed: the worker crashed".into(), true));
            assert_eq!(entry.status, failed_save);

            release.send(true).unwrap(); // A panic reply for a closed pin still releases.
            assert!(r.settle_holds().is_empty());
            assert!(closed_bitmap.upgrade().is_none());
            assert!(r.controller.holds.is_empty());
            assert_eq!(r.controller.state.bytes(), 48 + 61);
            assert!(!r.controller.state.contains(closed));
            assert_eq!(r.controller.entries[&live].status, failed_save);
            let entry = &r.controller.entries[&other];
            assert!(!entry.copying && !entry.saving);
            assert!(entry.status.is_none() && entry.saved.is_none());
            assert!(r.copies.borrow().is_empty());
            assert!(std::fs::read_dir(r.dir.path()).unwrap().next().is_none());

            r.controller.action(live, PinAction::Copy);
            entered.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(r.controller.state.holds(), 1);
            release.send(false).unwrap();
            r.settle(live);
            assert_eq!(r.controller.state.holds(), 0);
            assert!(r.controller.holds.is_empty());
            assert_eq!(*r.copies.borrow(), vec![b"PNG".to_vec()]);
            assert_eq!(
                r.controller.entries[&live].status,
                Some(("Copied".into(), false))
            );
            assert!(r.controller.entries[&other].status.is_none());
        }
    }

    #[test]
    fn normal_copy_and_save_completion_leave_no_hold() {
        let mut r = rig();
        let id = r.open(payload(false));
        r.controller.action(id, PinAction::Copy);
        r.settle(id);
        assert_eq!(r.controller.state.holds(), 0);
        assert!(r.controller.holds.is_empty());
        // The PNG is cached now: copy and save are immediate or save-only, never leaky.
        r.controller.action(id, PinAction::Copy);
        assert_eq!(r.controller.state.holds(), 0);
        r.controller.action(id, PinAction::Save);
        assert_eq!(r.controller.state.holds(), 1);
        r.settle(id);
        assert_eq!(r.controller.state.holds(), 0);
        assert_eq!(r.copies.borrow().len(), 2);
        let cached = r.controller.store.get(id.0).unwrap().bytes();
        assert_eq!(r.controller.state.bytes(), cached);
        // Opening an existing file (reveal) starts no job and takes no hold.
        r.controller.action(id, PinAction::Save);
        assert_eq!(r.controller.state.holds(), 0);
        r.controller.shutdown();
        assert_eq!(r.controller.state.bytes(), 0);
    }

    #[test]
    fn a_refused_hold_queues_nothing_and_shows_a_visible_failure() {
        let mut r = rig();
        let copy = r.open(payload(false));
        let save = r.open(payload(false));
        fill(&mut r, LIMIT - 96).unwrap();
        assert_eq!(r.controller.state.bytes(), LIMIT);
        // Swap in payloads the ledger has not counted: holding them would break the limit.
        let (fresh_copy, fresh_save) = (payload(false), payload(false));
        r.controller.store.insert(copy.0, fresh_copy);
        r.controller.store.insert(save.0, fresh_save);
        r.controller.action(copy, PinAction::Copy);
        r.controller.action(save, PinAction::Save);
        for (id, label) in [
            (copy, "Copy failed: memory limit"),
            (save, "Save failed: memory limit"),
        ] {
            let entry = &r.controller.entries[&id];
            assert_eq!(entry.status, Some((label.to_string(), true)));
            assert!(!entry.copying && !entry.saving);
        }
        assert_eq!(r.controller.state.holds(), 0);
        assert_eq!(r.controller.state.bytes(), LIMIT);
        std::thread::sleep(Duration::from_millis(20));
        assert!(r.controller.drain().is_empty());
        assert!(std::fs::read_dir(r.dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn open_failure_while_holds_are_pending_asks_to_finish_them() {
        let (store, entered, release) = gated_store(16);
        let mut r = rig_with(store);
        let a = r.open(payload(false));
        r.controller.action(a, PinAction::Copy);
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        r.controller.close(a, true);
        fill(&mut r, LIMIT - 48).unwrap();
        let error = r.controller.open(payload(false), None, (0, 0)).unwrap_err();
        assert!(error.contains("256 MiB") && error.contains("pending copies or saves"));
        release.send(()).unwrap();
        r.settle_holds();
        assert!(r.controller.open(payload(false), None, (0, 0)).is_ok());
        // The budget is full again; with no holds left the plain message is unchanged.
        let error = r.controller.open(payload(false), None, (0, 0)).unwrap_err();
        assert!(error.contains("256 MiB") && !error.contains("pending"));
    }

    #[test]
    fn keyboard_owner_is_the_live_pin_with_the_foreground() {
        let mut r = rig();
        let a = r.open(payload(true));
        let b = r.open(payload(true));
        assert_eq!(r.controller.keyboard_owner(), None);
        r.controller.show_keyboard(None);
        assert_eq!(r.controller.keyboard_owner(), Some(a));
        r.effects.borrow_mut().foreground = b.0;
        assert_eq!(r.controller.keyboard_owner(), Some(b));
        r.controller.close(b, false);
        assert_eq!(r.controller.keyboard_owner(), None);
        r.effects.borrow_mut().foreground = 123; // Another app.
        assert_eq!(r.controller.keyboard_owner(), None);
    }

    #[test]
    fn refocus_after_capture_keeps_the_external_target_for_escape() {
        let mut r = rig();
        let id = r.open(payload(true));
        r.controller.show_keyboard(Some(4242));
        assert_eq!(r.controller.keyboard_owner(), Some(id));
        r.capture(true);
        r.effects.borrow_mut().foreground = 555; // The capture overlay took the keyboard.
        r.capture(false);
        assert!(r.controller.entries[&id].shown);
        let focused = r.focus_events(id);
        assert!(r.controller.refocus(id));
        assert_eq!(r.focus_events(id), focused + 1);
        assert_eq!(r.controller.keyboard_owner(), Some(id));
        let entry = &r.controller.entries[&id];
        assert_eq!(entry.previous, Some(4242));
        assert_eq!(entry.focus, Some(PinAction::Copy));
        assert!(entry.dirty);
        r.key(id, 0x1B, false, false);
        RESTORED.with(|restored| assert_eq!(*restored.borrow(), vec![4242]));
        assert!(!r.controller.state.contains(id));
    }

    #[test]
    fn refocus_denied_activation_clears_the_ring_and_preserves_the_external_target() {
        for queued_focus_loss in [false, true] {
            let mut r = rig();
            let id = r.open(payload(true));
            r.controller.show_keyboard(Some(4242));
            r.capture(true);
            r.effects.borrow_mut().foreground = 555;
            if !queued_focus_loss {
                r.controller.input(id, PinInput::FocusLost);
            }
            r.capture(false);
            r.controller.frame();
            r.effects.borrow_mut().deny_focus = true;
            let attempts = r.focus_events(id);
            assert!(!r.controller.refocus(id));
            assert_eq!(r.focus_events(id), attempts + 1);
            assert_eq!(r.effects.borrow().foreground, 555);
            assert_eq!(r.controller.keyboard_owner(), None);
            let entry = &r.controller.entries[&id];
            assert_eq!(entry.focus, None);
            assert_eq!(entry.previous, Some(4242));
            assert!(entry.dirty); // Repaint any ring left by the queued FocusLost.
            r.key(id, 0x43, true, false);
            assert!(r.copies.borrow().is_empty());
            RESTORED.with(|restored| assert!(restored.borrow().is_empty()));

            r.effects.borrow_mut().deny_focus = false;
            assert!(r.controller.refocus(id));
            assert_eq!(r.controller.entries[&id].focus, Some(PinAction::Copy));
            r.key(id, 0x1B, false, false);
            RESTORED.with(|restored| assert_eq!(*restored.borrow(), vec![4242]));
        }
    }

    #[test]
    fn tray_recovery_denied_activation_clears_the_ring_without_replacing_the_target() {
        for queued_focus_loss in [false, true] {
            let mut r = rig();
            let id = r.open(payload(true));
            r.controller.show_keyboard(Some(4242));
            r.effects.borrow_mut().foreground = 555;
            if !queued_focus_loss {
                r.controller.input(id, PinInput::FocusLost);
            }
            r.controller.frame();
            r.effects.borrow_mut().deny_focus = true;
            let attempts = r.focus_events(id);
            r.controller.show_keyboard(Some(8484));
            assert_eq!(r.focus_events(id), attempts + 1);
            assert_eq!(r.effects.borrow().foreground, 555);
            assert_eq!(r.controller.keyboard_owner(), None);
            let entry = &r.controller.entries[&id];
            assert_eq!(entry.focus, None);
            assert_eq!(entry.previous, Some(4242));
            assert!(entry.dirty);
            r.key(id, 0x0D, false, false);
            assert!(r.copies.borrow().is_empty());
            RESTORED.with(|restored| assert!(restored.borrow().is_empty()));

            r.effects.borrow_mut().deny_focus = false;
            r.controller.show_keyboard(Some(8484));
            assert_eq!(r.controller.entries[&id].focus, Some(PinAction::Copy));
            r.key(id, 0x1B, false, false);
            RESTORED.with(|restored| assert_eq!(*restored.borrow(), vec![8484]));
        }
    }

    #[test]
    fn refocus_defaults_the_action_and_is_idempotent_when_already_foreground() {
        let mut r = rig();
        let id = r.open(payload(true));
        assert!(r.controller.entries[&id].focus.is_none());
        assert!(r.controller.refocus(id));
        assert_eq!(r.controller.entries[&id].focus, Some(PinAction::Copy));
        assert_eq!(r.controller.entries[&id].previous, None);
        r.controller.entries.get_mut(&id).unwrap().focus = Some(PinAction::Close);
        let focused = r.focus_events(id);
        assert!(r.controller.refocus(id));
        // Already foreground: no second activation, and a chosen action stays chosen.
        assert_eq!(r.focus_events(id), focused);
        assert_eq!(r.controller.entries[&id].focus, Some(PinAction::Close));
    }

    #[test]
    fn refocus_refuses_closed_capturing_hidden_and_unshown_pins_without_activating() {
        let mut r = rig();
        let closed = r.open(payload(true));
        let id = r.open(payload(true));
        let hidden = r.open(payload(true));
        r.controller.close(closed, false);
        assert!(!r.controller.refocus(closed));
        r.capture(true);
        assert!(!r.controller.refocus(id));
        r.capture(false);
        r.controller.state.set_allowed(false);
        r.controller.allowed = false;
        r.controller.sync_visibility();
        assert!(!r.controller.refocus(id));
        r.controller.state.set_allowed(true);
        r.controller.allowed = true;
        r.controller.sync_visibility();
        r.controller.state.set_visible(hidden, false);
        r.controller.sync_visibility();
        assert!(!r.controller.refocus(hidden));
        assert!(!r.controller.entries[&hidden].shown); // Refusal never reveals it.
        r.controller.entries.get_mut(&id).unwrap().shown = false;
        assert!(!r.controller.refocus(id));
        for pin in [closed, id, hidden] {
            assert_eq!(r.focus_events(pin), 0);
        }
        assert_eq!(r.effects.borrow().foreground, 999);
        assert!(r.controller.entries[&hidden].focus.is_none());
    }

    #[test]
    fn a_stale_focus_loss_after_refocus_does_not_clear_keyboard_focus() {
        let mut r = rig();
        let id = r.open(payload(true));
        r.controller.show_keyboard(Some(4242));
        r.capture(true);
        r.effects.borrow_mut().foreground = 555;
        r.capture(false);
        assert!(r.controller.refocus(id));
        // Queued by the capture overlay's activation before the keyboard came back.
        r.controller.input(id, PinInput::FocusLost);
        assert_eq!(r.controller.entries[&id].focus, Some(PinAction::Copy));
        r.key(id, 0x0D, false, false);
        assert_eq!(r.copies.borrow().len(), 1);
    }

    #[test]
    fn down_acknowledges_the_native_press_and_a_body_drag_moves_the_window() {
        let mut r = rig();
        let id = r.open(payload(true));
        let original = r.controller.entries[&id].bounds;
        r.down(id, (1, 1)); // Native press first, then the queued Down.
        assert_eq!(r.effects.borrow().native_press, Some(id.0));
        assert_eq!(r.effects.borrow().captured, Some(id.0));
        assert!(r.controller.entries[&id].drag.is_some());
        r.move_to(id, (-900, 300)); // Down was at (-1000, 200).
        let bounds = r.controller.entries[&id].bounds;
        assert!(r.controller.entries[&id].drag.is_some());
        assert_eq!((bounds.x, bounds.y), (original.x + 100, original.y + 100));
        assert_eq!(r.effects.borrow().placed, Some(bounds));
        assert_eq!(r.effects.borrow().native_press, Some(id.0));
    }

    #[test]
    fn down_and_up_on_copy_runs_copy_and_never_kills_the_press_early() {
        let mut r = rig();
        let id = r.open(payload(true));
        let copy = r.button(id, PinAction::Copy);
        r.down(id, copy);
        assert_eq!(r.effects.borrow().native_press, Some(id.0));
        assert_eq!(
            r.controller.entries[&id].pressed,
            Some(Hit::Action(PinAction::Copy))
        );
        assert!(r.controller.entries[&id].drag.is_none());
        r.up(id, copy);
        assert_eq!(r.copies.borrow().len(), 1);
        assert_eq!(r.effects.borrow().native_press, None);
        assert_eq!(r.effects.borrow().captured, None);
    }

    #[test]
    fn a_quick_click_whose_native_press_already_finished_still_runs_the_action() {
        let mut r = rig();
        let id = r.open(payload(true));
        let copy = r.button(id, PinAction::Copy);
        // Native Down and Up were both dispatched before the controller drained.
        r.native_down(id);
        r.native_end();
        r.down_at(id, copy, (-1000, 200));
        assert!(!r.controller.entries[&id].captured);
        assert_eq!(
            r.controller.entries[&id].pressed,
            Some(Hit::Action(PinAction::Copy))
        );
        r.up(id, copy);
        assert_eq!(r.copies.borrow().len(), 1);
        // A finished press never becomes a body drag.
        r.native_down(id);
        r.native_end();
        r.down_at(id, (1, 1), (-1000, 200));
        assert!(r.controller.entries[&id].drag.is_none());
        r.up(id, (1, 1));
        // A CancelCapture queued between Down and Up clears the action.
        r.native_down(id);
        r.native_end();
        r.down_at(id, copy, (-1000, 200));
        r.cancel(id);
        r.up(id, copy);
        assert_eq!(r.copies.borrow().len(), 1);
    }

    #[test]
    fn stale_cancels_and_relayouts_queued_before_a_new_press_do_not_kill_it() {
        let mut r = rig();
        let id = r.open(payload(true));
        let copy = r.button(id, PinAction::Copy);
        r.down(id, (1, 1));
        r.native_end(); // Press 1 ended; its CancelCapture is still queued.
        r.native_down(id); // Press 2 is already live natively.
        r.controller.input(id, PinInput::CancelCapture);
        assert_eq!(r.effects.borrow().native_press, Some(id.0));
        r.controller.input(id, PinInput::FocusLost);
        assert_eq!(r.effects.borrow().native_press, Some(id.0));
        r.controller.display_changed();
        assert_eq!(r.effects.borrow().native_press, Some(id.0));
        r.down_at(id, copy, (-1000, 200));
        assert_eq!(r.effects.borrow().captured, Some(id.0));
        assert!(r.controller.entries[&id].captured);
        r.up(id, copy);
        assert_eq!(r.copies.borrow().len(), 1);
    }

    #[test]
    fn controller_cancel_ends_only_a_press_it_acknowledged() {
        let mut r = rig();
        let id = r.open(payload(true));
        r.down(id, (1, 1));
        r.controller.display_changed(); // Display change cancels our own drag.
        assert_eq!(r.effects.borrow().native_press, None);
        assert!(r.controller.entries[&id].drag.is_none());
    }

    #[test]
    fn dpi_crossing_to_the_right_picks_the_monitor_by_centre_and_does_not_bounce() {
        let mut r = rig();
        let a = IRect::new(0, 32, 1600, 900);
        let b = IRect::new(1600, 32, 1600, 900);
        {
            let mut effects = r.effects.borrow_mut();
            effects.work = a; // Fallback for off-monitor points such as (0, 0).
            effects.monitors = vec![(a, 96), (b, 192)];
        }
        let id = r.controller.open(payload(true), None, (1000, 100)).unwrap();
        assert_eq!(r.controller.entries[&id].scale, 1.0);
        r.native_down(id);
        r.down_at(id, (1, 1), (1100, 150));
        r.move_to(id, (1550, 150));
        let before = r.controller.entries[&id].bounds;
        assert_eq!((before.x, before.y), (1450, 100));
        // The suggested top-left is still on A; its centre is on B.
        r.controller.input(
            id,
            PinInput::Dpi {
                dpi: 192,
                suggested: (1450, 100, 512, 182),
            },
        );
        let entry = &r.controller.entries[&id];
        assert_eq!(entry.scale, 2.0);
        assert!(entry.drag.is_some());
        assert_eq!(r.effects.borrow().native_press, Some(id.0));
        // Not clamped back onto A (which would put it at x = 1088); the grab point
        // (100, 50) scales to (200, 100) and stays under the cursor.
        assert_eq!((entry.bounds.x, entry.bounds.y), (1350, 50));
        assert!(entry.bounds.right() > a.right());
        r.move_to(id, (1600, 150));
        assert_eq!(r.controller.entries[&id].bounds.x, 1400);
        // The release clamps into the monitor it ended on.
        r.native_end();
        r.controller.input(
            id,
            PinInput::Up {
                x: 1,
                y: 1,
                screen_x: 1600,
                screen_y: 150,
            },
        );
        let bounds = r.controller.entries[&id].bounds;
        assert!(bounds.x >= b.x && bounds.right() <= b.right());
        assert!(bounds.y >= b.y && bounds.bottom() <= b.bottom());
        assert!(r.controller.entries[&id].drag.is_none());
    }

    #[test]
    fn closing_the_foreground_pin_hides_and_restores_focus_before_destroying_the_window() {
        let mut r = rig();
        let id = r.open(payload(true));
        r.controller.show_keyboard(Some(4242));
        assert!(r.controller.entries[&id].shown);
        r.key(id, 0x1B, false, false);
        ORDER.with(|order| {
            assert_eq!(
                *order.borrow(),
                vec![
                    format!("hide {}", id.0),
                    "restore 4242".to_string(),
                    format!("drop window {}", id.0),
                ]
            );
        });
        // Without foreground ownership nothing is restored, but the window still hides first.
        let other = r.open(payload(true));
        ORDER.with(|order| order.borrow_mut().clear());
        r.controller.close(other, true);
        ORDER.with(|order| {
            assert_eq!(
                *order.borrow(),
                vec![
                    format!("hide {}", other.0),
                    format!("drop window {}", other.0)
                ]
            );
        });
    }
}
