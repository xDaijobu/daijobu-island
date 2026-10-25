//! The vitals worker: one thread that owns the sampler. The UI thread decides when to
//! sample with a coalescable window timer and pokes the worker. The worker queues each
//! reading and wakes the UI with `WM_VITALS`.
//!
//! Cadence: 1 Hz while the Vitals panel is open, every 30 s while the health lamp needs
//! it, and not at all while the island is hidden or the lamp is off.

use dj_core::vitals::Reading;
use dj_vitals::Sampler;
use dj_win::window::{self, post_to, WM_VITALS};
use dj_win::HWND;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Window timer id for the sampling cadence (app timer ids stay below 0xD000).
pub const TIMER: usize = 3;

const PANEL_MS: u32 = 1_000;
const LAMP_MS: u32 = 30_000;
/// How late Windows may fire each tick so it can batch it with other timers.
const PANEL_TOLERANCE_MS: u32 = 100;
const LAMP_TOLERANCE_MS: u32 = 3_000;
/// The CPU and net deltas need a real interval, so the first sample waits this long
/// after the sampler's baseline.
const MIN_FIRST_GAP: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cadence {
    Off,
    /// Once every 30 s, for the idle lamp only.
    Lamp,
    /// Once a second, while the Vitals panel is open.
    Panel,
}

impl Cadence {
    /// What to sample for what is on screen: 1 Hz with the panel open, every 30 s for
    /// the lamp, nothing while the island is hidden or with the lamp off.
    pub fn for_view(visible: bool, panel_open: bool, health_lamp: bool) -> Self {
        if !visible {
            Cadence::Off
        } else if panel_open {
            Cadence::Panel
        } else if health_lamp {
            Cadence::Lamp
        } else {
            Cadence::Off
        }
    }

    fn timer(self) -> Option<(u32, u32)> {
        match self {
            Cadence::Off => None,
            Cadence::Lamp => Some((LAMP_MS, LAMP_TOLERANCE_MS)),
            Cadence::Panel => Some((PANEL_MS, PANEL_TOLERANCE_MS)),
        }
    }
}

/// One pending sample request. Requests made while one is pending coalesce.
#[derive(Default)]
struct Shared {
    pending: Mutex<bool>,
    changed: Condvar,
}

pub struct Worker {
    hwnd: HWND,
    shared: Arc<Shared>,
    readings: mpsc::Receiver<Reading>,
    cadence: Cadence,
    /// When the last sample was requested. None before the first one.
    last: Option<Instant>,
}

impl Worker {
    /// Starts the thread. It sleeps until the first request.
    pub fn start(hwnd: HWND) -> Self {
        let shared = Arc::new(Shared::default());
        let (tx, rx) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let raw = hwnd.0 as isize;
        // If the thread cannot start, the pill just never gets a health reading.
        let _ = thread::Builder::new()
            .name("vitals".into())
            .spawn(move || run(&worker_shared, raw, &tx));
        Self {
            hwnd,
            shared,
            readings: rx,
            cadence: Cadence::Off,
            last: None,
        }
    }

    /// Re-arms the timer for a new cadence. When the last sample is older than the new
    /// interval (opening the panel, or showing the island after a long hide), it also
    /// samples at once instead of waiting a full tick.
    pub fn set_cadence(&mut self, cadence: Cadence) {
        if cadence == self.cadence {
            return;
        }
        self.cadence = cadence;
        match cadence.timer() {
            None => window::kill_timer(self.hwnd, TIMER),
            Some((ms, tolerance)) => {
                window::set_coalescable_timer(self.hwnd, TIMER, ms, tolerance);
                let interval = Duration::from_millis(u64::from(ms));
                if self.last.is_none_or(|t| t.elapsed() >= interval) {
                    self.request();
                }
            }
        }
    }

    /// Timer tick: asks the worker for one sample.
    pub fn tick(&mut self) {
        if self.cadence != Cadence::Off {
            self.request();
        }
    }

    fn request(&mut self) {
        self.last = Some(Instant::now());
        *lock(&self.shared.pending) = true;
        self.shared.changed.notify_one();
    }

    /// Readings queued since the last call, oldest first.
    pub fn drain(&self) -> impl Iterator<Item = Reading> + '_ {
        self.readings.try_iter()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // A poisoned lock only means a sampler panicked; the flag is still valid.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn run(shared: &Shared, hwnd: isize, tx: &mpsc::Sender<Reading>) {
    let mut sampler = Sampler::new();
    let baseline = Instant::now();
    let mut first = true;
    loop {
        {
            let mut pending = lock(&shared.pending);
            while !*pending {
                pending = shared
                    .changed
                    .wait(pending)
                    .unwrap_or_else(|e| e.into_inner());
            }
            *pending = false;
        }
        if first {
            first = false;
            thread::sleep(MIN_FIRST_GAP.saturating_sub(baseline.elapsed()));
        }
        if tx.send(sampler.sample()).is_err() {
            return; // the app has gone
        }
        post_to(hwnd, WM_VITALS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadence_follows_visibility_panel_and_lamp() {
        assert_eq!(Cadence::for_view(false, true, true), Cadence::Off);
        assert_eq!(Cadence::for_view(false, false, true), Cadence::Off);
        assert_eq!(Cadence::for_view(true, true, false), Cadence::Panel);
        assert_eq!(Cadence::for_view(true, false, true), Cadence::Lamp);
        assert_eq!(Cadence::for_view(true, false, false), Cadence::Off);
    }

    #[test]
    fn timer_intervals() {
        assert_eq!(Cadence::Off.timer(), None);
        assert_eq!(Cadence::Panel.timer().map(|t| t.0), Some(1_000));
        assert_eq!(Cadence::Lamp.timer().map(|t| t.0), Some(30_000));
    }
}
