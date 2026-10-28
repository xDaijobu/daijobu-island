//! Explicit Clipboard replay and exact-target input. Tests use adapters, never the desktop.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GlobalFree, FILETIME, HANDLE, HGLOBAL, HWND};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, IsValidSid,
    TokenIntegrityLevel, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardOwner,
    GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{CF_DIB, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CONTROL, VK_LBUTTON, VK_LWIN, VK_MBUTTON, VK_MENU, VK_RBUTTON, VK_RETURN,
    VK_RWIN, VK_SHIFT, VK_SPACE, VK_V,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetPropW, GetWindowThreadProcessId, IsIconic, IsWindow, RemovePropW,
    SetForegroundWindow, SetPropW, ShowWindowAsync, SW_RESTORE,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Unicode,
    Html,
    Rtf,
    Png,
    Dib,
    Files,
    CopyEffect,
    NoCloud,
    Replay,
}

pub struct FormatData {
    pub format: Format,
    pub bytes: Vec<u8>,
}

/// A failed write may already have replaced some or all previous clipboard formats.
pub struct Publication {
    pub result: Result<(), String>,
    pub changed: bool,
    pub formats_set: usize,
    /// Final sequence proven by exact replay token and owner after publication closes.
    /// Zero means unavailable/unproven and must never authorize paste or suppression.
    pub sequence: u32,
}

trait ClipboardBackend {
    type Block;
    fn allocate(&mut self, part: &FormatData) -> Result<(u32, Self::Block), String>;
    fn open(&mut self) -> Result<(), String>;
    fn empty(&mut self) -> Result<(), String>;
    fn set(&mut self, format: u32, block: &mut Self::Block) -> Result<(), String>;
    fn close(&mut self) -> Result<(), String>;
    fn sequence(&mut self) -> u32;
    fn owns_replay(&mut self, token: &[u8; 16]) -> Result<bool, String>;
    fn retry_delay(&mut self);
}

struct ClipboardSession<'a, B: ClipboardBackend> {
    backend: &'a mut B,
    open: bool,
}

impl<B: ClipboardBackend> ClipboardSession<'_, B> {
    fn close(mut self) -> Result<(), String> {
        let result = self.backend.close();
        if result.is_ok() {
            self.open = false;
        }
        result
    }
}

impl<B: ClipboardBackend> Drop for ClipboardSession<'_, B> {
    fn drop(&mut self) {
        if self.open {
            let _ = self.backend.close();
        }
    }
}

const FINALIZE_OPEN_ATTEMPTS: usize = 5;

fn finalize_publication<B: ClipboardBackend>(
    backend: &mut B,
    token: &[u8; 16],
) -> Result<u32, String> {
    for attempt in 0..FINALIZE_OPEN_ATTEMPTS {
        if backend.open().is_err() {
            if attempt + 1 == FINALIZE_OPEN_ATTEMPTS {
                return Err("Clipboard is busy; publication could not be verified; automatic paste was not sent".into());
            }
            backend.retry_delay();
            continue;
        }
        let session = ClipboardSession {
            backend,
            open: true,
        };
        let verified = (|| {
            if !session.backend.owns_replay(token)? {
                return Err("The clipboard changed before publication could be verified; automatic paste was not sent".into());
            }
            // CloseClipboard may synthesize formats and advance the write sequence.
            // Only this locked, exact-publication proof can authorize using it.
            let sequence = session.backend.sequence();
            if sequence == 0 {
                return Err(
                    "Clipboard sequence is unavailable; automatic paste was not sent".into(),
                );
            }
            Ok(sequence)
        })();
        let closed = session.close();
        return verified.and_then(|sequence| closed.map(|()| sequence));
    }
    unreachable!("the finite retry loop always returns")
}

fn publish_with<B: ClipboardBackend>(backend: &mut B, parts: &[FormatData]) -> Publication {
    let mut changed = false;
    let mut formats_set = 0;
    let mut sequence = 0;
    let result = (|| {
        if parts.is_empty() || parts.iter().any(|part| part.bytes.is_empty()) {
            return Err("Clipboard publication has no usable data".into());
        }
        // Marker first: every partial payload publication must carry this operation's
        // exact token. Duplicate markers could otherwise replace that proof midway.
        let token: [u8; 16] = parts[0]
            .bytes
            .as_slice()
            .try_into()
            .ok()
            .filter(|_| {
                parts[0].format == Format::Replay
                    && !parts[1..].iter().any(|part| part.format == Format::Replay)
            })
            .ok_or("Clipboard publication has no unique replay token")?;
        // Every registered format and global block exists before opening/emptying.
        let mut blocks = Vec::with_capacity(parts.len());
        for part in parts {
            blocks.push(backend.allocate(part)?);
        }
        backend.open()?;
        let session = ClipboardSession {
            backend,
            open: true,
        };
        let write = (|| {
            session.backend.empty()?;
            changed = true;
            for (format, block) in &mut blocks {
                session.backend.set(*format, block)?;
                formats_set += 1;
            }
            Ok(())
        })();
        let closed = session.close();
        // Partial writes can still be suppressed, but only if the marker was set
        // and the post-close clipboard is proven to be this exact publication.
        let finalized = if closed.is_ok() && changed && formats_set > 0 {
            finalize_publication(backend, &token).map(|final_sequence| {
                sequence = final_sequence;
            })
        } else {
            Ok(())
        };
        match (write, closed) {
            (Err(_), _) if changed => Err(format!(
                "Clipboard publication failed after {formats_set} formats; automatic paste was not sent"
            )),
            (Err(error), _) => Err(error),
            (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => finalized,
        }
    })();
    Publication {
        result,
        changed,
        formats_set,
        sequence,
    }
}

struct Memory(Option<HGLOBAL>);

impl Drop for Memory {
    fn drop(&mut self) {
        if let Some(memory) = self.0.take() {
            // SAFETY: only untransferred GlobalAlloc blocks remain owned by this guard.
            unsafe {
                let _ = GlobalFree(Some(memory));
            }
        }
    }
}

struct NativeClipboard {
    owner: HWND,
    open: bool,
}

impl Drop for NativeClipboard {
    fn drop(&mut self) {
        if self.open {
            // SAFETY: this thread opened the clipboard and still owns the session.
            unsafe {
                let _ = CloseClipboard();
            }
        }
    }
}

fn format_id(format: Format) -> Result<u32, String> {
    // SAFETY: registered names are static, NUL-terminated UTF-16 strings.
    let id = unsafe {
        match format {
            Format::Unicode => CF_UNICODETEXT.0 as u32,
            Format::Dib => CF_DIB.0 as u32,
            Format::Files => CF_HDROP.0 as u32,
            Format::Html => RegisterClipboardFormatW(w!("HTML Format")),
            Format::Rtf => RegisterClipboardFormatW(w!("Rich Text Format")),
            Format::Png => RegisterClipboardFormatW(w!("PNG")),
            Format::CopyEffect => RegisterClipboardFormatW(w!("Preferred DropEffect")),
            Format::NoCloud => RegisterClipboardFormatW(w!("CanUploadToCloudClipboard")),
            Format::Replay => RegisterClipboardFormatW(w!("DaijobuIsland.ClipboardReplay.v1")),
        }
    };
    if id == 0 {
        Err("Clipboard format registration failed".into())
    } else {
        Ok(id)
    }
}

impl ClipboardBackend for NativeClipboard {
    type Block = Memory;

    fn allocate(&mut self, part: &FormatData) -> Result<(u32, Memory), String> {
        let format = format_id(part.format)?;
        // SAFETY: nonempty bounded bytes are copied into a freshly allocated movable
        // block. The guard frees it on failure and transfers it only after Set succeeds.
        unsafe {
            let block = Memory(Some(
                GlobalAlloc(GMEM_MOVEABLE, part.bytes.len())
                    .map_err(|_| "Clipboard allocation failed")?,
            ));
            let memory = block.0.expect("new allocation");
            let pointer = GlobalLock(memory).cast::<u8>();
            if pointer.is_null() {
                return Err("Clipboard allocation lock failed".into());
            }
            std::ptr::copy_nonoverlapping(part.bytes.as_ptr(), pointer, part.bytes.len());
            let _ = GlobalUnlock(memory);
            Ok((format, block))
        }
    }

    fn open(&mut self) -> Result<(), String> {
        // SAFETY: owner is the island window, used as clipboard ownership only.
        unsafe { OpenClipboard(Some(self.owner)) }.map_err(|_| "Clipboard is busy")?;
        self.open = true;
        Ok(())
    }

    fn empty(&mut self) -> Result<(), String> {
        // SAFETY: our thread owns the open session and every format is allocated.
        unsafe { EmptyClipboard() }.map_err(|_| "Clipboard could not be cleared".into())
    }

    fn set(&mut self, format: u32, block: &mut Memory) -> Result<(), String> {
        let memory = block.0.ok_or("Clipboard block was already transferred")?;
        // SAFETY: this is a valid movable block, unlocked, in our open session.
        unsafe { SetClipboardData(format, Some(HANDLE(memory.0))) }
            .map_err(|_| "Clipboard format publication failed")?;
        block.0 = None;
        Ok(())
    }

    fn close(&mut self) -> Result<(), String> {
        // SAFETY: balances this backend's successful OpenClipboard on the same thread.
        unsafe { CloseClipboard() }
            .map_err(|_| "Clipboard close failed; automatic paste was not sent")?;
        self.open = false;
        Ok(())
    }

    fn sequence(&mut self) -> u32 {
        // SAFETY: read-only sequence query; no clipboard contents are acquired.
        unsafe { GetClipboardSequenceNumber() }
    }

    fn owns_replay(&mut self, token: &[u8; 16]) -> Result<bool, String> {
        let replay = format_id(Format::Replay)?;
        // SAFETY: our thread holds the open clipboard. Check the exact nonnull HWND
        // before reading only our fixed-size, eagerly published marker. No payload
        // is read, allocated, retained, or freed; the lock is balanced on every path.
        unsafe {
            if self.owner.is_invalid() || GetClipboardOwner().ok() != Some(self.owner) {
                return Ok(false);
            }
            if IsClipboardFormatAvailable(replay).is_err() {
                return Ok(false);
            }
            let handle = GetClipboardData(replay)
                .map_err(|_| "Clipboard replay token could not be verified")?;
            let memory = HGLOBAL(handle.0);
            // GlobalSize includes allocator padding; match only the logical token.
            if !replay_allocation_valid(GlobalSize(memory)) {
                return Ok(false);
            }
            let pointer = GlobalLock(memory).cast::<u8>();
            if pointer.is_null() {
                return Err("Clipboard replay token could not be locked".into());
            }
            let matches = std::slice::from_raw_parts(pointer, token.len()) == token;
            let _ = GlobalUnlock(memory);
            Ok(matches)
        }
    }

    fn retry_delay(&mut self) {
        // Publication runs on the transfer worker, never on the UI thread.
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn replay_allocation_valid(size: usize) -> bool {
    (16..=64).contains(&size)
}

/// Call on a worker: allocation and foreign-owner clipboard callbacks may be expensive.
pub fn publish(owner: HWND, parts: &[FormatData]) -> Publication {
    publish_with(&mut NativeClipboard { owner, open: false }, parts)
}

/// Explicit search paste only. No retries or UI sleeps and no history side effects.
/// Call off UI if the current owner may provide delayed-rendered text.
pub fn read_search_text(owner: HWND) -> Result<Option<String>, String> {
    let mut clipboard = NativeClipboard { owner, open: false };
    clipboard.open()?;
    // SAFETY: the open-session guard closes on every exit. GlobalSize bounds the copy;
    // the clipboard owns the handle, which is unlocked but never freed here.
    let bytes = unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_err() {
            return Ok(None);
        }
        let handle = GetClipboardData(CF_UNICODETEXT.0 as u32)
            .map_err(|_| "Search clipboard text could not be read")?;
        let memory = HGLOBAL(handle.0);
        let size = GlobalSize(memory);
        if size < 2 || !size.is_multiple_of(2) || size > 2 * 1024 * 1024 + 2 {
            return Err("Search clipboard text exceeds limits or is malformed".into());
        }
        let pointer = GlobalLock(memory).cast::<u8>();
        if pointer.is_null() {
            return Err("Search clipboard text could not be locked".into());
        }
        let bytes = std::slice::from_raw_parts(pointer, size).to_vec();
        let _ = GlobalUnlock(memory);
        bytes
    };
    clipboard.close()?;
    decode_search(&bytes).map(Some)
}

fn decode_search(bytes: &[u8]) -> Result<String, String> {
    if !bytes.len().is_multiple_of(2) {
        return Err("Search clipboard text is malformed".into());
    }
    let units: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .ok_or("Search clipboard text is unterminated")?;
    let text =
        String::from_utf16(&units[..end]).map_err(|_| "Search clipboard text is malformed")?;
    if text.len() > 1024 * 1024 {
        return Err("Search clipboard text exceeds limits".into());
    }
    Ok(text)
}

/// Captured before opening Clipboard. Process birth rejects PID reuse; the window
/// property cookie rejects HWND reuse even within the same process and UI thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetIdentity {
    pub hwnd: isize,
    pub pid: u32,
    pub thread: u32,
    pub created: u64,
    pub window_cookie: usize,
}

const WINDOW_LIFETIME_PROPERTY: windows::core::PCWSTR =
    w!("DaijobuIsland.ClipboardTargetLifetime.v1");
static NEXT_WINDOW_COOKIE: AtomicUsize = AtomicUsize::new(1);
// Only the most recent intentional capture is retained. No per-window map or
// allocated property data survives a new capture or orderly transfer shutdown.
static WINDOW_LIFETIME: Mutex<Option<TargetIdentity>> = Mutex::new(None);

trait TargetWindow {
    fn process_identity(&mut self, hwnd: HWND) -> Option<TargetIdentity>;
    fn cookie(&mut self, hwnd: HWND) -> usize;
    fn install(&mut self, hwnd: HWND, cookie: usize) -> bool;
    fn remove(&mut self, hwnd: HWND);
}

struct NativeTargetWindow;
impl TargetWindow for NativeTargetWindow {
    fn process_identity(&mut self, hwnd: HWND) -> Option<TargetIdentity> {
        process_identity(hwnd)
    }
    fn cookie(&mut self, hwnd: HWND) -> usize {
        // SAFETY: read-only lookup on a possibly stale HWND. The value is an
        // integer cookie, never a pointer to memory or an owned kernel handle.
        unsafe { GetPropW(hwnd, WINDOW_LIFETIME_PROPERTY).0 as usize }
    }
    fn install(&mut self, hwnd: HWND, cookie: usize) -> bool {
        // SAFETY: a fixed private name and nonzero integer value. UIPI failure
        // rejects elevated targets; there is no escalation or attachment fallback.
        unsafe {
            SetPropW(
                hwnd,
                WINDOW_LIFETIME_PROPERTY,
                Some(HANDLE(cookie as *mut _)),
            )
        }
        .is_ok()
    }
    fn remove(&mut self, hwnd: HWND) {
        // SAFETY: callers remove only our matching cookie. It contains no allocated
        // data to free; a destroyed window's property list is no longer reusable.
        unsafe {
            let _ = RemovePropW(hwnd, WINDOW_LIFETIME_PROPERTY);
        }
    }
}

fn identity_with(window: &mut impl TargetWindow, hwnd: HWND) -> Option<TargetIdentity> {
    let cookie = window.cookie(hwnd);
    if cookie == 0 {
        return None;
    }
    let mut target = window.process_identity(hwnd)?;
    if window.cookie(hwnd) != cookie {
        return None;
    }
    target.window_cookie = cookie;
    Some(target)
}

fn remove_lifetime_with(window: &mut impl TargetWindow, target: TargetIdentity) {
    let hwnd = HWND(target.hwnd as *mut _);
    if target.window_cookie != 0 && window.cookie(hwnd) == target.window_cookie {
        window.remove(hwnd);
    }
}

fn capture_with(
    window: &mut impl TargetWindow,
    hwnd: HWND,
    cookie: usize,
    own_pid: u32,
) -> Option<TargetIdentity> {
    let mut expected = window.process_identity(hwnd)?;
    if expected.pid == own_pid || cookie == 0 || !window.install(hwnd, cookie) {
        return None;
    }
    expected.window_cookie = cookie;
    if identity_with(window, hwnd) == Some(expected) {
        Some(expected)
    } else {
        remove_lifetime_with(window, expected);
        None
    }
}

fn capture_replacing_with(
    window: &mut impl TargetWindow,
    lifetime: &mut Option<TargetIdentity>,
    hwnd: HWND,
    cookie: usize,
    own_pid: u32,
) -> Option<TargetIdentity> {
    // A tray popup can make our island the foreground between two capture
    // attempts. Reject that candidate without invalidating the external lease.
    // Retire the old cookie only after the replacement was captured successfully.
    let target = capture_with(window, hwnd, cookie, own_pid)?;
    if let Some(previous) = lifetime.replace(target) {
        remove_lifetime_with(window, previous);
    }
    Some(target)
}

/// Release the single capture property on orderly controller shutdown. Validation
/// never installs, refreshes or removes a property, so stale HWNDs stay rejected.
pub fn release_target_lifetime() {
    let mut lifetime = WINDOW_LIFETIME.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(target) = lifetime.take() {
        remove_lifetime_with(&mut NativeTargetWindow, target);
    }
}

struct KernelHandle(HANDLE);
impl Drop for KernelHandle {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns a real process/token handle.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn process_identity(hwnd: HWND) -> Option<TargetIdentity> {
    // SAFETY: read-only queries on a possibly stale HWND; any failure rejects it.
    unsafe {
        if hwnd.is_invalid() || !IsWindow(Some(hwnd)).as_bool() {
            return None;
        }
        let mut pid = 0;
        let thread = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || thread == 0 {
            return None;
        }
        let process =
            KernelHandle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let (mut created, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        GetProcessTimes(process.0, &mut created, &mut exit, &mut kernel, &mut user).ok()?;
        // Recheck the HWND after opening the process to close the acquisition race.
        let mut after_pid = 0;
        if GetWindowThreadProcessId(hwnd, Some(&mut after_pid)) != thread || after_pid != pid {
            return None;
        }
        Some(TargetIdentity {
            hwnd: hwnd.0 as isize,
            pid,
            thread,
            created: (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
            window_cookie: 0,
        })
    }
}

pub fn capture_external() -> Option<TargetIdentity> {
    let hwnd = crate::snap_pins::focus_target()?.validated_window()?;
    // SAFETY: reads the current process ID without touching other processes.
    let own_pid = unsafe { GetCurrentProcessId() };
    let mut lifetime = WINDOW_LIFETIME.lock().unwrap_or_else(|e| e.into_inner());
    // Never wrap and reuse a cookie: exhaustion fails closed.
    let cookie = NEXT_WINDOW_COOKIE
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .ok()?;
    capture_replacing_with(
        &mut NativeTargetWindow,
        &mut lifetime,
        hwnd,
        cookie,
        own_pid,
    )
}

/// Read-only, fail-closed validation for the retained external target.
pub fn target_is_valid(target: TargetIdentity) -> bool {
    NativeInput.valid(target)
}

/// Escape restoration only; never injects input or bypasses foreground restrictions.
pub fn restore_target(target: TargetIdentity) -> bool {
    NativeInput.activate(target, None)
}

fn integrity(process: HANDLE) -> Option<u32> {
    // SAFETY: queried token is guarded; aligned storage is sized from the token API.
    // The returned SID is valid only while storage lives and is never retained.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let token = KernelHandle(token);
        let mut needed = 0;
        let _ = GetTokenInformation(token.0, TokenIntegrityLevel, None, 0, &mut needed);
        if needed < size_of::<TOKEN_MANDATORY_LABEL>() as u32 || needed > 64 * 1024 {
            return None;
        }
        let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
        GetTokenInformation(
            token.0,
            TokenIntegrityLevel,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
        .ok()?;
        let label = &*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>();
        if !IsValidSid(label.Label.Sid).as_bool() {
            return None;
        }
        let count = *GetSidSubAuthorityCount(label.Label.Sid);
        if count == 0 {
            return None;
        }
        Some(*GetSidSubAuthority(label.Label.Sid, u32::from(count - 1)))
    }
}

trait FocusInput {
    fn valid(&mut self, target: TargetIdentity) -> bool;
    fn eligible(&mut self, target: TargetIdentity) -> bool;
    fn activate(&mut self, target: TargetIdentity, owner: Option<isize>) -> bool;
    fn foreground(&mut self, target: TargetIdentity) -> bool;
    fn activation_allowed(&mut self, target: TargetIdentity, owner: isize) -> bool;
    fn modifiers_down(&mut self) -> bool;
    fn sequence(&mut self) -> u32;
    /// Performs a final native identity/foreground/key check adjacent to SendInput.
    fn paste(&mut self, target: TargetIdentity, sequence: u32) -> bool;
}

struct NativeInput;

impl FocusInput for NativeInput {
    fn valid(&mut self, target: TargetIdentity) -> bool {
        // SAFETY: read-only process identity query; never target our own island with
        // global input. Internal Terminal requests must use their exact-tab adapter.
        let ours = unsafe { GetCurrentProcessId() };
        target.pid != ours
            && target.window_cookie != 0
            && identity_with(&mut NativeTargetWindow, HWND(target.hwnd as *mut _)) == Some(target)
    }
    fn eligible(&mut self, target: TargetIdentity) -> bool {
        // SAFETY: handles are queried, never used to change process privileges.
        unsafe {
            let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, target.pid)
            else {
                return false;
            };
            let process = KernelHandle(handle);
            match (integrity(GetCurrentProcess()), integrity(process.0)) {
                (Some(ours), Some(theirs)) => theirs <= ours,
                _ => false,
            }
        }
    }
    fn activate(&mut self, target: TargetIdentity, owner: Option<isize>) -> bool {
        if !self.valid(target) || !self.eligible(target) {
            return false;
        }
        if owner.is_some_and(|owner| !self.activation_allowed(target, owner)) {
            return false;
        }
        let hwnd = HWND(target.hwnd as *mut _);
        // SAFETY: the exact target identity was verified. Async restore avoids waiting
        // for a foreign window thread; no thread attachment or privilege workaround.
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindowAsync(hwnd, SW_RESTORE);
            }
            if owner.is_some_and(|owner| !self.activation_allowed(target, owner)) {
                return false;
            }
            SetForegroundWindow(hwnd).as_bool()
        }
    }
    fn foreground(&mut self, target: TargetIdentity) -> bool {
        // SAFETY: read-only foreground query followed by full identity validation.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0 as isize != target.hwnd || !self.valid(target) {
            return false;
        }
        // SAFETY: repeat the cheap foreground query after process-token queries, so
        // losing focus during identity validation cannot authorize a paste.
        unsafe { GetForegroundWindow() == hwnd }
    }
    fn activation_allowed(&mut self, target: TargetIdentity, owner: isize) -> bool {
        // SAFETY: read-only foreground/PID queries; the owner is our live island.
        // A user's move to any other app while modifiers are held must not be
        // undone by restoring the previously captured paste target.
        unsafe {
            let foreground = GetForegroundWindow();
            if foreground.is_invalid() {
                return false;
            }
            if foreground.0 as isize == target.hwnd {
                return self.foreground(target);
            }
            if owner == 0 || foreground.0 as isize != owner {
                return false;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(foreground, Some(&mut pid));
            pid == GetCurrentProcessId() && GetForegroundWindow() == foreground
        }
    }
    fn modifiers_down(&mut self) -> bool {
        [
            VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN, VK_V, VK_RETURN, VK_SPACE, VK_LBUTTON,
            VK_RBUTTON, VK_MBUTTON,
        ]
        .into_iter()
        .chain((0x31..=0x39).map(VIRTUAL_KEY))
        .any(|key| {
            // SAFETY: physical key-state query only; never release the user's keys.
            unsafe { GetAsyncKeyState(i32::from(key.0)) < 0 }
        })
    }
    fn sequence(&mut self) -> u32 {
        // SAFETY: read-only sequence query; never opens or reads clipboard payloads.
        unsafe { GetClipboardSequenceNumber() }
    }
    fn paste(&mut self, target: TargetIdentity, sequence: u32) -> bool {
        if !self.eligible(target)
            || self.sequence() != sequence
            || self.modifiers_down()
            || !self.foreground(target)
        {
            return false;
        }
        send_paste_with(&mut NativeKeyboard)
    }
}

trait KeyboardInput {
    fn send(&mut self, keys: &[INPUT]) -> usize;
}
struct NativeKeyboard;
impl KeyboardInput for NativeKeyboard {
    fn send(&mut self, keys: &[INPUT]) -> usize {
        // SAFETY: the caller checked target/foreground/physical keys immediately before
        // the chord. Only Ctrl+V or key-up-only cleanup is constructed by this module.
        unsafe { SendInput(keys, size_of::<INPUT>() as i32) as usize }
    }
}

fn send_paste_with(input: &mut impl KeyboardInput) -> bool {
    let keys = [
        key(VK_CONTROL, false),
        key(VK_V, false),
        key(VK_V, true),
        key(VK_CONTROL, true),
    ];
    let sent = input.send(&keys);
    if sent == keys.len() {
        return true;
    }
    let cleanup = cleanup_keys(sent);
    if !cleanup.is_empty() {
        input.send(&cleanup);
    }
    false
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    Default::default()
                },
                ..Default::default()
            },
        },
    }
}

fn cleanup_keys(sent: usize) -> Vec<INPUT> {
    match sent {
        1 | 3 => vec![key(VK_CONTROL, true)],
        2 => vec![key(VK_V, true), key(VK_CONTROL, true)],
        _ => Vec::new(),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PasteProgress {
    Waiting,
    Sent,
    Manual(String),
}

/// The caller hides Clipboard before the first poll. Poll from its bounded UI timer.
pub struct PasteSession {
    target: TargetIdentity,
    sequence: u32,
    owner: isize,
    deadline: Instant,
    activated: bool,
    done: bool,
}

impl PasteSession {
    pub fn new(target: TargetIdentity, sequence: u32, owner: isize, now: Instant) -> Self {
        Self {
            target,
            sequence,
            owner,
            deadline: now + Duration::from_millis(1200),
            activated: false,
            done: false,
        }
    }
    pub fn tick(&mut self, now: Instant) -> PasteProgress {
        self.poll(now, &mut NativeInput)
    }
    fn poll(&mut self, now: Instant, input: &mut impl FocusInput) -> PasteProgress {
        if self.done {
            return PasteProgress::Waiting;
        }
        let progress = self.step(now, input);
        if !matches!(progress, PasteProgress::Waiting) {
            self.done = true;
        }
        progress
    }
    fn step(&mut self, now: Instant, input: &mut impl FocusInput) -> PasteProgress {
        let manual =
            |reason: &str| PasteProgress::Manual(format!("Copied; {reason}. Paste manually."));
        if now >= self.deadline {
            return manual("automatic paste timed out");
        }
        if input.sequence() != self.sequence {
            return manual("the clipboard changed before paste");
        }
        if !input.valid(self.target) {
            return manual("the target closed or changed identity");
        }
        if !input.eligible(self.target) {
            return manual("the target does not allow automatic input");
        }
        if !self.activated {
            if !input.activation_allowed(self.target, self.owner) {
                return manual("focus changed before target activation");
            }
            if input.modifiers_down() {
                return PasteProgress::Waiting;
            }
            if !input.activation_allowed(self.target, self.owner) {
                return manual("focus changed before target activation");
            }
            if !input.activate(self.target, Some(self.owner)) {
                return manual("the target could not be activated");
            }
            self.activated = true;
            // Give async restoration a UI turn; no blocking wait or injected key yet.
            return PasteProgress::Waiting;
        }
        if !input.foreground(self.target) {
            return manual("focus changed before paste");
        }
        if input.modifiers_down() {
            return if now >= self.deadline {
                manual("selection or shortcut keys are still held")
            } else {
                PasteProgress::Waiting
            };
        }
        if now >= self.deadline {
            return manual("automatic paste timed out");
        }
        if !input.valid(self.target)
            || !input.eligible(self.target)
            || !input.foreground(self.target)
        {
            return manual("the target changed before paste");
        }
        if input.paste(self.target, self.sequence) {
            PasteProgress::Sent
        } else {
            manual("automatic input was refused or incomplete")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct Audit {
        calls: Vec<&'static str>,
        freed: usize,
        transferred: usize,
    }
    struct FakeBlock {
        audit: Rc<RefCell<Audit>>,
        transferred: bool,
    }
    impl Drop for FakeBlock {
        fn drop(&mut self) {
            if !self.transferred {
                self.audit.borrow_mut().freed += 1;
            }
        }
    }
    #[derive(Default)]
    struct FakeClipboard {
        audit: Rc<RefCell<Audit>>,
        allocation: usize,
        writes: usize,
        fail_allocation: Option<usize>,
        fail_write: Option<usize>,
        busy: bool,
        fail_empty: bool,
        fail_close: bool,
        closed: bool,
        opens: usize,
        closes: usize,
        busy_reopens: usize,
        retry_delays: usize,
        foreign_owner: bool,
        changed_token: bool,
        fail_verify: bool,
        fail_final_close: bool,
        zero_sequence: bool,
    }
    impl ClipboardBackend for FakeClipboard {
        type Block = FakeBlock;
        fn allocate(&mut self, _: &FormatData) -> Result<(u32, FakeBlock), String> {
            self.audit.borrow_mut().calls.push("allocate");
            self.allocation += 1;
            if self.fail_allocation == Some(self.allocation) {
                return Err("allocation failed".into());
            }
            Ok((
                self.allocation as u32,
                FakeBlock {
                    audit: self.audit.clone(),
                    transferred: false,
                },
            ))
        }
        fn open(&mut self) -> Result<(), String> {
            self.audit.borrow_mut().calls.push("open");
            self.opens += 1;
            if self.busy || (self.opens > 1 && self.opens <= self.busy_reopens + 1) {
                Err("busy".into())
            } else {
                Ok(())
            }
        }
        fn empty(&mut self) -> Result<(), String> {
            self.audit.borrow_mut().calls.push("empty");
            if self.fail_empty {
                Err("empty failed".into())
            } else {
                Ok(())
            }
        }
        fn set(&mut self, _: u32, block: &mut FakeBlock) -> Result<(), String> {
            self.audit.borrow_mut().calls.push("set");
            self.writes += 1;
            if self.fail_write == Some(self.writes) {
                return Err("set failed".into());
            }
            block.transferred = true;
            self.audit.borrow_mut().transferred += 1;
            Ok(())
        }
        fn close(&mut self) -> Result<(), String> {
            self.audit.borrow_mut().calls.push("close");
            self.closes += 1;
            self.closed = true;
            if self.fail_close || (self.fail_final_close && self.closes >= 2) {
                Err("close failed".into())
            } else {
                Ok(())
            }
        }
        fn sequence(&mut self) -> u32 {
            self.audit.borrow_mut().calls.push("sequence");
            if self.zero_sequence {
                0
            } else if self.closed {
                88
            } else {
                42
            }
        }
        fn owns_replay(&mut self, token: &[u8; 16]) -> Result<bool, String> {
            self.audit.borrow_mut().calls.push("verify");
            assert_eq!(token, &[1; 16]);
            if self.fail_verify {
                Err("verification failed".into())
            } else {
                Ok(!self.foreign_owner && !self.changed_token)
            }
        }
        fn retry_delay(&mut self) {
            self.retry_delays += 1;
        }
    }
    fn parts() -> Vec<FormatData> {
        [
            Format::Replay,
            Format::NoCloud,
            Format::Unicode,
            Format::Html,
        ]
        .into_iter()
        .map(|format| FormatData {
            format,
            bytes: if format == Format::Replay {
                vec![1; 16]
            } else {
                vec![1, 2]
            },
        })
        .collect()
    }

    #[test]
    fn replay_allocation_allows_bounded_padding_but_not_truncation_or_oversize() {
        for size in [16, 24, 32, 64] {
            assert!(replay_allocation_valid(size));
        }
        for size in [0, 15, 65, usize::MAX] {
            assert!(!replay_allocation_valid(size));
        }
    }

    #[test]
    fn all_allocations_precede_empty_and_success_transfers_every_block() {
        let mut clipboard = FakeClipboard::default();
        let publication = publish_with(&mut clipboard, &parts());
        assert!(publication.result.is_ok());
        assert!(publication.changed);
        assert_eq!(publication.formats_set, 4);
        assert_eq!(
            publication.sequence, 88,
            "sequence must include close-time changes and be proven while reopened"
        );
        let audit = clipboard.audit.borrow();
        assert_eq!(
            &audit.calls[..6],
            &["allocate", "allocate", "allocate", "allocate", "open", "empty"]
        );
        assert_eq!(
            &audit.calls[10..],
            &["close", "open", "verify", "sequence", "close"]
        );
        assert_eq!((audit.freed, audit.transferred), (0, 4));
    }

    #[test]
    fn allocation_failure_never_opens_or_empties_and_frees_prior_allocations() {
        for failed in 1..=4 {
            let mut clipboard = FakeClipboard {
                fail_allocation: Some(failed),
                ..Default::default()
            };
            let publication = publish_with(&mut clipboard, &parts());
            assert!(publication.result.is_err());
            assert!(!publication.changed);
            assert_eq!(clipboard.audit.borrow().freed, failed - 1);
            assert!(!clipboard.audit.borrow().calls.contains(&"open"));
        }
    }

    #[test]
    fn failed_publication_closes_and_frees_only_untransferred_handles() {
        for failed in 1..=4 {
            let mut clipboard = FakeClipboard {
                fail_write: Some(failed),
                ..Default::default()
            };
            let publication = publish_with(&mut clipboard, &parts());
            assert!(publication
                .result
                .unwrap_err()
                .contains(&format!("after {} formats", failed - 1)));
            assert!(publication.changed);
            assert_eq!(publication.formats_set, failed - 1);
            assert_eq!(publication.sequence, if failed > 1 { 88 } else { 0 });
            assert!(clipboard.closed);
            let audit = clipboard.audit.borrow();
            assert_eq!((audit.freed, audit.transferred), (5 - failed, failed - 1));
        }
    }

    #[test]
    fn busy_empty_and_close_failures_keep_their_honest_changed_state() {
        let mut busy = FakeClipboard {
            busy: true,
            ..Default::default()
        };
        let result = publish_with(&mut busy, &parts());
        assert!(!result.changed && result.result.is_err());
        assert!(!busy.audit.borrow().calls.contains(&"empty"));
        assert_eq!(busy.audit.borrow().freed, 4);
        let mut empty = FakeClipboard {
            fail_empty: true,
            ..Default::default()
        };
        let result = publish_with(&mut empty, &parts());
        assert!(!result.changed && result.result.is_err() && empty.closed);
        let mut close = FakeClipboard {
            fail_close: true,
            ..Default::default()
        };
        let result = publish_with(&mut close, &parts());
        assert!(result.changed && result.result.is_err());
        assert_eq!(result.formats_set, 4);
        assert_eq!(result.sequence, 0);
    }

    #[test]
    fn finalization_retries_contention_finitely_without_republishing() {
        for busy_reopens in [2, FINALIZE_OPEN_ATTEMPTS] {
            let mut clipboard = FakeClipboard {
                busy_reopens,
                ..Default::default()
            };
            let result = publish_with(&mut clipboard, &parts());
            assert_eq!(result.result.is_ok(), busy_reopens == 2);
            assert_eq!(result.sequence, if busy_reopens == 2 { 88 } else { 0 });
            assert_eq!(
                clipboard.retry_delays,
                busy_reopens.min(FINALIZE_OPEN_ATTEMPTS - 1)
            );
            assert_eq!(clipboard.writes, 4);
            assert_eq!(clipboard.audit.borrow().transferred, 4);
        }
    }

    #[test]
    fn unproven_publications_fail_closed_and_release_reopened_session() {
        for case in 0..5 {
            let mut clipboard = FakeClipboard {
                foreign_owner: case == 0,
                changed_token: case == 1,
                fail_verify: case == 2,
                fail_final_close: case == 3,
                zero_sequence: case == 4,
                ..Default::default()
            };
            let result = publish_with(&mut clipboard, &parts());
            assert!(result.result.is_err());
            assert!(result.changed);
            assert_eq!(result.sequence, 0);
            assert_eq!(result.formats_set, 4);
            assert_eq!(clipboard.closes, if case == 3 { 3 } else { 2 });
            assert_eq!(clipboard.audit.borrow().freed, 0);
            if case < 3 {
                assert!(!clipboard.audit.borrow().calls.contains(&"sequence"));
            }
        }
    }

    #[test]
    fn invalid_or_duplicate_replay_markers_never_touch_clipboard() {
        for case in 0..3 {
            let mut values = parts();
            match case {
                0 => values[0].bytes.pop().map(|_| ()).unwrap(),
                1 => values[0].format = Format::Unicode,
                _ => values[1].format = Format::Replay,
            }
            let mut clipboard = FakeClipboard::default();
            let result = publish_with(&mut clipboard, &values);
            assert!(result.result.is_err());
            assert!(!result.changed);
            assert!(clipboard.audit.borrow().calls.is_empty());
        }
    }

    fn target() -> TargetIdentity {
        TargetIdentity {
            hwnd: 10,
            pid: 20,
            thread: 30,
            created: 40,
            window_cookie: 50,
        }
    }

    struct FakeTargetWindow {
        current: TargetIdentity,
        writable: bool,
        installs: usize,
        removals: usize,
        change_during_query: bool,
    }
    impl Default for FakeTargetWindow {
        fn default() -> Self {
            Self {
                current: target(),
                writable: true,
                installs: 0,
                removals: 0,
                change_during_query: false,
            }
        }
    }
    impl TargetWindow for FakeTargetWindow {
        fn process_identity(&mut self, hwnd: HWND) -> Option<TargetIdentity> {
            if hwnd.is_invalid() {
                return None;
            }
            if hwnd.0 as isize != self.current.hwnd {
                // A separate synthetic island window shown by the tray popup.
                return Some(TargetIdentity {
                    hwnd: hwnd.0 as isize,
                    pid: 100,
                    window_cookie: 0,
                    ..self.current
                });
            }
            if self.change_during_query {
                self.current.window_cookie += 1;
            }
            Some(TargetIdentity {
                window_cookie: 0,
                ..self.current
            })
        }
        fn cookie(&mut self, _: HWND) -> usize {
            self.current.window_cookie
        }
        fn install(&mut self, _: HWND, cookie: usize) -> bool {
            self.installs += 1;
            if self.writable {
                self.current.window_cookie = cookie;
            }
            self.writable
        }
        fn remove(&mut self, _: HWND) {
            self.removals += 1;
            self.current.window_cookie = 0;
        }
    }

    #[test]
    fn lifetime_capture_installs_once_and_validation_never_refreshes_cookie() {
        let hwnd = HWND(target().hwnd as *mut _);
        let mut window = FakeTargetWindow::default();
        let saved = capture_with(&mut window, hwnd, 99, 100).unwrap();
        assert_eq!(saved.window_cookie, 99);
        assert_eq!(identity_with(&mut window, hwnd), Some(saved));
        assert_eq!((window.installs, window.removals), (1, 0));
        // Same HWND, PID, thread and process birth, but a replacement lifetime.
        for replacement in [0, 100] {
            window.current.window_cookie = replacement;
            assert_ne!(identity_with(&mut window, hwnd), Some(saved));
            assert_eq!((window.installs, window.removals), (1, 0));
        }
        // Old cleanup cannot remove a later intentional capture's cookie.
        remove_lifetime_with(&mut window, saved);
        assert_eq!(window.current.window_cookie, 100);
        assert_eq!(window.removals, 0);
        let fresh = capture_with(&mut window, hwnd, 101, 100).unwrap();
        remove_lifetime_with(&mut window, fresh);
        assert_eq!((window.current.window_cookie, window.removals), (0, 1));
    }

    #[test]
    fn property_failure_zero_cookie_and_own_process_fail_capture_closed() {
        let hwnd = HWND(target().hwnd as *mut _);
        let mut window = FakeTargetWindow {
            writable: false,
            ..Default::default()
        };
        assert!(capture_with(&mut window, hwnd, 99, 100).is_none());
        assert_eq!(window.installs, 1);
        assert_eq!(window.current.window_cookie, target().window_cookie);
        window.writable = true;
        assert!(capture_with(&mut window, hwnd, 0, 100).is_none());
        assert!(capture_with(&mut window, hwnd, 99, target().pid).is_none());
        assert_eq!(window.installs, 1);
    }

    #[test]
    fn unsuccessful_tray_recapture_preserves_previous_external_lifetime() {
        let external = HWND(target().hwnd as *mut _);
        let island = HWND(99usize as *mut _);
        let mut window = FakeTargetWindow::default();
        let mut lifetime = None;
        let saved = capture_replacing_with(&mut window, &mut lifetime, external, 99, 100).unwrap();
        // Popup activation makes the island foreground. Its rejection must not
        // remove the cookie installed for the earlier external foreground.
        assert!(capture_replacing_with(&mut window, &mut lifetime, island, 100, 100).is_none());
        assert_eq!(lifetime, Some(saved));
        assert_eq!(identity_with(&mut window, external), Some(saved));
        assert_eq!((window.installs, window.removals), (1, 0));
        assert!(
            capture_replacing_with(&mut window, &mut lifetime, HWND::default(), 101, 100).is_none()
        );
        window.writable = false;
        assert!(capture_replacing_with(&mut window, &mut lifetime, external, 102, 100).is_none());
        assert_eq!(lifetime, Some(saved));
        assert_eq!(identity_with(&mut window, external), Some(saved));
        assert_eq!(window.removals, 0);
    }

    #[test]
    fn cookie_change_during_process_query_rejects_validation_without_mutation() {
        let mut window = FakeTargetWindow {
            change_during_query: true,
            ..Default::default()
        };
        assert!(identity_with(&mut window, HWND(target().hwnd as *mut _)).is_none());
        assert_eq!((window.installs, window.removals), (0, 0));
    }

    struct FakeInput {
        current: Option<TargetIdentity>,
        eligible: bool,
        activate: bool,
        foreground: bool,
        activation_allowed: bool,
        held: bool,
        send: bool,
        steal_on_send: bool,
        activate_count: usize,
        paste_count: usize,
        sequence: u32,
    }
    impl Default for FakeInput {
        fn default() -> Self {
            Self {
                current: Some(target()),
                eligible: true,
                activate: true,
                foreground: true,
                activation_allowed: true,
                held: false,
                send: true,
                steal_on_send: false,
                activate_count: 0,
                paste_count: 0,
                sequence: 42,
            }
        }
    }
    impl FocusInput for FakeInput {
        fn valid(&mut self, target: TargetIdentity) -> bool {
            self.current == Some(target)
        }
        fn eligible(&mut self, _: TargetIdentity) -> bool {
            self.eligible
        }
        fn activate(&mut self, _: TargetIdentity, _: Option<isize>) -> bool {
            self.activate_count += 1;
            self.activate
        }
        fn foreground(&mut self, _: TargetIdentity) -> bool {
            self.foreground
        }
        fn activation_allowed(&mut self, _: TargetIdentity, _: isize) -> bool {
            self.activation_allowed
        }
        fn modifiers_down(&mut self) -> bool {
            self.held
        }
        fn sequence(&mut self) -> u32 {
            self.sequence
        }
        fn paste(&mut self, _: TargetIdentity, _: u32) -> bool {
            if self.steal_on_send {
                return false;
            }
            self.paste_count += 1;
            self.send
        }
    }

    #[test]
    fn valid_target_waits_a_turn_then_sends_once() {
        let now = Instant::now();
        let mut session = PasteSession::new(target(), 42, 0, now);
        let mut input = FakeInput::default();
        assert_eq!(session.poll(now, &mut input), PasteProgress::Waiting);
        assert_eq!(input.paste_count, 0);
        assert_eq!(
            session.poll(now + Duration::from_millis(16), &mut input),
            PasteProgress::Sent
        );
        assert_eq!(
            session.poll(now + Duration::from_millis(32), &mut input),
            PasteProgress::Waiting
        );
        assert_eq!((input.activate_count, input.paste_count), (1, 1));
    }

    #[test]
    fn reused_window_or_missing_cookie_never_activates_or_sends_input() {
        for cookie in [0, target().window_cookie + 1] {
            let now = Instant::now();
            let mut session = PasteSession::new(target(), 42, 0, now);
            let mut input = FakeInput {
                current: Some(TargetIdentity {
                    window_cookie: cookie,
                    ..target()
                }),
                ..Default::default()
            };
            assert!(matches!(
                session.poll(now, &mut input),
                PasteProgress::Manual(_)
            ));
            assert_eq!((input.activate_count, input.paste_count), (0, 0));

            // A destruction/recreation after activation also cannot send keys.
            let mut session = PasteSession::new(target(), 42, 0, now);
            input.current = Some(target());
            assert_eq!(session.poll(now, &mut input), PasteProgress::Waiting);
            input.current.as_mut().unwrap().window_cookie = cookie;
            assert!(matches!(
                session.poll(now, &mut input),
                PasteProgress::Manual(_)
            ));
            assert_eq!((input.activate_count, input.paste_count), (1, 0));
        }
    }

    #[test]
    fn held_modifiers_wait_without_releasing_or_sending_then_timeout() {
        let now = Instant::now();
        let mut session = PasteSession::new(target(), 42, 0, now);
        let mut input = FakeInput {
            held: true,
            ..Default::default()
        };
        for ms in [0, 16, 500, 1199] {
            assert_eq!(
                session.poll(now + Duration::from_millis(ms), &mut input),
                PasteProgress::Waiting
            );
        }
        assert!(matches!(
            session.poll(now + Duration::from_millis(1200), &mut input),
            PasteProgress::Manual(_)
        ));
        assert_eq!(input.paste_count, 0);
        assert!(
            input.held,
            "never synthesize release of a user's physical modifier"
        );
        assert_eq!(
            input.activate_count, 0,
            "wait for physical release before restoring focus"
        );
    }

    #[test]
    fn modifier_release_before_deadline_allows_paste() {
        let now = Instant::now();
        let mut session = PasteSession::new(target(), 42, 0, now);
        let mut input = FakeInput {
            held: true,
            ..Default::default()
        };
        session.poll(now, &mut input);
        session.poll(now + Duration::from_millis(50), &mut input);
        input.held = false;
        assert_eq!(
            session.poll(now + Duration::from_millis(100), &mut input),
            PasteProgress::Waiting
        );
        assert_eq!(
            session.poll(now + Duration::from_millis(116), &mut input),
            PasteProgress::Sent
        );
    }

    #[test]
    fn foreground_change_while_modifiers_held_never_activates_old_target() {
        for release_before_poll in [false, true] {
            let now = Instant::now();
            let mut session = PasteSession::new(target(), 42, 99, now);
            let mut input = FakeInput {
                held: true,
                ..Default::default()
            };
            assert_eq!(session.poll(now, &mut input), PasteProgress::Waiting);
            input.activation_allowed = false;
            input.held = !release_before_poll;
            assert!(matches!(
                session.poll(now + Duration::from_millis(16), &mut input),
                PasteProgress::Manual(_)
            ));
            input.held = false;
            // Even returning to the original app cannot revive an aborted paste.
            input.activation_allowed = true;
            assert_eq!(
                session.poll(now + Duration::from_millis(32), &mut input),
                PasteProgress::Waiting
            );
            assert_eq!((input.activate_count, input.paste_count), (0, 0));
        }
    }

    #[test]
    fn intervening_clipboard_write_abandons_paste_without_touching_focus_or_keys() {
        let now = Instant::now();
        let mut session = PasteSession::new(target(), 42, 0, now);
        let mut input = FakeInput {
            sequence: 43,
            ..Default::default()
        };
        assert!(matches!(
            session.poll(now, &mut input),
            PasteProgress::Manual(_)
        ));
        assert_eq!((input.activate_count, input.paste_count), (0, 0));
        let mut session = PasteSession::new(target(), 42, 0, now);
        input.sequence = 42;
        assert_eq!(session.poll(now, &mut input), PasteProgress::Waiting);
        input.sequence = 43;
        assert!(matches!(
            session.poll(now, &mut input),
            PasteProgress::Manual(_)
        ));
        assert_eq!(input.paste_count, 0);
    }

    #[test]
    fn closed_reused_elevated_and_unactivatable_targets_send_no_keys() {
        let now = Instant::now();
        let mut reused = target();
        reused.created += 1;
        let mut reused_pid = target();
        reused_pid.pid += 1;
        let mut reused_window = target();
        reused_window.window_cookie += 1;
        let mut missing_property = target();
        missing_property.window_cookie = 0;
        for mut input in [
            FakeInput {
                current: None,
                ..Default::default()
            },
            FakeInput {
                current: Some(reused),
                ..Default::default()
            },
            FakeInput {
                current: Some(reused_pid),
                ..Default::default()
            },
            FakeInput {
                current: Some(reused_window),
                ..Default::default()
            },
            FakeInput {
                current: Some(missing_property),
                ..Default::default()
            },
            FakeInput {
                eligible: false,
                ..Default::default()
            },
            FakeInput {
                activate: false,
                ..Default::default()
            },
        ] {
            let mut session = PasteSession::new(target(), 42, 0, now);
            assert!(matches!(
                session.poll(now, &mut input),
                PasteProgress::Manual(_)
            ));
            assert_eq!(input.paste_count, 0);
        }
    }

    #[test]
    fn stolen_focus_and_final_check_failure_never_retry_key_downs() {
        let now = Instant::now();
        let mut input = FakeInput::default();
        let mut session = PasteSession::new(target(), 42, 0, now);
        session.poll(now, &mut input);
        input.foreground = false;
        assert!(matches!(
            session.poll(now, &mut input),
            PasteProgress::Manual(_)
        ));
        assert_eq!((input.activate_count, input.paste_count), (1, 0));
        let mut input = FakeInput {
            steal_on_send: true,
            ..Default::default()
        };
        let mut session = PasteSession::new(target(), 42, 0, now);
        session.poll(now, &mut input);
        assert!(matches!(
            session.poll(now, &mut input),
            PasteProgress::Manual(_)
        ));
        assert_eq!(input.paste_count, 0);
        let mut input = FakeInput {
            send: false,
            ..Default::default()
        };
        let mut session = PasteSession::new(target(), 42, 0, now);
        session.poll(now, &mut input);
        assert!(matches!(
            session.poll(now, &mut input),
            PasteProgress::Manual(_)
        ));
        session.poll(now, &mut input);
        assert_eq!(input.paste_count, 1);
    }

    #[test]
    fn partial_send_cleanup_contains_only_necessary_key_ups_and_never_enter() {
        for (sent, expected) in [
            (0, vec![]),
            (1, vec![VK_CONTROL]),
            (2, vec![VK_V, VK_CONTROL]),
            (3, vec![VK_CONTROL]),
            (4, vec![]),
        ] {
            let cleanup = cleanup_keys(sent);
            let mut actual = Vec::new();
            for input in cleanup {
                assert_eq!(input.r#type, INPUT_KEYBOARD);
                // SAFETY: `key` creates only the keyboard member of INPUT's union.
                let keyboard = unsafe { input.Anonymous.ki };
                assert_eq!(keyboard.dwFlags, KEYEVENTF_KEYUP);
                actual.push(keyboard.wVk);
            }
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn injected_chord_is_exact_and_partial_input_never_retries_a_press_or_enter() {
        struct FakeKeyboard {
            sent: usize,
            calls: Vec<Vec<(VIRTUAL_KEY, bool)>>,
        }
        impl KeyboardInput for FakeKeyboard {
            fn send(&mut self, keys: &[INPUT]) -> usize {
                self.calls.push(
                    keys.iter()
                        .map(|input| {
                            // SAFETY: this module constructs INPUT_KEYBOARD entries only.
                            let keyboard = unsafe { input.Anonymous.ki };
                            (keyboard.wVk, keyboard.dwFlags == KEYEVENTF_KEYUP)
                        })
                        .collect(),
                );
                if self.calls.len() == 1 {
                    self.sent
                } else {
                    keys.len()
                }
            }
        }
        for sent in 0..=4 {
            let mut keyboard = FakeKeyboard {
                sent,
                calls: Vec::new(),
            };
            assert_eq!(send_paste_with(&mut keyboard), sent == 4);
            assert_eq!(
                keyboard.calls[0],
                [
                    (VK_CONTROL, false),
                    (VK_V, false),
                    (VK_V, true),
                    (VK_CONTROL, true)
                ]
            );
            for cleanup in &keyboard.calls[1..] {
                assert!(cleanup
                    .iter()
                    .all(|(key, up)| *up && [VK_CONTROL, VK_V].contains(key)));
            }
            assert_eq!(
                keyboard.calls.len(),
                if sent > 0 && sent < 4 { 2 } else { 1 }
            );
        }
    }

    #[test]
    fn search_text_is_strict_bounded_utf16_with_a_required_terminator() {
        let encode = |text: &str| {
            text.encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            decode_search(&encode("emoji 🙂\r\n日本語")).unwrap(),
            "emoji 🙂\r\n日本語"
        );
        assert_eq!(decode_search(&encode("")).unwrap(), "");
        assert!(decode_search(&[1]).is_err());
        assert!(decode_search(&[1, 0]).is_err());
        assert!(decode_search(&[0, 0xD8, 0, 0]).is_err());
        assert!(decode_search(&encode(&"x".repeat(1024 * 1024 + 1))).is_err());
    }
}
