//! What the Snap preview stack keeps behind its cards, and the one thread that does
//! its slow work. The stack's own state (order, countdowns, selection) is `dj_core`'s
//! and its window is `dj_win`'s; this module owns only the image bytes and the IO.
//!
//! Memory: a card holds a [`Payload`], the annotated image and (once it exists) its
//! PNG, both behind `Arc`s that are shared with the thumb's `Shot` and never copied.
//! A shot therefore costs its pixels plus its PNG once, however many owners it has, and
//! the bytes are freed when the last owner (the thumb, the card, a queued job) lets go.
//! Removing a card (dismissed, expired, evicted) only drops its `Arc`s: it never
//! touches the clipboard and never deletes a file, and nothing is written to disk
//! except by an explicit [`Store::save`] or drag preparation in `snap_drag_store`.
//!
//! Threads: everything on `Store` is called from the UI thread and returns at once. A
//! copy that has its PNG goes to the clipboard right away, as the thumb's does. What can
//! be slow (encoding a PNG, writing a file) runs on one IO thread that starts with the
//! first job and sleeps on its queue the rest of the time. The queue is bounded, so
//! when it is full a request is refused (`save` returns false) instead of blocking the
//! UI. Jobs carry `Arc` clones, so a card dismissed mid-save does not matter to the
//! worker; its reply carries only the card's id, and the caller ignores ids that are
//! gone. After queueing a reply the worker calls the `wake` closure (the app posts
//! `WM_WAKE`) and the UI collects the replies with [`Store::drain`].
//!
//! Dropping the `Store` never blocks: the thread is not joined. It finishes the job in
//! hand, skips the queue and exits.

use crate::snap::{put_image, write_new, Clipboard};
use dj_core::snap::{Bitmap, Stamp};
use dj_sweep::Ops;
use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread;

/// Jobs that may wait for the worker before `save` and `copy` say no.
const QUEUE: usize = 16;

const NO_IMAGE: &str = "no image";

/// Encodes a bitmap as PNG. The real one is `dj_capture::png::encode`; the tests plug
/// in fakes (and gates) through `Store::with_parts`.
type Encoder = Arc<dyn Fn(&Bitmap) -> Result<Vec<u8>, String> + Send + Sync>;

/// One card's image: the final annotated pixels and, once encoded, their PNG. Both are
/// shared with the thumb's shot (`Snapper::payload`).
#[derive(Clone)]
pub struct Payload {
    pub bitmap: Arc<Bitmap>,
    /// None until something encoded it (the copy that made the shot normally did).
    pub png: Option<Arc<[u8]>>,
}

impl Payload {
    /// The bytes this payload retains: pixels plus PNG. They are shared with the thumb,
    /// so the app counts them once, when it admits the card.
    pub fn bytes(&self) -> u64 {
        self.bitmap.pixels.len() as u64 + self.png.as_ref().map_or(0, |p| p.len() as u64)
    }
}

impl fmt::Debug for Payload {
    // Not derived: it would print every byte.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Payload")
            .field("width", &self.bitmap.width)
            .field("height", &self.bitmap.height)
            .field("png_bytes", &self.png.as_ref().map(|p| p.len()))
            .finish()
    }
}

/// What the worker has to say about one card. The caller looks the id up and ignores
/// it if the card is gone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub id: u64,
    pub kind: ReplyKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplyKind {
    /// The PNG is in this folder under this (new) name.
    Saved(PathBuf),
    /// Nothing was written (or the partial file was removed); why, for the log.
    SaveFailed(String),
    /// A copy was waiting for the PNG: it is now in the payload, call `finish_copy`.
    Encoded,
    EncodeFailed(String),
}

/// How [`Store::copy`] started.
#[derive(Debug, PartialEq, Eq)]
pub enum CopyStart {
    /// Finished on the spot (the clipboard now has the image) or refused (no such
    /// card, or the worker's queue is full).
    Done(Result<(), String>),
    /// The PNG had to be made first: wait for this card's `Encoded` (or
    /// `EncodeFailed`) reply, then call `finish_copy`.
    Pending,
}

/// Why a file could not be shown in Explorer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevealError {
    /// The file is gone (moved or deleted since it was saved): the card says so and
    /// lets the user save again.
    Missing,
    Other(String),
}

impl fmt::Display for RevealError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("the file is gone"),
            Self::Other(e) => f.write_str(e),
        }
    }
}

/// Work for the IO thread. Each carries the `Arc`s it needs, not an id to look up.
enum Job {
    /// Write the PNG (made first if the card has none) to a new file in `dir`.
    Save {
        id: u64,
        bitmap: Arc<Bitmap>,
        png: Option<Arc<[u8]>>,
        dir: PathBuf,
        stamp: Stamp,
    },
    /// Make the PNG for a copy.
    Encode { id: u64, bitmap: Arc<Bitmap> },
}

/// A reply plus what the store needs to apply it. Only `reply` leaves the module.
struct Done {
    reply: Reply,
    /// The image the job was about. A PNG is cached only into a payload that still
    /// holds this very image, so a stale job can never land on a different card.
    bitmap: Arc<Bitmap>,
    /// A PNG the worker made, to cache in the payload (so later copies and saves do
    /// not encode again), whether or not the job as a whole succeeded.
    png: Option<Arc<[u8]>>,
}

struct Worker {
    jobs: SyncSender<Job>,
    replies: Receiver<Done>,
    /// Set when the store goes: the thread skips what is still queued.
    closed: Arc<AtomicBool>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        // `jobs` drops after this, which ends the thread's `recv` loop.
    }
}

/// The payloads of the cards on screen, by the stack's item id, and the worker that
/// saves and encodes them.
pub struct Store {
    payloads: HashMap<u64, Payload>,
    /// Handed to the thread when it starts.
    wake: Option<Box<dyn Fn() + Send>>,
    encoder: Encoder,
    queue: usize,
    /// Started by the first job.
    worker: Option<Worker>,
}

impl Store {
    /// `wake` is called from the worker after it has queued a reply (the app passes
    /// `dj_win::window::wake(hwnd)`). No thread is started yet.
    pub fn new(wake: Box<dyn Fn() + Send>) -> Store {
        Self::with_parts(wake, Arc::new(dj_capture::png::encode), QUEUE)
    }

    fn with_parts(wake: Box<dyn Fn() + Send>, encoder: Encoder, queue: usize) -> Store {
        Store {
            payloads: HashMap::new(),
            wake: Some(wake),
            encoder,
            queue,
            worker: None,
        }
    }

    /// Takes a card's payload (a repeat of an id replaces it).
    pub fn insert(&mut self, id: u64, p: Payload) {
        self.payloads.insert(id, p);
    }

    /// The card is gone: releases its `Arc`s. Nothing else happens: no file is touched
    /// and the clipboard keeps whatever it has. A job already queued for it finishes
    /// and its reply is simply ignored.
    pub fn remove(&mut self, id: u64) {
        self.payloads.remove(&id);
    }

    pub fn get(&self, id: u64) -> Option<&Payload> {
        self.payloads.get(&id)
    }

    /// Puts the card's image on the clipboard (PNG + `CF_DIB`). With the PNG at hand
    /// that is done here and now, like the thumb's copy; without it the worker makes it
    /// and the caller finishes with [`Store::finish_copy`] when the card's `Encoded`
    /// reply arrives. Refused (`Done(Err)`) for an unknown id or a full queue.
    pub fn copy(&mut self, id: u64, clip: &mut dyn Clipboard) -> CopyStart {
        let Some(p) = self.payloads.get(&id) else {
            return CopyStart::Done(Err(NO_IMAGE.into()));
        };
        if let Some(png) = &p.png {
            return CopyStart::Done(put_image(&p.bitmap, png, clip));
        }
        let job = Job::Encode {
            id,
            bitmap: Arc::clone(&p.bitmap),
        };
        if self.submit(job) {
            CopyStart::Pending
        } else {
            CopyStart::Done(Err("busy, try again".into()))
        }
    }

    /// The second half of a pending copy, after its `Encoded` reply: puts the image on
    /// the clipboard. Fails if the card is gone or the PNG could not be made.
    pub fn finish_copy(&mut self, id: u64, clip: &mut dyn Clipboard) -> Result<(), String> {
        let p = self.payloads.get(&id).ok_or(NO_IMAGE)?;
        let png = p.png.as_deref().ok_or("the image could not be encoded")?;
        put_image(&p.bitmap, png, clip)
    }

    /// Queues a save into `dir` (created if missing) under the first free name for
    /// `stamp`; it never overwrites. The worker encodes the PNG first if the card has
    /// none. Returns false, queueing nothing, if there is no such card or the queue is
    /// full: the caller then shows the save as failed. The result arrives as a `Saved`
    /// or `SaveFailed` reply.
    pub fn save(&mut self, id: u64, dir: PathBuf, stamp: Stamp) -> bool {
        let Some(p) = self.payloads.get(&id) else {
            return false;
        };
        let job = Job::Save {
            id,
            bitmap: Arc::clone(&p.bitmap),
            png: p.png.clone(),
            dir,
            stamp,
        };
        self.submit(job)
    }

    /// The replies the worker has queued since the last call, oldest first. A PNG the
    /// worker made is cached into its payload here, before the reply is handed out.
    pub fn drain(&mut self) -> Vec<Reply> {
        let Some(w) = self.worker.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(done) = w.replies.try_recv() {
            if let (Some(png), Some(p)) = (done.png, self.payloads.get_mut(&done.reply.id)) {
                if p.png.is_none() && Arc::ptr_eq(&p.bitmap, &done.bitmap) {
                    p.png = Some(png);
                }
            }
            out.push(done.reply);
        }
        out
    }

    /// Shows a saved file in Explorer. `Missing` if it is gone, so the card can say so
    /// and allow saving again.
    pub fn reveal(path: &Path) -> Result<(), RevealError> {
        reveal_with(&dj_sweep::WinOps, path)
    }

    /// Queues a job, starting the thread first if this is the first one.
    fn submit(&mut self, job: Job) -> bool {
        if self.worker.is_none() {
            self.worker = self.spawn();
        }
        let Some(w) = self.worker.as_ref() else {
            return false;
        };
        match w.jobs.try_send(job) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        }
    }

    /// None if the thread cannot start (and then never will: `wake` is spent).
    fn spawn(&mut self) -> Option<Worker> {
        let wake = self.wake.take()?;
        let (jobs, job_rx) = mpsc::sync_channel(self.queue);
        let (out, replies) = mpsc::channel();
        let closed = Arc::new(AtomicBool::new(false));
        let encoder = Arc::clone(&self.encoder);
        let flag = Arc::clone(&closed);
        thread::Builder::new()
            .name("snap-store".into())
            .spawn(move || work(&job_rx, &out, &encoder, &flag, &*wake))
            .ok()?;
        Some(Worker {
            jobs,
            replies,
            closed,
        })
    }
}

fn reveal_with(ops: &dyn Ops, path: &Path) -> Result<(), RevealError> {
    if !path.exists() {
        return Err(RevealError::Missing);
    }
    ops.reveal(path)
        .map_err(|e| RevealError::Other(e.to_string()))
}

/// Materializes the final shared PNG for a drag, without changing normal Copy/Save.
/// Only an unchanged regular file is reused: an edited, unreadable or missing saved
/// file gets a fresh name. Neither that old path nor any completed PNG is deleted.
/// Called only on the drag IO worker; comparing large files never blocks the UI.
pub(crate) fn materialize_drag_png(
    png: &[u8],
    saved: Option<&Path>,
    dir: &Path,
    stamp: Stamp,
) -> Result<PathBuf, String> {
    if png.is_empty() {
        return Err("the image has no PNG bytes".into());
    }
    if let Some(path) = saved {
        if file_matches(path, png) {
            return std::path::absolute(path)
                .map_err(|e| format!("cannot resolve {}: {e}", path.display()));
        }
    }
    let dir =
        std::path::absolute(dir).map_err(|e| format!("cannot resolve {}: {e}", dir.display()))?;
    match saved {
        None => write_new(&dir, stamp, png),
        Some(old) => write_drag_png(&dir, stamp, png, old, |f, bytes| {
            f.write_all(bytes).and_then(|()| f.flush())
        }),
    }
}

fn file_matches(path: &Path, bytes: &[u8]) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let Ok(meta) = file.metadata() else {
        return false;
    };
    if !meta.is_file() || meta.len() != bytes.len() as u64 {
        return false;
    }
    let mut buffer = [0; 64 * 1024];
    for chunk in bytes.chunks(buffer.len()) {
        let read = &mut buffer[..chunk.len()];
        if file.read_exact(read).is_err() || read != chunk {
            return false;
        }
    }
    // Also detect a file that grew after metadata was read.
    matches!(file.read(&mut buffer[..1]), Ok(0))
}

/// The same create-new/write/flush discipline and name range as `snap::write_new`,
/// with one extra exclusion for a disappeared saved file. Skipping the basename
/// even across directories avoids Windows path-case/alias differences. The old
/// file is never reserved, truncated or removed, even when it does not exist.
fn write_drag_png(
    dir: &Path,
    stamp: Stamp,
    bytes: &[u8],
    old: &Path,
    write: impl Fn(&mut File, &[u8]) -> io::Result<()>,
) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    for n in 1..=999 {
        let name = dj_core::snap::file_name(stamp, n);
        if old
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        let path = dir.join(name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                return match write(&mut file, bytes) {
                    Ok(()) => Ok(path),
                    Err(e) => {
                        drop(file);
                        // This operation created this partial file; never remove
                        // a collision or the previously saved path.
                        let _ = fs::remove_file(&path);
                        Err(format!("cannot write {}: {e}", path.display()))
                    }
                };
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("cannot create {}: {e}", path.display())),
        }
    }
    Err(format!("no free file name in {}", dir.display()))
}

/// The IO thread: one job at a time until the store lets go of the queue.
fn work(
    jobs: &Receiver<Job>,
    out: &mpsc::Sender<Done>,
    encode: &Encoder,
    closed: &AtomicBool,
    wake: &(dyn Fn() + Send),
) {
    while let Ok(job) = jobs.recv() {
        if closed.load(Ordering::SeqCst) {
            break;
        }
        // A panic (in WIC, say) must not leave the card on "saving…" for ever.
        let (id, is_save, bitmap) = match &job {
            Job::Save { id, bitmap, .. } => (*id, true, Arc::clone(bitmap)),
            Job::Encode { id, bitmap } => (*id, false, Arc::clone(bitmap)),
        };
        let done =
            panic::catch_unwind(AssertUnwindSafe(|| run(job, encode))).unwrap_or_else(|_| {
                let why = "the worker crashed".to_string();
                let kind = if is_save {
                    ReplyKind::SaveFailed(why)
                } else {
                    ReplyKind::EncodeFailed(why)
                };
                Done {
                    reply: Reply { id, kind },
                    bitmap,
                    png: None,
                }
            });
        if out.send(done).is_err() {
            break;
        }
        wake();
    }
}

fn run(job: Job, encode: &Encoder) -> Done {
    match job {
        Job::Encode { id, bitmap } => {
            let (kind, png) = match encode(&bitmap) {
                Ok(bytes) => (ReplyKind::Encoded, Some(Arc::from(bytes))),
                Err(e) => (ReplyKind::EncodeFailed(e), None),
            };
            Done {
                reply: Reply { id, kind },
                bitmap,
                png,
            }
        }
        Job::Save {
            id,
            bitmap,
            png,
            dir,
            stamp,
        } => {
            // A PNG made here goes back to the store even if the write then fails, so
            // the retry does not encode again.
            let mut made = None;
            let written = match &png {
                Some(png) => write_new(&dir, stamp, png),
                None => encode(&bitmap).and_then(|bytes| {
                    let written = write_new(&dir, stamp, &bytes);
                    made = Some(Arc::from(bytes));
                    written
                }),
            };
            let kind = match written {
                Ok(path) => ReplyKind::Saved(path),
                Err(e) => ReplyKind::SaveFailed(e),
            };
            Done {
                reply: Reply { id, kind },
                bitmap,
                png: made,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::RecvTimeoutError;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const STAMP: Stamp = Stamp {
        year: 2026,
        month: 10,
        day: 26,
        hour: 14,
        minute: 3,
        second: 9,
    };
    const STAMP_NAME: &str = "snap-2026-10-26_140309";
    /// The fake encoder fails on bitmaps this wide.
    const BAD_WIDTH: u32 = 13;
    const WAIT: Duration = Duration::from_secs(5);

    /// Records every `put` as (png, dib); fails while `fail` is set.
    #[derive(Default)]
    struct FakeClipboard {
        puts: Vec<(Vec<u8>, Vec<u8>)>,
        fail: bool,
    }

    impl Clipboard for FakeClipboard {
        fn put(&mut self, png: &[u8], dib: &[u8]) -> Result<(), String> {
            if self.fail {
                return Err("busy".into());
            }
            self.puts.push((png.to_vec(), dib.to_vec()));
            Ok(())
        }
    }

    /// An encoder that counts its calls and writes `PNG<w>x<h>`; it fails for
    /// `BAD_WIDTH`.
    fn fake_encoder() -> (Encoder, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let n = Arc::clone(&calls);
        let encoder: Encoder = Arc::new(move |b: &Bitmap| {
            n.fetch_add(1, Ordering::SeqCst);
            if b.width == BAD_WIDTH {
                return Err("cannot encode".into());
            }
            Ok(format!("PNG{}x{}", b.width, b.height).into_bytes())
        });
        (encoder, calls)
    }

    /// An encoder that tells `entered` it has started, then waits for a message on
    /// `release` (or 5 s), so a test can hold the worker busy.
    fn gated_encoder() -> (Encoder, Receiver<()>, mpsc::Sender<()>) {
        let (entered_tx, entered) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let (entered_tx, release_rx) = (Mutex::new(entered_tx), Mutex::new(release_rx));
        let encoder: Encoder = Arc::new(move |_| {
            let _ = entered_tx.lock().unwrap().send(());
            match release_rx.lock().unwrap().recv_timeout(WAIT) {
                Ok(()) => Ok(b"PNG".to_vec()),
                Err(_) => Err("the gate was never opened".into()),
            }
        });
        (encoder, entered, release)
    }

    fn counting_wake() -> (Box<dyn Fn() + Send>, Arc<AtomicUsize>) {
        let wakes = Arc::new(AtomicUsize::new(0));
        let n = Arc::clone(&wakes);
        let wake = Box::new(move || {
            n.fetch_add(1, Ordering::SeqCst);
        });
        (wake, wakes)
    }

    fn store_with(encoder: Encoder) -> (Store, Arc<AtomicUsize>) {
        let (wake, wakes) = counting_wake();
        (Store::with_parts(wake, encoder, QUEUE), wakes)
    }

    fn payload(w: u32, h: u32, png: Option<&[u8]>) -> Payload {
        Payload {
            bitmap: Arc::new(Bitmap::black(w, h)),
            png: png.map(Arc::from),
        }
    }

    #[test]
    fn drag_reuses_only_unchanged_regular_files_and_preserves_edited_ones() {
        let dir = tempfile::tempdir().unwrap();
        let saved = dir.path().join("annotated \u{732b}.png");
        // Larger than the comparison buffer, including a short final chunk.
        let png = vec![91; 64 * 1024 + 7];
        fs::write(&saved, &png).unwrap();
        assert_eq!(
            materialize_drag_png(&png, Some(&saved), dir.path(), STAMP).unwrap(),
            saved
        );
        let mut changed = png.clone();
        *changed.last_mut().unwrap() = 19;
        fs::write(&saved, &changed).unwrap();
        let new = materialize_drag_png(&png, Some(&saved), dir.path(), STAMP).unwrap();
        assert_ne!(new, saved);
        assert_eq!(fs::read(&saved).unwrap(), changed);
        assert_eq!(fs::read(&new).unwrap(), png);
        let as_directory = dir.path().join("directory.png");
        fs::create_dir(&as_directory).unwrap();
        let new = materialize_drag_png(b"PNG", Some(&as_directory), dir.path(), STAMP).unwrap();
        assert!(as_directory.is_dir());
        assert_eq!(fs::read(new).unwrap(), b"PNG");
    }

    #[test]
    fn drag_recreation_skips_the_disappeared_name_even_with_different_case() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join(format!("{STAMP_NAME}.png").to_uppercase());
        let collision = dir.path().join(format!("{STAMP_NAME}-2.png"));
        fs::write(&collision, b"other user's image").unwrap();
        let path = materialize_drag_png(b"final PNG", Some(&old), dir.path(), STAMP).unwrap();
        assert_eq!(path, dir.path().join(format!("{STAMP_NAME}-3.png")));
        assert!(!old.exists());
        assert_eq!(fs::read(&collision).unwrap(), b"other user's image");
        assert_eq!(fs::read(&path).unwrap(), b"final PNG");
    }

    #[test]
    fn drag_partial_write_removes_only_the_file_this_operation_created() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join(format!("{STAMP_NAME}.png"));
        let occupied = dir.path().join(format!("{STAMP_NAME}-2.png"));
        fs::write(&old, b"user-saved file").unwrap();
        fs::write(&occupied, b"another image").unwrap();
        let result = write_drag_png(dir.path(), STAMP, b"PNG complete", &old, |file, png| {
            file.write_all(&png[..4])?;
            Err(io::Error::other("synthetic disk-full failure"))
        });
        assert!(result.unwrap_err().contains("synthetic disk-full failure"));
        assert_eq!(fs::read(&old).unwrap(), b"user-saved file");
        assert_eq!(fs::read(&occupied).unwrap(), b"another image");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
        assert!(!dir.path().join(format!("{STAMP_NAME}-3.png")).exists());
    }

    #[test]
    fn drag_empty_png_never_creates_a_file_or_replaces_a_saved_one() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("saved.png");
        fs::write(&old, b"keep").unwrap();
        assert!(materialize_drag_png(&[], Some(&old), dir.path(), STAMP).is_err());
        assert_eq!(fs::read(&old).unwrap(), b"keep");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// Polls `drain` until `n` replies are in (or fails after 5 s).
    fn collect(store: &mut Store, n: usize) -> Vec<Reply> {
        let end = Instant::now() + WAIT;
        let mut got = Vec::new();
        loop {
            got.extend(store.drain());
            if got.len() >= n {
                return got;
            }
            assert!(
                Instant::now() < end,
                "{} of {n} replies: {got:?}",
                got.len()
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn by_id(replies: Vec<Reply>) -> HashMap<u64, ReplyKind> {
        replies.into_iter().map(|r| (r.id, r.kind)).collect()
    }

    fn saved(kind: &ReplyKind) -> &Path {
        match kind {
            ReplyKind::Saved(p) => p,
            other => panic!("not saved: {other:?}"),
        }
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_payload_counts_its_pixels_and_png_and_shares_them_when_cloned() {
        let p = payload(8, 6, Some(b"PNGDATA"));
        assert_eq!(p.bytes(), 8 * 6 * 4 + 7);
        assert_eq!(payload(8, 6, None).bytes(), 8 * 6 * 4);
        let q = p.clone();
        assert!(Arc::ptr_eq(&p.bitmap, &q.bitmap));
        assert!(Arc::ptr_eq(
            p.png.as_ref().unwrap(),
            q.png.as_ref().unwrap()
        ));
        assert!(format!("{p:?}").contains("png_bytes"), "no byte dump");
    }

    #[test]
    fn insert_get_and_remove_hold_and_release_the_shared_image() {
        let (mut store, _) = store_with(fake_encoder().0);
        let p = payload(4, 4, Some(b"PNG"));
        let image = Arc::downgrade(&p.bitmap);
        let png = Arc::downgrade(p.png.as_ref().unwrap());
        store.insert(7, p);
        assert!(store.get(7).is_some() && store.get(8).is_none());
        assert!(image.upgrade().is_some());
        store.remove(7);
        assert!(store.get(7).is_none());
        assert!(image.upgrade().is_none(), "the card's Arcs are released");
        assert!(png.upgrade().is_none());
        store.remove(7);
        assert!(store.drain().is_empty(), "no thread, nothing to say");
    }

    #[test]
    fn a_save_writes_the_exact_png_and_never_encodes_it_again() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Pictures").join("Daijobu");
        let (encoder, calls) = fake_encoder();
        let (mut store, _) = store_with(encoder);
        store.insert(1, payload(5, 3, Some(b"\x89PNG exact bytes")));
        assert!(store.save(1, target.clone(), STAMP));
        let replies = collect(&mut store, 1);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].id, 1);
        let path = saved(&replies[0].kind);
        assert_eq!(path, target.join(format!("{STAMP_NAME}.png")));
        assert_eq!(fs::read(path).unwrap(), b"\x89PNG exact bytes");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "it already had its PNG");
        assert!(store.drain().is_empty(), "each reply is handed out once");
    }

    #[test]
    fn a_save_without_a_png_encodes_once_and_the_store_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let (encoder, calls) = fake_encoder();
        let (mut store, _) = store_with(encoder);
        store.insert(1, payload(5, 3, None));
        assert!(store.get(1).unwrap().png.is_none());
        assert!(store.save(1, dir.path().into(), STAMP));
        let first = by_id(collect(&mut store, 1));
        assert_eq!(fs::read(saved(&first[&1])).unwrap(), b"PNG5x3");
        let cached = store.get(1).unwrap().png.clone().expect("cached by drain");
        assert_eq!(&*cached, b"PNG5x3");
        // A second save and a copy reuse it.
        assert!(store.save(1, dir.path().into(), STAMP));
        let second = by_id(collect(&mut store, 1));
        assert_eq!(fs::read(saved(&second[&1])).unwrap(), b"PNG5x3");
        let mut clip = FakeClipboard::default();
        assert_eq!(store.copy(1, &mut clip), CopyStart::Done(Ok(())));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "encoded exactly once");
        assert!(Arc::ptr_eq(
            &cached,
            store.get(1).unwrap().png.as_ref().unwrap()
        ));
    }

    #[test]
    fn two_saves_with_the_same_stamp_get_distinct_names() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, _) = store_with(fake_encoder().0);
        store.insert(1, payload(2, 2, Some(b"one")));
        store.insert(2, payload(2, 2, Some(b"two")));
        assert!(store.save(1, dir.path().into(), STAMP));
        assert!(store.save(2, dir.path().into(), STAMP));
        let got = by_id(collect(&mut store, 2));
        // One worker, in order: the first takes the plain name.
        assert_eq!(
            saved(&got[&1]),
            dir.path().join(format!("{STAMP_NAME}.png"))
        );
        assert_eq!(
            saved(&got[&2]),
            dir.path().join(format!("{STAMP_NAME}-2.png"))
        );
        assert_eq!(fs::read(saved(&got[&1])).unwrap(), b"one");
        assert_eq!(fs::read(saved(&got[&2])).unwrap(), b"two");
        // It never overwrites a file that is already there either.
        fs::write(dir.path().join(format!("{STAMP_NAME}-3.png")), b"mine").unwrap();
        store.insert(3, payload(2, 2, Some(b"three")));
        assert!(store.save(3, dir.path().into(), STAMP));
        let third = by_id(collect(&mut store, 1));
        assert_eq!(
            saved(&third[&3]),
            dir.path().join(format!("{STAMP_NAME}-4.png"))
        );
        assert_eq!(
            fs::read(dir.path().join(format!("{STAMP_NAME}-3.png"))).unwrap(),
            b"mine"
        );
    }

    #[test]
    fn a_save_that_cannot_make_its_folder_fails_and_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the folder should be.
        let blocker = dir.path().join("taken");
        fs::write(&blocker, b"x").unwrap();
        let (mut store, _) = store_with(fake_encoder().0);
        store.insert(1, payload(2, 2, Some(b"png")));
        assert!(store.save(1, blocker.join("sub"), STAMP));
        let got = collect(&mut store, 1);
        match &got[0].kind {
            ReplyKind::SaveFailed(why) => assert!(why.starts_with("cannot create"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(files_in(dir.path()), ["taken"], "no partial file or folder");
        assert_eq!(fs::read(&blocker).unwrap(), b"x");
        // The card is still there, and a retry into a good folder works.
        assert!(store.get(1).is_some());
        assert!(store.save(1, dir.path().join("ok"), STAMP));
        assert!(matches!(
            collect(&mut store, 1)[0].kind,
            ReplyKind::Saved(_)
        ));
    }

    #[test]
    fn a_batch_reports_each_card_on_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("taken");
        fs::write(&blocker, b"x").unwrap();
        let good = dir.path().join("good");
        let (encoder, calls) = fake_encoder();
        let (mut store, _) = store_with(encoder);
        store.insert(1, payload(4, 4, Some(b"one")));
        store.insert(2, payload(BAD_WIDTH, 4, None)); // cannot be encoded
        store.insert(3, payload(6, 4, None));
        store.insert(4, payload(4, 4, Some(b"four")));
        assert!(store.save(1, good.clone(), STAMP));
        assert!(store.save(2, good.clone(), STAMP));
        assert!(store.save(3, good.clone(), STAMP));
        assert!(store.save(4, blocker.join("sub"), STAMP));
        let got = by_id(collect(&mut store, 4));
        assert_eq!(got.len(), 4, "one reply per card");
        assert_eq!(fs::read(saved(&got[&1])).unwrap(), b"one");
        assert_eq!(
            got[&2],
            ReplyKind::SaveFailed("cannot encode".into()),
            "a failing card does not stop the others"
        );
        assert_eq!(fs::read(saved(&got[&3])).unwrap(), b"PNG6x4");
        assert!(matches!(&got[&4], ReplyKind::SaveFailed(w) if w.starts_with("cannot create")));
        assert_eq!(files_in(&good).len(), 2, "only the two that worked");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "cards 2 and 3");
        assert!(
            store.get(2).unwrap().png.is_none(),
            "nothing to cache for 2"
        );
        assert!(store.get(3).unwrap().png.is_some());
        // The failed ones can be retried (2 would fail again; 4 into a good folder).
        assert!(store.save(4, good.clone(), STAMP));
        assert!(matches!(
            collect(&mut store, 1)[0].kind,
            ReplyKind::Saved(_)
        ));
    }

    #[test]
    fn a_save_of_a_card_dismissed_meanwhile_still_replies_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, _) = store_with(fake_encoder().0);
        store.insert(1, payload(3, 3, None));
        let image = Arc::downgrade(&store.get(1).unwrap().bitmap);
        assert!(store.save(1, dir.path().into(), STAMP));
        store.remove(1);
        let got = collect(&mut store, 1);
        assert_eq!(got[0].id, 1, "the caller finds no card 1 and ignores it");
        assert!(
            matches!(got[0].kind, ReplyKind::Saved(_)),
            "the job had its own Arcs"
        );
        assert!(
            store.get(1).is_none(),
            "and the PNG is not cached into nothing"
        );
        assert_eq!(files_in(dir.path()).len(), 1);
        assert!(image.upgrade().is_none(), "the job let go of the image too");
    }

    #[test]
    fn a_stale_encode_never_lands_on_a_different_image() {
        let (encoder, _) = fake_encoder();
        let (mut store, _) = store_with(encoder);
        store.insert(1, payload(3, 3, None));
        let mut clip = FakeClipboard::default();
        assert_eq!(store.copy(1, &mut clip), CopyStart::Pending);
        // The card goes and another image takes the id before the reply is read.
        store.remove(1);
        store.insert(1, payload(9, 9, None));
        let got = collect(&mut store, 1);
        assert_eq!(got[0].kind, ReplyKind::Encoded);
        assert!(
            store.get(1).unwrap().png.is_none(),
            "the 3x3 PNG is not for it"
        );
        assert!(store.finish_copy(1, &mut clip).is_err());
        assert!(clip.puts.is_empty());
    }

    #[test]
    fn copy_with_a_png_puts_the_png_and_dib_on_the_spot() {
        let (mut store, wakes) = store_with(fake_encoder().0);
        let p = payload(3, 2, Some(b"the png"));
        let dib = dj_capture::dib::from_bitmap(&p.bitmap);
        store.insert(1, p);
        let mut clip = FakeClipboard::default();
        assert_eq!(store.copy(1, &mut clip), CopyStart::Done(Ok(())));
        assert_eq!(clip.puts, vec![(b"the png".to_vec(), dib)]);
        assert_eq!(clip.puts[0].1.len(), 40 + 3 * 2 * 4);
        assert!(store.drain().is_empty(), "no worker involved");
        assert_eq!(wakes.load(Ordering::SeqCst), 0);
        // A busy clipboard is reported, and the card stays.
        clip.fail = true;
        assert_eq!(
            store.copy(1, &mut clip),
            CopyStart::Done(Err("busy".into()))
        );
        assert!(store.get(1).is_some());
        // An unknown card is refused without touching the clipboard.
        clip.fail = false;
        assert_eq!(
            store.copy(2, &mut clip),
            CopyStart::Done(Err("no image".into()))
        );
        assert_eq!(clip.puts.len(), 1);
    }

    #[test]
    fn copy_without_a_png_waits_for_the_encode_then_finishes() {
        let (encoder, calls) = fake_encoder();
        let (mut store, _) = store_with(encoder);
        let p = payload(3, 2, None);
        let dib = dj_capture::dib::from_bitmap(&p.bitmap);
        store.insert(1, p);
        let mut clip = FakeClipboard::default();
        assert_eq!(store.copy(1, &mut clip), CopyStart::Pending);
        assert!(clip.puts.is_empty(), "nothing is copied before the encode");
        let got = collect(&mut store, 1);
        assert_eq!(
            got,
            vec![Reply {
                id: 1,
                kind: ReplyKind::Encoded
            }]
        );
        assert_eq!(store.finish_copy(1, &mut clip), Ok(()));
        assert_eq!(clip.puts, vec![(b"PNG3x2".to_vec(), dib)]);
        // From now on it is the quick path.
        assert_eq!(store.copy(1, &mut clip), CopyStart::Done(Ok(())));
        assert_eq!(clip.puts.len(), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_encode_is_reported_and_the_copy_can_be_tried_again() {
        let (encoder, calls) = fake_encoder();
        let (mut store, _) = store_with(encoder);
        store.insert(1, payload(BAD_WIDTH, 2, None));
        let mut clip = FakeClipboard::default();
        assert_eq!(store.copy(1, &mut clip), CopyStart::Pending);
        let got = collect(&mut store, 1);
        assert_eq!(got[0].kind, ReplyKind::EncodeFailed("cannot encode".into()));
        assert!(store.finish_copy(1, &mut clip).is_err());
        assert!(clip.puts.is_empty());
        assert_eq!(store.copy(1, &mut clip), CopyStart::Pending, "tries again");
        collect(&mut store, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // A card removed while its copy was pending: nothing to finish.
        store.remove(1);
        assert_eq!(store.finish_copy(1, &mut clip), Err("no image".into()));
    }

    #[test]
    fn no_card_means_no_save() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, _) = store_with(fake_encoder().0);
        assert!(!store.save(1, dir.path().into(), STAMP));
        store.insert(1, payload(2, 2, Some(b"png")));
        store.remove(1);
        assert!(!store.save(1, dir.path().into(), STAMP));
        assert!(files_in(dir.path()).is_empty());
        assert!(store.drain().is_empty(), "and no thread was started for it");
    }

    #[test]
    fn a_full_queue_refuses_instead_of_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let (encoder, entered, release) = gated_encoder();
        let (wake, _) = counting_wake();
        let mut store = Store::with_parts(wake, encoder, 2);
        for id in 1..=5 {
            store.insert(id, payload(2, 2, None));
        }
        assert!(store.save(1, dir.path().into(), STAMP));
        entered
            .recv_timeout(WAIT)
            .expect("the worker took the first");
        assert!(store.save(2, dir.path().into(), STAMP), "queued");
        assert!(store.save(3, dir.path().into(), STAMP), "queued");
        let t = Instant::now();
        assert!(!store.save(4, dir.path().into(), STAMP), "full");
        let mut clip = FakeClipboard::default();
        assert!(matches!(store.copy(5, &mut clip), CopyStart::Done(Err(_))));
        assert!(t.elapsed() < Duration::from_secs(1), "the UI never waits");
        for _ in 0..3 {
            release.send(()).unwrap();
        }
        let got = by_id(collect(&mut store, 3));
        assert!((1..=3).all(|id| matches!(got[&id], ReplyKind::Saved(_))));
        // Room again.
        release.send(()).unwrap();
        assert!(store.save(4, dir.path().into(), STAMP));
        assert!(matches!(
            collect(&mut store, 1)[0].kind,
            ReplyKind::Saved(_)
        ));
        assert_eq!(files_in(dir.path()).len(), 4);
    }

    #[test]
    fn the_worker_wakes_the_ui_after_it_queues_a_reply() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, wakes) = store_with(fake_encoder().0);
        store.insert(1, payload(2, 2, Some(b"png")));
        assert_eq!(wakes.load(Ordering::SeqCst), 0, "not before a job");
        assert!(store.save(1, dir.path().into(), STAMP));
        assert_eq!(collect(&mut store, 1).len(), 1);
        let end = Instant::now() + WAIT;
        while wakes.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < end, "never woken");
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "once per reply");
    }

    #[test]
    fn dropping_the_store_does_not_wait_for_the_worker_or_run_the_queue() {
        let dir = tempfile::tempdir().unwrap();
        let (encoder, entered, release) = gated_encoder();
        // The thread owns the wake closure and so this sender: it disconnects when the
        // thread has exited.
        let (alive, thread_gone) = mpsc::channel::<()>();
        let wake: Box<dyn Fn() + Send> = Box::new(move || {
            let _ = &alive;
        });
        let mut store = Store::with_parts(wake, encoder, 4);
        store.insert(1, payload(2, 2, None));
        store.insert(2, payload(2, 2, None));
        assert!(store.save(1, dir.path().join("a"), STAMP));
        entered.recv_timeout(WAIT).unwrap();
        assert!(store.save(2, dir.path().join("b"), STAMP));
        let t = Instant::now();
        drop(store);
        assert!(t.elapsed() < Duration::from_secs(1), "no join");
        release.send(()).unwrap();
        assert_eq!(
            thread_gone.recv_timeout(WAIT),
            Err(RecvTimeoutError::Disconnected),
            "the thread exits"
        );
        assert!(entered.try_recv().is_err(), "the queued job never started");
        assert!(!dir.path().join("b").exists(), "and wrote nothing");
    }

    #[test]
    fn reveal_says_missing_for_a_file_that_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("snap-gone.png");
        assert_eq!(Store::reveal(&gone), Err(RevealError::Missing));
        assert_eq!(RevealError::Missing.to_string(), "the file is gone");
    }

    #[test]
    fn reveal_hands_an_existing_file_to_the_shell() {
        use std::io;
        use std::sync::Mutex;

        #[derive(Default)]
        struct FakeOps {
            revealed: Mutex<Vec<PathBuf>>,
            fail: bool,
        }

        impl Ops for FakeOps {
            fn remove_permanent(&self, _: &Path) -> io::Result<u64> {
                unreachable!("a preview never deletes")
            }
            fn recycle(&self, _: &Path) -> io::Result<dj_sweep::Recycled> {
                unreachable!("a preview never deletes")
            }
            fn flush_dns(&self) -> io::Result<()> {
                unreachable!()
            }
            fn empty_recycle_bin(&self) -> io::Result<()> {
                unreachable!()
            }
            fn reveal(&self, p: &Path) -> io::Result<()> {
                if self.fail {
                    return Err(io::Error::other("no explorer"));
                }
                self.revealed.lock().unwrap().push(p.to_path_buf());
                Ok(())
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("snap.png");
        let ops = FakeOps::default();
        // Missing: the shell is never asked.
        assert_eq!(reveal_with(&ops, &file), Err(RevealError::Missing));
        assert!(ops.revealed.lock().unwrap().is_empty());
        fs::write(&file, b"png").unwrap();
        assert_eq!(reveal_with(&ops, &file), Ok(()));
        assert_eq!(*ops.revealed.lock().unwrap(), vec![file.clone()]);
        let broken = FakeOps {
            fail: true,
            ..FakeOps::default()
        };
        assert_eq!(
            reveal_with(&broken, &file),
            Err(RevealError::Other("no explorer".into()))
        );
        assert!(file.exists(), "revealing never deletes");
    }

    /// The real encoder on the worker thread (it sets up COM itself).
    #[test]
    fn the_real_encoder_round_trips_through_a_save_and_a_copy() {
        let dir = tempfile::tempdir().unwrap();
        let (wake, _) = counting_wake();
        let mut store = Store::new(wake);
        let mut bitmap = Bitmap::black(5, 3);
        for (i, px) in bitmap.pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *px = [i as u8, (i * 7) as u8, (i * 13) as u8, 255];
        }
        let pixels = bitmap.pixels.clone();
        let dib = dj_capture::dib::from_bitmap(&bitmap);
        store.insert(
            1,
            Payload {
                bitmap: Arc::new(bitmap.clone()),
                png: None,
            },
        );
        store.insert(
            2,
            Payload {
                bitmap: Arc::new(bitmap),
                png: None,
            },
        );
        // Save: encoded on the worker, written, cached.
        assert!(store.save(1, dir.path().into(), STAMP));
        let got = by_id(collect(&mut store, 1));
        let bytes = fs::read(saved(&got[&1])).unwrap();
        assert_eq!(dj_capture::png::decode(&bytes).unwrap().pixels, pixels);
        assert_eq!(&**store.get(1).unwrap().png.as_ref().unwrap(), &bytes[..]);
        // Copy of another card: pending, then finished from the worker's PNG.
        let mut clip = FakeClipboard::default();
        assert_eq!(store.copy(2, &mut clip), CopyStart::Pending);
        assert_eq!(collect(&mut store, 1)[0].kind, ReplyKind::Encoded);
        assert_eq!(store.finish_copy(2, &mut clip), Ok(()));
        let (png, got_dib) = &clip.puts[0];
        assert_eq!(dj_capture::png::decode(png).unwrap().pixels, pixels);
        assert_eq!(*got_dib, dib);
        assert_eq!(got_dib.len(), 40 + 5 * 3 * 4);
    }
}
