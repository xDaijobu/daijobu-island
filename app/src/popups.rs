//! The Settings window and Spotlight (Stage V): their windows, renderers and the
//! Spotlight index and icon workers. The app owns these and routes `Event::Popup` here.

use crate::storage;
use dj_core::spotlight::{self as spot, Entry, EntryKind, SizeInfo};
use dj_render::{IconCache, PopupRenderer};
use dj_win::launcher::{self, IconBitmap};
use dj_win::popup::{self, PopupKind};
use dj_win::{window, HWND};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// A rebuilt index older than this is refreshed when Spotlight opens.
const INDEX_MAX_AGE: Duration = Duration::from_secs(5 * 60);
/// The file scan's limits.
const SCAN_DEPTH: usize = 6;
const SCAN_MAX: usize = 60_000;
/// Icons are fetched at this size (sharp at the 28 px row icon up to 2x).
pub const ICON_PX: u32 = 64;

/// A popup window and the GPU surface drawing it.
pub struct PopupWin {
    pub hwnd: HWND,
    /// Dropped before the window is destroyed.
    pub renderer: Option<PopupRenderer>,
    /// Device px per logical px.
    pub scale: f32,
    /// Client size in device px.
    pub size: (i32, i32),
    pub dirty: bool,
    pub shown: bool,
}

impl PopupWin {
    /// Creates the (hidden) window at `x, y` and its renderer, `w` x `h` logical px.
    pub fn create(kind: PopupKind, x: i32, y: i32, w: f32, h: f32, scale: f32) -> Option<Self> {
        let size = ((w * scale).ceil() as i32, (h * scale).ceil() as i32);
        let hwnd = match popup::create(kind, x, y, size.0, size.1) {
            Ok(h) => h,
            Err(e) => {
                storage::log(&format!("{kind:?} window: {e}"));
                return None;
            }
        };
        let renderer = match PopupRenderer::new(hwnd.0 as isize, size.0 as u32, size.1 as u32) {
            Ok(r) => Some(r),
            Err(e) => {
                storage::log(&format!("{kind:?} renderer: {e}"));
                popup::destroy(hwnd);
                return None;
            }
        };
        Some(Self {
            hwnd,
            renderer,
            scale,
            size,
            dirty: true,
            shown: false,
        })
    }

    pub fn destroy(mut self) {
        self.renderer = None;
        popup::destroy(self.hwnd);
    }

    /// Window device px → window logical px.
    pub fn local(&self, x: f32, y: f32) -> (f32, f32) {
        (x / self.scale, y / self.scale)
    }

    /// A new monitor DPI: the window was resized by the system; the swap chain follows.
    pub fn set_dpi(&mut self, dpi: u32, w: f32, h: f32) {
        self.scale = dpi.max(96) as f32 / 96.0;
        self.size = (
            (w * self.scale).ceil() as i32,
            (h * self.scale).ceil() as i32,
        );
        if let Some(r) = self.renderer.as_mut() {
            if let Err(e) = r.resize(self.size.0 as u32, self.size.1 as u32) {
                storage::log(&format!("popup resize: {e}"));
            }
        }
        self.dirty = true;
    }
}

// ---------------------------------------------------------------------------------------
// The Spotlight index: apps from shell:AppsFolder and file names in the user's folders,
// built on a background thread.

#[derive(Default)]
struct IndexState {
    entries: Vec<Entry>,
    built: Option<Instant>,
    /// Bumped on every finished build, so the app sees a new list.
    version: u64,
}

pub struct SpotIndex {
    state: Arc<Mutex<IndexState>>,
    building: Arc<AtomicBool>,
    /// The entries the app last took, and their version.
    pub entries: Vec<Entry>,
    seen: u64,
}

impl SpotIndex {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(IndexState::default())),
            building: Arc::new(AtomicBool::new(false)),
            entries: Vec::new(),
            seen: 0,
        }
    }

    pub fn building(&self) -> bool {
        self.building.load(Ordering::Acquire)
    }

    /// Starts a rebuild unless one runs or (without `force`) the list is fresh. The thread
    /// wakes the app (`raw`) when it is done.
    pub fn rebuild(&self, files: bool, force: bool, raw: isize) {
        let fresh = self
            .state
            .lock()
            .map(|s| s.built.is_some_and(|t| t.elapsed() < INDEX_MAX_AGE))
            .unwrap_or(false);
        if (fresh && !force) || self.building.swap(true, Ordering::AcqRel) {
            return;
        }
        let state = Arc::clone(&self.state);
        let building = Arc::clone(&self.building);
        let spawned = std::thread::Builder::new()
            .name("dj-spotlight-index".into())
            .spawn(move || {
                let entries = build_index(files);
                if let Ok(mut s) = state.lock() {
                    s.entries = entries;
                    s.built = Some(Instant::now());
                    s.version += 1;
                }
                building.store(false, Ordering::Release);
                window::wake(raw);
            });
        if spawned.is_err() {
            self.building.store(false, Ordering::Release);
        }
    }

    /// Takes a finished build. True when the entries changed.
    pub fn take(&mut self) -> bool {
        let Ok(s) = self.state.lock() else {
            return false;
        };
        if s.version == self.seen {
            return false;
        }
        self.seen = s.version;
        self.entries = s.entries.clone();
        true
    }
}

fn build_index(files: bool) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for app in launcher::list_apps() {
        if app.name.trim().is_empty() || !seen.insert(app.parsing_name.clone()) {
            continue;
        }
        out.push(Entry {
            kind: EntryKind::App,
            name: app.name,
            target: app.parsing_name,
            detail: "App".to_string(),
        });
    }
    if files {
        let profile = std::env::var_os("USERPROFILE").map(PathBuf::from);
        let cancel = AtomicBool::new(false);
        let roots = launcher::user_folders();
        for hit in launcher::scan_files(&roots, SCAN_DEPTH, SCAN_MAX, &cancel) {
            let Some(name) = hit
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
            else {
                continue;
            };
            let detail = hit
                .path
                .parent()
                .map(|p| tilde(p, profile.as_deref()))
                .unwrap_or_default();
            out.push(Entry {
                kind: if hit.is_dir {
                    EntryKind::Folder
                } else {
                    EntryKind::File
                },
                name,
                target: hit.path.to_string_lossy().into_owned(),
                detail,
            });
        }
    }
    out
}

/// `C:\Users\me\Documents` → `~\Documents`.
fn tilde(p: &Path, profile: Option<&Path>) -> String {
    match profile.and_then(|home| p.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~\\{}", rest.display()),
        None => p.display().to_string(),
    }
}

/// Opens an entry: an app through `shell:AppsFolder` (a web link or a plain path, which
/// some Start menu entries are, opens directly), a file or folder with its handler.
pub fn launch(entry: &Entry) -> bool {
    match entry.kind {
        EntryKind::App => {
            let t = &entry.target;
            let direct =
                t.starts_with("http://") || t.starts_with("https://") || Path::new(t).is_absolute();
            if direct {
                launcher::open_path(Path::new(t))
            } else {
                launcher::launch_app(t)
            }
        }
        EntryKind::File | EntryKind::Folder => launcher::open_path(Path::new(&entry.target)),
    }
}

// ---------------------------------------------------------------------------------------
// Icons, loaded on a background thread for the rows on screen.

enum IconJob {
    App(String),
    File(String),
}

pub struct IconLoader {
    jobs: Option<Sender<(String, IconJob)>>,
    done: Receiver<(String, Option<IconBitmap>)>,
    /// Keys asked for (done or not), so each is fetched once.
    asked: HashSet<String>,
}

impl IconLoader {
    pub fn new(raw: isize) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<(String, IconJob)>();
        let (done_tx, done_rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("dj-spotlight-icons".into())
            .spawn(move || {
                for (key, job) in job_rx {
                    let icon = match &job {
                        IconJob::App(target) => launcher::app_icon(target, ICON_PX),
                        IconJob::File(path) => launcher::file_icon(path, ICON_PX),
                    };
                    if done_tx.send((key, icon)).is_err() {
                        break;
                    }
                    window::wake(raw);
                }
            });
        Self {
            jobs: spawned.ok().map(|_| job_tx),
            done: done_rx,
            asked: HashSet::new(),
        }
    }

    /// Asks for the icon of a row (`Entry::icon_key`: an app's target, or a file
    /// extension / "<folder>") unless it was asked for already.
    pub fn want(&mut self, kind: EntryKind, key: &str) {
        if !self.asked.insert(key.to_string()) {
            return;
        }
        let job = match kind {
            EntryKind::App if key.starts_with("http") => IconJob::File(".url".to_string()),
            EntryKind::App if Path::new(key).is_absolute() => IconJob::File(key.to_string()),
            EntryKind::App => IconJob::App(key.to_string()),
            // Folders and files share an icon per extension.
            EntryKind::Folder | EntryKind::File => IconJob::File(key.to_string()),
        };
        let key = key.to_string();
        if let Some(tx) = self.jobs.as_ref() {
            let _ = tx.send((key, job));
        }
    }

    /// Moves finished icons into `cache`. True when any arrived.
    pub fn drain(&mut self, cache: &mut IconCache) -> bool {
        let mut any = false;
        while let Ok((key, icon)) = self.done.try_recv() {
            if let Some(i) = icon {
                cache.insert_bgra(&key, i.w, i.h, &i.bgra);
                any = true;
            }
        }
        any
    }
}

// ---------------------------------------------------------------------------------------
// Sizes (and a folder's file count), loaded lazily on a background thread for the rows on
// screen. The queue always holds only the latest rows: what scrolled away is dropped.

/// A folder count stops here or after `FOLDER_TIME` (the size then reads "≥").
const FOLDER_MAX_FILES: u64 = 200_000;
const FOLDER_TIME: Duration = Duration::from_secs(2);
/// How long a known size stays good: folders and files change, an install size doesn't.
const FOLDER_TTL: Duration = Duration::from_secs(5 * 60);
const FILE_TTL: Duration = Duration::from_secs(60);

#[derive(Clone)]
enum SizeJob {
    File(PathBuf),
    Folder(PathBuf),
    App(String),
}

type SizeQueue = Arc<(Mutex<VecDeque<(String, SizeJob)>>, Condvar)>;

pub struct SizeLoader {
    queue: SizeQueue,
    done: Receiver<(String, SizeInfo, Duration)>,
    cache: HashMap<String, (SizeInfo, Instant, Duration)>,
    /// Asked for and not back yet.
    pending: HashSet<String>,
}

impl SizeLoader {
    pub fn new(raw: isize) -> Self {
        let queue: SizeQueue = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let (done_tx, done_rx) = mpsc::channel();
        let q = Arc::clone(&queue);
        let _ = std::thread::Builder::new()
            .name("dj-spotlight-sizes".into())
            .spawn(move || {
                // The install sizes are read once, on first need.
                let mut installed: Option<Vec<(String, u64)>> = None;
                let never = AtomicBool::new(false);
                loop {
                    let (key, job) = {
                        let (lock, cv) = &*q;
                        let Ok(mut jobs) = lock.lock() else {
                            return;
                        };
                        loop {
                            if let Some(j) = jobs.pop_front() {
                                break j;
                            }
                            jobs = match cv.wait(jobs) {
                                Ok(j) => j,
                                Err(_) => return,
                            };
                        }
                    };
                    let ttl = match job {
                        SizeJob::File(_) => FILE_TTL,
                        SizeJob::Folder(_) => FOLDER_TTL,
                        SizeJob::App(_) => Duration::MAX,
                    };
                    let info = match job {
                        SizeJob::File(p) => match std::fs::metadata(&p) {
                            Ok(m) => SizeInfo::Known {
                                bytes: m.len(),
                                files: None,
                                capped: false,
                            },
                            Err(_) => SizeInfo::Unknown,
                        },
                        SizeJob::Folder(p) => {
                            let deadline = Instant::now() + FOLDER_TIME;
                            let (bytes, files, capped) =
                                launcher::folder_size(&p, FOLDER_MAX_FILES, deadline, &never);
                            SizeInfo::Known {
                                bytes,
                                files: Some(files),
                                capped,
                            }
                        }
                        SizeJob::App(name) => {
                            let list = installed.get_or_insert_with(launcher::installed_sizes);
                            match spot::installed_size(&name, list) {
                                Some(bytes) => SizeInfo::Known {
                                    bytes,
                                    files: None,
                                    capped: false,
                                },
                                None => SizeInfo::Unknown,
                            }
                        }
                    };
                    if done_tx.send((key, info, ttl)).is_err() {
                        return;
                    }
                    window::wake(raw);
                }
            });
        Self {
            queue,
            done: done_rx,
            cache: HashMap::new(),
            pending: HashSet::new(),
        }
    }

    /// What is known about `target`: its size, `Pending` while it loads, None if never
    /// asked for (or expired).
    pub fn get(&self, target: &str) -> Option<SizeInfo> {
        if let Some((info, at, ttl)) = self.cache.get(target) {
            if at.elapsed() < *ttl {
                return Some(*info);
            }
        }
        self.pending.contains(target).then_some(SizeInfo::Pending)
    }

    /// The rows now on screen `(kind, target, name)`: those without a fresh size are
    /// queued, in order, replacing whatever was still waiting (rows that scrolled away
    /// or were typed away are dropped).
    pub fn want(&mut self, rows: &[(EntryKind, String, String)]) {
        let (lock, cv) = &*self.queue;
        let Ok(mut jobs) = lock.lock() else {
            return;
        };
        for (key, _) in jobs.drain(..) {
            self.pending.remove(&key);
        }
        for (kind, target, name) in rows {
            let fresh = self
                .cache
                .get(target)
                .is_some_and(|(_, at, ttl)| at.elapsed() < *ttl);
            if fresh || self.pending.contains(target) {
                continue;
            }
            let job = match kind {
                EntryKind::App => SizeJob::App(name.clone()),
                EntryKind::Folder => SizeJob::Folder(PathBuf::from(target)),
                EntryKind::File => SizeJob::File(PathBuf::from(target)),
            };
            self.pending.insert(target.clone());
            jobs.push_back((target.clone(), job));
        }
        cv.notify_one();
    }

    /// Takes finished sizes. True when any arrived.
    pub fn drain(&mut self) -> bool {
        let mut any = false;
        while let Ok((key, info, ttl)) = self.done.try_recv() {
            self.pending.remove(&key);
            self.cache.insert(key, (info, Instant::now(), ttl));
            any = true;
        }
        any
    }
}
