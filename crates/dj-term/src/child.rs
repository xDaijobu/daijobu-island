//! Our own handle on the shell process, so ending a session ends the shell.
//!
//! The emulator crate keeps the process handle inside its `Pty` and offers no way to stop the
//! child, and closing a pseudo console alone does not promptly end a shell that ignores it. A
//! duplicate handle (ours, closed on drop) lets `Session::drop` call `TerminateProcess`.

use std::os::windows::io::{AsRawHandle, BorrowedHandle, OwnedHandle, RawHandle};

use alacritty_terminal::tty::Pty;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::Threading::TerminateProcess;

pub(crate) struct Child {
    handle: OwnedHandle,
    pid: Option<u32>,
}

impl Child {
    /// A handle on the shell that `pty` started.
    pub(crate) fn of(pty: &Pty) -> Option<Child> {
        let watcher = pty.child_watcher();
        let pid = watcher.pid().map(|p| p.get());
        // SAFETY: the watcher's handle is the process handle the `Pty` keeps open for its
        // whole life (the emulator never closes it), and `pty` is borrowed for this call, so it
        // is valid while `try_clone_to_owned` duplicates it.
        let borrowed = unsafe { BorrowedHandle::borrow_raw(watcher.raw_handle() as RawHandle) };
        let handle = borrowed.try_clone_to_owned().ok()?;
        Some(Child { handle, pid })
    }

    pub(crate) fn pid(&self) -> Option<u32> {
        self.pid
    }

    fn raw(&self) -> HANDLE {
        HANDLE(self.handle.as_raw_handle())
    }

    /// Ends the shell now. Harmless when it has already exited.
    pub(crate) fn terminate(&self) {
        // SAFETY: `self.handle` is an owned, open process handle; the call only reads it.
        let _ = unsafe { TerminateProcess(self.raw(), 1) };
    }
}

/// Test helper: has the process `pid` ended (or can it no longer be opened)?
#[cfg(test)]
pub(crate) fn process_ended(pid: u32, timeout_ms: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    // SAFETY: `OpenProcess` returns a handle we own; it is waited on once and closed.
    unsafe {
        match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            // The process object is gone: it ended and nothing holds it.
            Err(_) => true,
            Ok(h) => {
                let done = WaitForSingleObject(h, timeout_ms) == WAIT_OBJECT_0;
                let _ = CloseHandle(h);
                done
            }
        }
    }
}

/// Lets the shell started next get Ctrl+C. A process inherits "ignore Ctrl+C" from the one
/// that starts it, and the island may have been started with it (from a script or a
/// launcher that runs its children in their own process group); the shell, and the `ping`
/// it runs, would then ignore every Ctrl+C typed in the panel.
pub(crate) fn allow_ctrl_c() {
    // SAFETY: a null handler only clears this process's "ignore Ctrl+C" flag (the island has
    // no console of its own, so no handler runs); no pointers are passed.
    let _ = unsafe { SetConsoleCtrlHandler(None, false) };
}
