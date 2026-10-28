//! Bounded listener coordination. Acquisition/normalization runs on one worker.
//! The controller increments generation before pause/disable/clear; a private
//! epoch also invalidates pending work on every configuration change.
use crate::{Captured, Generation, MonitorEvent, MonitorPolicy, ReplayTracker, Wake};
use dj_core::clipboard_types::{Content, MAX_RAM_BYTES};
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

const MAX_EVENTS: usize = 16;

pub struct Monitor {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) struct Shared {
    pub(crate) generation: Generation,
    pub(crate) replay: ReplayTracker,
    state: Mutex<State>,
    // Wake must be a short notification, normally PostMessage. Shutdown takes
    // this gate to fence even an already-running notification.
    wake: Mutex<Option<Wake>>,
}

struct State {
    live: bool,
    policy: MonitorPolicy,
    epoch: u64,
    baseline: u32,
    hwnd: isize,
    events: VecDeque<Queued>,
    bytes: usize,
}

struct Queued {
    generation: u64,
    epoch: u64,
    bytes: usize,
    event: MonitorEvent,
}

#[derive(Clone)]
pub(crate) struct Ticket {
    pub(crate) sequence: u32,
    pub(crate) generation: u64,
    epoch: u64,
    pub(crate) policy: MonitorPolicy,
}

impl Shared {
    fn new(generation: Generation, replay: ReplayTracker, wake: Wake) -> Self {
        Self {
            generation,
            replay,
            state: Mutex::new(State {
                live: true,
                policy: MonitorPolicy::default(),
                epoch: 0,
                baseline: 0,
                hwnd: 0,
                events: VecDeque::new(),
                bytes: 0,
            }),
            wake: Mutex::new(Some(wake)),
        }
    }

    pub(crate) fn live(&self) -> bool {
        lock(&self.state).live
    }

    pub(crate) fn attach(&self, hwnd: isize) -> bool {
        let mut state = lock(&self.state);
        if !state.live {
            return false;
        }
        state.hwnd = hwnd;
        true
    }

    pub(crate) fn detach(&self) {
        // Same lock as posting; clear BEFORE destroying the HWND.
        lock(&self.state).hwnd = 0;
    }

    pub(crate) fn ticket(&self, sequence: u32) -> Option<Ticket> {
        let state = lock(&self.state);
        (state.live
            && state.policy.enabled
            && !state.policy.paused
            && sequence != 0
            && sequence != state.baseline)
            .then(|| Ticket {
                sequence,
                generation: self.generation.load(Ordering::Acquire),
                epoch: state.epoch,
                policy: state.policy.clone(),
            })
    }

    pub(crate) fn valid(&self, ticket: &Ticket) -> bool {
        let state = lock(&self.state);
        state.live
            && state.policy.enabled
            && !state.policy.paused
            && state.epoch == ticket.epoch
            && self.generation.load(Ordering::Acquire) == ticket.generation
    }

    pub(crate) fn worker_error(&self, reason: &'static str) {
        let ticket = {
            let state = lock(&self.state);
            Ticket {
                sequence: 0,
                generation: self.generation.load(Ordering::Acquire),
                epoch: state.epoch,
                policy: state.policy.clone(),
            }
        };
        self.publish(&ticket, MonitorEvent::Error(reason.into()));
    }

    fn configure_with(
        &self,
        policy: MonitorPolicy,
        sequence: impl FnOnce() -> u32,
    ) -> Result<(), String> {
        if policy.excluded_sources.len() > 128
            || policy.excluded_sources.iter().any(|s| s.len() > 32_768)
        {
            return Err("Clipboard exclusion policy exceeds limits".into());
        }
        let mut state = lock(&self.state);
        if !state.live {
            return Err("Clipboard monitor is stopped".into());
        }
        // Linearize baseline and policy with ticket creation. No payload read.
        state.baseline = sequence();
        state.policy = policy;
        state.epoch = state.epoch.wrapping_add(1);
        state.events.clear();
        state.bytes = 0;
        if state.hwnd != 0 {
            crate::native::post_control(state.hwnd);
        }
        Ok(())
    }

    pub(crate) fn publish(&self, ticket: &Ticket, event: MonitorEvent) {
        let bytes = event_bytes(&event);
        let mut state = lock(&self.state);
        if !state.live
            || state.epoch != ticket.epoch
            || self.generation.load(Ordering::Acquire) != ticket.generation
        {
            return;
        }
        if bytes > MAX_RAM_BYTES {
            drop(state);
            self.publish(
                ticket,
                MonitorEvent::Skipped {
                    reason: "Normalized item exceeds limits".into(),
                    count: 1,
                },
            );
            return;
        }
        let mut skipped = 0u64;
        // Keep latest under count/byte overload; account for removed captures.
        while state.events.len() >= MAX_EVENTS - 1
            || state.bytes.saturating_add(bytes) > MAX_RAM_BYTES
        {
            let Some(old) = state.events.pop_front() else {
                break;
            };
            state.bytes -= old.bytes;
            skipped = skipped.saturating_add(match old.event {
                MonitorEvent::Captured(_) => 1,
                MonitorEvent::Skipped { count, .. } => count,
                MonitorEvent::Error(_) => 0,
            });
        }
        if skipped > 0 {
            state.events.push_back(Queued {
                generation: ticket.generation,
                epoch: ticket.epoch,
                bytes: 0,
                event: MonitorEvent::Skipped {
                    reason: "Clipboard updates coalesced".into(),
                    count: skipped,
                },
            });
        }
        state.bytes += bytes;
        state.events.push_back(Queued {
            generation: ticket.generation,
            epoch: ticket.epoch,
            bytes,
            event,
        });
        drop(state);
        let wake = lock(&self.wake);
        if self.live() {
            if let Some(wake) = wake.as_ref() {
                wake();
            }
        }
    }

    pub(crate) fn drain(&self) -> Vec<MonitorEvent> {
        let mut state = lock(&self.state);
        let generation = self.generation.load(Ordering::Acquire);
        let epoch = state.epoch;
        state.bytes = 0;
        state
            .events
            .drain(..)
            .filter(|q| q.generation == generation && q.epoch == epoch)
            .map(|q| q.event)
            .collect()
    }

    pub(crate) fn stop(&self) {
        {
            let mut state = lock(&self.state);
            if state.live {
                state.live = false;
                state.epoch = state.epoch.wrapping_add(1);
                state.events.clear();
                state.bytes = 0;
                if state.hwnd != 0 {
                    crate::native::post_control(state.hwnd);
                }
            }
        }
        lock(&self.wake).take();
    }
}

fn event_bytes(event: &MonitorEvent) -> usize {
    match event {
        MonitorEvent::Captured(Captured {
            payload, source, ..
        }) => {
            let content = match &payload.content {
                Content::Text { text, html, rtf } => text
                    .len()
                    .saturating_add(html.as_ref().map_or(0, Vec::len))
                    .saturating_add(rtf.as_ref().map_or(0, Vec::len)),
                Content::Image(image) => image
                    .bgra
                    .len()
                    .saturating_add(image.png.len())
                    .saturating_add(image.thumbnail.bgra.len()),
                Content::Files(paths) => {
                    paths.iter().fold(0usize, |n, p| n.saturating_add(p.len()))
                }
            };
            content.saturating_add(source.as_ref().map_or(0, String::len))
        }
        MonitorEvent::Skipped { .. } | MonitorEvent::Error(_) => 0,
    }
}

impl Monitor {
    pub fn new(generation: Generation, replay: ReplayTracker, wake: Wake) -> Result<Self, String> {
        let shared = Arc::new(Shared::new(generation, replay, wake));
        let worker = shared.clone();
        let (ready, response) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("clipboard-listener".into())
            .spawn(move || crate::native::run(worker, ready))
            .map_err(|_| "Could not start clipboard monitor".to_owned())?;
        let mut monitor = Self {
            shared,
            thread: Some(thread),
        };
        match response.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(monitor),
            Ok(Err(error)) => {
                monitor.shutdown();
                Err(error)
            }
            Err(_) => {
                monitor.shutdown();
                Err("Clipboard monitor startup timed out".into())
            }
        }
    }

    pub fn configure(&self, policy: MonitorPolicy) -> Result<(), String> {
        self.shared.configure_with(policy, crate::native::sequence)
    }

    pub fn drain(&mut self) -> Vec<MonitorEvent> {
        self.shared.drain()
    }

    pub fn shutdown(&mut self) {
        self.shared.stop();
        // Delayed rendering is not cancellable. Detach rather than joining it
        // indefinitely; its Arc state remains valid but can no longer publish.
        if let Some(thread) = self.thread.take() {
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use dj_core::clipboard_types::Payload;
    use std::sync::atomic::AtomicUsize;

    pub(crate) fn shared() -> Arc<Shared> {
        Arc::new(Shared::new(
            Arc::new(crate::GenerationGate::new(1)),
            ReplayTracker::default(),
            Arc::new(|| {}),
        ))
    }

    pub(crate) fn enabled(shared: &Shared, baseline: u32) {
        shared
            .configure_with(
                MonitorPolicy {
                    enabled: true,
                    ..Default::default()
                },
                || baseline,
            )
            .unwrap();
    }

    fn capture(ticket: &Ticket, bytes: usize) -> MonitorEvent {
        MonitorEvent::Captured(Captured {
            sequence: ticket.sequence,
            generation: ticket.generation,
            at_ms: 0,
            source: None,
            payload: Arc::new(Payload {
                no_cloud: false,
                content: Content::Text {
                    text: "x".repeat(bytes),
                    html: None,
                    rtf: None,
                },
            }),
        })
    }

    #[test]
    fn future_only_and_policy_epoch_invalidate_pending_work() {
        let shared = shared();
        assert!(shared.ticket(10).is_none());
        enabled(&shared, 10);
        assert!(shared.ticket(10).is_none());
        let ticket = shared.ticket(11).unwrap();
        enabled(&shared, 11);
        assert!(!shared.valid(&ticket));
        shared.publish(&ticket, capture(&ticket, 1));
        assert!(shared.drain().is_empty());
        assert!(shared.ticket(11).is_none());
    }

    #[test]
    fn pause_disable_and_resume_only_admit_future_sequences() {
        let shared = shared();
        enabled(&shared, 1);
        let pending = shared.ticket(2).unwrap();
        shared
            .configure_with(
                MonitorPolicy {
                    enabled: true,
                    paused: true,
                    ..Default::default()
                },
                || 2,
            )
            .unwrap();
        assert!(!shared.valid(&pending));
        assert!(shared.ticket(3).is_none());
        enabled(&shared, 3);
        assert!(shared.ticket(3).is_none());
        assert!(shared.ticket(4).is_some());
        shared
            .configure_with(MonitorPolicy::default(), || 4)
            .unwrap();
        assert!(shared.ticket(5).is_none());
        enabled(&shared, 5);
        assert!(shared.ticket(5).is_none());
        assert!(shared.ticket(6).is_some());
    }

    #[test]
    fn detached_endpoint_is_cleared_before_any_later_command() {
        let shared = shared();
        assert!(shared.attach(123)); // synthetic identity; never posted to
        shared.detach();
        assert_eq!(lock(&shared.state).hwnd, 0);
        enabled(&shared, 1); // must not call PostMessage with the detached identity
        shared.stop();
        assert_eq!(lock(&shared.state).hwnd, 0);
    }

    #[test]
    fn generation_invalidates_captured_and_status_events() {
        let shared = shared();
        enabled(&shared, 1);
        let ticket = shared.ticket(2).unwrap();
        shared.publish(&ticket, capture(&ticket, 1));
        shared.publish(&ticket, MonitorEvent::Error("synthetic".into()));
        shared.generation.fetch_add(1, Ordering::AcqRel);
        assert!(shared.drain().is_empty());
        assert!(!shared.valid(&ticket));
    }

    #[test]
    fn overload_is_bounded_and_keeps_latest_with_skip_accounting() {
        let shared = shared();
        enabled(&shared, 1);
        for sequence in 2..102 {
            let ticket = shared.ticket(sequence).unwrap();
            shared.publish(&ticket, capture(&ticket, 1024));
            assert!(lock(&shared.state).events.len() <= MAX_EVENTS);
        }
        let events = shared.drain();
        let captures = events
            .iter()
            .filter(|e| matches!(e, MonitorEvent::Captured(_)))
            .count();
        let skipped: u64 = events
            .iter()
            .map(|e| match e {
                MonitorEvent::Skipped { count, .. } => *count,
                _ => 0,
            })
            .sum();
        assert_eq!(captures as u64 + skipped, 100);
        assert!(matches!(events.last(), Some(MonitorEvent::Captured(c)) if c.sequence == 101));
    }

    #[test]
    fn byte_budget_coalesces_even_below_count_limit() {
        let shared = shared();
        enabled(&shared, 1);
        let ticket = shared.ticket(2).unwrap();
        {
            // Synthetic accounting avoids a huge fixture allocation.
            let mut state = lock(&shared.state);
            state.bytes = MAX_RAM_BYTES;
            state.events.push_back(Queued {
                generation: 1,
                epoch: ticket.epoch,
                bytes: MAX_RAM_BYTES,
                event: capture(&ticket, 1),
            });
        }
        shared.publish(&ticket, capture(&ticket, 1));
        assert_eq!(lock(&shared.state).bytes, 1);
        assert!(shared
            .drain()
            .iter()
            .any(|e| matches!(e, MonitorEvent::Skipped { count: 1, .. })));
    }

    #[test]
    fn shutdown_fences_wakes_and_releases_payloads() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let observed = wakes.clone();
        let shared = Shared::new(
            Arc::new(crate::GenerationGate::new(1)),
            ReplayTracker::default(),
            Arc::new(move || {
                observed.fetch_add(1, Ordering::Relaxed);
            }),
        );
        enabled(&shared, 1);
        let ticket = shared.ticket(2).unwrap();
        shared.publish(&ticket, capture(&ticket, 10));
        shared.stop();
        shared.publish(&ticket, capture(&ticket, 10));
        assert_eq!(wakes.load(Ordering::Relaxed), 1);
        assert!(shared.drain().is_empty());
        assert!(!shared.attach(123));
    }
}
