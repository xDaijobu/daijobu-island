//! Asynchronous, all-or-nothing preparation of a Snap drag. Jobs snapshot the final
//! annotated payloads; this module never captures pixels, opens the clipboard or
//! calls OLE. Completed files survive cancellation, failed batches and store drop.
//!
//! One lazy worker sleeps on its queue. The admission limit includes running jobs
//! and undrained replies, so neither side can accumulate an unbounded image queue.
//! MAX_BYTES budgets submitted pixels and cached PNGs across pending requests;
//! PNG encoding, the single-image DIB and the bounded ghost add worker allocations.
//! It is an admission budget, not a ceiling on peak process memory.
//! Cancellation is checked around slow work and again in `drain`: cancelling after
//! the worker replied must still suppress a transport waiting in the channel.

use crate::preview_store::{materialize_drag_png, Payload};
use dj_core::snap::{Bitmap, Stamp};
use dj_core::snap_stack::{MAX_BYTES, MAX_ITEMS};
use dj_win::drag_drop::{DragPayload, Ghost, ImageData};
use std::collections::{HashMap, HashSet};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread;

const QUEUE: usize = 4;
type Encoder = Arc<dyn Fn(&Bitmap) -> Result<Vec<u8>, String> + Send + Sync>;

#[derive(Clone, Debug)]
pub struct DragItem {
    pub id: u64,
    pub payload: Payload,
    pub saved: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct PreparedItem {
    pub id: u64,
    pub payload: Payload,
    pub path: PathBuf,
}

pub struct Completion {
    pub request: u64,
    /// Every completed file, in request order, even after cancellation/failure.
    pub saved: Vec<PreparedItem>,
    pub failed: Vec<(u64, String)>,
    pub cancelled: bool,
    /// Present only when every requested card was prepared and is still wanted.
    pub transport: Option<DragPayload>,
}

struct Job {
    request: u64,
    items: Vec<DragItem>,
    dir: PathBuf,
    stamp: Stamp,
    cancelled: Arc<AtomicBool>,
}

struct Pending {
    ids: Vec<u64>,
    bytes: u64,
    cancelled: Arc<AtomicBool>,
}

struct Worker {
    jobs: SyncSender<Job>,
    replies: Receiver<Completion>,
    closed: Arc<AtomicBool>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        // No join on the UI thread. An in-progress write completes; queued work
        // is skipped, and the sender dropping wakes an idle worker.
    }
}

pub struct DragStore {
    wake: Option<Box<dyn Fn() + Send>>,
    encoder: Encoder,
    worker: Option<Worker>,
    pending: HashMap<u64, Pending>,
    limit: usize,
}

impl DragStore {
    pub fn new(wake: Box<dyn Fn() + Send>) -> Self {
        Self::with_parts(wake, Arc::new(dj_capture::png::encode), QUEUE)
    }

    fn with_parts(wake: Box<dyn Fn() + Send>, encoder: Encoder, limit: usize) -> Self {
        Self {
            wake: Some(wake),
            encoder,
            worker: None,
            pending: HashMap::new(),
            limit,
        }
    }

    /// Admission does no encoding or filesystem IO. Rejected requests queue no
    /// completion; the caller can immediately clear their busy state.
    pub fn submit(
        &mut self,
        request: u64,
        items: Vec<DragItem>,
        dir: PathBuf,
        stamp: Stamp,
    ) -> Result<(), String> {
        if self.pending.contains_key(&request) {
            return Err("this drag request is already pending".into());
        }
        if self.pending.len() >= self.limit {
            return Err("drag preparation is busy, try again".into());
        }
        if items.is_empty() || items.len() > MAX_ITEMS {
            return Err("invalid number of drag images".into());
        }
        if dir.as_os_str().is_empty() {
            return Err("the drag save folder is empty".into());
        }
        let ids: Vec<_> = items.iter().map(|item| item.id).collect();
        if ids.iter().copied().collect::<HashSet<_>>().len() != items.len() {
            return Err("a drag contains a repeated card".into());
        }
        let bytes = items
            .iter()
            .try_fold(0u64, |sum, item| sum.checked_add(item.payload.bytes()));
        let bytes = bytes.ok_or("drag images exceed the memory limit")?;
        let retained: u64 = self.pending.values().map(|p| p.bytes).sum();
        if bytes > MAX_BYTES.saturating_sub(retained) {
            return Err("drag images exceed the memory limit".into());
        }
        if self.worker.is_none() {
            self.worker = Some(self.spawn()?);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let job = Job {
            request,
            items,
            dir,
            stamp,
            cancelled: Arc::clone(&cancelled),
        };
        let worker = self.worker.as_ref().expect("worker just started");
        match worker.jobs.try_send(job) {
            Ok(()) => {
                self.pending.insert(
                    request,
                    Pending {
                        ids,
                        bytes,
                        cancelled,
                    },
                );
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err("drag preparation is busy, try again".into()),
            Err(TrySendError::Disconnected(_)) => Err("the drag worker stopped".into()),
        }
    }

    pub fn cancel(&mut self, request: u64) {
        if let Some(pending) = self.pending.get(&request) {
            pending.cancelled.store(true, Ordering::SeqCst);
        }
    }

    pub fn drain(&mut self) -> Vec<Completion> {
        let Some(worker) = &self.worker else {
            return Vec::new();
        };
        let mut completions = Vec::new();
        loop {
            match worker.replies.try_recv() {
                Ok(mut done) => {
                    let Some(pending) = self.pending.remove(&done.request) else {
                        continue;
                    };
                    done.cancelled |= pending.cancelled.load(Ordering::SeqCst);
                    if done.cancelled {
                        done.transport = None;
                    }
                    completions.push(done);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // Even an unexpected worker failure must release every busy
                    // request. Normal encoder panics are handled per image below.
                    let mut lost: Vec<_> = self.pending.drain().collect();
                    lost.sort_by_key(|(request, _)| *request);
                    completions.extend(lost.into_iter().map(|(request, pending)| {
                        Completion {
                            request,
                            saved: Vec::new(),
                            failed: pending
                                .ids
                                .into_iter()
                                .map(|id| (id, "the drag worker stopped".into()))
                                .collect(),
                            cancelled: pending.cancelled.load(Ordering::SeqCst),
                            transport: None,
                        }
                    }));
                    break;
                }
            }
        }
        completions
    }

    fn spawn(&mut self) -> Result<Worker, String> {
        let wake = self.wake.take().ok_or("the drag worker could not start")?;
        let (jobs, incoming) = mpsc::sync_channel(self.limit);
        let (out, replies) = mpsc::channel();
        let closed = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&closed);
        let encoder = Arc::clone(&self.encoder);
        thread::Builder::new()
            .name("snap-drag-store".into())
            .spawn(move || {
                while let Ok(job) = incoming.recv() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let done = prepare(job, &encoder, &stop, &materialize_drag_png);
                    if out.send(done).is_err() {
                        break;
                    }
                    // A notification hook must not kill the IO queue.
                    let _ = panic::catch_unwind(AssertUnwindSafe(&wake));
                }
            })
            .map_err(|e| format!("cannot start the drag worker: {e}"))?;
        Ok(Worker {
            jobs,
            replies,
            closed,
        })
    }
}

fn stopped(cancelled: &AtomicBool, closed: &AtomicBool) -> bool {
    cancelled.load(Ordering::SeqCst) || closed.load(Ordering::SeqCst)
}

fn prepare(
    job: Job,
    encoder: &Encoder,
    closed: &AtomicBool,
    materialize: &impl Fn(&[u8], Option<&Path>, &Path, Stamp) -> Result<PathBuf, String>,
) -> Completion {
    let total = job.items.len();
    let mut done = Completion {
        request: job.request,
        saved: Vec::with_capacity(total),
        failed: Vec::new(),
        cancelled: false,
        transport: None,
    };
    for mut item in job.items {
        if stopped(&job.cancelled, closed) {
            break;
        }
        let id = item.id;
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            validate_bitmap(&item.payload.bitmap)?;
            if item.payload.png.is_none() {
                item.payload.png = Some(Arc::from(encoder(&item.payload.bitmap)?));
            }
            if stopped(&job.cancelled, closed) {
                return Ok(None);
            }
            let path = materialize(
                item.payload.png.as_deref().ok_or("no PNG bytes")?,
                item.saved.as_deref(),
                &job.dir,
                job.stamp,
            )?;
            Ok::<_, String>(Some(PreparedItem {
                id,
                payload: item.payload,
                path,
            }))
        }))
        .unwrap_or_else(|_| Err("the drag image worker crashed".into()));
        match result {
            Ok(Some(item)) => done.saved.push(item),
            Ok(None) => break,
            Err(why) => done.failed.push((id, why)),
        }
    }
    done.cancelled = stopped(&job.cancelled, closed);
    if !done.cancelled && done.failed.is_empty() && done.saved.len() == total {
        match panic::catch_unwind(AssertUnwindSafe(|| transport(&done.saved)))
            .unwrap_or_else(|_| Err("the drag transport worker crashed".into()))
        {
            Ok(payload) => done.transport = Some(payload),
            Err(why) => {
                done.failed = done
                    .saved
                    .iter()
                    .map(|item| (item.id, why.clone()))
                    .collect();
            }
        }
    }
    done.cancelled |= stopped(&job.cancelled, closed);
    if done.cancelled {
        done.transport = None;
    }
    done
}

/// Guard the existing PNG/DIB helpers before they do row arithmetic. All WIC/DIB
/// sizes fit u32, signed bitmap dimensions and usize, even for malformed inputs.
fn validate_bitmap(bitmap: &Bitmap) -> Result<(), String> {
    if bitmap.width == 0
        || bitmap.height == 0
        || bitmap.width > i32::MAX as u32
        || bitmap.height > i32::MAX as u32
    {
        return Err("invalid drag image dimensions".into());
    }
    let bytes = u64::from(bitmap.width)
        .checked_mul(u64::from(bitmap.height))
        .and_then(|pixels| pixels.checked_mul(4))
        .filter(|bytes| *bytes <= u64::from(u32::MAX))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .and_then(|bytes| bytes.checked_add(dj_capture::dib::HEADER_LEN))
        .ok_or("drag image dimensions overflow")?;
    if bitmap.pixels.len() < bytes - dj_capture::dib::HEADER_LEN {
        return Err("the drag image has incomplete pixels".into());
    }
    Ok(())
}

fn transport(items: &[PreparedItem]) -> Result<DragPayload, String> {
    let first = items.first().ok_or("the drag has no images")?;
    let image = if items.len() == 1 {
        let bitmap = &first.payload.bitmap;
        let dib = dj_capture::dib::from_bgra(
            bitmap.width,
            bitmap.height,
            bitmap.stride(),
            &bitmap.pixels,
        )
        .ok_or("the drag image cannot be packed as DIB")?;
        Some(ImageData {
            png: Arc::clone(first.payload.png.as_ref().ok_or("no PNG bytes")?),
            dib: Arc::from(dib),
        })
    } else {
        None
    };
    Ok(DragPayload {
        paths: items.iter().map(|item| item.path.clone()).collect(),
        image,
        ghost: Some(ghost(&first.payload.bitmap, items.len())),
    })
}

// The design's 110 x 72 thumbnail plus space for its count badge. Allocation and
// sampling work stay constant regardless of the full-resolution image size.
const GHOST_W: u32 = 128;
const GHOST_H: u32 = 88;
const THUMB_W: u32 = 110;
const THUMB_H: u32 = 72;

fn ghost(bitmap: &Bitmap, count: usize) -> Ghost {
    let mut pixels = vec![0; (GHOST_W * GHOST_H * 4) as usize];
    let (w, h) = if bitmap.width <= THUMB_W && bitmap.height <= THUMB_H {
        (bitmap.width, bitmap.height)
    } else if u64::from(bitmap.width) * u64::from(THUMB_H)
        >= u64::from(bitmap.height) * u64::from(THUMB_W)
    {
        (
            THUMB_W,
            (u64::from(bitmap.height) * u64::from(THUMB_W) / u64::from(bitmap.width)).max(1) as u32,
        )
    } else {
        (
            (u64::from(bitmap.width) * u64::from(THUMB_H) / u64::from(bitmap.height)).max(1) as u32,
            THUMB_H,
        )
    };
    let left = (THUMB_W - w) / 2;
    let top = 8 + (THUMB_H - h) / 2;
    for y in 0..h {
        for x in 0..w {
            let sx = (u64::from(x) * u64::from(bitmap.width) / u64::from(w)) as usize;
            let sy = (u64::from(y) * u64::from(bitmap.height) / u64::from(h)) as usize;
            let from = sy * bitmap.stride() + sx * 4;
            let to = (((top + y) * GHOST_W + left + x) * 4) as usize;
            // IDragSourceHelper takes straight alpha and premultiplies itself.
            pixels[to..to + 4].copy_from_slice(&bitmap.pixels[from..from + 4]);
        }
    }
    // The badge is already in the pixels; the native helper need not draw text.
    for y in 0..24i32 {
        for x in 98..122i32 {
            if (x - 110).pow(2) + (y - 12).pow(2) <= 11 * 11 {
                ghost_pixel(&mut pixels, x as u32, y as u32, [88, 164, 224, 255]);
            }
        }
    }
    const DIGITS: [[u8; 5]; 10] = [
        [7, 5, 5, 5, 7],
        [2, 6, 2, 2, 7],
        [7, 1, 7, 4, 7],
        [7, 1, 7, 1, 7],
        [5, 5, 7, 1, 1],
        [7, 4, 7, 1, 7],
        [7, 4, 7, 5, 7],
        [7, 1, 1, 1, 1],
        [7, 5, 7, 5, 7],
        [7, 5, 7, 1, 7],
    ];
    // Snap's MAX_ITEMS is five, so an admitted batch always has one digit.
    for (y, row) in DIGITS[count.min(9)].iter().enumerate() {
        for x in 0..3u32 {
            if row & (1 << (2 - x)) != 0 {
                for dy in 0..2 {
                    for dx in 0..2 {
                        ghost_pixel(
                            &mut pixels,
                            107 + x * 2 + dx,
                            7 + y as u32 * 2 + dy,
                            [24, 24, 24, 255],
                        );
                    }
                }
            }
        }
    }
    Ghost {
        width: GHOST_W,
        height: GHOST_H,
        bgra: Arc::from(pixels),
        count,
    }
}

fn ghost_pixel(pixels: &mut [u8], x: u32, y: u32, color: [u8; 4]) {
    let start = ((y * GHOST_W + x) * 4) as usize;
    pixels[start..start + 4].copy_from_slice(&color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::RecvTimeoutError;
    use std::sync::Mutex;
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(5);
    const STAMP: Stamp = Stamp {
        year: 2026,
        month: 10,
        day: 28,
        hour: 12,
        minute: 34,
        second: 56,
    };

    fn item(id: u64, width: u32, png: Option<&[u8]>) -> DragItem {
        DragItem {
            id,
            payload: Payload {
                bitmap: Arc::new(Bitmap::black(width, 3)),
                png: png.map(Arc::from),
            },
            saved: None,
        }
    }

    fn encoder() -> (Encoder, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let n = Arc::clone(&calls);
        (
            Arc::new(move |bitmap| {
                n.fetch_add(1, Ordering::SeqCst);
                if bitmap.width == 13 {
                    return Err("synthetic encoder failure".into());
                }
                assert_ne!(bitmap.width, 17, "synthetic encoder panic");
                Ok(format!("PNG{}x{}", bitmap.width, bitmap.height).into_bytes())
            }),
            calls,
        )
    }

    fn store(encoder: Encoder, limit: usize) -> (DragStore, Receiver<()>) {
        let (wake, wakes) = mpsc::channel();
        (
            DragStore::with_parts(
                Box::new(move || {
                    let _ = wake.send(());
                }),
                encoder,
                limit,
            ),
            wakes,
        )
    }

    fn gate() -> (Encoder, Receiver<()>, mpsc::Sender<()>) {
        let (enter, entered) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let released = Mutex::new(released);
        (
            Arc::new(move |_| {
                enter.send(()).unwrap();
                released.lock().unwrap().recv_timeout(WAIT).unwrap();
                Ok(b"PNG gate".to_vec())
            }),
            entered,
            release,
        )
    }

    fn collect(store: &mut DragStore, wakes: &Receiver<()>, count: usize) -> Vec<Completion> {
        let mut out = Vec::new();
        loop {
            out.extend(store.drain());
            if out.len() >= count {
                assert_eq!(out.len(), count);
                return out;
            }
            wakes.recv_timeout(WAIT).expect("worker wake");
        }
    }

    fn one(store: &mut DragStore, wakes: &Receiver<()>) -> Completion {
        collect(store, wakes, 1).pop().unwrap()
    }

    #[test]
    fn single_image_shares_png_and_packs_its_dib_on_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let (encode, calls) = encoder();
        let (mut store, wakes) = store(encode, QUEUE);
        let input = item(41, 4, Some(b"final annotated PNG"));
        let png = input.payload.png.clone().unwrap();
        let bitmap = Arc::clone(&input.payload.bitmap);
        store
            .submit(9, vec![input], dir.path().into(), STAMP)
            .unwrap();
        let done = one(&mut store, &wakes);
        assert_eq!(done.request, 9);
        assert!(!done.cancelled && done.failed.is_empty());
        assert_eq!(done.saved.len(), 1);
        assert_eq!(done.saved[0].id, 41);
        assert!(done.saved[0].path.is_absolute());
        assert!(Arc::ptr_eq(&bitmap, &done.saved[0].payload.bitmap));
        let transport = done.transport.unwrap();
        assert_eq!(transport.paths, [done.saved[0].path.clone()]);
        let image = transport.image.unwrap();
        assert!(Arc::ptr_eq(&image.png, &png));
        assert_eq!(&*image.dib, dj_capture::dib::from_bitmap(&bitmap));
        assert_eq!(fs::read(&transport.paths[0]).unwrap(), &*png);
        assert_eq!(transport.ghost.unwrap().count, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(store.drain().is_empty(), "completion is delivered once");
        drop(store);
        assert_eq!(fs::read(&done.saved[0].path).unwrap(), &*png);
    }

    #[test]
    fn multiple_images_preserve_order_and_never_offer_a_single_image_format() {
        let dir = tempfile::tempdir().unwrap();
        let (encode, calls) = encoder();
        let (mut store, wakes) = store(encode, QUEUE);
        let inputs = vec![item(8, 4, None), item(2, 5, None), item(6, 6, None)];
        store.submit(1, inputs, dir.path().into(), STAMP).unwrap();
        let done = one(&mut store, &wakes);
        assert_eq!(
            done.saved.iter().map(|p| p.id).collect::<Vec<_>>(),
            [8, 2, 6]
        );
        let transport = done.transport.unwrap();
        assert!(transport.image.is_none());
        assert_eq!(transport.ghost.unwrap().count, 3);
        for (i, path) in transport.paths.iter().enumerate() {
            assert_eq!(
                *path,
                dir.path()
                    .join(dj_core::snap::file_name(STAMP, i as u32 + 1))
            );
            assert_eq!(
                fs::read(path).unwrap(),
                format!("PNG{}x3", i + 4).as_bytes()
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn reuse_and_disappeared_saved_file_retry_keep_the_new_path_and_png() {
        let dir = tempfile::tempdir().unwrap();
        let (encode, calls) = encoder();
        let (mut store, wakes) = store(encode, QUEUE);
        store
            .submit(1, vec![item(1, 4, None)], dir.path().into(), STAMP)
            .unwrap();
        let initial = one(&mut store, &wakes).saved.pop().unwrap();
        let retry = DragItem {
            id: initial.id,
            payload: initial.payload.clone(),
            saved: Some(initial.path.clone()),
        };
        store
            .submit(2, vec![retry.clone()], dir.path().into(), STAMP)
            .unwrap();
        let reused = one(&mut store, &wakes);
        assert_eq!(reused.saved[0].path, initial.path);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        fs::remove_file(&initial.path).unwrap();
        store
            .submit(3, vec![retry], dir.path().into(), STAMP)
            .unwrap();
        let recreated = one(&mut store, &wakes);
        assert_ne!(recreated.saved[0].path, initial.path);
        assert_eq!(
            recreated.saved[0].path,
            dir.path().join(dj_core::snap::file_name(STAMP, 2))
        );
        assert!(
            !initial.path.exists(),
            "missing previous path stays missing"
        );
        assert_eq!(fs::read(&recreated.saved[0].path).unwrap(), b"PNG4x3");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_or_panicking_image_never_drags_a_subset_and_later_jobs_still_work() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, wakes) = store(encoder().0, QUEUE);
        store
            .submit(
                1,
                vec![
                    item(1, 4, None),
                    item(2, 13, None),
                    item(3, 5, None),
                    item(4, 17, None),
                ],
                dir.path().into(),
                STAMP,
            )
            .unwrap();
        let done = one(&mut store, &wakes);
        assert!(done.transport.is_none() && !done.cancelled);
        assert_eq!(
            done.failed.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            [2, 4]
        );
        assert_eq!(done.saved.iter().map(|p| p.id).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(fs::read(&done.saved[0].path).unwrap(), b"PNG4x3");
        assert_eq!(fs::read(&done.saved[1].path).unwrap(), b"PNG5x3");
        let retry = done
            .saved
            .into_iter()
            .map(|p| DragItem {
                id: p.id,
                payload: p.payload,
                saved: Some(p.path),
            })
            .collect();
        store.submit(2, retry, dir.path().into(), STAMP).unwrap();
        assert!(one(&mut store, &wakes).transport.is_some());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn write_failure_retains_reused_files_and_reports_each_failed_card() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("user.png");
        fs::write(&old, b"existing PNG").unwrap();
        let blocker = dir.path().join("not a folder");
        fs::write(&blocker, b"do not change").unwrap();
        let (mut store, wakes) = store(encoder().0, QUEUE);
        let mut reused = item(1, 4, Some(b"existing PNG"));
        reused.saved = Some(old.clone());
        store
            .submit(
                1,
                vec![reused, item(2, 5, None), item(3, 6, None)],
                blocker.join("child"),
                STAMP,
            )
            .unwrap();
        let done = one(&mut store, &wakes);
        assert!(done.transport.is_none());
        assert_eq!(done.saved.len(), 1);
        assert_eq!(done.saved[0].path, old);
        assert_eq!(
            done.failed.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            [2, 3]
        );
        assert_eq!(fs::read(&old).unwrap(), b"existing PNG");
        assert_eq!(fs::read(&blocker).unwrap(), b"do not change");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn retry_after_partial_failure_prepares_the_whole_group_without_duplicate_files() {
        let dir = tempfile::tempdir().unwrap();
        let fail = Arc::new(AtomicBool::new(true));
        let fail_encoder = Arc::clone(&fail);
        let calls = Arc::new(AtomicUsize::new(0));
        let encode_calls = Arc::clone(&calls);
        let (mut store, wakes) = store(
            Arc::new(move |bitmap| {
                encode_calls.fetch_add(1, Ordering::SeqCst);
                if bitmap.width == 5 && fail_encoder.load(Ordering::SeqCst) {
                    Err("temporary encoding failure".into())
                } else {
                    Ok(format!("PNG{}", bitmap.width).into_bytes())
                }
            }),
            QUEUE,
        );
        let failed_input = item(2, 5, None);
        store
            .submit(
                1,
                vec![item(1, 4, None), failed_input.clone(), item(3, 6, None)],
                dir.path().into(),
                STAMP,
            )
            .unwrap();
        let initial = one(&mut store, &wakes);
        assert!(initial.transport.is_none());
        assert_eq!(initial.failed.iter().map(|p| p.0).collect::<Vec<_>>(), [2]);
        assert_eq!(
            initial.saved.iter().map(|p| p.id).collect::<Vec<_>>(),
            [1, 3]
        );
        let retained_paths: Vec<_> = initial.saved.iter().map(|p| p.path.clone()).collect();
        let mut retry: Vec<_> = initial
            .saved
            .into_iter()
            .map(|p| DragItem {
                id: p.id,
                payload: p.payload,
                saved: Some(p.path),
            })
            .collect();
        retry.insert(1, failed_input);
        fail.store(false, Ordering::SeqCst);
        store.submit(2, retry, dir.path().into(), STAMP).unwrap();
        let done = one(&mut store, &wakes);
        assert!(done.failed.is_empty() && !done.cancelled);
        assert_eq!(
            done.saved.iter().map(|p| p.id).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        let transport = done.transport.unwrap();
        assert_eq!(transport.paths[0], retained_paths[0]);
        assert_eq!(transport.paths[2], retained_paths[1]);
        assert!(transport.image.is_none());
        for (path, expected) in transport.paths.iter().zip([b"PNG4", b"PNG5", b"PNG6"]) {
            assert_eq!(fs::read(path).unwrap(), expected);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    }

    #[test]
    fn cancellation_during_encoding_keeps_completed_files_and_skips_remaining_items() {
        let dir = tempfile::tempdir().unwrap();
        let (encode, entered, release) = gate();
        let (mut store, wakes) = store(encode, QUEUE);
        store
            .submit(
                9,
                vec![
                    item(1, 4, Some(b"already complete")),
                    item(2, 5, None),
                    item(3, 6, Some(b"do not write")),
                ],
                dir.path().into(),
                STAMP,
            )
            .unwrap();
        entered.recv_timeout(WAIT).unwrap();
        assert!(store.drain().is_empty(), "encoding is still gated");
        store.cancel(9);
        store.cancel(9);
        release.send(()).unwrap();
        let done = one(&mut store, &wakes);
        assert!(done.cancelled && done.transport.is_none() && done.failed.is_empty());
        assert_eq!(done.saved.len(), 1);
        assert_eq!(done.saved[0].id, 1);
        assert_eq!(fs::read(&done.saved[0].path).unwrap(), b"already complete");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn cancellation_after_reply_was_queued_still_removes_transport() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, wakes) = store(encoder().0, QUEUE);
        store
            .submit(11, vec![item(1, 4, None)], dir.path().into(), STAMP)
            .unwrap();
        wakes.recv_timeout(WAIT).unwrap();
        store.cancel(11);
        let done = one(&mut store, &wakes);
        assert!(done.cancelled && done.transport.is_none());
        assert_eq!(fs::read(&done.saved[0].path).unwrap(), b"PNG4x3");
        store.cancel(11);
        assert!(store.drain().is_empty());
    }

    #[test]
    fn cancellation_during_materialization_retains_the_completed_file_and_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let job = Job {
            request: 12,
            items: vec![item(1, 4, Some(b"finished despite cancel"))],
            dir: dir.path().into(),
            stamp: STAMP,
            cancelled: Arc::clone(&cancelled),
        };
        let (entered, wait) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let task = thread::spawn(move || {
            prepare(
                job,
                &encoder().0,
                &AtomicBool::new(false),
                &|png, old, dir, stamp| {
                    let path = materialize_drag_png(png, old, dir, stamp)?;
                    entered.send(()).unwrap();
                    gate.recv_timeout(WAIT).unwrap();
                    Ok(path)
                },
            )
        });
        wait.recv_timeout(WAIT).unwrap();
        cancelled.store(true, Ordering::SeqCst);
        release.send(()).unwrap();
        let done = task.join().unwrap();
        assert!(done.cancelled && done.transport.is_none() && done.failed.is_empty());
        assert_eq!(done.saved.len(), 1);
        assert_eq!(
            fs::read(&done.saved[0].path).unwrap(),
            b"finished despite cancel"
        );
    }

    #[test]
    fn queued_cancellation_skips_all_io_without_cancelling_another_request() {
        let dir = tempfile::tempdir().unwrap();
        let (encode, entered, release) = gate();
        let (mut store, wakes) = store(encode, QUEUE);
        store
            .submit(1, vec![item(1, 4, None)], dir.path().into(), STAMP)
            .unwrap();
        entered.recv_timeout(WAIT).unwrap();
        store
            .submit(2, vec![item(2, 5, None)], dir.path().into(), STAMP)
            .unwrap();
        store.cancel(2);
        store.cancel(999);
        release.send(()).unwrap();
        let done = collect(&mut store, &wakes, 2);
        assert!(done[0].transport.is_some() && !done[0].cancelled);
        assert_eq!(done[1].request, 2);
        assert!(done[1].cancelled && done[1].saved.is_empty() && done[1].transport.is_none());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn cancelling_an_old_request_does_not_cancel_a_new_generation() {
        let dir = tempfile::tempdir().unwrap();
        let (encode, entered, release) = gate();
        let (mut store, wakes) = store(encode, QUEUE);
        store
            .submit(8, vec![item(1, 4, None)], dir.path().into(), STAMP)
            .unwrap();
        entered.recv_timeout(WAIT).unwrap();
        store.cancel(8);
        store
            .submit(9, vec![item(1, 4, None)], dir.path().into(), STAMP)
            .unwrap();
        store.cancel(8);
        release.send(()).unwrap();
        entered.recv_timeout(WAIT).unwrap();
        release.send(()).unwrap();
        let done = collect(&mut store, &wakes, 2);
        assert!(done[0].cancelled && done[0].saved.is_empty());
        assert!(!done[1].cancelled && done[1].transport.is_some());
        assert_eq!(done[1].request, 9);
        assert_eq!(done[1].saved[0].id, 1);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn admission_bounds_undrained_completions_and_rejects_duplicate_ids() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, wakes) = store(encoder().0, 1);
        assert!(store
            .submit(1, Vec::new(), dir.path().into(), STAMP)
            .is_err());
        assert!(store
            .submit(
                1,
                vec![item(1, 4, None), item(1, 5, None)],
                dir.path().into(),
                STAMP
            )
            .is_err());
        assert!(store
            .submit(
                1,
                (0..=MAX_ITEMS as u64).map(|id| item(id, 4, None)).collect(),
                dir.path().into(),
                STAMP
            )
            .is_err());
        assert!(store.worker.is_none(), "invalid jobs never start a worker");
        store
            .submit(1, vec![item(1, 4, None)], dir.path().into(), STAMP)
            .unwrap();
        wakes.recv_timeout(WAIT).unwrap();
        assert!(store
            .submit(1, vec![item(2, 5, None)], dir.path().into(), STAMP)
            .unwrap_err()
            .contains("already pending"));
        assert!(store
            .submit(2, vec![item(2, 5, None)], dir.path().into(), STAMP)
            .unwrap_err()
            .contains("busy"));
        assert!(one(&mut store, &wakes).transport.is_some());
        store
            .submit(2, vec![item(2, 5, None)], dir.path().into(), STAMP)
            .unwrap();
        assert!(one(&mut store, &wakes).transport.is_some());
    }

    #[test]
    fn malformed_images_fail_before_encoding_or_writing() {
        let dir = tempfile::tempdir().unwrap();
        let (encode, calls) = encoder();
        let (mut store, wakes) = store(encode, QUEUE);
        let images = [
            Bitmap::default(),
            Bitmap {
                width: 4,
                height: 3,
                pixels: vec![0; 2],
            },
            Bitmap {
                width: u32::MAX,
                height: u32::MAX,
                pixels: Vec::new(),
            },
            Bitmap {
                width: i32::MAX as u32,
                height: 4,
                pixels: Vec::new(),
            },
        ];
        let items = images
            .into_iter()
            .enumerate()
            .map(|(i, bitmap)| DragItem {
                id: i as u64,
                payload: Payload {
                    bitmap: Arc::new(bitmap),
                    png: None,
                },
                saved: None,
            })
            .collect();
        store.submit(1, items, dir.path().into(), STAMP).unwrap();
        let done = one(&mut store, &wakes);
        assert_eq!(done.failed.len(), 4);
        assert!(done.saved.is_empty() && done.transport.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn dropping_store_never_waits_for_encoding_and_skips_queued_io() {
        let dir = tempfile::tempdir().unwrap();
        let (encode, entered, release) = gate();
        let (mut store, wakes) = store(encode, QUEUE);
        let input = item(1, 4, None);
        let image = Arc::downgrade(&input.payload.bitmap);
        store
            .submit(1, vec![input], dir.path().into(), STAMP)
            .unwrap();
        entered.recv_timeout(WAIT).unwrap();
        store
            .submit(2, vec![item(2, 5, None)], dir.path().into(), STAMP)
            .unwrap();
        let (dropped, done) = mpsc::channel();
        let dropper = thread::spawn(move || {
            drop(store);
            dropped.send(()).unwrap();
        });
        done.recv_timeout(Duration::from_secs(1))
            .expect("store drop must not join the gated worker");
        release.send(()).unwrap();
        dropper.join().unwrap();
        assert_eq!(
            wakes.recv_timeout(WAIT),
            Err(RecvTimeoutError::Disconnected)
        );
        assert!(image.upgrade().is_none());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn ghost_is_bounded_and_its_pixels_include_the_group_count() {
        let mut bitmap = Bitmap::black(1100, 720);
        for pixel in bitmap.pixels.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&[40, 80, 120, 128]);
        }
        let single = ghost(&bitmap, 1);
        let group = ghost(&bitmap, 3);
        assert_eq!((group.width, group.height, group.count), (128, 88, 3));
        assert_eq!(group.bgra.len(), 128 * 88 * 4);
        let center = ((40 * GHOST_W + 50) * 4) as usize;
        assert_eq!(&group.bgra[center..center + 4], [40, 80, 120, 128]);
        assert_ne!(single.bgra, group.bgra, "count changes the badge pixels");
        assert_eq!(&group.bgra[..4], [0, 0, 0, 0]);
        assert_eq!(
            ghost(&Bitmap::black(1, 1000), 1).bgra.len(),
            group.bgra.len()
        );
        assert_eq!(
            ghost(&Bitmap::black(1000, 1), 1).bgra.len(),
            group.bgra.len()
        );
    }

    #[test]
    fn real_png_preparation_round_trips_final_annotated_pixels() {
        let dir = tempfile::tempdir().unwrap();
        let (wake, wakes) = mpsc::channel();
        let mut store = DragStore::new(Box::new(move || {
            let _ = wake.send(());
        }));
        let mut bitmap = Bitmap::black(5, 3);
        for (i, pixel) in bitmap.pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            pixel.copy_from_slice(&[i as u8 * 7, i as u8 * 11, 219, 255]);
        }
        let expected = bitmap.clone();
        store
            .submit(
                1,
                vec![DragItem {
                    id: 8,
                    payload: Payload {
                        bitmap: Arc::new(bitmap),
                        png: None,
                    },
                    saved: None,
                }],
                dir.path().into(),
                STAMP,
            )
            .unwrap();
        let done = one(&mut store, &wakes);
        assert!(done.failed.is_empty());
        let png = fs::read(&done.saved[0].path).unwrap();
        assert_eq!(dj_capture::png::decode(&png).unwrap(), expected);
        let image = done.transport.unwrap().image.unwrap();
        assert_eq!(&*image.png, png);
        assert_eq!(&*image.dib, dj_capture::dib::from_bitmap(&expected));
    }
}
