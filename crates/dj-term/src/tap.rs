//! The PTY with a scanner on its read side: every byte the shell prints is passed on unchanged
//! and unbuffered, and a copy of the view of it goes through [`CwdScanner`], which reports the
//! shell's current folder as [`TermEvent::Cwd`].

use std::io;
use std::sync::Arc;

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{self, ChildEvent, EventedPty, EventedReadWrite};
use polling::{Event, PollMode, Poller};

use crate::listener::{Shared, TermEvent};
use crate::osc::CwdScanner;

pub(crate) struct Tapped {
    pty: tty::Pty,
    scanner: CwdScanner,
    shared: Arc<Shared>,
}

impl Tapped {
    pub(crate) fn new(pty: tty::Pty, shared: Arc<Shared>) -> Self {
        Tapped {
            pty,
            scanner: CwdScanner::new(),
            shared,
        }
    }
}

impl io::Read for Tapped {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.pty.reader().read(buf)?;
        if n > 0 {
            if let Some(path) = self.scanner.feed(&buf[..n]) {
                self.shared.push(TermEvent::Cwd(path));
            }
        }
        Ok(n)
    }
}

impl EventedReadWrite for Tapped {
    type Reader = Self;
    type Writer = <tty::Pty as EventedReadWrite>::Writer;

    unsafe fn register(
        &mut self,
        poll: &Arc<Poller>,
        interest: Event,
        mode: PollMode,
    ) -> io::Result<()> {
        // SAFETY: the caller's contract is passed on: the sources inside `self.pty` outlive
        // their registration, because `self` owns the pty and is only dropped after the event
        // loop has deregistered it.
        unsafe { self.pty.register(poll, interest, mode) }
    }

    fn reregister(
        &mut self,
        poll: &Arc<Poller>,
        interest: Event,
        mode: PollMode,
    ) -> io::Result<()> {
        self.pty.reregister(poll, interest, mode)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        self.pty.deregister(poll)
    }

    fn reader(&mut self) -> &mut Self::Reader {
        self
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self.pty.writer()
    }
}

impl EventedPty for Tapped {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.pty.next_child_event()
    }
}

impl OnResize for Tapped {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.pty.on_resize(window_size);
    }
}
