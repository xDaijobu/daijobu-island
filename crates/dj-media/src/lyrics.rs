//! Optional sequential LRCLIB lookups. Disabled means no network calls.
use super::{Signal, Update};
use dj_core::music::Session;
use serde::Deserialize;
use std::sync::{
    mpsc::{self, SyncSender},
    Arc, Mutex,
};
use std::{
    collections::VecDeque,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

pub(super) struct Worker {
    pending: Arc<Mutex<Option<Update>>>,
    ready: Arc<Mutex<Option<(Query, Lyrics)>>>,
    tx: SyncSender<()>,
}
impl Worker {
    pub fn start(
        signal: Signal,
        out: Arc<Mutex<Option<Update>>>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Option<Self> {
        let pending = Arc::new(Mutex::new(None::<Update>));
        let tasks = pending.clone();
        let ready = Arc::new(Mutex::new(None));
        let completed = ready.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("media-lyrics".into())
            .spawn(move || {
                let mut cache = Cache::default();
                while rx.recv().is_ok() {
                    if signal.stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let Some(mut update) = tasks.lock().unwrap_or_else(|e| e.into_inner()).take()
                    else {
                        continue;
                    };
                    if update.session.target.0 != signal.revision.load(Ordering::Acquire) {
                        continue;
                    }
                    update.lyrics = cache.get(&update.session, &signal);
                    if update.session.target.0 == signal.revision.load(Ordering::Acquire) {
                        if let Some(q) = query(
                            &update.session,
                            signal.lyrics_enabled.load(Ordering::Acquire),
                        ) {
                            *completed.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some((q, update.lyrics.clone()));
                        }
                    }
                    if super::publish(&signal, &out, update) {
                        wake();
                    }
                }
            })
            .ok()?;
        Some(Self { pending, ready, tx })
    }
    pub fn cached(&self, session: &Session, enabled: bool) -> Option<Lyrics> {
        let q = query(session, enabled)?;
        self.ready
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|(key, _)| key == &q)
            .map(|(_, lyrics)| lyrics.clone())
    }
    pub fn submit(&self, update: Update) {
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(update);
        let _ = self.tx.try_send(());
    }
}

const MAX_BODY: u64 = 256 * 1024;
const MAX_LINES: usize = 2000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LyricsState {
    #[default]
    Off,
    Loading,
    Ready,
    Unavailable,
    Offline,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lyrics {
    pub state: LyricsState,
    pub lines: Vec<(f32, String)>,
}
impl Lyrics {
    pub(crate) fn initial(enabled: bool) -> Self {
        Self {
            state: if enabled {
                LyricsState::Loading
            } else {
                LyricsState::Off
            },
            lines: Vec::new(),
        }
    }
    pub fn line(&self, position: f32) -> &str {
        if self.state != LyricsState::Ready {
            return match self.state {
                LyricsState::Off => "Lyrics off",
                LyricsState::Loading => "Looking for synced lyrics...",
                LyricsState::Offline => "Lyrics unavailable offline",
                _ => "No synced lyrics available",
            };
        }
        if !position.is_finite() {
            return "";
        }
        let i = self.lines.partition_point(|(t, _)| *t <= position);
        i.checked_sub(1).map_or("", |i| self.lines[i].1.as_str())
    }
}

fn parse(lrc: &str) -> Vec<(f32, String)> {
    let mut lines = Vec::new();
    if lrc.len() > MAX_BODY as usize {
        return lines;
    }
    for raw in lrc.lines().take(MAX_LINES) {
        let mut rest = raw;
        let mut times = Vec::new();
        while let Some(tail) = rest.strip_prefix('[') {
            let Some((stamp, tail)) = tail.split_once(']') else {
                break;
            };
            let Some((min, sec)) = stamp.split_once(':') else {
                break;
            };
            let (Ok(min), Ok(sec)) = (min.parse::<u32>(), sec.parse::<f32>()) else {
                break;
            };
            if min > 60 || !sec.is_finite() || !(0.0..60.0).contains(&sec) {
                break;
            }
            times.push(min as f32 * 60.0 + sec);
            rest = tail;
            if times.len() == 16 {
                break;
            }
        }
        let text = rest
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(256)
            .collect::<String>();
        for t in times {
            if lines.len() < MAX_LINES {
                lines.push((t, text.clone()));
            }
        }
    }
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    lines
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Query {
    title: String,
    artist: String,
    duration: u32,
}
fn query(session: &Session, enabled: bool) -> Option<Query> {
    if !enabled {
        return None;
    }
    let n = session.now.as_ref()?;
    if n.title.is_empty()
        || n.artist.is_empty()
        || !n.dur.is_finite()
        || !(1.0..=3600.0).contains(&n.dur)
    {
        return None;
    }
    Some(Query {
        title: n.title.clone(),
        artist: n.artist.clone(),
        duration: n.dur.round() as u32,
    })
}

#[derive(Default)]
pub(crate) struct Cache {
    entries: VecDeque<(Query, Instant, Lyrics)>,
    next_request: Option<Instant>,
    throttled: bool,
}
impl Cache {
    pub(super) fn get(&mut self, session: &Session, signal: &Signal) -> Lyrics {
        self.get_with(session, signal, |cache, query| cache.fetch(query))
    }
    fn get_with(
        &mut self,
        session: &Session,
        signal: &Signal,
        fetch: impl FnOnce(&mut Self, &Query) -> Lyrics,
    ) -> Lyrics {
        let enabled = signal.lyrics_enabled.load(Ordering::Acquire);
        let Some(q) = query(session, enabled) else {
            return Lyrics {
                state: if enabled {
                    LyricsState::Unavailable
                } else {
                    LyricsState::Off
                },
                ..Default::default()
            };
        };
        self.entries
            .retain(|(_, at, _)| at.elapsed() < Duration::from_secs(15 * 60));
        if let Some((_, _, result)) = self.entries.iter().find(|(key, _, _)| key == &q) {
            return result.clone();
        }
        if self.throttled || self.next_request.is_some_and(|at| Instant::now() < at) {
            return Lyrics {
                state: LyricsState::Unavailable,
                ..Default::default()
            };
        }
        if !signal.lyrics_enabled.load(Ordering::Acquire)
            || signal.stopped.load(Ordering::Acquire)
            || session.target.0 != signal.revision.load(Ordering::Acquire)
        {
            return Lyrics::default();
        }
        let result = fetch(self, &q);
        self.entries.push_back((q, Instant::now(), result.clone()));
        while self.entries.len() > 16 {
            self.entries.pop_front();
        }
        result
    }
    fn fetch(&mut self, q: &Query) -> Lyrics {
        self.next_request = Some(Instant::now() + Duration::from_secs(1));
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into();
        let response = agent
            .get("https://lrclib.net/api/get")
            .header(
                "User-Agent",
                "DaijobuIsland/0.3.1 (https://github.com/xDaijobu/daijobu-island)",
            )
            .query("track_name", &q.title)
            .query("artist_name", &q.artist)
            .query("duration", q.duration.to_string())
            .call();
        let Ok(mut response) = response else {
            return Lyrics {
                state: LyricsState::Offline,
                ..Default::default()
            };
        };
        if matches!(response.status().as_u16(), 429 | 503) {
            // No automatic retry. Suppress all further queries for Retry-After
            // plus deterministic positive jitter; unreasonable delays stay blocked.
            let delay = response
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(60);
            self.next_request =
                Instant::now().checked_add(Duration::from_secs(delay.saturating_add(1)));
            self.throttled = self.next_request.is_none();
        }
        let unavailable = || Lyrics {
            state: LyricsState::Unavailable,
            ..Default::default()
        };
        if response.status().as_u16() != 200 {
            return unavailable();
        }
        if !response
            .headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("application/json"))
        {
            return unavailable();
        }
        #[derive(Deserialize)]
        struct Record {
            #[serde(rename = "syncedLyrics")]
            synced: Option<String>,
        }
        let record = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_vec()
            .ok()
            .and_then(|body| serde_json::from_slice::<Record>(&body).ok());
        let lines = record
            .and_then(|r| r.synced)
            .map(|s| parse(&s))
            .unwrap_or_default();
        if lines.is_empty() {
            unavailable()
        } else {
            Lyrics {
                state: LyricsState::Ready,
                lines,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::music::NowPlaying;
    #[test]
    fn disabled_stale_and_offline_queries_use_no_network_or_busy_retry() {
        let signal = crate::tests::signal();
        let mut cache = Cache::default();
        let mut session = Session {
            target: dj_core::music::Target(1),
            now: Some(NowPlaying {
                title: "Synthetic".into(),
                artist: "Test".into(),
                dur: 100.0,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            cache
                .get_with(&session, &signal, |_, _| panic!(
                    "disabled lookup must not run"
                ))
                .state,
            LyricsState::Off
        );
        signal.lyrics_enabled.store(true, Ordering::Release);
        session.target.0 = 0;
        cache.get_with(&session, &signal, |_, _| {
            panic!("stale lookup must not run")
        });
        session.target.0 = 1;
        let offline = cache.get_with(&session, &signal, |_, _| Lyrics {
            state: LyricsState::Offline,
            ..Default::default()
        });
        assert_eq!(offline.state, LyricsState::Offline);
        assert_eq!(
            cache
                .get_with(&session, &signal, |_, _| panic!(
                    "offline result must be cached"
                ))
                .state,
            LyricsState::Offline
        );
        signal.lyrics_enabled.store(false, Ordering::Release);
        assert_eq!(
            cache
                .get_with(&session, &signal, |_, _| panic!(
                    "disabled cache must not start lookup"
                ))
                .state,
            LyricsState::Off
        );
    }
    #[test]
    fn disabled_and_unknown_metadata_never_create_query() {
        let s = Session {
            now: Some(NowPlaying {
                title: "Synthetic".into(),
                artist: "Test".into(),
                dur: 100.0,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(query(&s, false).is_none());
        assert!(query(&s, true).is_some());
        assert!(query(&Session::default(), true).is_none());
    }
    #[test]
    fn synthetic_lrc_bounds_multiple_stamps_and_time_selection() {
        let lines = parse("[00:02.5]Second synthetic line\n[00:01.00][00:03]Test line\n[99:99]Invalid\n[offset:123]Ignored");
        assert_eq!(lines.len(), 3);
        let l = Lyrics {
            state: LyricsState::Ready,
            lines,
        };
        assert_eq!(l.line(0.0), "");
        assert_eq!(l.line(1.1), "Test line");
        assert_eq!(l.line(2.6), "Second synthetic line");
        assert_eq!(l.line(f32::NAN), "");
        assert!(parse(&"x".repeat(MAX_BODY as usize + 1)).is_empty());
        assert!(parse("[00:NaN]Invalid").is_empty());
    }
    #[test]
    fn off_no_match_and_offline_are_distinct() {
        assert_eq!(Lyrics::default().line(1.0), "Lyrics off");
        assert_eq!(
            Lyrics {
                state: LyricsState::Offline,
                ..Default::default()
            }
            .line(1.0),
            "Lyrics unavailable offline"
        );
        assert_eq!(
            Lyrics {
                state: LyricsState::Unavailable,
                ..Default::default()
            }
            .line(1.0),
            "No synced lyrics available"
        );
    }
}
