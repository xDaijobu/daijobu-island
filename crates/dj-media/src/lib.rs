//! Event-driven Windows GSMTC adapter. No player process or global media keys.
mod lyrics;
mod native;

use dj_core::music::{Command, Session, Target};
pub use lyrics::{Lyrics, LyricsState};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};

pub const YOUTUBE_URL: &str = "https://music.youtube.com/";
pub const SPOTIFY_URL: &str = "https://open.spotify.com/";

#[derive(Clone, Debug, Default)]
pub struct Update {
    pub session: Session,
    pub art: Option<Arc<[u8]>>,
    pub lyrics: Lyrics,
}

enum Request {
    Refresh,
    Control(Target, Command),
}

#[derive(Clone)]
struct Signal {
    tx: SyncSender<Request>,
    revision: Arc<AtomicU64>,
    stopped: Arc<AtomicBool>,
    lyrics_enabled: Arc<AtomicBool>,
    metadata_dirty: Arc<AtomicBool>,
    retry_requested: Arc<AtomicBool>,
    session_changed: Arc<AtomicBool>,
}
impl Signal {
    fn refresh(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        let _ = self.tx.try_send(Request::Refresh);
    }
    fn finish_metadata(&self, consumed: bool, published: bool) {
        if consumed && !published {
            self.metadata_dirty.store(true, Ordering::Release);
        }
    }
}

fn publish(signal: &Signal, out: &Mutex<Option<Update>>, update: Update) -> bool {
    let mut slot = out.lock().unwrap_or_else(|e| e.into_inner());
    if update.session.target.0 != signal.revision.load(Ordering::Acquire)
        || signal.stopped.load(Ordering::Acquire)
    {
        return false;
    }
    *slot = Some(update);
    true
}

pub struct Media {
    signal: Signal,
    latest: Arc<Mutex<Option<Update>>>,
}
impl Media {
    /// Wake must only enqueue an application event; it runs on the worker thread.
    pub fn start(lyrics: bool, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (tx, rx) = mpsc::sync_channel(16);
        let signal = Signal {
            tx,
            revision: Arc::new(AtomicU64::new(1)),
            stopped: Arc::new(AtomicBool::new(false)),
            lyrics_enabled: Arc::new(AtomicBool::new(lyrics)),
            metadata_dirty: Arc::new(AtomicBool::new(true)),
            retry_requested: Arc::new(AtomicBool::new(false)),
            session_changed: Arc::new(AtomicBool::new(false)),
        };
        let latest = Arc::new(Mutex::new(None));
        let out = latest.clone();
        let worker_signal = signal.clone();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let worker_wake = wake.clone();
        let failed = std::thread::Builder::new()
            .name("native-media".into())
            .spawn(move || {
                native::run(worker_signal, rx, out, worker_wake);
            })
            .is_err();
        if failed {
            *latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(Update {
                session: Session {
                    target: Target(1),
                    error: Some(
                        "Could not start Windows media worker. Reopen the app to retry.".into(),
                    ),
                    ..Default::default()
                },
                ..Default::default()
            });
            wake();
        }
        Self { signal, latest }
    }
    pub fn take(&self) -> Option<Update> {
        self.latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .filter(|u| u.session.target.0 == self.signal.revision.load(Ordering::Acquire))
    }
    pub fn command(&self, target: Target, command: Command) -> bool {
        target.0 == self.signal.revision.load(Ordering::Acquire)
            && self
                .signal
                .tx
                .try_send(Request::Control(target, command))
                .is_ok()
    }
    pub fn retry(&self) {
        self.signal.retry_requested.store(true, Ordering::Release);
        self.signal.metadata_dirty.store(true, Ordering::Release);
        self.signal.refresh();
    }
    pub fn set_lyrics(&self, enabled: bool) {
        if self.signal.lyrics_enabled.swap(enabled, Ordering::AcqRel) != enabled {
            self.signal.refresh();
        }
    }
}
impl Drop for Media {
    fn drop(&mut self) {
        self.signal.stopped.store(true, Ordering::Release);
        self.signal.refresh();
        // The worker revokes subscriptions after a bounded in-flight operation;
        // the UI never waits for a player or HTTP endpoint during shutdown.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn signal() -> Signal {
        let (tx, _) = mpsc::sync_channel(16);
        Signal {
            tx,
            revision: Arc::new(AtomicU64::new(1)),
            stopped: Arc::new(AtomicBool::new(false)),
            lyrics_enabled: Arc::new(AtomicBool::new(false)),
            metadata_dirty: Arc::new(AtomicBool::new(true)),
            retry_requested: Arc::new(AtomicBool::new(false)),
            session_changed: Arc::new(AtomicBool::new(false)),
        }
    }
    #[test]
    fn stale_art_and_lyrics_cannot_replace_new_session_or_shutdown() {
        let signal = signal();
        let out = Mutex::new(None);
        let update = Update {
            session: Session {
                target: Target(1),
                ..Default::default()
            },
            art: Some(vec![1, 2, 3].into()),
            ..Default::default()
        };
        assert!(publish(&signal, &out, update.clone()));
        signal.refresh();
        assert!(!publish(&signal, &out, update));
        let media = Media {
            signal,
            latest: Arc::new(out),
        };
        assert!(media.take().is_none());
        assert!(!media.command(Target(1), Command::PlayPause));
        media.signal.stopped.store(true, Ordering::Release);
        assert!(!publish(
            &media.signal,
            &media.latest,
            Update {
                session: Session {
                    target: Target(2),
                    ..Default::default()
                },
                ..Default::default()
            }
        ));
    }
    #[test]
    fn invalidated_metadata_fetch_remains_dirty_for_next_timeline_event() {
        let s = signal();
        let consumed = s.metadata_dirty.swap(false, Ordering::AcqRel);
        s.refresh();
        s.finish_metadata(consumed, false);
        assert!(s.metadata_dirty.swap(false, Ordering::AcqRel));
        s.finish_metadata(true, true);
        assert!(!s.metadata_dirty.load(Ordering::Acquire));
    }
    #[test]
    fn startup_error_with_current_generation_remains_visible() {
        let signal = signal();
        let update = Update {
            session: Session {
                target: Target(1),
                error: Some("Worker unavailable".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let media = Media {
            signal,
            latest: Arc::new(Mutex::new(Some(update))),
        };
        assert_eq!(
            media.take().unwrap().session.error.as_deref(),
            Some("Worker unavailable")
        );
    }
}
