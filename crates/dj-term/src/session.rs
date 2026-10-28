//! A shell in a ConPTY, wired to the emulator.
//!
//! Threads: the emulator crate's event loop runs on its own thread ("PTY reader") and owns the
//! `Pty`; it parses the shell's output into the shared `Term` and calls our `Listener`. Two
//! helper threads inside the crate pump the ConPTY pipes. The UI thread only locks the `Term`
//! briefly to read a `Screen` or change the selection, and talks to the loop through its
//! channel. A small watcher thread joins the loop thread: when the loop ends, for whatever
//! reason, it reports `Exited` (once) and closes the ConPTY. Nothing polls: the loop blocks in
//! the OS until there is output or a message, and the watcher blocks in `join`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Osc52, Term, MIN_COLUMNS, MIN_SCREEN_LINES};
use alacritty_terminal::tty;
use dj_core::terminal::{Modes, Palette, Screen};

use crate::child::Child;
use crate::listener::{Listener, Shared, TermEvent};
use crate::screen;
use crate::shell;

/// How long `Drop` waits for the loop thread and the pseudo console to close.
const DROP_BOUND: Duration = Duration::from_millis(2000);
/// The largest grid we accept (ConPTY sizes are 16-bit signed).
const MAX_DIM: u16 = 500;

/// How to start the shell. `program` and `args` are normally the result of
/// [`crate::resolve_shell`].
#[derive(Clone, Debug)]
pub struct SpawnOptions {
    pub program: String,
    pub args: Vec<String>,
    /// `None`, or a directory that does not exist: the user's home.
    pub cwd: Option<PathBuf>,
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
    /// Lines of history to keep.
    pub scrollback: usize,
}

/// The emulator's size type for `Term`.
pub(crate) struct Size {
    pub(crate) cols: usize,
    pub(crate) rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub(crate) fn clamp_dims(cols: u16, rows: u16) -> (u16, u16) {
    (
        cols.clamp(MIN_COLUMNS as u16, MAX_DIM),
        rows.clamp(MIN_SCREEN_LINES as u16, MAX_DIM),
    )
}

/// Cell sizes are only reported back to programs (CSI 14 t), where the emulator multiplies them
/// by the grid size in `u16`; the cap keeps 500 x 100 inside that.
pub(crate) fn window_size(cols: u16, rows: u16, cell_w: u16, cell_h: u16) -> WindowSize {
    WindowSize {
        num_lines: rows,
        num_cols: cols,
        cell_width: cell_w.clamp(1, 100),
        cell_height: cell_h.clamp(1, 100),
    }
}

/// The emulator's settings for us. OSC 52 is off, so a program cannot put text on (or read) the
/// user's clipboard.
pub(crate) fn term_config(scrollback: usize) -> Config {
    Config {
        scrolling_history: scrollback,
        osc52: Osc52::Disabled,
        ..Config::default()
    }
}

/// One running shell: the emulated screen, the ConPTY and the thread that feeds the screen.
/// Dropping it ends the shell.
pub struct Session {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    shared: Arc<Shared>,
    /// Joins the event loop thread, reports its end and closes the ConPTY.
    watcher: Option<JoinHandle<()>>,
    child: Option<Child>,
    size: WindowSize,
}

impl Session {
    /// Starts the shell in a ConPTY. `wake` is called (from the PTY thread) when there is
    /// something new, at most once until the next [`Session::take_events`].
    ///
    /// `wake` runs while the emulator holds the terminal lock: it must only post a message
    /// (for example `PostMessageW`) and never call back into the `Session`, or it deadlocks.
    ///
    /// Fails, without panicking, when the program does not exist or the pseudo console cannot
    /// be created.
    pub fn spawn(
        opts: &SpawnOptions,
        wake: Box<dyn Fn() + Send + Sync>,
    ) -> Result<Session, String> {
        let located = shell::locate_program(&opts.program)
            .ok_or_else(|| format!("shell not found: {}", opts.program))?;
        let (cols, rows) = clamp_dims(opts.cols, opts.rows);
        let size = window_size(cols, rows, opts.cell_w, opts.cell_h);

        let shared = Arc::new(Shared::new(wake, size));
        let listener = Listener(shared.clone());
        let term = Term::new(
            term_config(opts.scrollback),
            &Size {
                cols: cols as usize,
                rows: rows as usize,
            },
            listener.clone(),
        );
        let term = Arc::new(FairMutex::new(term));

        let options = tty::Options {
            shell: Some(tty::Shell::new(
                shell::quote_program(&located.to_string_lossy()),
                opts.args.clone(),
            )),
            working_directory: shell::pick_cwd(opts.cwd.as_deref()),
            // Read what the shell printed just before it exited ("exit", `cmd /c echo`).
            drain_on_exit: true,
            // What Alacritty advertises; harmless to Windows programs, useful to Git's tools.
            env: HashMap::from([
                ("TERM".to_string(), "xterm-256color".to_string()),
                ("COLORTERM".to_string(), "truecolor".to_string()),
            ]),
            #[cfg(windows)]
            escape_args: true,
        };

        // The shell inherits our Ctrl+C setting: make sure it gets Ctrl+C.
        crate::child::allow_ctrl_c();
        // `tty::new` asserts on a failed `CreatePseudoConsole`; turn that into an error too.
        let pty = catch_unwind(AssertUnwindSafe(|| tty::new(&options, size, 0)))
            .map_err(|_| "the pseudo console could not be created".to_string())?
            .map_err(|e| format!("could not start {}: {e}", opts.program))?;

        let child = Child::of(&pty);

        let pty = crate::tap::Tapped::new(pty, shared.clone());

        let event_loop = match EventLoop::new(term.clone(), listener, pty, true, false) {
            Ok(l) => l,
            Err(e) => {
                // The `Pty` was dropped by the failed constructor; make sure the shell dies.
                if let Some(child) = &child {
                    child.terminate();
                }
                return Err(format!("could not start the terminal loop: {e}"));
            }
        };
        let sender = event_loop.channel();
        let reply_to = sender.clone();
        let _ = shared.reply.set(Box::new(move |bytes| {
            let _ = reply_to.send(Msg::Input(Cow::Owned(bytes)));
        }));
        let loop_thread = event_loop.spawn();

        let watched = shared.clone();
        let watcher = std::thread::Builder::new()
            .name("dj-term-watch".into())
            .spawn(move || {
                // Blocks until the loop ends: the shell exited (it already said so), or the loop
                // failed or panicked (it did not). Say so once, then close the ConPTY.
                let ended = loop_thread.join();
                watched.mark_exited(None);
                drop(ended);
            });
        let watcher = match watcher {
            Ok(w) => w,
            Err(e) => {
                let _ = sender.send(Msg::Shutdown);
                if let Some(child) = &child {
                    child.terminate();
                }
                return Err(format!("could not start the terminal watcher: {e}"));
            }
        };

        Ok(Session {
            term,
            sender,
            shared,
            watcher: Some(watcher),
            child,
            size,
        })
    }

    /// Sends bytes to the shell. Output that was scrolled back snaps to the bottom, like typing
    /// in any terminal; the selection is left to the caller.
    pub fn write(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        {
            let mut term = self.term.lock();
            if term.grid().display_offset() > 0 {
                term.scroll_display(Scroll::Bottom);
            }
        }
        let _ = self.sender.send(Msg::Input(Cow::Owned(bytes)));
    }

    /// Resizes the grid and the ConPTY. Sizes are clamped to 2..=500 columns and 1..=500 rows.
    pub fn resize(&mut self, cols: u16, rows: u16, cell_w: u16, cell_h: u16) {
        let (cols, rows) = clamp_dims(cols, rows);
        let new = window_size(cols, rows, cell_w, cell_h);
        let grid_changed = (cols, rows) != (self.size.num_cols, self.size.num_lines);
        self.size = new;
        self.shared.set_size(new);
        if !grid_changed {
            return;
        }
        self.term.lock().resize(Size {
            cols: cols as usize,
            rows: rows as usize,
        });
        let _ = self.sender.send(Msg::Resize(new));
    }

    /// Scrolls the view; positive goes back into history.
    pub fn scroll(&mut self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    /// Wheel over a visible cell: applications that enable mouse tracking receive
    /// reports, alternate-screen programs receive arrows, ordinary shells scroll history.
    pub fn wheel(&mut self, lines: i32, col: usize, row: usize) {
        let bytes = crate::wheel::route(&mut self.term.lock(), lines, col, row);
        if !bytes.is_empty() {
            // Do not use `write`: wheel reports aren't typing and must not reset
            // the viewport or selection.
            let _ = self.sender.send(Msg::Input(Cow::Owned(bytes)));
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    /// Starts a selection at a visible cell. `word` selects the word under it (double click).
    pub fn select_start(&mut self, col: usize, row: usize, right_half: bool, word: bool) {
        screen::select_start(&mut self.term.lock(), col, row, right_half, word);
    }

    pub fn select_update(&mut self, col: usize, row: usize, right_half: bool) {
        screen::select_update(&mut self.term.lock(), col, row, right_half);
    }

    pub fn select_clear(&mut self) {
        self.term.lock().selection = None;
    }

    /// `true` for a non-empty selection (a click without a drag selects nothing).
    pub fn has_selection(&self) -> bool {
        screen::has_selection(&self.term.lock())
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string()
    }

    pub fn modes(&self) -> Modes {
        screen::modes_of(&self.term.lock())
    }

    /// The visible grid with colours resolved through `palette`. `focused: false` turns a block
    /// cursor into `HollowBlock`. The palette is remembered to answer the shell's colour
    /// queries (OSC 4/10/11), which need a first call before they get a reply.
    pub fn screen(&self, palette: &Palette, focused: bool) -> Screen {
        self.shared.set_palette(palette);
        screen::build_screen(&self.term.lock(), palette, focused)
    }

    /// Title, bell and exit since the last call; also re-arms `wake`.
    pub fn take_events(&mut self) -> Vec<TermEvent> {
        self.shared.take_events()
    }

    /// Whether the shell printed (or sent an event) since the last call, and the events;
    /// also re-arms `wake`.
    pub fn drain(&mut self) -> (bool, Vec<TermEvent>) {
        self.shared.drain()
    }

    /// `true` once the shell has ended (or its terminal loop has).
    pub fn exited(&self) -> bool {
        self.shared.exited.load(Ordering::Acquire)
    }

    /// The shell's process id, for diagnostics.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().and_then(Child::pid)
    }
}

impl Drop for Session {
    /// Never hangs: it asks the loop to stop, ends the shell, and waits at most two seconds for
    /// the watcher, which joins the loop thread and closes the ConPTY. Closing a pseudo console
    /// can block on unread output, so a watcher that is still busy after the bound is left
    /// behind (it holds nothing the app needs) rather than waited for.
    fn drop(&mut self) {
        self.shared.close();
        let _ = self.sender.send(Msg::Shutdown);
        if let Some(child) = &self.child {
            child.terminate();
        }
        if let Some(watcher) = self.watcher.take() {
            let deadline = Instant::now() + DROP_BOUND;
            while !watcher.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

#[cfg(test)]
impl Session {
    /// Ends the event loop the way an I/O failure would: the shell lives on and the emulator
    /// sends no exit event.
    pub(crate) fn end_loop_for_test(&self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}
