//! The named pipe: its name and security, the island's server thread and the client.
//!
//! Every read, write and connect is an overlapped operation with a deadline and an
//! optional stop event ([`Io::run`]), so nothing on either side can block for good: a
//! silent client costs the island 2 s, a client that never reads its reply 2 s, and
//! dropping the [`Server`] interrupts whatever the thread is waiting for.

use crate::proto::{self, Reply, Request, MAX_FRAME};
use std::fmt;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::{Error, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING,
    ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ,
    GENERIC_WRITE, HANDLE, HLOCAL, WAIT_EVENT, WAIT_OBJECT_0, WAIT_TIMEOUT, WIN32_ERROR,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED,
    FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, WaitNamedPipeW, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, GetCurrentProcessId, OpenProcessToken, ResetEvent, SetEvent,
    WaitForMultipleObjects, WaitForSingleObject, INFINITE,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

/// The server drops a client that has not sent a whole request after this long.
const READ_LIMIT: Duration = Duration::from_secs(2);
/// A reply that the client does not take in this time is abandoned.
const WRITE_LIMIT: Duration = Duration::from_secs(2);
/// How long the UI thread has to answer. A little under the client's 3 s, so a client
/// that waits 3 s still receives "busy" rather than giving up first.
const REPLY_LIMIT: Duration = Duration::from_millis(2500);
/// After the reply, how long to wait for the client to close its end.
const CLOSE_LIMIT: Duration = Duration::from_secs(2);
/// While waiting for the UI thread's answer the stop event is checked this often.
const REPLY_POLL: Duration = Duration::from_millis(20);
/// Pause after an unexpected connect failure, and how many in a row end the thread.
const RETRY_PAUSE_MS: u32 = 100;
const MAX_CONNECT_FAILURES: u32 = 50;
/// Pipe buffer sizes: one frame each way.
const PIPE_BUFFER: u32 = MAX_FRAME as u32;

const PIPE_PREFIX: &str = r"\\.\pipe\daijobu-island-";

// ---------------------------------------------------------------------------------------
// Handles and small helpers

/// A kernel handle closed exactly once, when this is dropped.
#[derive(Debug)]
struct OwnedHandle(HANDLE);

// SAFETY: a kernel HANDLE is a process-wide identifier that any thread may use; this
// wrapper is the handle's only owner and closes it once.
unsafe impl Send for OwnedHandle {}
// SAFETY: as above; the handles shared through `&OwnedHandle` (the stop event) are only
// signalled and waited on, which the kernel serializes.
unsafe impl Sync for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: we own the handle and nothing uses it after this.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// NUL-terminated UTF-16.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn is_error(e: &Error, code: WIN32_ERROR) -> bool {
    e.code() == HRESULT::from_win32(code.0)
}

/// Milliseconds until `deadline`, rounded up; `INFINITE` without a deadline.
fn wait_ms(deadline: Option<Instant>) -> u32 {
    match deadline {
        None => INFINITE,
        Some(d) => {
            let nanos = d.saturating_duration_since(Instant::now()).as_nanos();
            (nanos.div_ceil(1_000_000)).min(u128::from(INFINITE - 1)) as u32
        }
    }
}

// ---------------------------------------------------------------------------------------
// Name and security

/// `\\.\pipe\daijobu-island-<sid>-<session>`.
fn format_pipe_name(sid: &str, session: u32) -> String {
    format!("{PIPE_PREFIX}{sid}-{session}")
}

/// The current user's SID as a string such as `S-1-5-21-...-1001`.
fn user_sid_string() -> Result<String, String> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle from GetCurrentProcess needs no closing and `token` is a
    // valid out-pointer.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(|e| format!("OpenProcessToken: {e}"))?;
    let token = OwnedHandle(token);

    let mut needed = 0u32;
    // SAFETY: a size query (no buffer, zero length, valid out-pointer). It fails with
    // ERROR_INSUFFICIENT_BUFFER by design, so the result is ignored.
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut needed) };
    if needed == 0 {
        return Err("GetTokenInformation: no size for the user token".to_string());
    }
    // u64 elements keep the buffer 8-byte aligned, as TOKEN_USER (it holds a pointer) needs.
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: the buffer is at least `needed` bytes long and `needed` is a valid out-pointer.
    unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
    }
    .map_err(|e| format!("GetTokenInformation: {e}"))?;
    // SAFETY: GetTokenInformation filled the aligned buffer with a TOKEN_USER; the SID it
    // points to lives in the same buffer, which outlives this use.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text = PWSTR::null();
    // SAFETY: `sid` is valid; on success `text` is a LocalAlloc'ed string freed below.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }
        .map_err(|e| format!("ConvertSidToStringSid: {e}"))?;
    // SAFETY: `text` is the NUL-terminated UTF-16 string just returned; it is read once
    // and freed exactly once, after the copy.
    let (string, _freed) = unsafe {
        let s = text.to_string();
        (s, LocalFree(Some(HLOCAL(text.0.cast()))))
    };
    string.map_err(|e| format!("SID string: {e}"))
}

/// The logon session this process runs in.
fn session_id() -> Result<u32, String> {
    let mut session = 0u32;
    // SAFETY: `session` is a valid out-pointer for the call.
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) }
        .map_err(|e| format!("ProcessIdToSessionId: {e}"))?;
    Ok(session)
}

/// `\\.\pipe\daijobu-island-<current user SID string>-<current session id>`.
///
/// If the SID or the session cannot be read (it practically never fails) the name uses
/// "unknown" / 0; such a name can never be served ([`Server::start`] refuses it), so a
/// client just reports that the island is not running.
pub fn pipe_name() -> String {
    match (user_sid_string(), session_id()) {
        (Ok(sid), Ok(session)) => format_pipe_name(&sid, session),
        _ => format_pipe_name("unknown", 0),
    }
}

/// Protected DACL: full access for `sid` (the current user) and SYSTEM, nobody else.
fn sddl_for(sid: &str) -> String {
    format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)")
}

/// A security descriptor from SDDL, freed with `LocalFree`.
struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    fn from_sddl(sddl: &str) -> Result<Self, String> {
        let text = wide(sddl);
        let mut sd = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
        // SAFETY: `text` is NUL-terminated and outlives the call; `sd` is a valid
        // out-pointer and on success holds a LocalAlloc'ed descriptor owned by `Self`.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(text.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
        }
        .map_err(|e| format!("security descriptor \"{sddl}\": {e}"))?;
        Ok(SecurityDescriptor(sd))
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: the descriptor was allocated by the call that created it and is freed once.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0 .0)));
        }
    }
}

/// Creates the one pipe instance. Fails if the name is taken (`FIRST_PIPE_INSTANCE`).
fn create_pipe(name: &str, sid: &str) -> Result<OwnedHandle, String> {
    let sd = SecurityDescriptor::from_sddl(&sddl_for(sid))?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0 .0,
        bInheritHandle: false.into(),
    };
    let name_w = wide(name);
    // SAFETY: `name_w` is NUL-terminated; `attributes` and the descriptor it points to
    // stay alive for the call (the system copies the descriptor).
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name_w.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            PIPE_BUFFER,
            PIPE_BUFFER,
            0,
            Some(&attributes),
        )
    };
    if handle.is_invalid() {
        return Err(format!("CreateNamedPipe {name}: {}", Error::from_thread()));
    }
    Ok(OwnedHandle(handle))
}

// ---------------------------------------------------------------------------------------
// Overlapped I/O with a deadline

#[derive(Debug)]
enum IoError {
    /// The deadline passed first.
    Timeout,
    /// The stop event fired first.
    Stopped,
    /// The other end is gone.
    Closed,
    /// A frame did not end within [`MAX_FRAME`] bytes.
    TooLarge,
    Os(String),
}

impl From<Error> for IoError {
    fn from(e: Error) -> Self {
        let gone = [ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED, ERROR_NO_DATA];
        if gone.iter().any(|c| is_error(&e, *c)) {
            IoError::Closed
        } else {
            IoError::Os(e.to_string())
        }
    }
}

/// The event every operation of one connection (or one client call) signals.
struct Io {
    event: OwnedHandle,
}

impl Io {
    fn new() -> Result<Io, String> {
        // SAFETY: no security attributes and no name.
        let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map_err(|e| format!("CreateEvent: {e}"))?;
        Ok(Io {
            event: OwnedHandle(event),
        })
    }

    /// Issues one overlapped operation on `file` through `start`, then waits for it to
    /// finish, for `deadline` (none: forever) or for `stop`. Returns the byte count.
    ///
    /// The operation is always finished or cancelled and reaped before this returns, so
    /// the kernel never touches the `OVERLAPPED` or the caller's buffer afterwards.
    fn run(
        &self,
        file: HANDLE,
        deadline: Option<Instant>,
        stop: Option<HANDLE>,
        start: impl FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>,
    ) -> Result<u32, IoError> {
        let mut overlapped = OVERLAPPED {
            hEvent: self.event.0,
            ..Default::default()
        };
        // SAFETY: the event handle is owned by `self` and valid.
        unsafe {
            let _ = ResetEvent(self.event.0);
        }
        match start(&mut overlapped) {
            Ok(()) => {}
            Err(e) if is_error(&e, ERROR_IO_PENDING) => {}
            // Only ConnectNamedPipe reports this: the client connected before the call,
            // so the connection is complete and no event will be signalled.
            Err(e) if is_error(&e, ERROR_PIPE_CONNECTED) => return Ok(0),
            Err(e) => return Err(e.into()),
        }

        // From here on the operation may still be running: leave only after reaping it.
        let handles = [self.event.0, stop.unwrap_or_default()];
        let handles = &handles[..if stop.is_some() { 2 } else { 1 }];
        // SAFETY: the handles are valid for the call (the stop event is owned by the
        // caller's `Server`, the event by `self`).
        let waited = unsafe { WaitForMultipleObjects(handles, false, wait_ms(deadline)) };
        let early = if waited == WAIT_OBJECT_0 {
            None
        } else if waited == WAIT_EVENT(WAIT_OBJECT_0.0 + 1) && handles.len() == 2 {
            Some(IoError::Stopped)
        } else if waited == WAIT_TIMEOUT {
            Some(IoError::Timeout)
        } else {
            Some(IoError::Os("wait failed".to_string()))
        };

        let mut bytes = 0u32;
        match early {
            None => {
                // SAFETY: `overlapped` belongs to the operation started above, which has
                // completed (its event is signalled).
                unsafe { GetOverlappedResult(file, &overlapped, &mut bytes, false) }
                    .map(|()| bytes)
                    .map_err(IoError::from)
            }
            Some(why) => {
                // SAFETY: `overlapped` belongs to the pending operation; waiting for the
                // cancelled operation to finish keeps `overlapped` and the buffer alive
                // until the kernel is done with them.
                unsafe {
                    let _ = CancelIoEx(file, Some(&overlapped));
                    let _ = GetOverlappedResult(file, &overlapped, &mut bytes, true);
                }
                Err(why)
            }
        }
    }
}

/// Reads up to and including the first `\n`, at most [`MAX_FRAME`] bytes, before `deadline`.
fn read_frame(
    io: &Io,
    file: HANDLE,
    deadline: Instant,
    stop: Option<HANDLE>,
) -> Result<Vec<u8>, IoError> {
    let mut buf = vec![0u8; MAX_FRAME];
    let mut filled = 0usize;
    loop {
        if filled == MAX_FRAME {
            return Err(IoError::TooLarge);
        }
        let read = io.run(file, Some(deadline), stop, |overlapped| {
            // SAFETY: `buf[filled..]` stays alive and untouched until `run` returns, which
            // is after the read has completed or been cancelled.
            unsafe { ReadFile(file, Some(&mut buf[filled..]), None, Some(overlapped)) }
        });
        let n = match read {
            Ok(0) | Err(IoError::Closed) => {
                // The peer closed. A frame that just lacks its newline still counts.
                if filled > 0 {
                    buf.truncate(filled);
                    return Ok(buf);
                }
                return Err(IoError::Closed);
            }
            Ok(n) => n as usize,
            Err(e) => return Err(e),
        };
        let from = filled;
        filled += n;
        if let Some(i) = buf[from..filled].iter().position(|&b| b == b'\n') {
            buf.truncate(from + i + 1);
            return Ok(buf);
        }
    }
}

/// Writes all of `data` before `deadline`.
fn write_all(
    io: &Io,
    file: HANDLE,
    data: &[u8],
    deadline: Instant,
    stop: Option<HANDLE>,
) -> Result<(), IoError> {
    let mut sent = 0usize;
    while sent < data.len() {
        let n = io.run(file, Some(deadline), stop, |overlapped| {
            // SAFETY: `data[sent..]` outlives the call to `run`, which returns only after
            // the write has completed or been cancelled.
            unsafe { WriteFile(file, Some(&data[sent..]), None, Some(overlapped)) }
        })?;
        if n == 0 {
            return Err(IoError::Closed);
        }
        sent += n as usize;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// The island's end

/// A request taken off the pipe, waiting for the UI thread's answer.
#[derive(Debug)]
pub struct Incoming {
    pub request: Request,
    reply: Sender<Reply>,
}

impl Incoming {
    /// Answers the client. A reply that arrives after the server gave up ("busy") is
    /// discarded. Dropping an `Incoming` without calling this answers "busy" at once.
    pub fn reply(self, r: Reply) {
        let _ = self.reply.send(r);
    }
}

/// The island's end of the pipe: a thread that owns the pipe, reads one request per
/// connection, hands it over through the channel returned by [`Server::start`] and
/// writes the answer back.
///
/// Dropping the `Server` stops the thread (it interrupts a pending connect, read or
/// reply wait) and joins it, which closes the pipe.
#[derive(Debug)]
pub struct Server {
    stop: Arc<OwnedHandle>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    /// Serves [`pipe_name`]. `wake` is called, on the pipe thread, each time an
    /// [`Incoming`] has been queued: use it to wake the UI thread (it must not block).
    /// The creating thread should drain the receiver and answer each [`Incoming`]
    /// within about 2.5 s.
    pub fn start(wake: Box<dyn Fn() + Send>) -> Result<(Server, Receiver<Incoming>), String> {
        let name = format_pipe_name(&user_sid_string()?, session_id()?);
        Server::start_named(&name, wake)
    }

    /// As [`Server::start`] with a given pipe name (tests use a unique one). Fails if an
    /// instance with that name already exists.
    pub fn start_named(
        name: &str,
        wake: Box<dyn Fn() + Send>,
    ) -> Result<(Server, Receiver<Incoming>), String> {
        let pipe = create_pipe(name, &user_sid_string()?)?;
        let io = Io::new()?;
        // SAFETY: no security attributes and no name.
        let stop = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map_err(|e| format!("CreateEvent: {e}"))?;
        let stop = Arc::new(OwnedHandle(stop));
        let (tx, rx) = mpsc::channel();
        let worker = Worker {
            pipe,
            io,
            stop: Arc::clone(&stop),
            wake,
            tx,
        };
        let thread = std::thread::Builder::new()
            .name("dj-ipc".to_string())
            .spawn(move || worker.run())
            .map_err(|e| format!("spawn the pipe thread: {e}"))?;
        Ok((
            Server {
                stop,
                thread: Some(thread),
            },
            rx,
        ))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // SAFETY: the stop event is owned by `self.stop` and alive.
        unsafe {
            let _ = SetEvent(self.stop.0);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the pipe thread does with one finished wait for the UI thread.
enum Answer {
    Reply(Reply),
    Stopped,
}

struct Worker {
    pipe: OwnedHandle,
    io: Io,
    stop: Arc<OwnedHandle>,
    wake: Box<dyn Fn() + Send>,
    tx: Sender<Incoming>,
}

impl Worker {
    fn stopped(&self) -> bool {
        // SAFETY: the stop event is valid; a zero timeout only polls it.
        unsafe { WaitForSingleObject(self.stop.0, 0) == WAIT_OBJECT_0 }
    }

    fn disconnect(&self) {
        // SAFETY: the pipe handle is ours. Failing (no client connected) is fine.
        unsafe {
            let _ = DisconnectNamedPipe(self.pipe.0);
        }
    }

    fn run(self) {
        let mut failures = 0u32;
        while !self.stopped() {
            let connected = self
                .io
                .run(self.pipe.0, None, Some(self.stop.0), |overlapped| {
                    // SAFETY: the pipe handle is ours and `overlapped` is valid until
                    // `run` has reaped the operation.
                    unsafe { ConnectNamedPipe(self.pipe.0, Some(overlapped)) }
                });
            match connected {
                Ok(_) => {
                    failures = 0;
                    if !self.serve_one() {
                        break;
                    }
                }
                Err(IoError::Stopped) => break,
                // A client connected and left before this call (ERROR_NO_DATA): the
                // instance only needs the disconnect below to listen again.
                Err(IoError::Closed) => {}
                Err(_) => {
                    failures += 1;
                    if failures >= MAX_CONNECT_FAILURES {
                        break;
                    }
                    // SAFETY: the stop event is valid; this is an interruptible sleep.
                    if unsafe { WaitForSingleObject(self.stop.0, RETRY_PAUSE_MS) } == WAIT_OBJECT_0
                    {
                        break;
                    }
                }
            }
            self.disconnect();
        }
        self.disconnect();
    }

    /// Serves the connected client. Returns false when the server was asked to stop.
    fn serve_one(&self) -> bool {
        let deadline = Instant::now() + READ_LIMIT;
        let frame = match read_frame(&self.io, self.pipe.0, deadline, Some(self.stop.0)) {
            Ok(frame) => frame,
            Err(IoError::Stopped) => return false,
            Err(IoError::TooLarge) => {
                return self.answer(&Reply::error("request is larger than 4096 bytes"));
            }
            // Silent, gone or broken: drop the client without an answer.
            Err(_) => return true,
        };
        let reply = match proto::decode_request(&frame) {
            Err(why) => Reply::error(why),
            Ok(request) => match self.hand_over(request) {
                Answer::Reply(reply) => reply,
                Answer::Stopped => return false,
            },
        };
        self.answer(&reply)
    }

    /// Queues the request for the UI thread and waits for its reply (or "busy").
    fn hand_over(&self, request: Request) -> Answer {
        let busy = || Answer::Reply(Reply::error("busy"));
        let (reply_tx, reply_rx) = mpsc::channel();
        let incoming = Incoming {
            request,
            reply: reply_tx,
        };
        if self.tx.send(incoming).is_err() {
            // The UI side is gone.
            return busy();
        }
        (self.wake)();
        let deadline = Instant::now() + REPLY_LIMIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return busy();
            }
            match reply_rx.recv_timeout(left.min(REPLY_POLL)) {
                Ok(reply) => return Answer::Reply(reply),
                // The Incoming was dropped without a reply.
                Err(RecvTimeoutError::Disconnected) => return busy(),
                Err(RecvTimeoutError::Timeout) => {
                    if self.stopped() {
                        return Answer::Stopped;
                    }
                }
            }
        }
    }

    /// Writes the reply and waits for the client to take it. Returns false on stop.
    fn answer(&self, reply: &Reply) -> bool {
        let frame = proto::encode_reply(reply);
        let written = write_all(
            &self.io,
            self.pipe.0,
            &frame,
            Instant::now() + WRITE_LIMIT,
            Some(self.stop.0),
        );
        match written {
            Ok(()) => {}
            Err(IoError::Stopped) => return false,
            // The client left before its answer: nothing more to do.
            Err(_) => return true,
        }
        // FlushFileBuffers would block until the client has read the reply, and cannot be
        // cancelled: a client that never reads would hang the thread (and the island's
        // exit). Waiting for the client to close its end is the same guarantee, with a
        // limit and a way out: a client closes as soon as it has its reply.
        let deadline = Instant::now() + CLOSE_LIMIT;
        let mut scratch = [0u8; 64];
        loop {
            let read = self.io.run(
                self.pipe.0,
                Some(deadline),
                Some(self.stop.0),
                |overlapped| {
                    // SAFETY: `scratch` outlives `run`, which reaps the read before returning.
                    unsafe { ReadFile(self.pipe.0, Some(&mut scratch), None, Some(overlapped)) }
                },
            );
            match read {
                Ok(n) if n > 0 => continue,
                Err(IoError::Stopped) => return false,
                _ => return true,
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// The client

/// Why [`send`] / [`send_to`] failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendError {
    /// Nothing serves the pipe: the island is not running.
    NotRunning,
    /// The pipe stayed busy, or the island did not answer, within the timeout.
    Timeout,
    /// A pipe or system error.
    Io(String),
    /// The island answered with something that is not a valid reply.
    Protocol(String),
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SendError::NotRunning => f.write_str("Daijobu Island is not running"),
            SendError::Timeout => f.write_str("Daijobu Island did not answer in time"),
            SendError::Io(e) => write!(f, "pipe error: {e}"),
            SendError::Protocol(e) => write!(f, "unexpected answer from Daijobu Island: {e}"),
        }
    }
}

impl std::error::Error for SendError {}

impl From<IoError> for SendError {
    fn from(e: IoError) -> Self {
        match e {
            IoError::Timeout => SendError::Timeout,
            IoError::Closed => SendError::Io("the island closed the connection".to_string()),
            IoError::TooLarge => SendError::Protocol("reply is larger than 4096 bytes".to_string()),
            IoError::Stopped => SendError::Io("interrupted".to_string()),
            IoError::Os(m) => SendError::Io(m),
        }
    }
}

/// Opens the pipe, waiting while another client holds the single instance.
fn open_pipe(name: &str, deadline: Instant) -> Result<OwnedHandle, SendError> {
    let name_w = wide(name);
    loop {
        // SAFETY: `name_w` is NUL-terminated and outlives the call; no security
        // attributes or template. SQOS "identification" stops the server from
        // impersonating us.
        let opened = unsafe {
            CreateFileW(
                PCWSTR(name_w.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                None,
            )
        };
        match opened {
            Ok(handle) => return Ok(OwnedHandle(handle)),
            Err(e) if is_error(&e, ERROR_FILE_NOT_FOUND) => return Err(SendError::NotRunning),
            Err(e) if is_error(&e, ERROR_PIPE_BUSY) => {
                if deadline.saturating_duration_since(Instant::now()).is_zero() {
                    return Err(SendError::Timeout);
                }
                // SAFETY: `name_w` is NUL-terminated and outlives the call. A timeout just
                // loops back to the deadline check above.
                unsafe {
                    let _ = WaitNamedPipeW(PCWSTR(name_w.as_ptr()), wait_ms(Some(deadline)).max(1));
                }
            }
            Err(e) => return Err(SendError::Io(format!("open the pipe: {e}"))),
        }
    }
}

/// Sends `request` to the island on [`pipe_name`] and returns its reply. `timeout` covers
/// everything: waiting for the pipe, writing the request and waiting for the answer.
pub fn send(request: &Request, timeout: Duration) -> Result<Reply, SendError> {
    send_to(&pipe_name(), request, timeout)
}

/// As [`send`] on a given pipe name.
pub fn send_to(name: &str, request: &Request, timeout: Duration) -> Result<Reply, SendError> {
    let deadline = Instant::now() + timeout;
    let pipe = open_pipe(name, deadline)?;
    let io = Io::new().map_err(SendError::Io)?;
    let frame = proto::encode_request(request);
    write_all(&io, pipe.0, &frame, deadline, None)?;
    let answer = read_frame(&io, pipe.0, deadline, None)?;
    proto::decode_reply(&answer).map_err(SendError::Protocol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SID: &str = "S-1-5-21-1111111111-2222222222-3333333333-1001";

    fn unique_name(tag: &str) -> String {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        format!(
            r"\\.\pipe\daijobu-island-test-{}-{}-{tag}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        )
    }

    /// A server with a unique name and a counter of `wake` calls.
    fn start(tag: &str) -> (String, Server, Receiver<Incoming>, Arc<AtomicUsize>) {
        let name = unique_name(tag);
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakes);
        let (server, rx) = Server::start_named(
            &name,
            Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .expect("start the test server");
        (name, server, rx, wakes)
    }

    fn client(
        name: &str,
        request: Request,
        timeout: Duration,
    ) -> JoinHandle<Result<Reply, SendError>> {
        let name = name.to_string();
        std::thread::spawn(move || send_to(&name, &request, timeout))
    }

    fn next(rx: &Receiver<Incoming>) -> Incoming {
        rx.recv_timeout(Duration::from_secs(10))
            .expect("a request on the channel")
    }

    /// A plain client with std I/O, to say things `send_to` never would. Retries while the
    /// single instance is between two connections.
    fn raw_client(name: &str) -> std::fs::File {
        for _ in 0..400 {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(name)
            {
                Ok(file) => return file,
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => panic!("open {name}: {e}"),
            }
        }
        panic!("{name} stayed busy");
    }

    fn read_reply(file: &mut std::fs::File) -> Reply {
        let mut line = Vec::new();
        BufReader::new(file)
            .read_until(b'\n', &mut line)
            .expect("read the reply");
        proto::decode_reply(&line).expect("a valid reply")
    }

    // ---- name and security ----

    #[test]
    fn pipe_name_format() {
        assert_eq!(
            format_pipe_name(SID, 2),
            format!(r"\\.\pipe\daijobu-island-{SID}-2")
        );
        let name = pipe_name();
        eprintln!("pipe name on this machine: {name}");
        let rest = name
            .strip_prefix(r"\\.\pipe\daijobu-island-")
            .unwrap_or_else(|| panic!("{name}"));
        let (sid, session) = rest.rsplit_once('-').expect("a session suffix");
        assert!(sid.starts_with("S-1-"), "{name}");
        assert!(
            sid.chars()
                .all(|c| c.is_ascii_digit() || c == 'S' || c == '-'),
            "{name}"
        );
        assert_eq!(sid, user_sid_string().unwrap());
        assert_eq!(session.parse::<u32>().unwrap(), session_id().unwrap());
        assert_eq!(name, pipe_name(), "the name is stable");
    }

    #[test]
    fn sddl_grants_only_the_user_and_system() {
        assert_eq!(sddl_for(SID), format!("D:P(A;;GA;;;{SID})(A;;GA;;;SY)"));
        // The string is accepted by Windows, for a real SID too.
        SecurityDescriptor::from_sddl(&sddl_for(SID)).expect("example SID");
        SecurityDescriptor::from_sddl(&sddl_for(&user_sid_string().unwrap())).expect("own SID");
        assert!(SecurityDescriptor::from_sddl("not sddl").is_err());
    }

    #[test]
    fn the_created_pipe_carries_exactly_that_dacl() {
        use windows::Win32::Foundation::ERROR_SUCCESS;
        use windows::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SE_KERNEL_OBJECT,
        };
        use windows::Win32::Security::DACL_SECURITY_INFORMATION;

        let sid = user_sid_string().unwrap();
        let pipe = create_pipe(&unique_name("dacl"), &sid).expect("create the pipe");
        let mut descriptor = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
        // SAFETY: `pipe.0` is a live pipe handle and `descriptor` a valid out-pointer; on
        // success the descriptor is LocalAlloc'ed and owned by the guard below.
        let status = unsafe {
            GetSecurityInfo(
                pipe.0,
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                None,
                None,
                Some(&mut descriptor),
            )
        };
        assert_eq!(status, ERROR_SUCCESS);
        let descriptor = SecurityDescriptor(descriptor);
        let mut text = PWSTR::null();
        // SAFETY: a valid descriptor and out-pointer; `text` is LocalAlloc'ed on success.
        unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor.0,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                None,
            )
        }
        .expect("descriptor to string");
        // SAFETY: `text` is the NUL-terminated string just returned; freed once after the copy.
        let sddl = unsafe {
            let s = text.to_string();
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
            s.unwrap()
        };
        // A protected DACL with the user and SYSTEM and nobody else (no Everyone, no
        // Administrators, no anonymous).
        assert!(sddl.starts_with("D:P"), "{sddl}");
        let aces: Vec<&str> = sddl.split('(').skip(1).collect();
        assert_eq!(aces.len(), 2, "{sddl}");
        assert!(aces.iter().all(|a| a.starts_with("A;")), "{sddl}");
        assert!(aces[0].contains(&sid), "{sddl}");
        assert!(aces[1].contains(";SY)"), "{sddl}");
    }

    // ---- end to end, in process ----

    #[test]
    fn request_reply_round_trip() {
        let (name, server, rx, wakes) = start("roundtrip");
        let cases = vec![
            (
                Request::Progress {
                    percent: Some(40),
                    label: "deploy".into(),
                },
                Reply::ok(),
            ),
            (
                Request::Done {
                    message: "build ok".into(),
                },
                Reply::ok(),
            ),
            (
                Request::Notify {
                    message: "tests failed".into(),
                },
                Reply::ok(),
            ),
            (
                Request::Theme { id: None },
                Reply::text("* crt\n  paper-zine"),
            ),
            (
                Request::Theme {
                    id: Some("nope".into()),
                },
                Reply::error("unknown theme 'nope'"),
            ),
            (Request::Snap, Reply::ok()),
        ];
        for (request, reply) in &cases {
            let t = client(&name, request.clone(), Duration::from_secs(5));
            let incoming = next(&rx);
            assert_eq!(&incoming.request, request);
            incoming.reply(reply.clone());
            assert_eq!(t.join().unwrap(), Ok(reply.clone()));
        }
        assert_eq!(wakes.load(Ordering::SeqCst), cases.len());
        drop(server);
    }

    #[test]
    fn requests_are_cleaned_before_the_ui_sees_them() {
        let (name, _server, rx, _) = start("clean");
        let t = client(
            &name,
            Request::Notify {
                message: format!("  a\tb\u{7} {}", "x".repeat(300)),
            },
            Duration::from_secs(5),
        );
        let incoming = next(&rx);
        match &incoming.request {
            Request::Notify { message } => {
                assert!(message.starts_with("a b"));
                assert_eq!(message.chars().count(), proto::MAX_MESSAGE);
            }
            other => panic!("{other:?}"),
        }
        incoming.reply(Reply::ok());
        assert_eq!(t.join().unwrap(), Ok(Reply::ok()));
    }

    #[test]
    fn a_bad_frame_gets_an_error_reply_and_the_server_carries_on() {
        let (name, _server, rx, wakes) = start("badframe");
        for (bytes, needle) in [
            (&b"this is not json\n"[..], "bad JSON"),
            (b"{\"v\":2,\"cmd\":\"snap\"}\n", "version"),
            (b"{\"v\":1,\"cmd\":\"reboot\"}\n", "reboot"),
            (
                b"{\"v\":1,\"cmd\":\"notify\",\"message\":\"  \"}\n",
                "message",
            ),
            (b"{\"v\":1,\"cmd\":\"progress\",\"percent\":101}\n", "0-100"),
        ] {
            let mut file = raw_client(&name);
            file.write_all(bytes).unwrap();
            let reply = read_reply(&mut file);
            assert!(!reply.ok);
            assert!(
                reply.error.as_deref().unwrap_or("").contains(needle),
                "{reply:?} should mention {needle}"
            );
        }
        assert_eq!(wakes.load(Ordering::SeqCst), 0, "nothing reached the UI");
        assert!(rx.try_recv().is_err());
        // still serving
        let t = client(&name, Request::Snap, Duration::from_secs(5));
        next(&rx).reply(Reply::ok());
        assert_eq!(t.join().unwrap(), Ok(Reply::ok()));
    }

    #[test]
    fn an_oversized_request_is_refused() {
        let (name, _server, rx, _) = start("oversize");
        let mut file = raw_client(&name);
        file.write_all(&vec![b'x'; MAX_FRAME + 100]).unwrap();
        let reply = read_reply(&mut file);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("4096"));
        drop(file);
        assert!(rx.try_recv().is_err());
        let t = client(&name, Request::Snap, Duration::from_secs(5));
        next(&rx).reply(Reply::ok());
        assert_eq!(t.join().unwrap(), Ok(Reply::ok()));
    }

    #[test]
    fn a_silent_client_is_dropped_after_two_seconds_without_blocking_the_next() {
        let (name, _server, rx, _) = start("silent");
        let mut silent = raw_client(&name); // connected, says nothing
        let started = Instant::now();
        let t = client(&name, Request::Snap, Duration::from_secs(10));
        let incoming = next(&rx);
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_millis(1500) && waited < Duration::from_secs(5),
            "the silent client held the pipe for {waited:?}"
        );
        incoming.reply(Reply::ok());
        assert_eq!(t.join().unwrap(), Ok(Reply::ok()));
        // The silent client was cut off: reading from it ends instead of blocking.
        let mut byte = [0u8; 1];
        assert!(matches!(silent.read(&mut byte), Ok(0) | Err(_)));
    }

    #[test]
    fn a_client_that_leaves_without_a_newline_is_served_what_it_sent() {
        let (name, _server, rx, _) = start("nonewline");
        // A client that connects and leaves before the thread's very first connect call
        // finds the pipe closing (ERROR_NO_DATA) and is dropped, so let the thread settle
        // into its loop first.
        let t = client(&name, Request::Snap, Duration::from_secs(5));
        next(&rx).reply(Reply::ok());
        assert_eq!(t.join().unwrap(), Ok(Reply::ok()));

        let mut file = raw_client(&name);
        file.write_all(b"{\"v\":1,\"cmd\":\"snap\"}").unwrap();
        drop(file); // closes: the frame just lacks its newline
        let incoming = next(&rx);
        assert_eq!(incoming.request, Request::Snap);
        incoming.reply(Reply::ok()); // nobody listens any more; must not hang or panic
        let t = client(&name, Request::Snap, Duration::from_secs(5));
        next(&rx).reply(Reply::ok());
        assert_eq!(t.join().unwrap(), Ok(Reply::ok()));
    }

    #[test]
    fn no_reply_means_busy() {
        let (name, _server, rx, _) = start("busy");
        let t = client(
            &name,
            Request::Notify {
                message: "hello".into(),
            },
            Duration::from_secs(10),
        );
        let incoming = next(&rx);
        let started = Instant::now();
        // The UI thread never answers (it keeps the Incoming alive, too).
        assert_eq!(t.join().unwrap(), Ok(Reply::error("busy")));
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_secs(2) && waited < Duration::from_secs(4),
            "busy after {waited:?}"
        );
        incoming.reply(Reply::ok()); // too late: discarded, no panic
                                     // The server is fine afterwards.
        let t = client(&name, Request::Snap, Duration::from_secs(5));
        next(&rx).reply(Reply::ok());
        assert_eq!(t.join().unwrap(), Ok(Reply::ok()));
    }

    #[test]
    fn a_dropped_incoming_answers_busy_at_once() {
        let (name, _server, rx, _) = start("dropped");
        let t = client(&name, Request::Snap, Duration::from_secs(10));
        let incoming = next(&rx);
        let started = Instant::now();
        drop(incoming);
        assert_eq!(t.join().unwrap(), Ok(Reply::error("busy")));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_gone_ui_side_answers_busy() {
        let (name, _server, rx, _) = start("noui");
        drop(rx);
        let r = send_to(&name, &Request::Snap, Duration::from_secs(5));
        assert_eq!(r, Ok(Reply::error("busy")));
    }

    #[test]
    fn clients_queue_up_and_each_gets_its_own_reply() {
        let (name, _server, rx, _) = start("queue");
        let clients: Vec<_> = (0..4)
            .map(|i| {
                client(
                    &name,
                    Request::Notify {
                        message: format!("n{i}"),
                    },
                    Duration::from_secs(10),
                )
            })
            .collect();
        for _ in 0..clients.len() {
            let incoming = next(&rx);
            let Request::Notify { message } = incoming.request.clone() else {
                panic!("{:?}", incoming.request);
            };
            incoming.reply(Reply::text(format!("re:{message}")));
        }
        let mut got: Vec<_> = clients
            .into_iter()
            .map(|c| c.join().unwrap().unwrap().text.unwrap())
            .collect();
        got.sort();
        assert_eq!(got, ["re:n0", "re:n1", "re:n2", "re:n3"]);
    }

    #[test]
    fn a_second_server_on_the_same_name_fails_until_the_first_is_dropped() {
        let (name, first, _rx, _) = start("twice");
        let second = Server::start_named(&name, Box::new(|| {}));
        let why = second.expect_err("the name is taken");
        assert!(why.contains("CreateNamedPipe"), "{why}");

        let started = Instant::now();
        drop(first);
        assert!(started.elapsed() < Duration::from_secs(1), "stop is prompt");
        // The first server closed its handle: the name is free again.
        let (again, _rx) = Server::start_named(&name, Box::new(|| {})).expect("name is free");
        drop(again);
    }

    #[test]
    fn a_server_with_nobody_connected_stops_promptly() {
        let (_name, server, _rx, _) = start("idlestop");
        std::thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        drop(server);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn dropping_the_server_interrupts_a_silent_client_and_a_pending_reply() {
        // a silent client
        let (name, server, _rx, _) = start("stopsilent");
        let _silent = raw_client(&name);
        std::thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        drop(server);
        assert!(started.elapsed() < Duration::from_secs(1));

        // a request the UI thread has not answered yet
        let (name, server, rx, _) = start("stoppending");
        let t = client(&name, Request::Snap, Duration::from_secs(10));
        let incoming = next(&rx);
        let started = Instant::now();
        drop(server);
        assert!(started.elapsed() < Duration::from_secs(1));
        let result = t.join().unwrap();
        assert!(
            result.is_err() || result == Ok(Reply::error("busy")),
            "{result:?}"
        );
        drop(incoming);
    }

    #[test]
    fn send_to_a_name_nobody_serves_is_not_running() {
        let name = unique_name("nobody");
        assert_eq!(
            send_to(&name, &Request::Snap, Duration::from_secs(1)),
            Err(SendError::NotRunning)
        );
    }

    #[test]
    fn send_error_messages() {
        assert_eq!(
            SendError::NotRunning.to_string(),
            "Daijobu Island is not running"
        );
        assert!(SendError::Timeout.to_string().contains("in time"));
        assert!(SendError::Io("x".into()).to_string().contains('x'));
        assert!(SendError::Protocol("y".into()).to_string().contains('y'));
    }
}
