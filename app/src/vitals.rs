//! The vitals worker: one thread that owns the sampler, samples at the cadence the UI asks
//! for, queues each reading and wakes the UI with `WM_VITALS`.
//!
//! Cadence: 1 Hz while the Vitals panel is open, every 30 s while the health lamp needs
//! it, and not at all while the island is hidden or the lamp is off.

use dj_core::vitals::Reading;
use dj_vitals::Sampler;
use dj_win::window::{post_to, WM_VITALS};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const PANEL_INTERVAL: Duration = Duration::from_secs(1);
const LAMP_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cadence {
    Off,
    /// Once every 30 s, for the idle lamp only.
    Lamp,
    /// Once a second, while the Vitals panel is open.
    Panel,
}

struct Shared {
    cadence: Mutex<Cadence>,
    /// Signalled on every cadence change, so the worker re-reads it without waiting out
    /// a long interval.
    changed: Condvar,
}

pub struct Worker {
    shared: Arc<Shared>,
    readings: mpsc::Receiver<Reading>,
    cadence: Cadence,
}

impl Worker {
    /// Starts the thread. `hwnd` is the overlay window's raw handle, used only to post.
    pub fn start(hwnd: isize) -> Self {
        let shared = Arc::new(Shared {
            cadence: Mutex::new(Cadence::Off),
            changed: Condvar::new(),
        });
        let (tx, rx) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        // If the thread cannot start, the pill just never gets a health reading.
        let _ = thread::Builder::new()
            .name("vitals".into())
            .spawn(move || run(&worker_shared, hwnd, &tx));
        Self {
            shared,
            readings: rx,
            cadence: Cadence::Off,
        }
    }

    pub fn set_cadence(&mut self, cadence: Cadence) {
        if cadence == self.cadence {
            return;
        }
        self.cadence = cadence;
        let mut current = lock(&self.shared.cadence);
        *current = cadence;
        self.shared.changed.notify_one();
    }

    /// Readings queued since the last call, oldest first.
    pub fn drain(&self) -> impl Iterator<Item = Reading> + '_ {
        self.readings.try_iter()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // A poisoned lock only means a sampler panicked; the cadence value is still valid.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn run(shared: &Shared, hwnd: isize, tx: &mpsc::Sender<Reading>) {
    let mut sampler = Sampler::new();
    // Time of the last sample. None until the first one, which waits a full second so
    // the CPU and net deltas cover a real interval.
    let mut last: Option<Instant> = None;
    loop {
        let mut current = lock(&shared.cadence);
        while *current == Cadence::Off {
            current = shared
                .changed
                .wait(current)
                .unwrap_or_else(|e| e.into_inner());
        }
        let interval = match *current {
            Cadence::Panel => PANEL_INTERVAL,
            _ => LAMP_INTERVAL,
        };
        // Due relative to the last sample, so opening the panel (or showing the island
        // after a long hide) samples at once instead of waiting out the old interval.
        let due = match last {
            Some(t) => interval.saturating_sub(t.elapsed()),
            None => PANEL_INTERVAL,
        };
        if !due.is_zero() {
            let (_guard, timeout) = shared
                .changed
                .wait_timeout(current, due)
                .unwrap_or_else(|e| e.into_inner());
            // A cadence change (or a spurious wake) re-evaluates with the new value.
            if !timeout.timed_out() {
                continue;
            }
        } else {
            drop(current);
        }
        last = Some(Instant::now());
        if tx.send(sampler.sample()).is_err() {
            return; // the app has gone
        }
        post_to(hwnd, WM_VITALS);
    }
}
