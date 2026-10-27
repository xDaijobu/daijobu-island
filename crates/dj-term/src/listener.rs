//! What the emulator tells us, and the replies it expects.
//!
//! The emulator calls [`Listener::send_event`] from the PTY thread, usually while it holds the
//! terminal lock. Nothing here may lock the terminal or block: events are queued, replies go
//! through a channel, and the UI is woken at most once until it has drained the queue.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::vte::ansi::Rgb as VRgb;
use dj_core::terminal::Palette;

use crate::screen::palette_color;

/// Something the app may want to react to, delivered by [`crate::Session::take_events`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermEvent {
    /// The program set the window title (OSC 0 / 2). Control characters are removed.
    Title(String),
    /// The program asked for the default title again.
    ResetTitle,
    Bell,
    /// The shell ended, with its exit code when Windows gave one. Sent once.
    Exited(Option<i32>),
}

/// Writes bytes to the shell's input. Set once the event loop exists.
pub(crate) type Reply = Box<dyn Fn(Vec<u8>) + Send + Sync>;

/// State shared between the PTY thread (through [`Listener`]) and the UI thread.
pub(crate) struct Shared {
    wake: Box<dyn Fn() + Send + Sync>,
    /// `true` from the moment `wake` was called until the next `take_events`.
    armed: AtomicBool,
    /// Set while the session is being dropped: no more wakes.
    closed: AtomicBool,
    events: Mutex<Vec<TermEvent>>,
    pub(crate) exited: AtomicBool,
    /// The palette of the last `screen()` call, used to answer colour queries (OSC 4/10/11).
    palette: Mutex<Option<Palette>>,
    size: Mutex<WindowSize>,
    pub(crate) reply: OnceLock<Reply>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    pub(crate) fn new(wake: Box<dyn Fn() + Send + Sync>, size: WindowSize) -> Self {
        Shared {
            wake,
            armed: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            events: Mutex::new(Vec::new()),
            exited: AtomicBool::new(false),
            palette: Mutex::new(None),
            size: Mutex::new(size),
            reply: OnceLock::new(),
        }
    }

    /// Calls `wake` only when it flips `armed` from false to true.
    pub(crate) fn wake_once(&self) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        if !self.armed.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }

    pub(crate) fn push(&self, event: TermEvent) {
        lock(&self.events).push(event);
        self.wake_once();
    }

    /// Clears the wake flag first, then drains, so an event that lands in between is seen
    /// now or triggers a new wake, never neither.
    pub(crate) fn take_events(&self) -> Vec<TermEvent> {
        self.armed.store(false, Ordering::Release);
        std::mem::take(&mut *lock(&self.events))
    }

    /// As `take_events`, and whether anything woke the UI since the last call (output or an
    /// event): a tab in the background uses it to show that it printed something.
    pub(crate) fn drain(&self) -> (bool, Vec<TermEvent>) {
        let woke = self.armed.swap(false, Ordering::AcqRel);
        (woke, std::mem::take(&mut *lock(&self.events)))
    }

    /// Queues `Exited` the first time only.
    pub(crate) fn mark_exited(&self, code: Option<i32>) {
        if !self.exited.swap(true, Ordering::AcqRel) {
            self.push(TermEvent::Exited(code));
        }
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub(crate) fn set_palette(&self, palette: &Palette) {
        *lock(&self.palette) = Some(*palette);
    }

    pub(crate) fn set_size(&self, size: WindowSize) {
        *lock(&self.size) = size;
    }

    fn send(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        if let Some(reply) = self.reply.get() {
            reply(bytes);
        }
    }
}

/// Removes control characters from a title: it ends up in the island's header.
pub(crate) fn clean_title(title: &str) -> String {
    title.chars().filter(|c| !c.is_control()).collect()
}

#[derive(Clone)]
pub(crate) struct Listener(pub(crate) std::sync::Arc<Shared>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let shared = &self.0;
        match event {
            Event::Wakeup => shared.wake_once(),
            Event::Title(title) => shared.push(TermEvent::Title(clean_title(&title))),
            Event::ResetTitle => shared.push(TermEvent::ResetTitle),
            Event::Bell => shared.push(TermEvent::Bell),
            Event::ChildExit(status) => shared.mark_exited(status.code()),
            // Sent by the loop right after `ChildExit`; it carries no code of its own.
            Event::Exit => shared.mark_exited(None),
            // Answers the shell expects (cursor position report, device attributes, ...).
            Event::PtyWrite(text) => shared.send(text.into_bytes()),
            Event::ColorRequest(index, format) => {
                let palette = *lock(&shared.palette);
                if let Some(c) = palette.and_then(|p| palette_color(index, &p)) {
                    let [r, g, b] = c.to_u8();
                    shared.send(format(VRgb { r, g, b }).into_bytes());
                }
            }
            Event::TextAreaSizeRequest(format) => {
                let size = *lock(&shared.size);
                shared.send(format(size).into_bytes());
            }
            // OSC 52 is disabled in the emulator's config; these are never sent, and if a
            // future version did, a program still must not touch the user's clipboard.
            Event::ClipboardStore(..) | Event::ClipboardLoad(..) => {}
            // The UI redraws on its own schedule; there is no blink and no mouse cursor.
            Event::MouseCursorDirty | Event::CursorBlinkingChange => {}
        }
    }
}
