//! Platform-free native media snapshots, exact targets and bounded timeline math.
use crate::{color::Rgb, island::MusicUi};
use std::time::{Duration, Instant};

pub const MAX_ART_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_ART_DIMENSION: i32 = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Target(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub play: bool,
    pub pause: bool,
    pub previous: bool,
    pub next: bool,
    pub seek: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    PlayPause,
    Previous,
    Next,
    Seek(f32),
}

impl Capabilities {
    pub fn allows(self, cmd: Command, playing: bool) -> bool {
        match cmd {
            Command::PlayPause => {
                if playing {
                    self.pause
                } else {
                    self.play
                }
            }
            Command::Previous => self.previous,
            Command::Next => self.next,
            Command::Seek(f) => self.seek && f.is_finite() && (0.0..=1.0).contains(&f),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Timeline {
    pub start: f64,
    pub end: f64,
    pub position: f64,
    pub min_seek: f64,
    pub max_seek: f64,
    pub rate: f64,
    pub source_stamp: i64,
    pub source_position: f64,
}
impl Default for Timeline {
    fn default() -> Self {
        Self::new(0.0, 0.0, 0.0, 0.0, 0.0, 1.0)
    }
}
fn finite(x: f64) -> f64 {
    if x.is_finite() {
        x.clamp(0.0, i64::MAX as f64 / 10_000_000.0)
    } else {
        0.0
    }
}
impl Timeline {
    pub fn new(start: f64, end: f64, pos: f64, min: f64, max: f64, rate: f64) -> Self {
        let start = finite(start);
        let end = finite(end).max(start);
        let position = finite(pos).max(start);
        Self {
            start,
            end,
            position: if end > start {
                position.min(end)
            } else {
                position
            },
            min_seek: finite(min).max(start),
            max_seek: finite(max).min(end),
            rate: if rate.is_finite() {
                rate.clamp(0.0, 16.0)
            } else {
                1.0
            },
            source_stamp: 0,
            source_position: position,
        }
    }
    /// Preserve elapsed playback when a pause/rate event repeats the same OS
    /// timeline. A new source stamp or position (seek) replaces the old anchor.
    pub fn rebase(
        mut self,
        stamp: i64,
        age: f64,
        playing: bool,
        previous: Option<(&Self, bool, Duration)>,
    ) -> Self {
        self.source_stamp = stamp;
        let continuous = previous.filter(|(p, _, _)| {
            p.source_stamp == stamp
                && p.source_position == self.source_position
                && p.start == self.start
                && p.end == self.end
        });
        self.position = if let Some((p, was_playing, elapsed)) = continuous {
            self.start + p.position(elapsed, was_playing) as f64
        } else {
            self.position
                + if playing && age.is_finite() && (0.0..=86_400.0).contains(&age) {
                    age * self.rate
                } else {
                    0.0
                }
        };
        if self.end > self.start {
            self.position = self.position.clamp(self.start, self.end);
        }
        self
    }
    pub fn duration(&self) -> f32 {
        (self.end - self.start) as f32
    }
    pub fn position(&self, elapsed: Duration, playing: bool) -> f32 {
        let live = if playing {
            elapsed.as_secs_f64() * self.rate
        } else {
            0.0
        };
        let pos = self.position - self.start + live;
        if self.end > self.start {
            pos.clamp(0.0, self.end - self.start) as f32
        } else {
            finite(pos) as f32
        }
    }
    pub fn seek(&self, fraction: f32) -> Option<i64> {
        if !fraction.is_finite()
            || !(0.0..=1.0).contains(&fraction)
            || self.end <= self.start
            || self.max_seek < self.min_seek
        {
            return None;
        }
        Some(
            ((self.start + (self.end - self.start) * fraction as f64)
                .clamp(self.min_seek, self.max_seek)
                * 10_000_000.0)
                .min(i64::MAX as f64) as i64,
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub source: String,
    pub playing: bool,
    pub dur: f32,
    pub timeline: Timeline,
    pub controls: Capabilities,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub target: Target,
    pub now: Option<NowPlaying>,
    pub error: Option<String>,
    pub integration_required: bool,
    pub updated: Instant,
}
impl Default for Session {
    fn default() -> Self {
        Self {
            target: Target::default(),
            now: None,
            error: None,
            integration_required: false,
            updated: Instant::now(),
        }
    }
}
impl Session {
    pub fn ui(&self) -> MusicUi {
        MusicUi {
            integration_required: self.integration_required,
            has_track: self.now.is_some(),
            available: self.error.is_none(),
            controls: self
                .now
                .as_ref()
                .map_or(Capabilities::default(), |n| n.controls),
            playing: self.now.as_ref().is_some_and(|n| n.playing),
        }
    }
    pub fn position(&self) -> f32 {
        self.now.as_ref().map_or(0.0, |n| {
            n.timeline.position(self.updated.elapsed(), n.playing)
        })
    }
    pub fn accepts(&self, target: Target, cmd: Command) -> bool {
        self.target == target
            && self
                .now
                .as_ref()
                .is_some_and(|n| n.controls.allows(cmd, n.playing))
    }
}

pub fn artwork_allowed(bytes: usize, width: i32, height: i32) -> bool {
    bytes <= MAX_ART_BYTES
        && width > 0
        && height > 0
        && width <= MAX_ART_DIMENSION
        && height <= MAX_ART_DIMENSION
}
pub fn fmt_time(secs: f32) -> String {
    let s = if secs.is_finite() {
        secs.clamp(0.0, 359_999.0) as u32
    } else {
        0
    };
    format!("{}:{:02}", s / 60, s % 60)
}
/// Accent taken from cover art: the cover is clamped to S 45–65 % and L 62–72 %,
/// then mixed into the theme accent by `amount` (`accent_from_cover`).
pub fn cover_accent(cover: Rgb, base: Rgb, amount: f32) -> Rgb {
    let (h, s, l) = rgb_to_hsl(cover);
    let clamped = hsl_to_rgb(h, s.clamp(0.45, 0.65), l.clamp(0.62, 0.72));
    base.lerp(clamped, amount.clamp(0.0, 1.0))
}

fn rgb_to_hsl(c: Rgb) -> (f32, f32, f32) {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d <= f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == c.r {
        (c.g - c.b) / d + if c.g < c.b { 6.0 } else { 0.0 }
    } else if max == c.g {
        (c.b - c.r) / d + 2.0
    } else {
        (c.r - c.g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Rgb {
    if s <= 0.0 {
        return Rgb::new(l, l, l);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    Rgb::new(
        hue_to_channel(p, q, h + 1.0 / 3.0),
        hue_to_channel(p, q, h),
        hue_to_channel(p, q, h - 1.0 / 3.0),
    )
}

fn hue_to_channel(p: f32, q: f32, t: f32) -> f32 {
    let t = if t < 0.0 {
        t + 1.0
    } else if t > 1.0 {
        t - 1.0
    } else {
        t
    };
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pause_seek_end_unknown_and_invalid_timeline() {
        let t = Timeline::new(10.0, 110.0, 30.0, 20.0, 90.0, 1.0);
        assert_eq!(t.position(Duration::from_secs(7), false), 20.0);
        assert_eq!(t.position(Duration::from_secs(7), true), 27.0);
        assert_eq!(t.position(Duration::from_secs(200), true), 100.0);
        assert_eq!(t.seek(0.0), Some(200_000_000));
        assert_eq!(t.seek(1.0), Some(900_000_000));
        assert_eq!(t.seek(f32::NAN), None);
        assert_eq!(Timeline::default().seek(0.5), None);
        let t = Timeline::new(f64::NAN, -1.0, f64::INFINITY, 0.0, 0.0, f64::NAN);
        assert_eq!(t.position(Duration::ZERO, true), 0.0);
    }
    #[test]
    fn stale_target_unsupported_controls_and_no_session() {
        let mut s = Session::default();
        assert!(!s.ui().has_track);
        assert!(s.ui().available);
        assert!(!s.accepts(Target(0), Command::Next));
        s.target = Target(2);
        s.now = Some(NowPlaying {
            controls: Capabilities {
                next: true,
                ..Default::default()
            },
            ..Default::default()
        });
        assert!(!s.accepts(Target(1), Command::Next));
        assert!(s.accepts(Target(2), Command::Next));
        assert!(!s.accepts(Target(2), Command::PlayPause));
        s.now = None;
        assert!(!s.accepts(Target(2), Command::Next));
        s.error = Some("Unavailable".into());
        assert!(!s.ui().available);
    }
    #[test]
    fn thumbnail_byte_and_dimension_caps() {
        assert!(artwork_allowed(MAX_ART_BYTES, 256, 256));
        assert!(!artwork_allowed(MAX_ART_BYTES + 1, 256, 256));
        assert!(!artwork_allowed(100, 4097, 20));
        assert!(!artwork_allowed(100, 0, 20));
    }
    #[test]
    fn repeated_os_anchor_freezes_on_pause_and_external_seek_replaces_it() {
        let playing = Timeline::new(0.0, 100.0, 10.0, 0.0, 100.0, 1.0).rebase(123, 2.0, true, None);
        let paused = Timeline::new(0.0, 100.0, 10.0, 0.0, 100.0, 1.0).rebase(
            123,
            5.0,
            false,
            Some((&playing, true, Duration::from_secs(3))),
        );
        assert_eq!(paused.position(Duration::from_secs(10), false), 15.0);
        let repeat = Timeline::new(0.0, 100.0, 10.0, 0.0, 100.0, 1.0).rebase(
            123,
            20.0,
            false,
            Some((&paused, false, Duration::from_secs(10))),
        );
        assert_eq!(repeat.position(Duration::ZERO, false), 15.0);
        let seek = Timeline::new(0.0, 100.0, 70.0, 0.0, 100.0, 1.0).rebase(
            124,
            0.0,
            false,
            Some((&repeat, false, Duration::from_secs(10))),
        );
        assert_eq!(seek.position(Duration::ZERO, false), 70.0);
        let resumed = Timeline::new(0.0, 100.0, 70.0, 0.0, 100.0, 2.0).rebase(
            124,
            100.0,
            true,
            Some((&seek, false, Duration::from_secs(10))),
        );
        assert_eq!(resumed.position(Duration::from_secs(2), true), 74.0);
    }
    #[test]
    fn time_and_accent_remain_safe() {
        assert_eq!(fmt_time(f32::NAN), "0:00");
        assert_eq!(fmt_time(65.9), "1:05");
        let base = Rgb::new(0.3, 0.6, 0.2);
        assert_eq!(cover_accent(Rgb::new(0.9, 0.1, 0.1), base, 0.0), base);
    }
}
