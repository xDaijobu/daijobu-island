//! The sweep worker: one thread that runs scans, cleans and quick actions off the UI
//! thread. Jobs come in over a channel; results are queued and the UI is woken with
//! `WM_SWEEP`. The thread blocks on the channel while idle, so it costs nothing.
//!
//! Scans are cancellable (a shared flag the scanners poll), and each category scan
//! runs under the time budget that `dj_sweep::ScanEnv` carries.

use dj_core::sweep::{Category, ItemView};
use dj_sweep::{Action, OpLog, Ops, ScanEnv, WinOps};
use dj_win::window::{post_to, WM_SWEEP};
use dj_win::HWND;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

/// Clean progress is posted at most this often (plus once at the end).
const PROGRESS_EVERY: Duration = Duration::from_millis(50);

/// Tells the results of one scan apart from an earlier, cancelled one.
pub type ScanId = u64;

#[derive(Debug)]
pub enum Job {
    /// The background estimate for the Vitals button: totals only, no big files.
    Estimate(ScanId, Arc<AtomicBool>),
    /// The Sweep view's preview: every category, one message per category.
    Preview(ScanId, Arc<AtomicBool>),
    Clean(Vec<(Category, PathBuf)>),
    /// Recycles one big file (through `clean`, so it is guarded and logged).
    Recycle(PathBuf),
    Reveal(PathBuf),
    FlushDns,
    EmptyBin,
}

#[derive(Debug)]
pub enum Msg {
    /// One category of a preview finished.
    Category {
        id: ScanId,
        cat: Category,
        items: Vec<ItemView>,
        note: Option<String>,
    },
    /// A scan finished (or was cancelled: then `totals` is None).
    ScanDone {
        id: ScanId,
        estimate: bool,
        totals: Option<Vec<(Category, u64)>>,
    },
    Progress {
        done: usize,
        total: usize,
    },
    Cleaned {
        freed: u64,
        failed: usize,
        gone: Vec<PathBuf>,
    },
    Recycled {
        path: PathBuf,
        ok: bool,
    },
    /// A one-line status for the Sweep view.
    Status(String),
}

pub struct Sweeper {
    jobs: mpsc::Sender<Job>,
    results: mpsc::Receiver<Msg>,
    /// The cancel flag of the most recent scan.
    cancel: Arc<AtomicBool>,
    next_id: ScanId,
}

impl Sweeper {
    /// Starts the thread. It sleeps until the first job.
    pub fn start(hwnd: HWND, project_roots: Vec<PathBuf>, oplog: PathBuf) -> Self {
        let (jobs, job_rx) = mpsc::channel();
        let (tx, results) = mpsc::channel();
        let raw = hwnd.0 as isize;
        // If the thread cannot start, scans never report and the button stays "sweep".
        let _ = thread::Builder::new().name("sweep".into()).spawn(move || {
            let log = OpLog::new(oplog);
            while let Ok(job) = job_rx.recv() {
                let post = |m: Msg| {
                    if tx.send(m).is_ok() {
                        post_to(raw, WM_SWEEP);
                    }
                };
                run(job, &project_roots, &log, &post);
            }
        });
        Self {
            jobs,
            results,
            cancel: Arc::new(AtomicBool::new(false)),
            next_id: 0,
        }
    }

    /// Queues a scan (`preview` false: the estimate) and returns its id. Each scan has
    /// its own cancel flag, so cancelling one never touches the next.
    pub fn scan(&mut self, preview: bool) -> ScanId {
        self.next_id += 1;
        self.cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&self.cancel);
        let job = if preview {
            Job::Preview(self.next_id, flag)
        } else {
            Job::Estimate(self.next_id, flag)
        };
        let _ = self.jobs.send(job);
        self.next_id
    }

    /// Queues a clean or a quick action.
    pub fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    /// Stops the latest scan early (cleans are never interrupted).
    pub fn cancel_scan(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Results queued since the last call, oldest first.
    pub fn drain(&self) -> impl Iterator<Item = Msg> + '_ {
        self.results.try_iter()
    }
}

fn run(job: Job, roots: &[PathBuf], log: &OpLog, post: &dyn Fn(Msg)) {
    let ops = WinOps;
    let estimate = matches!(job, Job::Estimate(..));
    match job {
        Job::Estimate(id, cancel) | Job::Preview(id, cancel) => {
            let cancel = &*cancel;
            let env = ScanEnv::from_system(roots.to_vec());
            let mut totals = Vec::new();
            for cat in Category::ALL {
                if estimate && !cat.in_clean() {
                    continue;
                }
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                let scan = dj_sweep::scan(&env, cat, cancel);
                totals.push((cat, scan.total));
                if !estimate {
                    post(Msg::Category {
                        id,
                        cat,
                        items: scan
                            .items
                            .into_iter()
                            .map(|i| ItemView {
                                path: i.path,
                                bytes: i.bytes,
                                label: i.label,
                            })
                            .collect(),
                        note: scan.note,
                    });
                }
            }
            let complete = !cancel.load(Ordering::SeqCst);
            post(Msg::ScanDone {
                id,
                estimate,
                totals: complete.then_some(totals),
            });
        }
        Job::Clean(items) => {
            let env = ScanEnv::from_system(roots.to_vec());
            let run_id = dj_sweep::new_run_id();
            let mut last = Instant::now();
            let mut progress = |done: usize, total: usize| {
                if done == total || last.elapsed() >= PROGRESS_EVERY {
                    last = Instant::now();
                    post(Msg::Progress { done, total });
                }
            };
            let out = dj_sweep::clean(&env, &items, &ops, log, &run_id, &mut progress);
            post(Msg::Cleaned {
                freed: out.freed,
                failed: out.failed,
                gone: out.gone,
            });
        }
        Job::Recycle(path) => {
            let env = ScanEnv::from_system(roots.to_vec());
            let run_id = dj_sweep::new_run_id();
            let items = [(Category::BigFiles, path.clone())];
            let out = dj_sweep::clean(&env, &items, &ops, log, &run_id, &mut |_, _| {});
            let ok = out.failed == 0 && out.gone.contains(&path);
            post(Msg::Recycled { path, ok });
        }
        Job::Reveal(path) => {
            if ops.reveal(&path).is_err() {
                post(Msg::Status("cannot open explorer".into()));
            }
        }
        Job::FlushDns => {
            let run_id = dj_sweep::new_run_id();
            let target = PathBuf::from("DNS resolver cache");
            let text = match ops.flush_dns() {
                Ok(()) => {
                    log.record(&run_id, Action::Removed, &target, "flushed");
                    "dns cache flushed"
                }
                Err(e) => {
                    log.record(&run_id, Action::Failed, &target, &e.to_string());
                    "dns flush failed"
                }
            };
            post(Msg::Status(text.into()));
        }
        Job::EmptyBin => {
            let run_id = dj_sweep::new_run_id();
            let target = PathBuf::from("Recycle Bin");
            let text = match ops.empty_recycle_bin() {
                Ok(()) => {
                    log.record(&run_id, Action::Removed, &target, "emptied");
                    "recycle bin emptied"
                }
                Err(e) => {
                    log.record(&run_id, Action::Failed, &target, &e.to_string());
                    "could not empty the bin"
                }
            };
            post(Msg::Status(text.into()));
        }
    }
}
