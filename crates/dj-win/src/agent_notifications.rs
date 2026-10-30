//! Native, local-only, bounded hook IPC. The pipe is secured to the process user SID.
mod probe;
use dj_core::agent_notifications::{BridgeEvent, Credential, MAX_WIRE_BYTES};
pub use probe::provider_version;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    LocalFree, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_MODE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, WaitNamedPipeW, PIPE_READMODE_MESSAGE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT,
};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, OpenProcessToken, SetEvent, WaitForMultipleObjects, INFINITE,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

const PREFIX: &str = r"\\.\pipe\DaijobuIsland.Agent.";
const QUEUE_LIMIT: usize = 64;
const IO_TIMEOUT_MS: u32 = 500;

fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}

fn owned(handle: HANDLE) -> Result<OwnedHandle, String> {
    if handle.is_invalid() {
        return Err(windows::core::Error::from_thread().to_string());
    }
    // SAFETY: a successful API returned a uniquely owned kernel handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

pub fn random_credential() -> Result<Credential, String> {
    let mut bytes = [0; 32];
    // SAFETY: the OS writes exactly the valid byte slice; no algorithm handle is needed.
    unsafe { BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
        .ok()
        .map_err(|e| e.to_string())?;
    Credential::parse(bytes.iter().map(|b| format!("{b:02x}")).collect()).map_err(str::to_owned)
}

fn new_event() -> Result<OwnedHandle, String> {
    // SAFETY: unnamed, noninheritable, manual-reset event with no external pointers.
    owned(unsafe { CreateEventW(None, true, false, None) }.map_err(|e| e.to_string())?)
}

fn owner_sddl() -> Result<String, String> {
    let mut token = HANDLE::default();
    // SAFETY: the process handle is valid; token is a writable output.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(|e| e.to_string())?;
    let token = owned(token)?;
    let mut needed = 0;
    // SAFETY: size query only; no output buffer is supplied.
    let _ = unsafe { GetTokenInformation(raw(&token), TokenUser, None, 0, &mut needed) };
    if needed == 0 || needed > 65536 {
        return Err("cannot query process user".into());
    }
    // usize storage gives TOKEN_USER the required native pointer alignment.
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    // SAFETY: aligned allocation is at least `needed` bytes and remains alive through SID conversion.
    unsafe {
        GetTokenInformation(
            raw(&token),
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
    }
    .map_err(|e| e.to_string())?;
    // SAFETY: GetTokenInformation initialized a TOKEN_USER at this aligned address.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut sid = PWSTR::null();
    // SAFETY: the SID points inside the live token buffer; the API allocates its output.
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) }.map_err(|e| e.to_string())?;
    // SAFETY: the API returned a NUL-terminated string owned by LocalAlloc.
    let result = unsafe { sid.to_string() }.map_err(|e| e.to_string());
    // SAFETY: free the string allocation exactly once after copying it.
    unsafe { LocalFree(Some(HLOCAL(sid.0.cast()))) };
    Ok(format!("D:P(A;;GA;;;{})", result?))
}

fn create_pipe(name: &str) -> Result<OwnedHandle, String> {
    let sddl = wide(&owner_sddl()?);
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: valid NUL-terminated SDDL; the API allocates a self-relative descriptor.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .map_err(|e| e.to_string())?;
    let attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let name = wide(name);
    // SAFETY: all input buffers are live; CreateNamedPipe copies the descriptor.
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            1,
            MAX_WIRE_BYTES as u32,
            IO_TIMEOUT_MS,
            Some(&attrs),
        )
    };
    // SAFETY: conversion allocated this descriptor and CreateNamedPipe has finished using it.
    unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    owned(handle)
}

/// Wait for one overlapped operation or shutdown. Always drain canceled I/O before
/// the OVERLAPPED and its buffer leave scope, including timeout and wait failure.
fn finish_io(
    handle: HANDLE,
    op: &mut OVERLAPPED,
    stop: &OwnedHandle,
    timeout: u32,
) -> Result<u32, String> {
    // SAFETY: handles and OVERLAPPED stay alive through completion/cancellation below.
    unsafe {
        if WaitForMultipleObjects(&[raw(stop), op.hEvent], false, timeout).0 != WAIT_OBJECT_0.0 + 1
        {
            let _ = CancelIoEx(handle, Some(op));
            let mut ignored = 0;
            let _ = GetOverlappedResult(handle, op, &mut ignored, true);
            return Err("hook IPC canceled or timed out".into());
        }
        let mut count = 0;
        GetOverlappedResult(handle, op, &mut count, false).map_err(|e| e.to_string())?;
        Ok(count)
    }
}

fn connect(pipe: &OwnedHandle, stop: &OwnedHandle) -> bool {
    let Ok(event) = new_event() else {
        return false;
    };
    let mut op = OVERLAPPED {
        hEvent: raw(&event),
        ..Default::default()
    };
    // SAFETY: pipe was opened for overlapped I/O and op remains alive until completion.
    let result = unsafe { ConnectNamedPipe(raw(pipe), Some(&mut op)) };
    match result {
        Ok(()) => true,
        Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => true,
        Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
            finish_io(raw(pipe), &mut op, stop, INFINITE).is_ok()
        }
        Err(_) => false,
    }
}

fn read_event(pipe: &OwnedHandle, stop: &OwnedHandle) -> Option<BridgeEvent> {
    let done = new_event().ok()?;
    let mut op = OVERLAPPED {
        hEvent: raw(&done),
        ..Default::default()
    };
    let mut bytes = [0; MAX_WIRE_BYTES];
    let mut count = 0;
    // SAFETY: valid bounded buffer, handle and OVERLAPPED; pending I/O is drained below.
    match unsafe { ReadFile(raw(pipe), Some(&mut bytes), Some(&mut count), Some(&mut op)) } {
        Ok(()) => {}
        Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
            count = finish_io(raw(pipe), &mut op, stop, IO_TIMEOUT_MS).ok()?;
        }
        // ERROR_MORE_DATA rejects oversized message frames; never parse a partial frame.
        Err(_) => return None,
    }
    BridgeEvent::decode(&bytes[..count as usize]).ok()
}

fn write_message(handle: &OwnedHandle, bytes: &[u8], stop: &OwnedHandle) -> Result<(), String> {
    let done = new_event()?;
    let mut op = OVERLAPPED {
        hEvent: raw(&done),
        ..Default::default()
    };
    let mut count = 0;
    // SAFETY: the message and OVERLAPPED stay alive until pending I/O completes or is drained.
    match unsafe { WriteFile(raw(handle), Some(bytes), Some(&mut count), Some(&mut op)) } {
        Ok(()) => {}
        Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
            count = finish_io(raw(handle), &mut op, stop, IO_TIMEOUT_MS)?;
        }
        Err(e) => return Err(e.to_string()),
    }
    if count as usize != bytes.len() {
        return Err("incomplete bridge write".into());
    }
    Ok(())
}

pub struct BridgeServer {
    endpoint: String,
    stop: Arc<OwnedHandle>,
    thread: Option<JoinHandle<()>>,
    events: mpsc::Receiver<BridgeEvent>,
}

impl BridgeServer {
    /// Starts a single blocking worker. `wake` must only post an app wake, not touch UI state.
    pub fn start(wake: impl Fn() + Send + 'static) -> Result<Self, String> {
        let endpoint = format!("{PREFIX}{}", random_credential()?.expose());
        let pipe = create_pipe(&endpoint)?;
        let stop = Arc::new(new_event()?);
        let worker_stop = stop.clone();
        let (tx, events) = mpsc::sync_channel(QUEUE_LIMIT);
        let thread = thread::Builder::new()
            .name("agent-notification-ipc".into())
            .spawn(move || {
                while connect(&pipe, &worker_stop) {
                    let mut accepted = false;
                    if let Some(event) = read_event(&pipe, &worker_stop) {
                        if tx.try_send(event).is_ok() {
                            accepted = true;
                            wake();
                        }
                    }
                    // A fixed transport acknowledgement keeps the client alive until its
                    // message is consumed. It cannot carry permission decisions or content.
                    let _ = write_message(&pipe, &[u8::from(accepted)], &worker_stop);
                    // SAFETY: this worker owns the pipe and all its I/O has completed.
                    let _ = unsafe { DisconnectNamedPipe(raw(&pipe)) };
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            endpoint,
            stop,
            thread: Some(thread),
            events,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Bound each UI drain even if clients are continuously producing events.
    pub fn drain(&self) -> Vec<BridgeEvent> {
        self.events.try_iter().take(QUEUE_LIMIT).collect()
    }
}

impl Drop for BridgeServer {
    fn drop(&mut self) {
        // SAFETY: stop is valid for the lifetime of the worker and this object.
        let _ = unsafe { SetEvent(raw(&self.stop)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn valid_endpoint(endpoint: &str) -> bool {
    endpoint
        .strip_prefix(PREFIX)
        .is_some_and(|suffix| suffix.len() == 64 && suffix.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Atomic replacement must retain an existing provider config's access restrictions.
pub fn preserve_config_acl(
    existing: &std::path::Path,
    replacement: &std::path::Path,
) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows::Win32::Security::{
        GetSecurityDescriptorControl, SetFileSecurityW, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
        UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    let source: Vec<_> = existing.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<_> = replacement
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: live terminated paths and writable descriptor output; API allocates the result.
    unsafe {
        GetNamedSecurityInfoW(
            PCWSTR(source.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            &mut descriptor,
        )
    }
    .ok()
    .map_err(|e| e.to_string())?;
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: live allocated descriptor and valid control/revision outputs.
    let result = unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }
        .and_then(|_| {
            let protection = if control & SE_DACL_PROTECTED.0 != 0 {
                PROTECTED_DACL_SECURITY_INFORMATION
            } else {
                UNPROTECTED_DACL_SECURITY_INFORMATION
            };
            // SAFETY: copy only the DACL to our staged replacement, preserving its protection flag.
            unsafe {
                SetFileSecurityW(
                    PCWSTR(target.as_ptr()),
                    DACL_SECURITY_INFORMATION | protection,
                    descriptor,
                )
            }
            .ok()
        });
    // SAFETY: free the GetNamedSecurityInfo allocation exactly once after using it.
    unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    result.map_err(|e| e.to_string())
}

/// A hostile pipe server cannot impersonate the caller or make it execute content.
/// The only response is one transport acknowledgement byte, never a provider decision.
pub fn send(endpoint: &str, event: &BridgeEvent) -> Result<(), String> {
    let bytes = event.encode().map_err(str::to_owned)?;
    send_bytes(endpoint, &bytes)
}

fn send_bytes(endpoint: &str, bytes: &[u8]) -> Result<(), String> {
    if !valid_endpoint(endpoint) {
        return Err("invalid local bridge endpoint".into());
    }
    let name = wide(endpoint);
    let open = || {
        // SAFETY: fixed local endpoint shape, noninheritable handle, identification-only SQOS.
        unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                FILE_GENERIC_WRITE.0 | FILE_GENERIC_READ.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                None,
            )
        }
    };
    let handle = match open() {
        Ok(h) => h,
        Err(_) => {
            // SAFETY: bounded wait for this exact validated local pipe, only on hook invocation.
            if !unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), IO_TIMEOUT_MS) }.as_bool() {
                return Err("local bridge unavailable".into());
            }
            open().map_err(|e| e.to_string())?
        }
    };
    let handle = owned(handle)?;
    let stop = new_event()?;
    write_message(&handle, bytes, &stop)?;
    let done = new_event()?;
    let mut op = OVERLAPPED {
        hEvent: raw(&done),
        ..Default::default()
    };
    let mut count = 0;
    let mut ack = [0];
    // SAFETY: one-byte bounded acknowledgement; pending read is drained below.
    match unsafe {
        ReadFile(
            raw(&handle),
            Some(&mut ack),
            Some(&mut count),
            Some(&mut op),
        )
    } {
        Ok(()) => {}
        Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
            count = finish_io(raw(&handle), &mut op, &stop, IO_TIMEOUT_MS)?;
        }
        Err(e) => return Err(e.to_string()),
    }
    if count != 1 || ack[0] != 1 {
        return Err("bridge event rejected or queue full".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::agent_notifications::{EventKind, Provider, WIRE_VERSION};
    use std::time::{Duration, Instant};

    #[test]
    fn endpoint_and_security_are_user_scoped() {
        let sddl = owner_sddl().unwrap();
        assert!(sddl.starts_with("D:P(A;;GA;;;S-1-"));
        assert_eq!(sddl.matches("(A;").count(), 1);
        for bad in [
            r"\\server\pipe\DaijobuIsland.Agent.a",
            r"\\.\pipe\other",
            "https://example",
        ] {
            assert!(!valid_endpoint(bad));
        }
        let a = random_credential().unwrap();
        let b = random_credential().unwrap();
        assert_ne!(a.expose(), b.expose());
    }

    #[test]
    fn kernel_pipe_has_only_the_requested_user_ace() {
        use windows::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SE_KERNEL_OBJECT,
        };
        use windows::Win32::Security::DACL_SECURITY_INFORMATION;
        let name = format!("{PREFIX}{}", random_credential().unwrap().expose());
        let pipe = create_pipe(&name).unwrap();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: live owned kernel handle and valid writable descriptor output.
        unsafe {
            GetSecurityInfo(
                raw(&pipe),
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                None,
                None,
                Some(&mut descriptor),
            )
        }
        .ok()
        .unwrap();
        let mut string = PWSTR::null();
        // SAFETY: valid allocated descriptor, the conversion allocates its string output.
        unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut string,
                None,
            )
        }
        .unwrap();
        // SAFETY: the API supplied a terminated UTF-16 allocation.
        let actual = unsafe { string.to_string() }.unwrap();
        // SAFETY: each API allocation is freed exactly once after copying its contents.
        unsafe {
            LocalFree(Some(HLOCAL(string.0.cast())));
            LocalFree(Some(HLOCAL(descriptor.0)));
        }
        // The kernel maps GENERIC_ALL (GA) to the named pipe's specific FILE_ALL_ACCESS
        // mask (FA). Compare that normalized mask, retaining the exact sole user SID.
        assert_eq!(actual, owner_sddl().unwrap().replace(";;GA;", ";;FA;"));
    }

    #[test]
    fn native_receiver_rejects_bad_frames_and_bounds_its_queue() {
        let (tx, rx) = mpsc::channel();
        let server = BridgeServer::start(move || {
            let _ = tx.send(());
        })
        .unwrap();
        assert!(send_bytes(server.endpoint(), b"{\"command\":\"calc.exe\"}").is_err());
        // A message bigger than the server read buffer must never be parsed partially.
        let _ = send_bytes(server.endpoint(), &vec![b'x'; MAX_WIRE_BYTES + 1]);
        let event = BridgeEvent {
            version: WIRE_VERSION,
            provider: Provider::Claude,
            generation: 1,
            credential: random_credential().unwrap(),
            session: "fake".into(),
            kind: EventKind::Started,
            event_id: None,
        };
        for n in 0..QUEUE_LIMIT + 2 {
            assert_eq!(send(server.endpoint(), &event).is_ok(), n < QUEUE_LIMIT);
        }
        // The final connections guarantee prior bad frames have been processed. Only
        // valid events produce wakes, and the sync channel never exceeds its bound.
        for _ in 0..QUEUE_LIMIT {
            rx.recv_timeout(Duration::from_secs(3)).unwrap();
        }
        assert_eq!(server.drain().len(), QUEUE_LIMIT);
    }

    #[test]
    fn real_local_pipe_roundtrip_and_idle_shutdown() {
        let (tx, rx) = mpsc::channel();
        let server = BridgeServer::start(move || {
            let _ = tx.send(());
        })
        .unwrap();
        let event = BridgeEvent {
            version: WIRE_VERSION,
            provider: Provider::Codex,
            generation: 1,
            credential: random_credential().unwrap(),
            session: "thr_fake".into(),
            kind: EventKind::Started,
            event_id: None,
        };
        send(server.endpoint(), &event).unwrap();
        rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let received = server.drain();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].session, "thr_fake");
        assert!(server.drain().is_empty());
        let start = Instant::now();
        drop(server);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn stalled_client_does_not_prevent_shutdown() {
        let server = BridgeServer::start(|| {}).unwrap();
        let name = wide(server.endpoint());
        // SAFETY: test opens its own pipe and retains the owned client without sending data.
        let client = owned(
            unsafe {
                CreateFileW(
                    PCWSTR(name.as_ptr()),
                    FILE_GENERIC_WRITE.0,
                    FILE_SHARE_MODE(0),
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    None,
                )
            }
            .unwrap(),
        )
        .unwrap();
        let start = Instant::now();
        drop(server);
        drop(client);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
