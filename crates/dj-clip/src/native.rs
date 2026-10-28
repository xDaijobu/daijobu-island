//! Native message-window listener and bounded, owned clipboard snapshots.
//!
//! Privacy names/semantics: Microsoft, Clipboard Formats (Cloud Clipboard and
//! Clipboard History Formats):
//! https://learn.microsoft.com/windows/win32/dataxchg/clipboard-formats
//! Clipboard-owned handles are copied while open, never freed or retained:
//! https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-getclipboarddata
use crate::monitor::{Shared, Ticket};
use crate::{Captured, MonitorEvent, RawFormat, RawPart, RawSnapshot, REPLAY_FORMAT};
use dj_core::clipboard_types::{
    MAX_PATH_BYTES, MAX_PNG_BYTES, MAX_RAW_BYTES, MAX_RICH_BYTES, MAX_TEXT_BYTES,
};
use std::sync::{mpsc::SyncSender, Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const CONTROL: u32 = WM_APP + 73;
const RETRY_TIMER: usize = 1;
const RETRY_MS: u32 = 40;
const MAX_ATTEMPTS: u8 = 4;
const MAX_ALLOCATION_PADDING: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Exclude,
    History,
    Cloud,
    Replay,
    Payload(RawFormat),
}

#[derive(Default)]
struct Owner {
    own_process: bool,
    executable: Option<String>,
}

/// Kept private so tests cannot accidentally instantiate a real native adapter.
trait Adapter {
    fn sequence(&mut self) -> u32;
    fn open(&mut self) -> bool;
    fn close(&mut self);
    fn present(&mut self, format: Format) -> bool;
    fn read(&mut self, format: Format, limit: usize) -> Result<Vec<u8>, &'static str>;
    fn owner(&mut self) -> Owner;
}

struct Session<'a, A: Adapter>(&'a mut A);
impl<A: Adapter> Drop for Session<'_, A> {
    fn drop(&mut self) {
        self.0.close();
    }
}

enum Acquired {
    Busy,
    Stale,
    Skipped(&'static str),
    Snapshot(RawSnapshot),
}

fn excluded(source: Option<&str>, policy: &[String]) -> bool {
    let Some(source) = source else { return false };
    let source = source.replace('/', "\\").to_lowercase();
    let basename = source.rsplit('\\').next().unwrap_or(&source);
    policy.iter().any(|name| {
        let name = name.replace('/', "\\").to_lowercase();
        !name.is_empty()
            && if name.contains('\\') {
                source == name
            } else {
                basename == name
            }
    })
}

fn flag<A: Adapter>(adapter: &mut A, format: Format) -> Result<Option<bool>, &'static str> {
    if !adapter.present(format) {
        return Ok(None);
    }
    let bytes = adapter.read(format, 4)?;
    let value = u32::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| "Malformed clipboard privacy flag")?,
    );
    match value {
        0 => Ok(Some(false)),
        1 => Ok(Some(true)),
        _ => Err("Malformed clipboard privacy flag"),
    }
}

fn acquire<A: Adapter>(adapter: &mut A, shared: &Shared, ticket: &Ticket) -> Acquired {
    if !shared.valid(ticket) || adapter.sequence() != ticket.sequence {
        return Acquired::Stale;
    }
    if !adapter.open() {
        return Acquired::Busy;
    }
    let session = Session(adapter);
    let adapter = &mut *session.0;
    if !shared.valid(ticket) || adapter.sequence() != ticket.sequence {
        return Acquired::Stale;
    }
    // Presence alone excludes. Never fetch the exclusion marker or any payload.
    if adapter.present(Format::Exclude) {
        return Acquired::Skipped("Clipboard privacy exclusion");
    }
    match flag(adapter, Format::History) {
        Ok(Some(false)) | Err(_) => return Acquired::Skipped("Clipboard privacy exclusion"),
        _ => {}
    }
    let no_cloud = match flag(adapter, Format::Cloud) {
        Ok(value) => value == Some(false),
        Err(_) => return Acquired::Skipped("Malformed clipboard privacy flag"),
    };
    let owner = adapter.owner();
    if excluded(owner.executable.as_deref(), &ticket.policy.excluded_sources) {
        return Acquired::Skipped("Clipboard source excluded");
    }
    if owner.own_process && adapter.present(Format::Replay) {
        if let Ok(token) = adapter.read(Format::Replay, 16) {
            if shared.replay.matches(&token, ticket.sequence) {
                return Acquired::Skipped("Clipboard replay suppressed");
            }
        }
    }
    if !shared.valid(ticket) {
        return Acquired::Stale;
    }
    // Read only representations of the chosen primary object, avoiding redundant
    // bitmap copies and unrelated fallback text. One raw snapshot is in flight.
    let formats = if adapter.present(Format::Payload(RawFormat::Files)) {
        vec![(RawFormat::Files, MAX_PATH_BYTES * 2 + 22)]
    } else if [RawFormat::Png, RawFormat::DibV5, RawFormat::Dib]
        .iter()
        .any(|format| adapter.present(Format::Payload(*format)))
    {
        vec![
            (RawFormat::Png, MAX_PNG_BYTES),
            (RawFormat::DibV5, MAX_RAW_BYTES),
            (RawFormat::Dib, MAX_RAW_BYTES),
        ]
    } else if adapter.present(Format::Payload(RawFormat::Unicode)) {
        vec![
            // UTF-16 may use two bytes per ASCII byte, plus the terminator;
            // the normalizer separately enforces the 1 MiB UTF-8 text limit.
            (RawFormat::Unicode, MAX_TEXT_BYTES * 2 + 2),
            (RawFormat::Html, MAX_RICH_BYTES),
            (RawFormat::Rtf, MAX_RICH_BYTES),
        ]
    } else {
        return Acquired::Skipped("Unsupported clipboard format");
    };
    let mut total = 0usize;
    let mut parts = Vec::with_capacity(formats.len());
    for (format, limit) in formats {
        if !shared.valid(ticket) {
            return Acquired::Stale;
        }
        if !adapter.present(Format::Payload(format)) {
            continue;
        }
        let image_alternative =
            matches!(format, RawFormat::Png | RawFormat::DibV5 | RawFormat::Dib);
        let bytes = match adapter.read(Format::Payload(format), limit.min(MAX_RAW_BYTES - total)) {
            Ok(bytes) if bytes.len() <= limit && bytes.len() <= MAX_RAW_BYTES - total => bytes,
            // An unusable preferred encoding must not hide a valid compatible
            // image alternative. Still never exceed the aggregate raw budget.
            Err(_) if image_alternative => continue,
            // Optional malformed rich content must not discard valid Unicode.
            // Allocation/aggregate limit errors still reject the entire item.
            Err("Malformed clipboard allocation")
                if matches!(format, RawFormat::Html | RawFormat::Rtf) =>
            {
                continue
            }
            Ok(_) => return Acquired::Skipped("Clipboard item exceeds limits"),
            Err(reason) => return Acquired::Skipped(reason),
        };
        total += bytes.len();
        parts.push(RawPart { format, bytes });
    }
    if parts.is_empty() {
        return Acquired::Skipped("Invalid or oversized clipboard data");
    }
    if !shared.valid(ticket) || adapter.sequence() != ticket.sequence {
        return Acquired::Stale;
    }
    Acquired::Snapshot(RawSnapshot {
        sequence: ticket.sequence,
        generation: ticket.generation,
        at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64,
        source: owner.executable,
        no_cloud,
        parts,
    })
    // Session closes here, before the caller can normalize or notify.
}

struct Engine<A: Adapter> {
    adapter: A,
    shared: Arc<Shared>,
    pending: Option<(Ticket, u8)>,
    seen: Option<u32>,
}

impl<A: Adapter> Engine<A> {
    fn new(adapter: A, shared: Arc<Shared>) -> Self {
        Self {
            adapter,
            shared,
            pending: None,
            seen: None,
        }
    }

    fn update(&mut self) {
        let sequence = self.adapter.sequence();
        let Some(ticket) = self.shared.ticket(sequence) else {
            self.pending = None;
            return;
        };
        if self.seen == Some(sequence) {
            return;
        }
        if let Some((old, _)) = self.pending.take() {
            if old.sequence != sequence && self.shared.valid(&old) {
                self.shared.publish(
                    &ticket,
                    MonitorEvent::Skipped {
                        reason: "Clipboard updates coalesced".into(),
                        count: 1,
                    },
                );
            }
        }
        self.seen = Some(sequence);
        self.pending = Some((ticket, 0));
        self.attempt();
    }

    fn attempt(&mut self) {
        let Some((ticket, attempt)) = self.pending.take() else {
            return;
        };
        match acquire(&mut self.adapter, &self.shared, &ticket) {
            Acquired::Busy if attempt + 1 < MAX_ATTEMPTS => {
                self.pending = Some((ticket, attempt + 1))
            }
            Acquired::Busy => self.shared.publish(
                &ticket,
                MonitorEvent::Skipped {
                    reason: "Clipboard remained busy".into(),
                    count: 1,
                },
            ),
            Acquired::Stale => {}
            Acquired::Skipped(reason) => self.shared.publish(
                &ticket,
                MonitorEvent::Skipped {
                    reason: reason.into(),
                    count: 1,
                },
            ),
            Acquired::Snapshot(snapshot) => {
                if !self.shared.valid(&ticket) {
                    return;
                }
                // Native memory and open session have ended. Errors are replaced
                // with a fixed category even if a future decoder includes content.
                let result = crate::formats::normalize(&snapshot);
                // Release raw buffers before retaining the normalized result in
                // the queue. There is never a second raw snapshot in flight.
                drop(snapshot.parts);
                match result {
                    Ok(payload) => self.shared.publish(
                        &ticket,
                        MonitorEvent::Captured(Captured {
                            sequence: snapshot.sequence,
                            generation: snapshot.generation,
                            at_ms: snapshot.at_ms,
                            source: snapshot.source,
                            payload: Arc::new(payload),
                        }),
                    ),
                    Err(_) => self.shared.publish(
                        &ticket,
                        MonitorEvent::Skipped {
                            reason: "Invalid or unsupported clipboard data".into(),
                            count: 1,
                        },
                    ),
                }
            }
        }
    }

    fn reset(&mut self) {
        self.pending = None;
        self.seen = None;
    }
}

pub(crate) fn sequence() -> u32 {
    // SAFETY: No pointers or payload reads; queries current window station only.
    unsafe { GetClipboardSequenceNumber() }
}

pub(crate) fn post_control(hwnd: isize) {
    // SAFETY: Called under Shared's endpoint lock, cleared before HWND destruction.
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), CONTROL, WPARAM(0), LPARAM(0));
    }
}

struct WindowsAdapter {
    hwnd: HWND,
    registered: [u32; 7],
}

fn copy_size(format: Format, allocation: usize, limit: usize) -> Result<usize, &'static str> {
    // GlobalSize may exceed the requested allocation (Microsoft GlobalSize).
    // DWORD/token formats have a fixed logical length. Never read allocator
    // padding as part of their values; still cap the allocation conservatively.
    let fixed = match format {
        Format::History | Format::Cloud => Some(4),
        Format::Replay => Some(16),
        _ => None,
    };
    if let Some(fixed) = fixed {
        if allocation < fixed || allocation > 64 || fixed > limit {
            return Err("Malformed clipboard metadata");
        }
        Ok(fixed)
    } else if allocation == 0
        || allocation
            > limit
                .saturating_add(MAX_ALLOCATION_PADDING)
                .min(MAX_RAW_BYTES)
    {
        Err("Clipboard item exceeds limits")
    } else {
        Ok(allocation)
    }
}

fn logical_end(format: RawFormat, bytes: &[u8]) -> Option<usize> {
    match format {
        RawFormat::Unicode => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .position(|b| *b == [0, 0])
            .and_then(|index| index.checked_mul(2)?.checked_add(2)),
        RawFormat::Files => {
            if bytes.len() < 20 {
                return None;
            }
            let offset = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
            let wide = u32::from_le_bytes(bytes.get(16..20)?.try_into().ok()?) != 0;
            if offset < 20 || offset >= bytes.len() || (wide && !offset.is_multiple_of(2)) {
                return None;
            }
            if wide {
                bytes
                    .get(offset..)?
                    .windows(4)
                    .step_by(2)
                    .position(|w| w == [0, 0, 0, 0])
                    .and_then(|index| offset.checked_add(index.checked_mul(2)?)?.checked_add(4))
            } else {
                bytes
                    .get(offset..)?
                    .windows(2)
                    .position(|b| b == [0, 0])
                    .and_then(|index| offset.checked_add(index)?.checked_add(2))
            }
        }
        RawFormat::Png => {
            if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                return None;
            }
            let mut cursor = 8usize;
            loop {
                let header = bytes.get(cursor..cursor.checked_add(8)?)?;
                let length = u32::from_be_bytes(header[..4].try_into().ok()?) as usize;
                let end = cursor.checked_add(12)?.checked_add(length)?;
                if end > bytes.len() {
                    return None;
                }
                if &header[4..8] == b"IEND" {
                    return (length == 0).then_some(end);
                }
                cursor = end;
            }
        }
        RawFormat::Html => crate::formats::html_logical_end(bytes),
        RawFormat::Rtf => crate::formats::rtf_logical_end(bytes),
        // DIB layout validation belongs to the image codec.
        RawFormat::DibV5 | RawFormat::Dib => Some(bytes.len()),
    }
}

fn finish_bytes(format: Format, mut bytes: Vec<u8>, limit: usize) -> Result<Vec<u8>, &'static str> {
    if let Format::Payload(format) = format {
        let end = logical_end(format, &bytes).ok_or(if bytes.len() > limit {
            "Clipboard item exceeds limits"
        } else {
            "Malformed clipboard allocation"
        })?;
        if end > limit {
            return Err("Clipboard item exceeds limits");
        }
        bytes.truncate(end);
    }
    Ok(bytes)
}

impl WindowsAdapter {
    fn new(hwnd: HWND) -> Result<Self, String> {
        let names = [
            "ExcludeClipboardContentFromMonitorProcessing",
            "CanIncludeInClipboardHistory",
            "CanUploadToCloudClipboard",
            REPLAY_FORMAT,
            "HTML Format",
            "Rich Text Format",
            "PNG",
        ];
        let mut registered = [0; 7];
        for (id, name) in registered.iter_mut().zip(names) {
            let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            // SAFETY: NUL-terminated UTF-16 lives through this synchronous call.
            *id = unsafe { RegisterClipboardFormatW(PCWSTR(name.as_ptr())) };
            if *id == 0 {
                return Err("Could not register clipboard formats".into());
            }
        }
        Ok(Self { hwnd, registered })
    }

    fn id(&self, format: Format) -> u32 {
        match format {
            Format::Exclude => self.registered[0],
            Format::History => self.registered[1],
            Format::Cloud => self.registered[2],
            Format::Replay => self.registered[3],
            Format::Payload(RawFormat::Html) => self.registered[4],
            Format::Payload(RawFormat::Rtf) => self.registered[5],
            Format::Payload(RawFormat::Png) => self.registered[6],
            Format::Payload(RawFormat::Unicode) => 13,
            Format::Payload(RawFormat::Dib) => 8,
            Format::Payload(RawFormat::DibV5) => 17,
            Format::Payload(RawFormat::Files) => 15,
        }
    }
}

impl Adapter for WindowsAdapter {
    fn sequence(&mut self) -> u32 {
        sequence()
    }
    fn open(&mut self) -> bool {
        // SAFETY: HWND is owned by this worker and remains live for the adapter.
        unsafe { OpenClipboard(Some(self.hwnd)).is_ok() }
    }
    fn close(&mut self) {
        // SAFETY: Session invokes exactly once after a successful OpenClipboard.
        unsafe {
            let _ = CloseClipboard();
        }
    }
    fn present(&mut self, format: Format) -> bool {
        // SAFETY: Known standard/registered ID; does not acquire payload memory.
        unsafe { IsClipboardFormatAvailable(self.id(format)).is_ok() }
    }
    fn read(&mut self, format: Format, limit: usize) -> Result<Vec<u8>, &'static str> {
        // SAFETY: Called only while Session holds the clipboard open; these known
        // formats all use HGLOBAL memory, never bitmap/COM/private object handles.
        let handle = unsafe { GetClipboardData(self.id(format)) }
            .map_err(|_| "Clipboard data unavailable")?;
        let global = HGLOBAL(handle.0);
        // SAFETY: The clipboard owns a valid HGLOBAL for these formats; never free it.
        let size = copy_size(format, unsafe { GlobalSize(global) }, limit)?;
        // Allocate BEFORE locking memory, so no allocation failure can leave it locked.
        let mut bytes = vec![0u8; size];
        // SAFETY: The clipboard remains open and the HGLOBAL is not transferred.
        let pointer = unsafe { GlobalLock(global) };
        if pointer.is_null() {
            return Err("Clipboard memory unavailable");
        }
        // SAFETY: GlobalSize bounds the readable locked allocation, and bytes is
        // separately owned, initialized memory with precisely size bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(pointer.cast::<u8>(), bytes.as_mut_ptr(), size);
        }
        // SAFETY: Balances the one successful lock before returning owned bytes.
        unsafe {
            let _ = GlobalUnlock(global);
        }
        finish_bytes(format, bytes, limit)
    }
    fn owner(&mut self) -> Owner {
        // SAFETY: Only queries ownership/PID; never focus, window titles or file data.
        let owner = unsafe { GetClipboardOwner() }.ok();
        let Some(hwnd) = owner else {
            return Owner::default();
        };
        let mut pid = 0u32;
        // SAFETY: PID output is writable; querying a stale HWND safely returns zero.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        // SAFETY: This process ID has no pointer or ownership preconditions.
        let own_process = pid != 0 && pid == unsafe { GetCurrentProcessId() };
        if pid == 0 {
            return Owner::default();
        }
        // SAFETY: Limited query rights only, no inheritance; failure remains unknown.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) };
        let Ok(process) = process else {
            return Owner {
                own_process,
                executable: None,
            };
        };
        let mut path = vec![0u16; 32_768];
        let mut length = path.len() as u32;
        // SAFETY: Owned buffer and length pointer remain writable through the call.
        let result = unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(path.as_mut_ptr()),
                &mut length,
            )
        };
        // SAFETY: Exactly this successful OpenProcess owns this non-pseudo handle.
        unsafe {
            let _ = CloseHandle(process);
        }
        let executable = if result.is_ok() && (length as usize) <= path.len() {
            String::from_utf16(&path[..length as usize]).ok()
        } else {
            None
        };
        Owner {
            own_process,
            executable,
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Never acquire/normalize in a callback: GetClipboardData can pump sent
    // messages. There is no mutable Rust state attached to/reentered by HWND.
    if message == WM_CLOSE {
        return LRESULT(0);
    }
    // SAFETY: Win32 supplied callback arguments; unhandled messages go to default.
    unsafe { DefWindowProcW(hwnd, message, wp, lp) }
}

struct Window {
    hwnd: HWND,
    shared: Arc<Shared>,
    listening: bool,
}
impl Drop for Window {
    fn drop(&mut self) {
        self.shared.detach();
        // SAFETY: Worker owns this HWND and destroys it exactly once on its own
        // thread, after removing the published endpoint and listener registration.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), RETRY_TIMER);
            if self.listening {
                let _ = RemoveClipboardFormatListener(self.hwnd);
            }
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn create_window(shared: Arc<Shared>) -> Result<Window, String> {
    static CLASS: OnceLock<Result<(), String>> = OnceLock::new();
    // SAFETY: Null name queries the process executable's stable module handle.
    let instance = unsafe { GetModuleHandleW(None) }.map_err(|_| "Clipboard module unavailable")?;
    CLASS
        .get_or_init(|| {
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: w!("DaijobuClipboardMonitorV1"),
                ..Default::default()
            };
            // SAFETY: Class fields point at static code/name; registered for process life.
            if unsafe { RegisterClassW(&class) } == 0 {
                Err("Could not register clipboard window".into())
            } else {
                Ok(())
            }
        })
        .clone()?;
    // SAFETY: Registered class, static strings, message-only parent, no userdata.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("DaijobuClipboardMonitorV1"),
            w!(""),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
    }
    .map_err(|_| "Could not create clipboard message window")?;
    let mut window = Window {
        hwnd,
        shared,
        listening: false,
    };
    if !window.shared.attach(hwnd.0 as isize) {
        return Err("Clipboard monitor stopped".into());
    }
    // SAFETY: Message-only window is valid and belongs to this worker.
    unsafe { AddClipboardFormatListener(hwnd) }
        .map_err(|_| "Could not register clipboard listener")?;
    window.listening = true;
    Ok(window)
}

pub(crate) fn run(shared: Arc<Shared>, ready: SyncSender<Result<(), String>>) {
    let window = match create_window(shared.clone()) {
        Ok(window) => window,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let adapter = match WindowsAdapter::new(window.hwnd) {
        Ok(adapter) => adapter,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let mut engine = Engine::new(adapter, shared.clone());
    let _ = ready.send(Ok(()));
    while shared.live() {
        let mut message = MSG::default();
        // SAFETY: Worker owns message queue and writable MSG; blocks while idle.
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
        if result <= 0 {
            if shared.live() {
                shared.worker_error("Clipboard listener message loop failed");
            }
            break;
        }
        if !shared.live() {
            break;
        }
        if message.hwnd == window.hwnd {
            match message.message {
                WM_CLIPBOARDUPDATE => engine.update(),
                WM_TIMER if message.wParam.0 == RETRY_TIMER => engine.attempt(),
                CONTROL => engine.reset(),
                _ => {
                    // SAFETY: MSG came from this thread's GetMessageW.
                    unsafe {
                        DispatchMessageW(&message);
                    }
                }
            }
            // Only transient contention schedules a finite retry timer.
            // SAFETY: Live worker-owned HWND; no callback pointer or UI work.
            unsafe {
                let _ = KillTimer(Some(window.hwnd), RETRY_TIMER);
                if engine.pending.is_some()
                    && SetTimer(Some(window.hwnd), RETRY_TIMER, RETRY_MS, None) == 0
                {
                    if let Some((ticket, _)) = engine.pending.take() {
                        shared.publish(
                            &ticket,
                            MonitorEvent::Error("Clipboard retry scheduling failed".into()),
                        );
                    }
                }
            }
        } else {
            // SAFETY: Dispatch only valid messages returned by this thread's loop.
            unsafe {
                DispatchMessageW(&message);
            }
        }
    }
    // Window drops and detaches BEFORE its HWND is destroyed. Slow acquisition
    // may delay this, but shutdown has already closed publication/wake gates.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::tests::{enabled, shared};
    use std::sync::atomic::Ordering;

    fn dib_fixture() -> Vec<u8> {
        let mut bytes = vec![0u8; 44];
        bytes[0..4].copy_from_slice(&40u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&1i32.to_le_bytes());
        bytes[8..12].copy_from_slice(&1i32.to_le_bytes());
        bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
        bytes[14..16].copy_from_slice(&32u16.to_le_bytes());
        bytes[40..44].copy_from_slice(&[3, 2, 1, 255]);
        bytes
    }

    fn png_fixture() -> Vec<u8> {
        crate::image::decode_dib(&dib_fixture()).unwrap().png
    }

    fn files_fixture(wide: bool) -> Vec<u8> {
        let mut bytes = vec![0u8; 20];
        bytes[..4].copy_from_slice(&20u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&u32::from(wide).to_le_bytes());
        if wide {
            bytes.extend(
                "C:\\synthetic.txt"
                    .encode_utf16()
                    .chain([0, 0])
                    .flat_map(u16::to_le_bytes),
            );
        } else {
            bytes.extend_from_slice(b"C:\\synthetic.txt\0\0");
        }
        bytes
    }

    struct Fake {
        sequence: u32,
        parts: Vec<(Format, Vec<u8>)>,
        reads: Vec<Format>,
        opened: bool,
        closes: usize,
        opens: usize,
        busy: usize,
        own: bool,
        source: Option<String>,
        change_on_read: bool,
        invalidate: Option<crate::Generation>,
        blocked: Option<(SyncSender<()>, std::sync::mpsc::Receiver<()>)>,
    }

    impl Fake {
        fn text() -> Self {
            Self {
                sequence: 2,
                parts: vec![(
                    Format::Payload(RawFormat::Unicode),
                    "synthetic\0"
                        .encode_utf16()
                        .flat_map(u16::to_le_bytes)
                        .collect(),
                )],
                reads: Vec::new(),
                opened: false,
                closes: 0,
                opens: 0,
                busy: 0,
                own: false,
                source: None,
                change_on_read: false,
                invalidate: None,
                blocked: None,
            }
        }

        fn with(mut self, format: Format, bytes: Vec<u8>) -> Self {
            self.parts.push((format, bytes));
            self
        }
    }

    impl Adapter for Fake {
        fn sequence(&mut self) -> u32 {
            self.sequence
        }
        fn open(&mut self) -> bool {
            self.opens += 1;
            if self.busy > 0 {
                self.busy -= 1;
                return false;
            }
            assert!(!self.opened);
            self.opened = true;
            true
        }
        fn close(&mut self) {
            assert!(self.opened);
            self.opened = false;
            self.closes += 1;
        }
        fn present(&mut self, format: Format) -> bool {
            assert!(self.opened);
            self.parts.iter().any(|(f, _)| *f == format)
        }
        fn read(&mut self, format: Format, limit: usize) -> Result<Vec<u8>, &'static str> {
            assert!(self.opened);
            self.reads.push(format);
            if matches!(format, Format::Payload(_)) {
                if self.change_on_read {
                    self.sequence += 1;
                }
                if let Some(generation) = self.invalidate.take() {
                    generation.fetch_add(1, Ordering::AcqRel);
                }
                if let Some((entered, resume)) = self.blocked.take() {
                    entered.send(()).unwrap();
                    resume
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                }
            }
            let bytes = &self.parts.iter().find(|(f, _)| *f == format).unwrap().1;
            if matches!(format, Format::Payload(_)) {
                copy_size(format, bytes.len(), limit)?;
                finish_bytes(format, bytes.clone(), limit)
            } else if bytes.len() > limit {
                Err("Clipboard item exceeds limits")
            } else {
                Ok(bytes.clone())
            }
        }
        fn owner(&mut self) -> Owner {
            assert!(self.opened);
            Owner {
                own_process: self.own,
                executable: self.source.clone(),
            }
        }
    }

    #[test]
    fn privacy_presence_excludes_without_any_data_read() {
        let shared = shared();
        enabled(&shared, 1);
        let mut fake = Fake::text().with(Format::Exclude, vec![]);
        assert!(matches!(
            acquire(&mut fake, &shared, &shared.ticket(2).unwrap()),
            Acquired::Skipped(_)
        ));
        assert!(fake.reads.is_empty());
        assert_eq!(fake.closes, 1);
        assert!(!fake.opened);
    }

    #[test]
    fn zero_and_malformed_explicit_history_flags_never_read_payload() {
        for bytes in [
            0u32.to_le_bytes().to_vec(),
            vec![],
            vec![1, 0],
            2u32.to_le_bytes().to_vec(),
            vec![1; 8],
        ] {
            let shared = shared();
            enabled(&shared, 1);
            let mut fake = Fake::text().with(Format::History, bytes);
            assert!(matches!(
                acquire(&mut fake, &shared, &shared.ticket(2).unwrap()),
                Acquired::Skipped(_)
            ));
            assert_eq!(fake.reads, [Format::History]);
            assert_eq!(fake.closes, 1);
        }
    }

    #[test]
    fn flags_precede_payload_and_no_cloud_is_retained_locally() {
        let shared = shared();
        enabled(&shared, 1);
        let mut fake = Fake::text()
            .with(Format::History, 1u32.to_le_bytes().to_vec())
            .with(Format::Cloud, 0u32.to_le_bytes().to_vec());
        let Acquired::Snapshot(raw) = acquire(&mut fake, &shared, &shared.ticket(2).unwrap())
        else {
            panic!("expected owned snapshot")
        };
        assert!(raw.no_cloud);
        assert_eq!(
            fake.reads,
            [
                Format::History,
                Format::Cloud,
                Format::Payload(RawFormat::Unicode)
            ]
        );
        assert!(!fake.opened);
        assert!(crate::formats::normalize(&raw).unwrap().no_cloud);
    }

    #[test]
    fn malformed_cloud_flag_is_conservative() {
        let shared = shared();
        enabled(&shared, 1);
        let mut fake = Fake::text().with(Format::Cloud, vec![0]);
        assert!(matches!(
            acquire(&mut fake, &shared, &shared.ticket(2).unwrap()),
            Acquired::Skipped(_)
        ));
        assert_eq!(fake.reads, [Format::Cloud]);
    }

    #[test]
    fn owner_exclusion_matches_basename_or_full_path_without_focus() {
        let shared = shared();
        enabled(&shared, 1);
        let mut ticket = shared.ticket(2).unwrap();
        ticket.policy.excluded_sources = vec!["PRIVATE.EXE".into()];
        let mut fake = Fake::text();
        fake.source = Some("C:\\Synthetic\\private.exe".into());
        assert!(matches!(
            acquire(&mut fake, &shared, &ticket),
            Acquired::Skipped(_)
        ));
        assert!(fake.reads.is_empty());
        assert!(excluded(
            Some("C:\\Synthetic\\private.exe"),
            &["c:/synthetic/private.exe".into()]
        ));
        assert!(!excluded(
            Some("C:\\Synthetic\\notprivate.exe"),
            &["private.exe".into()]
        ));
        assert!(!excluded(None, &["private.exe".into()]));
    }

    #[test]
    fn replay_suppression_requires_own_process_exact_token_and_sequence() {
        let shared = shared();
        enabled(&shared, 1);
        let token = [42u8; 16];
        shared.replay.reserve(token);
        // CloseClipboard race: a reserved operation may match before complete.
        let mut own = Fake::text().with(Format::Replay, token.to_vec());
        own.own = true;
        assert!(matches!(
            acquire(&mut own, &shared, &shared.ticket(2).unwrap()),
            Acquired::Skipped(_)
        ));
        assert_eq!(own.reads, [Format::Replay]);
        shared.replay.complete(token, 2);
        let mut later = Fake::text().with(Format::Replay, token.to_vec());
        later.own = true;
        later.sequence = 3;
        assert!(matches!(
            acquire(&mut later, &shared, &shared.ticket(3).unwrap()),
            Acquired::Snapshot(_)
        ));
        let mut foreign = Fake::text().with(Format::Replay, token.to_vec());
        assert!(matches!(
            acquire(&mut foreign, &shared, &shared.ticket(2).unwrap()),
            Acquired::Snapshot(_)
        ));
        assert!(!foreign.reads.contains(&Format::Replay));
        let mut wrong = Fake::text().with(Format::Replay, vec![43; 16]);
        wrong.own = true;
        assert!(matches!(
            acquire(&mut wrong, &shared, &shared.ticket(2).unwrap()),
            Acquired::Snapshot(_)
        ));
        let mut snap_or_terminal = Fake::text();
        snap_or_terminal.own = true;
        assert!(matches!(
            acquire(&mut snap_or_terminal, &shared, &shared.ticket(2).unwrap()),
            Acquired::Snapshot(_)
        ));
    }

    #[test]
    fn sequence_and_generation_races_discard_snapshot_and_close() {
        for change_sequence in [true, false] {
            let shared = shared();
            enabled(&shared, 1);
            let ticket = shared.ticket(2).unwrap();
            let mut fake = Fake::text();
            fake.change_on_read = change_sequence;
            if !change_sequence {
                fake.invalidate = Some(shared.generation.clone());
            }
            assert!(matches!(
                acquire(&mut fake, &shared, &ticket),
                Acquired::Stale
            ));
            assert_eq!(fake.closes, 1);
            assert!(!fake.opened);
        }
    }

    #[test]
    fn oversized_payload_fails_without_partial_history_and_always_closes() {
        let shared = shared();
        enabled(&shared, 1);
        let mut fake = Fake::text().with(
            Format::Payload(RawFormat::Html),
            vec![0; MAX_RICH_BYTES + 1],
        );
        assert!(matches!(
            acquire(&mut fake, &shared, &shared.ticket(2).unwrap()),
            Acquired::Skipped(_)
        ));
        assert_eq!(fake.closes, 1);
        assert!(!fake.opened);
    }

    #[test]
    fn primary_precedence_avoids_unrelated_payload_reads() {
        let shared = shared();
        enabled(&shared, 1);
        let mut fake = Fake::text()
            .with(Format::Payload(RawFormat::Png), png_fixture())
            .with(Format::Payload(RawFormat::DibV5), dib_fixture())
            .with(Format::Payload(RawFormat::Files), files_fixture(true));
        let Acquired::Snapshot(raw) = acquire(&mut fake, &shared, &shared.ticket(2).unwrap())
        else {
            panic!("expected owned snapshot")
        };
        assert_eq!(fake.reads, [Format::Payload(RawFormat::Files)]);
        assert_eq!(raw.parts.len(), 1);
        fake.parts
            .retain(|(f, _)| *f != Format::Payload(RawFormat::Files));
        fake.reads.clear();
        assert!(matches!(
            acquire(&mut fake, &shared, &shared.ticket(2).unwrap()),
            Acquired::Snapshot(_)
        ));
        assert_eq!(
            fake.reads,
            [
                Format::Payload(RawFormat::Png),
                Format::Payload(RawFormat::DibV5)
            ]
        );
    }

    #[test]
    fn busy_retries_stop_at_finite_limit_and_do_not_duplicate_sequence() {
        let shared = shared();
        enabled(&shared, 1);
        let mut fake = Fake::text();
        fake.busy = 100;
        let mut engine = Engine::new(fake, shared.clone());
        engine.update();
        for _ in 0..10 {
            engine.attempt();
        }
        assert_eq!(engine.adapter.opens, MAX_ATTEMPTS as usize);
        assert!(engine.pending.is_none());
        assert!(matches!(
            shared.drain().as_slice(),
            [MonitorEvent::Skipped { count: 1, .. }]
        ));
        engine.update();
        assert_eq!(engine.adapter.opens, MAX_ATTEMPTS as usize);
    }

    #[test]
    fn busy_pending_update_coalesces_to_latest_and_normalizes_after_close() {
        let shared = shared();
        enabled(&shared, 1);
        let mut fake = Fake::text();
        fake.busy = 1;
        let mut engine = Engine::new(fake, shared.clone());
        engine.update();
        engine.adapter.sequence = 3;
        engine.update();
        assert!(!engine.adapter.opened);
        assert!(engine.pending.is_none());
        let events = shared.drain();
        assert!(events
            .iter()
            .any(|e| matches!(e, MonitorEvent::Skipped { count: 1, .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, MonitorEvent::Captured(c) if c.sequence == 3)));
        engine.update();
        assert!(shared.drain().is_empty());
    }

    #[test]
    fn clear_generation_does_not_recapture_the_old_sequence() {
        let shared = shared();
        enabled(&shared, 1);
        let mut engine = Engine::new(Fake::text(), shared.clone());
        engine.update();
        assert!(matches!(
            shared.drain().as_slice(),
            [MonitorEvent::Captured(_)]
        ));
        shared.generation.fetch_add(1, Ordering::AcqRel);
        engine.update(); // already-queued duplicate WM_CLIPBOARDUPDATE
        assert!(shared.drain().is_empty());
        assert_eq!(engine.adapter.opens, 1);
        engine.adapter.sequence = 3;
        engine.update();
        assert!(
            matches!(shared.drain().as_slice(), [MonitorEvent::Captured(c)] if c.sequence == 3 && c.generation == 2)
        );
    }

    #[test]
    fn metadata_has_fixed_logical_sizes_despite_allocator_padding() {
        assert_eq!(copy_size(Format::History, 8, 4), Ok(4));
        assert_eq!(copy_size(Format::Cloud, 4, 4), Ok(4));
        assert_eq!(copy_size(Format::Replay, 24, 16), Ok(16));
        for allocation in [0, 1, 3, 65, usize::MAX] {
            assert!(copy_size(Format::History, allocation, 4).is_err());
        }
        assert!(copy_size(Format::Replay, 15, 16).is_err());
        assert!(copy_size(Format::Payload(RawFormat::Unicode), 73, 8).is_err());
        assert_eq!(copy_size(Format::Payload(RawFormat::Unicode), 8, 8), Ok(8));
    }

    #[test]
    fn dirty_hglobal_padding_is_trimmed_before_normalize() {
        let unicode: Vec<u8> = "synthetic\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        for (format, logical) in [
            (RawFormat::Unicode, unicode),
            (RawFormat::Files, files_fixture(true)),
            (RawFormat::Files, files_fixture(false)),
            (RawFormat::Png, png_fixture()),
        ] {
            let shared = shared();
            enabled(&shared, 1);
            let mut padded = logical.clone();
            padded.extend_from_slice(&[0xA5; 13]);
            let mut fake = Fake::text();
            fake.parts = vec![(Format::Payload(format), padded)];
            let Acquired::Snapshot(raw) = acquire(&mut fake, &shared, &shared.ticket(2).unwrap())
            else {
                panic!("expected padded allocation snapshot")
            };
            assert_eq!(raw.parts[0].bytes, logical);
            assert!(crate::formats::normalize(&raw).is_ok());
            assert_eq!(fake.closes, 1);
        }
    }

    #[test]
    fn rich_allocation_padding_preserves_payload_and_dedupe_identity() {
        let header = "Version:1.0\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
        let start = header.len();
        let fragment = "<b>synthetic\u{1f642}</b>";
        let end = start + fragment.len();
        let html = format!("Version:1.0\r\nStartHTML:{start:010}\r\nEndHTML:{end:010}\r\nStartFragment:{start:010}\r\nEndFragment:{end:010}\r\n{fragment}").into_bytes();
        let free_header = "Version:1.0\nStartHTML:-1\nEndHTML:-1\nStartFragment:0000000000\nEndFragment:0000000000\n";
        let start = free_header.len();
        let end = start + fragment.len();
        let free_html = format!("Version:1.0\nStartHTML:-1\nEndHTML:-1\nStartFragment:{start:010}\nEndFragment:{end:010}\n{fragment}").into_bytes();
        for (format, logical) in [
            (RawFormat::Html, html),
            (RawFormat::Html, free_html),
            (RawFormat::Rtf, br"{\rtf1 escaped\{brace\} \\}".to_vec()),
            (RawFormat::Rtf, b"{\\rtf1\\bin4 {\0}\\tail}".to_vec()),
        ] {
            let mut previous = None;
            for padding in [vec![0; 8], vec![0xA5, 0, b'}', 0xFF, b'{']] {
                let shared = shared();
                enabled(&shared, 1);
                let mut padded = logical.clone();
                padded.extend(padding);
                let mut fake = Fake::text().with(Format::Payload(format), padded);
                let Acquired::Snapshot(raw) =
                    acquire(&mut fake, &shared, &shared.ticket(2).unwrap())
                else {
                    panic!("expected padded rich snapshot")
                };
                assert_eq!(raw.parts[1].bytes, logical);
                let payload = crate::formats::normalize(&raw).unwrap();
                match &payload.content {
                    dj_core::clipboard_types::Content::Text { html, rtf, .. } => {
                        let rich = if format == RawFormat::Html { html } else { rtf };
                        assert_eq!(rich.as_deref(), Some(logical.as_slice()));
                    }
                    _ => panic!("expected text with a rich alternative"),
                }
                // Store deduplication serializes this exact payload and checks
                // byte equality; allocation padding must not change its input.
                if let Some(previous) = &previous {
                    assert_eq!(&payload, previous);
                }
                previous = Some(payload);
                assert_eq!(fake.closes, 1);
            }
        }
    }

    #[test]
    fn rich_allocation_limits_and_malformed_ends_fail_boundedly() {
        for format in [RawFormat::Html, RawFormat::Rtf] {
            for allocation in [MAX_RICH_BYTES + MAX_ALLOCATION_PADDING + 1, usize::MAX] {
                // Native read checks this before allocating or locking memory.
                assert!(copy_size(Format::Payload(format), allocation, MAX_RICH_BYTES).is_err());
            }
        }
        for html in [
            b"Version:1.0\nStartHTML:0\nEndHTML:99999999999999999999999\n".as_slice(),
            b"Version:1.0\nStartHTML:-1\nEndHTML:-1\nStartFragment:0\nEndFragment:0\n",
            b"Version:1.0\nStartHTML:0\nEndHTML:999999999\nStartFragment:0\nEndFragment:0\n",
        ] {
            assert!(logical_end(RawFormat::Html, html).is_none());
        }
        assert!(logical_end(RawFormat::Html, &vec![b'x'; 16 * 1024 + 1]).is_none());
        for rtf in [
            b"{\\rtf1\\bin99999999999999999999999 x}".as_slice(),
            b"{\\rtf1\\bin-1 x}",
            b"{\\rtf1\\bin9 x}",
            b"{\\rtf1 escaped\\}",
            b"{\\rtf1 bad\0}",
        ] {
            assert!(logical_end(RawFormat::Rtf, rtf).is_none());
        }
        let mut oversized = b"{\\rtf1 ".to_vec();
        oversized.resize(MAX_RICH_BYTES, b'x');
        oversized.push(b'}');
        assert!(logical_end(RawFormat::Rtf, &oversized).is_none());
    }

    #[test]
    fn malformed_optional_rich_formats_preserve_unicode() {
        let shared = shared();
        enabled(&shared, 1);
        let mut fake = Fake::text()
            .with(
                Format::Payload(RawFormat::Html),
                b"Version:1.0\nEndHTML:999999\n".to_vec(),
            )
            .with(
                Format::Payload(RawFormat::Rtf),
                br"{\rtf1 unclosed".to_vec(),
            );
        let Acquired::Snapshot(raw) = acquire(&mut fake, &shared, &shared.ticket(2).unwrap())
        else {
            panic!("valid Unicode must survive malformed optional rich formats")
        };
        assert_eq!(raw.parts.len(), 1);
        assert_eq!(raw.parts[0].format, RawFormat::Unicode);
        assert!(matches!(
            crate::formats::normalize(&raw).unwrap().content,
            dj_core::clipboard_types::Content::Text { text, html: None, rtf: None } if text == "synthetic"
        ));
        assert_eq!(fake.closes, 1);
    }

    #[test]
    fn optional_rich_allocation_limits_still_reject_the_item() {
        for format in [RawFormat::Html, RawFormat::Rtf] {
            for size in [
                MAX_RICH_BYTES + 1,
                MAX_RICH_BYTES + MAX_ALLOCATION_PADDING + 1,
            ] {
                let shared = shared();
                enabled(&shared, 1);
                let mut fake = Fake::text().with(Format::Payload(format), vec![b'x'; size]);
                assert!(matches!(
                    acquire(&mut fake, &shared, &shared.ticket(2).unwrap()),
                    Acquired::Skipped("Clipboard item exceeds limits")
                ));
                assert_eq!(fake.closes, 1);
            }
        }
    }

    #[test]
    fn invalid_png_crc_falls_back_to_valid_dib_from_same_sequence() {
        let shared = shared();
        enabled(&shared, 1);
        let mut png = png_fixture();
        png[29] ^= 1; // IHDR CRC, not the chunk envelope
        let mut fake = Fake::text()
            .with(Format::Payload(RawFormat::Png), png)
            .with(Format::Payload(RawFormat::Dib), dib_fixture());
        let Acquired::Snapshot(raw) = acquire(&mut fake, &shared, &shared.ticket(2).unwrap())
        else {
            panic!("expected image alternatives")
        };
        assert_eq!(raw.sequence, 2);
        assert_eq!(raw.parts.len(), 2);
        assert!(matches!(
            crate::formats::normalize(&raw).unwrap().content,
            dj_core::clipboard_types::Content::Image(_)
        ));
        assert_eq!(
            fake.reads,
            [
                Format::Payload(RawFormat::Png),
                Format::Payload(RawFormat::Dib)
            ]
        );
    }

    #[test]
    fn malformed_png_envelope_does_not_hide_valid_dib() {
        let shared = shared();
        enabled(&shared, 1);
        let mut fake = Fake::text()
            .with(Format::Payload(RawFormat::Png), vec![1, 2, 3])
            .with(Format::Payload(RawFormat::Dib), dib_fixture());
        let Acquired::Snapshot(raw) = acquire(&mut fake, &shared, &shared.ticket(2).unwrap())
        else {
            panic!("expected fallback image")
        };
        assert_eq!(raw.parts.len(), 1);
        assert_eq!(raw.parts[0].format, RawFormat::Dib);
        assert!(crate::formats::normalize(&raw).is_ok());
    }

    #[test]
    fn malformed_logical_end_offsets_and_chunk_lengths_fail_boundedly() {
        assert!(logical_end(RawFormat::Unicode, &[1, 0, 2, 0]).is_none());
        let mut files = files_fixture(true);
        files[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(logical_end(RawFormat::Files, &files).is_none());
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&u32::MAX.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        assert!(logical_end(RawFormat::Png, &png).is_none());
    }

    #[test]
    fn full_utf8_text_limit_is_allowed_in_utf16_raw_snapshot() {
        let shared = shared();
        enabled(&shared, 1);
        let mut fake = Fake::text();
        fake.parts[0].1 = "a"
            .repeat(MAX_TEXT_BYTES)
            .encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        let Acquired::Snapshot(raw) = acquire(&mut fake, &shared, &shared.ticket(2).unwrap())
        else {
            panic!("expected snapshot at text limit")
        };
        assert_eq!(raw.parts[0].bytes.len(), MAX_TEXT_BYTES * 2 + 2);
        assert!(crate::formats::normalize(&raw).is_ok());
        assert_eq!(fake.closes, 1);
    }

    #[test]
    fn shutdown_during_blocked_read_returns_without_join_or_late_publication() {
        let shared = shared();
        enabled(&shared, 1);
        let (entered, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (resume, resume_rx) = std::sync::mpsc::sync_channel(1);
        let mut fake = Fake::text();
        fake.blocked = Some((entered, resume_rx));
        let worker_shared = shared.clone();
        let worker = std::thread::spawn(move || {
            let mut engine = Engine::new(fake, worker_shared);
            engine.update();
            engine.adapter.closes
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        shared.stop();
        assert!(!shared.live());
        assert!(!worker.is_finished());
        resume.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), 1);
        assert!(shared.drain().is_empty());
    }
}
