use super::{lyrics, Request, Signal, Update};
use dj_core::music::{Capabilities, Command, NowPlaying, Session, Target, Timeline, MAX_ART_BYTES};
use std::sync::{
    atomic::Ordering,
    mpsc::{self, Receiver},
    Arc, Mutex,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::core::{Error, Result, RuntimeType};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as NativeSession,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::{Buffer, DataReader, InputStreamOptions};
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
use windows_future::{AsyncOperationCompletedHandler, IAsyncOperation};

struct Apartment;
impl Apartment {
    fn new() -> Result<Self> {
        // SAFETY: initializes this worker's apartment, balanced on the same thread.
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: balances this worker's successful RoInitialize.
        unsafe {
            RoUninitialize();
        }
    }
}

fn timeout() -> Error {
    Error::new(
        windows::core::HRESULT(0x800705B4u32 as i32),
        "Windows media request timed out",
    )
}

/// Completion callbacks wake a bounded channel. Never spin on a WinRT operation.
fn wait<T: RuntimeType + 'static>(op: IAsyncOperation<T>) -> Result<T> {
    let (tx, rx) = mpsc::sync_channel(1);
    op.SetCompleted(&AsyncOperationCompletedHandler::new(move |_, _| {
        let _ = tx.try_send(());
        Ok(())
    }))?;
    if rx.recv_timeout(Duration::from_secs(5)).is_err() {
        let _ = op.Cancel();
        return Err(timeout());
    }
    op.GetResults()
}

struct ManagerEvents {
    manager: Manager,
    current: Option<i64>,
    sessions: Option<i64>,
}
impl ManagerEvents {
    fn new(manager: Manager, signal: &Signal) -> Result<Self> {
        let mut events = Self {
            manager,
            current: None,
            sessions: None,
        };
        let s = signal.clone();
        events.current = Some(
            events
                .manager
                .CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
                    s.session_changed.store(true, Ordering::Release);
                    s.refresh();
                    Ok(())
                }))?,
        );
        let s = signal.clone();
        events.sessions = Some(events.manager.SessionsChanged(&TypedEventHandler::new(
            move |_, _| {
                s.session_changed.store(true, Ordering::Release);
                s.refresh();
                Ok(())
            },
        ))?);
        Ok(events)
    }
}
impl Drop for ManagerEvents {
    fn drop(&mut self) {
        if let Some(t) = self.current {
            let _ = self.manager.RemoveCurrentSessionChanged(t);
        }
        if let Some(t) = self.sessions {
            let _ = self.manager.RemoveSessionsChanged(t);
        }
    }
}
struct SessionEvents {
    session: NativeSession,
    tokens: [Option<i64>; 3],
}
impl SessionEvents {
    fn new(session: NativeSession, signal: &Signal) -> Result<Self> {
        let mut events = Self {
            session,
            tokens: [None; 3],
        };
        events.tokens[0] = Some(
            events
                .session
                .MediaPropertiesChanged(&session_handler(signal, true))?,
        );
        events.tokens[1] = Some(
            events
                .session
                .PlaybackInfoChanged(&session_handler(signal, false))?,
        );
        events.tokens[2] = Some(
            events
                .session
                .TimelinePropertiesChanged(&session_handler(signal, false))?,
        );
        Ok(events)
    }
}
fn session_handler<T: RuntimeType + 'static>(
    signal: &Signal,
    metadata: bool,
) -> TypedEventHandler<NativeSession, T> {
    let s = signal.clone();
    TypedEventHandler::new(move |_, _| {
        if metadata {
            s.metadata_dirty.store(true, Ordering::Release);
            s.session_changed.store(true, Ordering::Release);
        }
        s.refresh();
        Ok(())
    })
}
impl Drop for SessionEvents {
    fn drop(&mut self) {
        if let Some(t) = self.tokens[0] {
            let _ = self.session.RemoveMediaPropertiesChanged(t);
        }
        if let Some(t) = self.tokens[1] {
            let _ = self.session.RemovePlaybackInfoChanged(t);
        }
        if let Some(t) = self.tokens[2] {
            let _ = self.session.RemoveTimelinePropertiesChanged(t);
        }
    }
}

fn bounded(s: String) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(256)
        .collect::<String>()
        .trim()
        .to_string()
}
fn caps(session: &NativeSession) -> Result<(Capabilities, bool, f64)> {
    let info = session.GetPlaybackInfo()?;
    let c = info.Controls()?;
    let playing = info.PlaybackStatus()? == Status::Playing;
    let rate = info.PlaybackRate().and_then(|r| r.Value()).unwrap_or(1.0);
    Ok((
        Capabilities {
            play: c.IsPlayEnabled()?,
            pause: c.IsPauseEnabled()?,
            previous: c.IsPreviousEnabled()?,
            next: c.IsNextEnabled()?,
            seek: c.IsPlaybackPositionEnabled()?,
        },
        playing,
        rate,
    ))
}
fn timeline(
    session: &NativeSession,
    rate: f64,
    playing: bool,
    previous: Option<&Session>,
) -> Result<Timeline> {
    let t = session.GetTimelineProperties()?;
    let seconds = |ticks: i64| ticks as f64 / 10_000_000.0;
    let updated = t.LastUpdatedTime()?.UniversalTime;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        + 11_644_473_600.0;
    // Windows timestamps are UTC FILETIME units. Reject missing/future/ancient
    // timestamps instead of extrapolating a stopped or broken source for years.
    let age = now - seconds(updated);
    let old = previous.and_then(|p| {
        p.now
            .as_ref()
            .map(|n| (&n.timeline, n.playing, p.updated.elapsed()))
    });
    Ok(Timeline::new(
        seconds(t.StartTime()?.Duration),
        seconds(t.EndTime()?.Duration),
        seconds(t.Position()?.Duration),
        seconds(t.MinSeekTime()?.Duration),
        seconds(t.MaxSeekTime()?.Duration),
        rate,
    )
    .rebase(updated, age, playing, old))
}
fn snapshot(
    session: &NativeSession,
    target: Target,
    previous: Option<&Update>,
    metadata: bool,
) -> Result<Update> {
    let status = session.GetPlaybackInfo()?.PlaybackStatus()?;
    if status == Status::Closed || status == Status::Stopped {
        return Ok(Update {
            session: Session {
                target,
                ..Default::default()
            },
            ..Default::default()
        });
    }
    let props = if metadata || previous.and_then(|p| p.session.now.as_ref()).is_none() {
        Some(wait(session.TryGetMediaPropertiesAsync()?)?)
    } else {
        None
    };
    let (mut controls, playing, rate) = caps(session)?;
    let timeline = timeline(
        session,
        rate,
        playing,
        previous.filter(|_| !metadata).map(|p| &p.session),
    )?;
    controls.seek &= timeline.seek(0.5).is_some();
    let mut now = previous
        .and_then(|p| p.session.now.clone())
        .unwrap_or_default();
    if let Some(props) = &props {
        now.title = bounded(props.Title()?.to_string_lossy());
        now.artist = bounded(props.Artist()?.to_string_lossy());
        now.album = bounded(props.AlbumTitle()?.to_string_lossy());
        now.source = bounded(session.SourceAppUserModelId()?.to_string_lossy());
    }
    now.playing = playing;
    now.dur = timeline.duration();
    now.timeline = timeline;
    now.controls = controls;
    // Empty titles still represent a real controllable session; show a fallback.
    let now = NowPlaying {
        title: if now.title.is_empty() {
            "Untitled media".into()
        } else {
            now.title
        },
        ..now
    };
    let updated = Instant::now();
    let art = if let Some(props) = &props {
        (|| -> Result<Vec<u8>> {
            let stream = wait(props.Thumbnail()?.OpenReadAsync()?)?;
            let size = stream.Size()?;
            if size == 0 || size > MAX_ART_BYTES as u64 {
                return Err(timeout());
            }
            let input = stream.GetInputStreamAt(0)?;
            let buffer = Buffer::Create(size as u32)?;
            // ReadAsync returns an operation with progress; its join is bounded by
            // a completion notification just like normal operations.
            let op = input.ReadAsync(&buffer, size as u32, InputStreamOptions::None)?;
            let (tx, rx) = mpsc::sync_channel(1);
            op.SetCompleted(
                &windows_future::AsyncOperationWithProgressCompletedHandler::new(move |_, _| {
                    let _ = tx.try_send(());
                    Ok(())
                }),
            )?;
            if rx.recv_timeout(Duration::from_secs(5)).is_err() {
                let _ = op.Cancel();
                return Err(timeout());
            }
            let buffer = op.GetResults()?;
            let reader = DataReader::FromBuffer(&buffer)?;
            let mut bytes =
                vec![0; reader.UnconsumedBufferLength()?.min(MAX_ART_BYTES as u32) as usize];
            reader.ReadBytes(&mut bytes)?;
            Ok(bytes)
        })()
        .ok()
        .map(Arc::from)
    } else {
        previous.and_then(|p| p.art.clone())
    };
    Ok(Update {
        session: Session {
            target,
            now: Some(now),
            error: None,
            integration_required: false,
            updated,
        },
        art,
        ..Default::default()
    })
}

fn control(session: &NativeSession, command: Command, valid: impl Fn() -> bool) -> Result<bool> {
    let (controls, playing, rate) = caps(session)?;
    if !controls.allows(command, playing) {
        return Ok(false);
    }
    let seek = if let Command::Seek(fraction) = command {
        let Some(ticks) = timeline(session, rate, playing, None)?.seek(fraction) else {
            return Ok(false);
        };
        Some(ticks)
    } else {
        None
    };
    // Revalidate after reading fresh capabilities/timeline, immediately before
    // starting the operation on this retained exact COM session.
    if !valid() {
        return Ok(false);
    }
    let operation = match command {
        Command::PlayPause if playing => session.TryPauseAsync()?,
        Command::PlayPause => session.TryPlayAsync()?,
        Command::Previous => session.TrySkipPreviousAsync()?,
        Command::Next => session.TrySkipNextAsync()?,
        Command::Seek(_) => session.TryChangePlaybackPositionAsync(seek.unwrap_or_default())?,
    };
    wait(operation)
}

pub(super) fn run(
    signal: Signal,
    rx: Receiver<Request>,
    out: Arc<Mutex<Option<Update>>>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    let lyrics = lyrics::Worker::start(signal.clone(), out.clone(), wake.clone());
    let mut previous = None;
    let mut manager: Option<ManagerEvents> = None;
    let mut current: Option<SessionEvents> = None;
    let mut last = 0;
    let apartment = Apartment::new();
    let mut command_error = None;
    let mut failure: Option<Error> = None;
    loop {
        if signal.stopped.load(Ordering::Acquire) {
            break;
        }
        let revision = signal.revision.load(Ordering::Acquire);
        if revision != last {
            last = revision;
            let retry = signal.retry_requested.swap(false, Ordering::AcqRel);
            let changed = signal.session_changed.swap(false, Ordering::AcqRel);
            if retry || (changed && failure.is_some()) {
                failure = None;
                current = None;
                if retry {
                    manager = None;
                }
                previous = None;
            }
            let mut metadata_consumed = false;
            let result = (|| -> Result<Update> {
                // Do not repeatedly start timed-out/failed OS operations on a
                // stream of timeline events. A session/metadata change or explicit
                // Retry reopens discovery without a timer or busy loop.
                if let Some(error) = &failure {
                    return Err(error.clone());
                }
                if let Err(e) = &apartment {
                    return Err(e.clone());
                }
                if manager.is_none() {
                    manager = Some(ManagerEvents::new(
                        wait(Manager::RequestAsync()?)?,
                        &signal,
                    )?);
                }
                let m = &manager.as_ref().unwrap().manager;
                let next = match m.GetCurrentSession() {
                    Ok(s) => Some(s),
                    Err(e) if e.code().0 == 0x80004003u32 as i32 => None,
                    Err(e) => return Err(e),
                };
                if current.as_ref().map(|s| &s.session) != next.as_ref() {
                    current = None;
                    previous = None;
                    if let Some(s) = next {
                        current = Some(SessionEvents::new(s, &signal)?);
                    }
                }
                let Some(current) = &current else {
                    return Ok(Update {
                        session: Session {
                            target: Target(revision),
                            ..Default::default()
                        },
                        ..Default::default()
                    });
                };
                metadata_consumed = signal.metadata_dirty.swap(false, Ordering::AcqRel);
                snapshot(
                    &current.session,
                    Target(revision),
                    previous.as_ref(),
                    metadata_consumed,
                )
            })();
            if let Err(error) = &result {
                failure = Some(error.clone());
            }
            let succeeded = result.is_ok();
            let mut update = result.unwrap_or_else(|e| Update { session: Session { target: Target(revision), integration_required: e.code().0 == 0x80070005u32 as i32, error: Some(format!("Windows media unavailable ({:#010X}). Retry, or update Windows integration if this installation has an older package.", e.code().0 as u32)), ..Default::default() }, ..Default::default() });
            if let Some(error) = command_error.take() {
                update.session.error = Some(error);
            }
            if signal.revision.load(Ordering::Acquire) == revision
                && !signal.stopped.load(Ordering::Acquire)
            {
                // Publish controls/art promptly; optional HTTP must not delay metadata.
                update.lyrics =
                    lyrics::Lyrics::initial(signal.lyrics_enabled.load(Ordering::Acquire));
                if let Some(cached) = lyrics.as_ref().and_then(|w| {
                    w.cached(
                        &update.session,
                        signal.lyrics_enabled.load(Ordering::Acquire),
                    )
                }) {
                    update.lyrics = cached;
                }
                if lyrics.is_none() && signal.lyrics_enabled.load(Ordering::Acquire) {
                    update.lyrics.state = lyrics::LyricsState::Unavailable;
                }
                let published = super::publish(&signal, &out, update.clone());
                signal.finish_metadata(metadata_consumed, published && succeeded);
                if published {
                    wake();
                    previous = Some(update.clone());
                    if let Some(lyrics) = &lyrics
                        .as_ref()
                        .filter(|_| signal.lyrics_enabled.load(Ordering::Acquire))
                    {
                        lyrics.submit(update);
                    }
                }
            } else {
                signal.finish_metadata(metadata_consumed, false);
            }
        }
        // Drain bounded commands; refresh signals can coalesce while operations
        // are pending because revision is authoritative, even when the queue fills.
        if signal.revision.load(Ordering::Acquire) != last {
            continue;
        }
        let Ok(request) = rx.recv() else {
            break;
        };
        if let Request::Control(target, command) = request {
            let exact = current.as_ref().filter(|s| {
                target.0 == signal.revision.load(Ordering::Acquire)
                    && manager
                        .as_ref()
                        .and_then(|m| m.manager.GetCurrentSession().ok())
                        .as_ref()
                        == Some(&s.session)
            });
            let result = exact.map(|s| {
                control(&s.session, command, || {
                    target.0 == signal.revision.load(Ordering::Acquire)
                        && manager
                            .as_ref()
                            .and_then(|m| m.manager.GetCurrentSession().ok())
                            .as_ref()
                            == Some(&s.session)
                })
            });
            command_error = match result {
                Some(Ok(true)) => None,
                Some(Ok(false)) => {
                    Some("This player does not support that control or declined it.".into())
                }
                Some(Err(_)) => Some(
                    "The player could not complete that control. Retry when it is available."
                        .into(),
                ),
                None => Some(
                    "The media session changed. Try the current session's control again.".into(),
                ),
            };
            signal.refresh();
        }
    }
    drop(current);
    drop(manager);
    drop(apartment);
}
