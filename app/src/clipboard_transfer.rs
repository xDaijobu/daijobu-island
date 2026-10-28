//! Clipboard transfers own their payload lease, prepare off UI, and never run OLE here.

use dj_clip::{ReplayTracker, Wake};
use dj_core::clipboard_types::{Content, Payload, MAX_PATHS, MAX_PATH_BYTES, MAX_RAW_BYTES};
use dj_core::term_tabs::TabId;
use dj_win::clipboard_transfer::{self as native, Format, FormatData, PasteProgress, PasteSession};
use dj_win::drag_drop::{DragPayload, Ghost, ImageData as DragImage};
use dj_win::HWND;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    External(native::TargetIdentity),
    Terminal(TabId),
}

/// Terminal liveness must be checked against the head's exact live-tab registry.
pub fn external_target_valid(target: &Target) -> bool {
    match target {
        Target::External(identity) => native::target_is_valid(*identity),
        Target::Terminal(_) => false,
    }
}

/// Escape restoration after the panel releases keyboard ownership. No input is sent.
pub fn restore_external(target: &Target) -> bool {
    match target {
        Target::External(identity) => native::restore_target(*identity),
        Target::Terminal(_) => false,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TransferEvent {
    Copied,
    ManualPaste(String),
    TerminalPaste { tab: TabId, text: String },
    Failed(String),
}

struct Prepared {
    parts: Vec<FormatData>,
    terminal_text: Option<String>,
}

struct Completion {
    result: Result<(), String>,
    terminal_text: Option<String>,
    sequence: u32,
}
struct Job {
    receiver: mpsc::Receiver<Completion>,
    cancelled: Arc<AtomicBool>,
    paste: bool,
    target: Option<Target>,
}

type Publisher = Arc<dyn Fn(isize, &[FormatData]) -> native::Publication + Send + Sync>;

pub struct Transfer {
    owner: isize,
    replay: ReplayTracker,
    wake: Arc<Mutex<Option<Wake>>>,
    publisher: Publisher,
    job: Option<Job>,
    paste: Option<PasteSession>,
    stopped: bool,
}

impl Transfer {
    pub fn new(owner: HWND, replay: ReplayTracker, wake: Wake) -> Self {
        Self {
            owner: owner.0 as isize,
            replay,
            wake: Arc::new(Mutex::new(Some(wake))),
            publisher: Arc::new(|owner, parts| native::publish(HWND(owner as *mut _), parts)),
            job: None,
            paste: None,
            stopped: false,
        }
    }

    pub fn capture_external() -> Option<Target> {
        native::capture_external().map(Target::External)
    }

    pub fn begin(
        &mut self,
        payload: Arc<Payload>,
        plain: bool,
        paste: bool,
        target: Option<Target>,
        _now: Instant,
    ) -> Result<(), String> {
        if self.stopped {
            return Err("Clipboard transfers have stopped".into());
        }
        if self.is_pending() {
            return Err("A Clipboard transfer is already pending".into());
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let replay = self.replay.clone();
        let wake = self.wake.clone();
        let publisher = self.publisher.clone();
        let owner = self.owner;
        let token = operation_token();
        std::thread::Builder::new()
            .name("clipboard-transfer".into())
            .spawn(move || {
                let completion = (|| {
                    let prepared = prepare(&payload, plain, token)?;
                    if cancel.load(Ordering::Acquire) {
                        return Err("Clipboard transfer cancelled".into());
                    }
                    // Reserve before publication; listener notification can beat CloseClipboard.
                    replay.reserve(token);
                    let publication = publisher(owner, &prepared.parts);
                    if publication.changed && publication.sequence != 0 {
                        replay.complete(token, publication.sequence);
                    } else if !publication.changed || publication.formats_set == 0 {
                        // Never bind an unverified post-close value (or zero) to our
                        // token: a foreign copy may have won the finalization race.
                        replay.cancel(token);
                    }
                    // If finalization failed after the marker was published, keep the
                    // bounded reservation. Only our PID plus this exact operation token
                    // can latch a listener sequence; no unproven value authorizes paste.
                    publication.result?;
                    if publication.sequence == 0 {
                        return Err("Clipboard publication could not be verified; automatic paste was not sent".into());
                    }
                    Ok((prepared.terminal_text, publication.sequence))
                })();
                let completion = match completion {
                    Ok((text, sequence)) => Completion {
                        result: Ok(()),
                        terminal_text: text,
                        sequence,
                    },
                    Err(error) => Completion {
                        result: Err(error),
                        terminal_text: None,
                        sequence: 0,
                    },
                };
                // Storage permits one live Fetch lease. Release our Arc before
                // publishing completion: the UI may start its next fetch as soon
                // as it observes the channel result, even before this thread exits.
                drop(payload);
                if sender.send(completion).is_ok() && !cancel.load(Ordering::Acquire) {
                    // Shutdown removes the callback under the same lock, so no worker can
                    // post a wake after shutdown returned. It never joins a foreign owner.
                    if let Some(wake) = wake.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                        wake();
                    }
                }
            })
            .map_err(|_| "Clipboard transfer worker could not start")?;
        self.job = Some(Job {
            receiver,
            cancelled,
            paste,
            target,
        });
        Ok(())
    }

    /// On Copied, the coordinator hides Clipboard before the next external-paste tick.
    /// Drive a short timer only while is_pending() is true. No sleeps occur here.
    pub fn tick(&mut self, now: Instant) -> Vec<TransferEvent> {
        let mut events = Vec::new();
        let completion = self
            .job
            .as_ref()
            .and_then(|job| match job.receiver.try_recv() {
                Ok(done) => Some(done),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Completion {
                    result: Err("Clipboard transfer worker stopped".into()),
                    terminal_text: None,
                    sequence: 0,
                }),
            });
        if let Some(completion) = completion {
            let job = self.job.take().expect("completion has a job");
            if !job.cancelled.load(Ordering::Acquire) {
                match completion.result {
                    Err(error) => events.push(TransferEvent::Failed(error)),
                    Ok(()) => {
                        events.push(TransferEvent::Copied);
                        if job.paste {
                            match job.target {
                                Some(Target::Terminal(tab)) => match completion.terminal_text {
                                    Some(text) => {
                                        events.push(TransferEvent::TerminalPaste { tab, text })
                                    }
                                    None => events.push(TransferEvent::ManualPaste(
                                        "Copied; this item cannot be pasted into Terminal as text."
                                            .into(),
                                    )),
                                },
                                Some(Target::External(target)) => {
                                    self.paste = Some(PasteSession::new(
                                        target,
                                        completion.sequence,
                                        self.owner,
                                        now,
                                    ));
                                    // Return Copied before any focus operation, allowing the
                                    // caller to release keyboard ownership/hide the panel.
                                    return events;
                                }
                                None => events.push(TransferEvent::ManualPaste(
                                    "Copied; no paste target is available.".into(),
                                )),
                            }
                        }
                    }
                }
            }
        }
        if let Some(session) = &mut self.paste {
            match session.tick(now) {
                PasteProgress::Waiting => {}
                PasteProgress::Sent => self.paste = None,
                PasteProgress::Manual(reason) => {
                    events.push(TransferEvent::ManualPaste(reason));
                    self.paste = None;
                }
            }
        }
        events
    }

    pub fn is_pending(&self) -> bool {
        self.job.is_some() || self.paste.is_some()
    }
    pub fn cancel(&mut self) {
        if let Some(job) = &self.job {
            job.cancelled.store(true, Ordering::Release);
        }
        self.paste = None;
    }
    pub fn shutdown(&mut self) {
        self.stopped = true;
        self.cancel();
        *self.wake.lock().unwrap_or_else(|e| e.into_inner()) = None;
        // Drop the receiver; a stalled foreign clipboard owner cannot block teardown.
        self.job = None;
        native::release_target_lifetime();
    }
}

impl Drop for Transfer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Call only after the user deliberately requests paste in the search field, off UI.
pub fn read_search_text() -> Result<Option<String>, String> {
    native::read_search_text(HWND::default())
}

fn operation_token() -> [u8; 16] {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    static PREFIX: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let prefix = *PREFIX.get_or_init(|| {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        now ^ (u64::from(std::process::id()) << 32)
    });
    let mut token = [0; 16];
    token[..8].copy_from_slice(&prefix.to_le_bytes());
    token[8..].copy_from_slice(&NEXT.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    token
}

fn unicode(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn validated_paths(paths: &[String]) -> Result<Vec<PathBuf>, String> {
    if paths.is_empty() || paths.len() > MAX_PATHS {
        return Err("Invalid Clipboard file list".into());
    }
    let mut bytes = 0usize;
    let mut wide_bytes = 22usize;
    let mut result = Vec::with_capacity(paths.len());
    for value in paths {
        let path = PathBuf::from(value);
        bytes = bytes
            .checked_add(value.len() + 1)
            .ok_or("Clipboard file list exceeds limits")?;
        wide_bytes = wide_bytes
            .checked_add((value.encode_utf16().count() + 1) * 2)
            .ok_or("Clipboard file list exceeds limits")?;
        if value.contains('\0')
            || !path.is_absolute()
            || bytes > MAX_PATH_BYTES
            || wide_bytes > MAX_PATH_BYTES * 2 + 22
        {
            return Err("Invalid Clipboard file list".into());
        }
        // Explicit transfer only: capture, search and display never stat these paths.
        let metadata =
            fs::metadata(&path).map_err(|_| "A Clipboard file is missing or unavailable")?;
        if !metadata.is_file() && !metadata.is_dir() {
            return Err("Unsupported Clipboard file type".into());
        }
        result.push(path);
    }
    Ok(result)
}

fn file_block(paths: &[String]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for field in [20u32, 0, 0, 0, 1] {
        bytes.extend_from_slice(&field.to_le_bytes());
    }
    for path in paths {
        bytes.extend(unicode(path));
    }
    bytes.extend_from_slice(&[0, 0]);
    bytes
}

fn prepare(payload: &Payload, plain: bool, token: [u8; 16]) -> Result<Prepared, String> {
    payload.validate()?;
    if let Content::Files(paths) = &payload.content {
        validated_paths(paths)?;
    }
    // Terminal's existing clipboard path accepts Unicode text only; it has no
    // CF_HDROP/file quoting semantics. Never turn a file list into shell lines.
    let terminal_text = match &payload.content {
        Content::Text { text, .. } => Some(text.clone()),
        Content::Files(_) | Content::Image(_) => None,
    };
    // The replay marker and privacy DWORD precede payloads, so even a partial write
    // cannot expose a successfully published payload without its retained restriction.
    let mut parts = vec![FormatData {
        format: Format::Replay,
        bytes: token.to_vec(),
    }];
    if payload.no_cloud {
        parts.push(FormatData {
            format: Format::NoCloud,
            bytes: 0u32.to_le_bytes().to_vec(),
        });
    }
    if plain {
        let plain_text = payload.plain_text();
        parts.push(FormatData {
            format: Format::Unicode,
            bytes: unicode(
                plain_text
                    .as_deref()
                    .ok_or("This item has no plain-text representation")?,
            ),
        });
    } else {
        match &payload.content {
            Content::Text { text, html, rtf } => {
                parts.push(FormatData {
                    format: Format::Unicode,
                    bytes: unicode(text),
                });
                if let Some(bytes) = html {
                    parts.push(FormatData {
                        format: Format::Html,
                        bytes: bytes.clone(),
                    });
                }
                if let Some(bytes) = rtf {
                    parts.push(FormatData {
                        format: Format::Rtf,
                        bytes: bytes.clone(),
                    });
                }
            }
            Content::Image(image) => {
                parts.push(FormatData {
                    format: Format::Png,
                    bytes: image.png.clone(),
                });
                parts.push(FormatData {
                    format: Format::Dib,
                    bytes: dj_clip::image::encode_dib(image)?,
                });
            }
            Content::Files(paths) => {
                // Files are always copy-only, even if the original object was cut.
                parts.push(FormatData {
                    format: Format::CopyEffect,
                    bytes: 1u32.to_le_bytes().to_vec(),
                });
                parts.push(FormatData {
                    format: Format::Files,
                    bytes: file_block(paths),
                });
            }
        }
    }
    let total = parts
        .iter()
        .try_fold(0usize, |sum, part| sum.checked_add(part.bytes.len()))
        .ok_or("Clipboard publication exceeds limits")?;
    // PNG+DIB replay can have two representations of the same checked image, up to
    // 32 MiB + 64 MiB. This is one bounded in-flight job, not a raw capture snapshot.
    if total > MAX_RAW_BYTES + dj_core::clipboard_types::MAX_PNG_BYTES + 1024 {
        return Err("Clipboard publication exceeds limits".into());
    }
    Ok(Prepared {
        parts,
        terminal_text,
    })
}

/// Explicit, off-UI preparation only. The caller runs OLE outside the App borrow.
/// Exports are retained after cancellation, drop, or history deletion.
pub fn prepare_drag(payload: Arc<Payload>, export_dir: PathBuf) -> Result<DragPayload, String> {
    payload.validate()?;
    match &payload.content {
        Content::Text { .. } => Err("Text drag is not supported".into()),
        Content::Files(paths) => Ok(DragPayload {
            paths: validated_paths(paths)?,
            image: None,
            ghost: None,
        }),
        Content::Image(image) => {
            let dib = dj_clip::image::encode_dib(image)?;
            let path = export_image(&export_dir, &image.png)?;
            let thumb = &image.thumbnail;
            Ok(DragPayload {
                paths: vec![path],
                image: Some(DragImage {
                    png: Arc::from(image.png.clone()),
                    dib: Arc::from(dib),
                }),
                ghost: Some(Ghost {
                    width: thumb.width,
                    height: thumb.height,
                    bgra: Arc::from(thumb.bgra.clone()),
                    count: 1,
                }),
            })
        }
    }
}

fn export_image(directory: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let token = operation_token();
    let prefix = u64::from_le_bytes(token[..8].try_into().expect("token prefix"));
    let operation = u64::from_le_bytes(token[8..].try_into().expect("token counter"));
    export_image_with(directory, bytes, prefix, operation)
}

fn export_image_with(
    directory: &Path,
    bytes: &[u8],
    prefix: u64,
    operation: u64,
) -> Result<PathBuf, String> {
    if !directory.is_absolute() {
        return Err("Clipboard image export directory must be absolute".into());
    }
    fs::create_dir_all(directory).map_err(|_| "Clipboard image export directory is unavailable")?;
    for collision in 0..128u32 {
        let path = directory.join(format!(
            "Clipboard-{prefix:016x}-{operation:016x}-{collision}.png"
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if file.write_all(bytes).and_then(|_| file.sync_all()).is_err() {
                    drop(file);
                    // This path was created exclusively by this failed operation.
                    let _ = fs::remove_file(&path);
                    return Err("Clipboard image export failed".into());
                }
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err("Clipboard image export failed".into()),
        }
    }
    Err("Clipboard image export name allocation failed".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::clipboard_types::{ImageData, Thumbnail};
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    fn text() -> Arc<Payload> {
        Arc::new(Payload {
            content: Content::Text {
                text: "synthetic 🙂\r\nline two".into(),
                html: Some(b"synthetic HTML fixture".to_vec()),
                rtf: Some(b"{\\rtf1 fixture}".to_vec()),
            },
            no_cloud: true,
        })
    }
    fn image() -> Arc<Payload> {
        let bitmap = dj_core::snap::Bitmap {
            width: 2,
            height: 1,
            pixels: vec![1, 2, 3, 255, 4, 5, 6, 255],
        };
        let png = dj_capture::png::encode(&bitmap).unwrap();
        Arc::new(Payload {
            content: Content::Image(ImageData {
                width: 2,
                height: 1,
                bgra: bitmap.pixels.clone(),
                png,
                thumbnail: Thumbnail {
                    width: 2,
                    height: 1,
                    bgra: bitmap.pixels,
                },
            }),
            no_cloud: false,
        })
    }
    fn fake_transfer(publisher: Publisher) -> (Transfer, mpsc::Receiver<()>) {
        let (sender, receiver) = mpsc::channel();
        let mut transfer = Transfer::new(
            HWND::default(),
            ReplayTracker::default(),
            Arc::new(move || {
                let _ = sender.send(());
            }),
        );
        transfer.publisher = publisher;
        (transfer, receiver)
    }
    fn success(_: isize, _: &[FormatData]) -> native::Publication {
        native::Publication {
            result: Ok(()),
            changed: true,
            formats_set: 1,
            sequence: 42,
        }
    }
    fn finish(transfer: &mut Transfer, wakes: &mpsc::Receiver<()>) -> Vec<TransferEvent> {
        wakes.recv_timeout(Duration::from_secs(5)).unwrap();
        transfer.tick(Instant::now())
    }

    #[test]
    fn exact_rich_alternatives_and_privacy_precede_payload_and_plain_excludes_rich() {
        let payload = text();
        let prepared = prepare(&payload, false, [7; 16]).unwrap();
        assert_eq!(
            prepared.parts.iter().map(|p| p.format).collect::<Vec<_>>(),
            [
                Format::Replay,
                Format::NoCloud,
                Format::Unicode,
                Format::Html,
                Format::Rtf
            ]
        );
        assert_eq!(prepared.parts[0].bytes, [7; 16]);
        assert_eq!(prepared.parts[1].bytes, 0u32.to_le_bytes());
        let Content::Text { text, html, rtf } = &payload.content else {
            unreachable!()
        };
        assert_eq!(prepared.parts[2].bytes, unicode(text));
        assert_eq!(&prepared.parts[3].bytes, html.as_ref().unwrap());
        assert_eq!(&prepared.parts[4].bytes, rtf.as_ref().unwrap());
        assert_eq!(prepared.terminal_text.as_deref(), Some(text.as_str()));
        let prepared = prepare(&payload, true, [8; 16]).unwrap();
        assert_eq!(
            prepared.parts.iter().map(|p| p.format).collect::<Vec<_>>(),
            [Format::Replay, Format::NoCloud, Format::Unicode]
        );
        assert_eq!(prepared.parts[2].bytes, unicode(text));
    }

    #[test]
    fn image_preparation_offers_png_and_dib_and_refuses_plain() {
        let image = image();
        let prepared = prepare(&image, false, [1; 16]).unwrap();
        assert_eq!(
            prepared.parts.iter().map(|p| p.format).collect::<Vec<_>>(),
            [Format::Replay, Format::Png, Format::Dib]
        );
        let decoded = dj_clip::image::decode_dib(&prepared.parts[2].bytes).unwrap();
        let Content::Image(original) = &image.content else {
            unreachable!()
        };
        assert_eq!(decoded.bgra, original.bgra);
        assert!(prepared.terminal_text.is_none());
        assert!(prepare(&image, true, [1; 16]).is_err());
    }

    #[test]
    fn all_file_members_are_required_and_success_is_ordered_copy_only() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("synthetic first.txt");
        let second = directory.path().join("synthetic second.txt");
        fs::write(&first, b"fixture").unwrap();
        let paths = vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ];
        let payload = Arc::new(Payload {
            content: Content::Files(paths.clone()),
            no_cloud: true,
        });
        assert!(prepare(&payload, false, [1; 16]).is_err());
        assert!(prepare(&payload, true, [1; 16]).is_err());
        assert!(prepare_drag(payload.clone(), directory.path().into()).is_err());
        fs::write(&second, b"fixture").unwrap();
        let prepared = prepare(&payload, false, [1; 16]).unwrap();
        assert_eq!(
            prepared.parts.iter().map(|p| p.format).collect::<Vec<_>>(),
            [
                Format::Replay,
                Format::NoCloud,
                Format::CopyEffect,
                Format::Files
            ]
        );
        assert_eq!(prepared.parts[2].bytes, 1u32.to_le_bytes());
        let block = &prepared.parts[3].bytes;
        assert_eq!(u32::from_le_bytes(block[..4].try_into().unwrap()), 20);
        assert_eq!(&block[block.len() - 4..], &[0, 0, 0, 0]);
        assert_eq!(
            &block[20..],
            [unicode(&paths[0]), unicode(&paths[1]), vec![0, 0]].concat()
        );
        assert!(prepared.terminal_text.is_none());
        assert!(prepare(&payload, true, [1; 16])
            .unwrap()
            .terminal_text
            .is_none());
        let drag = prepare_drag(payload, directory.path().into()).unwrap();
        assert_eq!(drag.paths, [first, second]);
        assert!(drag.image.is_none());
    }

    #[test]
    fn terminal_target_emits_exact_tab_and_unchanged_text_without_global_input() {
        let (mut transfer, wakes) = fake_transfer(Arc::new(success));
        transfer
            .begin(
                text(),
                false,
                true,
                Some(Target::Terminal(887)),
                Instant::now(),
            )
            .unwrap();
        let events = finish(&mut transfer, &wakes);
        assert_eq!(
            events,
            [
                TransferEvent::Copied,
                TransferEvent::TerminalPaste {
                    tab: 887,
                    text: text().plain_text().unwrap()
                }
            ]
        );
        assert!(!transfer.is_pending());
    }

    #[test]
    fn payload_fetch_lease_is_released_before_completion_wakes_next_operation() {
        for failed in [false, true] {
            let payload = text();
            let lease = Arc::downgrade(&payload);
            let (sender, receiver) = mpsc::channel();
            let mut transfer = Transfer::new(
                HWND::default(),
                ReplayTracker::default(),
                Arc::new(move || {
                    sender.send(lease.upgrade().is_none()).unwrap();
                }),
            );
            transfer.publisher = Arc::new(move |owner, parts| {
                let mut publication = success(owner, parts);
                if failed {
                    publication.result = Err("synthetic failure".into());
                }
                publication
            });
            transfer
                .begin(payload, false, false, None, Instant::now())
                .unwrap();
            assert!(receiver.recv_timeout(Duration::from_secs(5)).unwrap());
            let events = transfer.tick(Instant::now());
            assert_eq!(events.len(), 1);
            assert_eq!(matches!(&events[0], TransferEvent::Failed(_)), failed);
        }
    }

    #[test]
    fn missing_target_and_image_only_terminal_leave_copy_with_honest_feedback() {
        for (payload, target) in [(text(), None), (image(), Some(Target::Terminal(9)))] {
            let (mut transfer, wakes) = fake_transfer(Arc::new(success));
            transfer
                .begin(payload, false, true, target, Instant::now())
                .unwrap();
            let events = finish(&mut transfer, &wakes);
            assert_eq!(events.len(), 2);
            assert_eq!(events[0], TransferEvent::Copied);
            assert!(matches!(&events[1], TransferEvent::ManualPaste(_)));
            assert!(!transfer.is_pending());
        }
    }

    #[test]
    fn terminal_file_target_copies_without_converting_paths_to_shell_lines() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("synthetic file with spaces.txt");
        fs::write(&path, b"fixture").unwrap();
        for plain in [false, true] {
            let payload = Arc::new(Payload {
                content: Content::Files(vec![path.to_string_lossy().into_owned()]),
                no_cloud: false,
            });
            let (mut transfer, wakes) = fake_transfer(Arc::new(success));
            transfer
                .begin(
                    payload,
                    plain,
                    true,
                    Some(Target::Terminal(9)),
                    Instant::now(),
                )
                .unwrap();
            assert_eq!(
                finish(&mut transfer, &wakes),
                [
                    TransferEvent::Copied,
                    TransferEvent::ManualPaste(
                        "Copied; this item cannot be pasted into Terminal as text.".into()
                    )
                ]
            );
            assert!(!transfer.is_pending());
        }
    }

    #[test]
    fn external_paste_returns_copied_before_any_native_focus_or_input_operation() {
        let (mut transfer, wakes) = fake_transfer(Arc::new(success));
        let target = Target::External(native::TargetIdentity {
            hwnd: 123,
            pid: 456,
            thread: 789,
            created: 1011,
            window_cookie: 1213,
        });
        transfer
            .begin(text(), false, true, Some(target), Instant::now())
            .unwrap();
        // finish consumes only the worker result. Deliberately do not tick this fake
        // HWND again: native identity/focus calls are reserved for desktop acceptance.
        assert_eq!(finish(&mut transfer, &wakes), [TransferEvent::Copied]);
        assert!(transfer.is_pending());
        transfer.cancel();
        assert!(!transfer.is_pending());
    }

    #[test]
    fn failed_or_partial_publication_never_copies_or_pastes() {
        for changed in [false, true] {
            let (mut transfer, wakes) = fake_transfer(Arc::new(move |_, _| native::Publication {
                result: Err("synthetic publication failure".into()),
                changed,
                formats_set: usize::from(changed),
                sequence: 42,
            }));
            transfer
                .begin(
                    text(),
                    false,
                    true,
                    Some(Target::Terminal(7)),
                    Instant::now(),
                )
                .unwrap();
            assert_eq!(
                finish(&mut transfer, &wakes),
                [TransferEvent::Failed(
                    "synthetic publication failure".into()
                )]
            );
            assert!(!transfer.is_pending());
        }
    }

    #[test]
    fn replay_is_reserved_before_publish_and_bound_to_exact_sequence_even_on_partial_failure() {
        for (changed, failed) in [(true, false), (true, true), (false, true)] {
            let replay = ReplayTracker::default();
            let inspect = replay.clone();
            let token_seen = Arc::new(Mutex::new(None));
            let saved = token_seen.clone();
            let (mut transfer, wakes) = fake_transfer(Arc::new(move |_, parts| {
                let token = parts
                    .iter()
                    .find(|p| p.format == Format::Replay)
                    .unwrap()
                    .bytes
                    .clone();
                assert!(
                    inspect.matches(&token, 42),
                    "must reserve before notification can arrive"
                );
                *saved.lock().unwrap() = Some(token);
                native::Publication {
                    result: if failed {
                        Err("partial failure".into())
                    } else {
                        Ok(())
                    },
                    changed,
                    formats_set: usize::from(changed),
                    sequence: 88,
                }
            }));
            transfer.replay = replay.clone();
            transfer
                .begin(text(), false, false, None, Instant::now())
                .unwrap();
            finish(&mut transfer, &wakes);
            let token = token_seen.lock().unwrap().take().unwrap();
            assert_eq!(replay.matches(&token, 88), changed);
            assert!(!replay.matches(&token, 42));
            assert!(!replay.matches(&token, 43));
            assert!(!replay.matches(&[0; 16], 42));
        }
    }

    #[test]
    fn unverified_publication_never_pastes_but_retains_exact_operation_suppression() {
        for (changed, formats_set) in [(false, 0), (true, 0), (true, 1)] {
            for failed in [false, true] {
                for observed_before_completion in [false, true] {
                    let token_seen = Arc::new(Mutex::new(None));
                    let saved = token_seen.clone();
                    let replay = ReplayTracker::default();
                    let inspect = replay.clone();
                    let (mut transfer, wakes) = fake_transfer(Arc::new(move |_, parts| {
                        *saved.lock().unwrap() = Some(parts[0].bytes.clone());
                        if observed_before_completion {
                            assert!(inspect.matches(&parts[0].bytes, 88));
                        }
                        native::Publication {
                            result: if failed {
                                Err("verification failed".into())
                            } else {
                                Ok(())
                            },
                            changed,
                            formats_set,
                            sequence: 0,
                        }
                    }));
                    transfer.replay = replay;
                    transfer
                        .begin(
                            text(),
                            false,
                            true,
                            Some(Target::Terminal(1)),
                            Instant::now(),
                        )
                        .unwrap();
                    let events = finish(&mut transfer, &wakes);
                    assert!(matches!(events.as_slice(), [TransferEvent::Failed(_)]));
                    assert!(!transfer.is_pending());
                    let token = token_seen.lock().unwrap().take().unwrap();
                    assert!(!transfer.replay.matches(&[0; 16], 88));
                    assert_eq!(
                        transfer.replay.matches(&token, 88),
                        changed && formats_set > 0
                    );
                    assert!(!transfer.replay.matches(&token, 89));
                }
            }
        }
    }

    #[test]
    fn invalid_payload_never_calls_publisher_or_changes_clipboard() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let (mut transfer, wakes) = fake_transfer(Arc::new(move |owner, parts| {
            counted.fetch_add(1, Ordering::SeqCst);
            success(owner, parts)
        }));
        let payload = Arc::new(Payload {
            content: Content::Text {
                text: "bad\0text".into(),
                html: None,
                rtf: None,
            },
            no_cloud: false,
        });
        transfer
            .begin(
                payload,
                false,
                true,
                Some(Target::Terminal(1)),
                Instant::now(),
            )
            .unwrap();
        assert!(matches!(
            &finish(&mut transfer, &wakes)[..],
            [TransferEvent::Failed(_)]
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cancelled_work_never_emits_paste_and_remains_bounded_until_completion() {
        let (entered, arrival) = mpsc::sync_channel(1);
        let (release, gate) = mpsc::sync_channel(1);
        let gate = Mutex::new(gate);
        let (mut transfer, _) = fake_transfer(Arc::new(move |owner, parts| {
            entered.send(()).unwrap();
            gate.lock().unwrap().recv().unwrap();
            success(owner, parts)
        }));
        transfer
            .begin(
                text(),
                false,
                true,
                Some(Target::Terminal(1)),
                Instant::now(),
            )
            .unwrap();
        arrival.recv_timeout(Duration::from_secs(5)).unwrap();
        transfer.cancel();
        assert!(transfer.is_pending());
        assert!(transfer
            .begin(text(), false, false, None, Instant::now())
            .is_err());
        release.send(()).unwrap();
        // Waiting in a test is safe: no worker sleeps or foreign native API is used.
        let deadline = Instant::now() + Duration::from_secs(5);
        while transfer.is_pending() && Instant::now() < deadline {
            assert!(transfer.tick(Instant::now()).is_empty());
            std::thread::yield_now();
        }
        assert!(!transfer.is_pending());
    }

    #[test]
    fn shutdown_never_joins_blocked_publisher_or_wakes_a_dead_controller() {
        let (entered, arrival) = mpsc::sync_channel(1);
        let (release, gate) = mpsc::sync_channel(1);
        let (exited, done) = mpsc::sync_channel(1);
        let gate = Mutex::new(gate);
        let (mut transfer, wakes) = fake_transfer(Arc::new(move |owner, parts| {
            entered.send(()).unwrap();
            gate.lock().unwrap().recv().unwrap();
            let result = success(owner, parts);
            exited.send(()).unwrap();
            result
        }));
        transfer
            .begin(
                text(),
                false,
                true,
                Some(Target::Terminal(1)),
                Instant::now(),
            )
            .unwrap();
        arrival.recv_timeout(Duration::from_secs(5)).unwrap();
        transfer.shutdown();
        assert!(!transfer.is_pending());
        assert!(transfer
            .begin(text(), false, false, None, Instant::now())
            .is_err());
        release.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(wakes.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn image_drag_exports_distinct_retained_pngs_and_never_overwrites_existing_files() {
        let directory = tempfile::tempdir().unwrap();
        let sentinel = directory.path().join("Clipboard.png");
        fs::write(&sentinel, b"keep existing").unwrap();
        let payload = image();
        let first = prepare_drag(payload.clone(), directory.path().into()).unwrap();
        let second = prepare_drag(payload.clone(), directory.path().into()).unwrap();
        assert_ne!(first.paths, second.paths);
        let Content::Image(image) = &payload.content else {
            unreachable!()
        };
        for drag in [&first, &second] {
            assert_eq!(fs::read(&drag.paths[0]).unwrap(), image.png);
            assert_eq!(&*drag.image.as_ref().unwrap().png, image.png);
            assert_eq!(drag.ghost.as_ref().unwrap().count, 1);
        }
        let path = first.paths[0].clone();
        drop(first);
        drop(payload);
        assert!(
            path.exists(),
            "transport/history lifetime must not remove exports"
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep existing");
        assert!(prepare_drag(text(), directory.path().into()).is_err());
    }

    #[test]
    fn export_collision_retries_without_overwriting_and_exhaustion_fails_cleanly() {
        let directory = tempfile::tempdir().unwrap();
        let existing = directory
            .path()
            .join("Clipboard-0000000000000001-0000000000000002-0.png");
        fs::write(&existing, b"keep existing").unwrap();
        let exported = export_image_with(directory.path(), b"new fixture", 1, 2).unwrap();
        assert_eq!(
            exported.file_name().unwrap(),
            "Clipboard-0000000000000001-0000000000000002-1.png"
        );
        assert_eq!(fs::read(&existing).unwrap(), b"keep existing");
        assert_eq!(fs::read(&exported).unwrap(), b"new fixture");
        for collision in 2..128 {
            fs::write(
                directory.path().join(format!(
                    "Clipboard-0000000000000001-0000000000000002-{collision}.png"
                )),
                b"keep",
            )
            .unwrap();
        }
        assert!(export_image_with(directory.path(), b"must fail", 1, 2).is_err());
        assert_eq!(fs::read(existing).unwrap(), b"keep existing");
        assert_eq!(fs::read(exported).unwrap(), b"new fixture");
    }

    #[test]
    fn invalid_export_destinations_fail_without_touching_existing_data() {
        let directory = tempfile::tempdir().unwrap();
        assert!(export_image(Path::new("relative"), b"fixture").is_err());
        let occupied = directory.path().join("not a directory");
        fs::write(&occupied, b"keep").unwrap();
        assert!(export_image(&occupied, b"fixture").is_err());
        assert_eq!(fs::read(occupied).unwrap(), b"keep");
    }
}
