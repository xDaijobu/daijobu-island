//! Pure Music model: the messages the YouTube Music page sends, the now-playing
//! snapshot the island shows, and the cover accent. No Win32, no WebView2.

use crate::color::Rgb;
use crate::island::MusicUi;
use serde::Deserialize;

/// One message from the page bridge (`bridge.js`) or from the engine.
#[derive(Clone, Debug, PartialEq)]
pub enum MusicMsg {
    State {
        title: String,
        artist: String,
        album: String,
        art: Option<String>,
        playing: bool,
        pos: f32,
        dur: f32,
    },
    Auth {
        signed_in: bool,
    },
    /// 22 log-spaced band levels in 0..1, or `None` when the analyser yields nothing.
    Bands(Option<Vec<f32>>),
    /// Google refused the embedded sign-in. Set by the engine from the page URL/title.
    Blocked(bool),
}

/// The JSON shape the bridge posts. Private so the public enum stays Rust-shaped.
#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum Wire {
    State {
        #[serde(default)]
        title: String,
        #[serde(default)]
        artist: String,
        #[serde(default)]
        album: String,
        #[serde(default)]
        art: Option<String>,
        #[serde(default)]
        playing: bool,
        #[serde(default)]
        pos: f32,
        #[serde(default)]
        dur: f32,
    },
    Auth {
        #[serde(rename = "signedIn")]
        signed_in: bool,
    },
    Bands {
        v: Option<Vec<f32>>,
    },
}

impl MusicMsg {
    /// Parses one bridge message. Malformed input yields `None`, never a panic.
    pub fn parse(json: &str) -> Option<Self> {
        let msg = match serde_json::from_str::<Wire>(json).ok()? {
            Wire::State {
                title,
                artist,
                album,
                art,
                playing,
                pos,
                dur,
            } => MusicMsg::State {
                title: title.trim().to_string(),
                artist: artist.trim().to_string(),
                album: album.trim().to_string(),
                art: art.filter(|u| u.starts_with("https://")),
                playing,
                pos: finite_or_zero(pos),
                dur: finite_or_zero(dur),
            },
            Wire::Auth { signed_in } => MusicMsg::Auth { signed_in },
            Wire::Bands { v } => MusicMsg::Bands(v.map(|v| {
                v.into_iter()
                    .map(|x| if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 })
                    .collect()
            })),
        };
        Some(msg)
    }
}

fn finite_or_zero(x: f32) -> f32 {
    if x.is_finite() {
        x.max(0.0)
    } else {
        0.0
    }
}

/// What is loaded in the page right now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art_url: Option<String>,
    pub playing: bool,
    pub pos: f32,
    pub dur: f32,
}

/// Everything the island knows about YouTube Music.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub signed_in: bool,
    pub blocked: bool,
    pub now: Option<NowPlaying>,
    pub bands: Option<Vec<f32>>,
}

impl Session {
    /// Applies one message. Returns true when the island needs a redraw.
    /// Band frames return false: the VU timer redraws them at about 30 fps.
    pub fn apply(&mut self, msg: MusicMsg) -> bool {
        match msg {
            MusicMsg::State {
                title,
                artist,
                album,
                art,
                playing,
                pos,
                dur,
            } => {
                self.now = (!title.is_empty()).then_some(NowPlaying {
                    title,
                    artist,
                    album,
                    art_url: art,
                    playing,
                    pos,
                    dur,
                });
                true
            }
            MusicMsg::Auth { signed_in } => {
                self.signed_in = signed_in;
                true
            }
            MusicMsg::Bands(v) => {
                self.bands = v;
                false
            }
            MusicMsg::Blocked(b) => {
                self.blocked = b;
                true
            }
        }
    }

    pub fn has_track(&self) -> bool {
        self.now.is_some()
    }

    /// True while the VU should be redrawn on a timer.
    pub fn vu_live(&self) -> bool {
        self.bands.is_some() && self.now.as_ref().is_some_and(|n| n.playing)
    }

    pub fn ui(&self) -> MusicUi {
        MusicUi {
            signed_in: self.signed_in,
            has_track: self.has_track(),
            blocked: self.blocked,
        }
    }
}

/// Averages `bands` into `n` groups (for the 5-column compact VU).
pub fn vu_groups(bands: &[f32], n: usize) -> Vec<f32> {
    if n == 0 || bands.is_empty() {
        return vec![0.0; n];
    }
    (0..n)
        .map(|g| {
            let lo = (g * bands.len() / n).min(bands.len() - 1);
            let hi = ((g + 1) * bands.len() / n).clamp(lo + 1, bands.len());
            bands[lo..hi].iter().sum::<f32>() / (hi - lo) as f32
        })
        .collect()
}

/// `m:ss` for a position in seconds.
pub fn fmt_time(secs: f32) -> String {
    let s = secs.max(0.0) as u32;
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

    const SAMPLE: &str = r#"{"t":"state","title":"Song","artist":"Artist","album":"Album",
        "art":"https://lh3.example/a.jpg","playing":true,"pos":12.4,"dur":200}"#;

    #[test]
    fn parses_state_with_all_fields() {
        let m = MusicMsg::parse(SAMPLE).expect("valid");
        assert_eq!(
            m,
            MusicMsg::State {
                title: "Song".into(),
                artist: "Artist".into(),
                album: "Album".into(),
                art: Some("https://lh3.example/a.jpg".into()),
                playing: true,
                pos: 12.4,
                dur: 200.0,
            }
        );
    }

    #[test]
    fn parses_auth_and_bands() {
        assert_eq!(
            MusicMsg::parse(r#"{"t":"auth","signedIn":false}"#),
            Some(MusicMsg::Auth { signed_in: false })
        );
        assert_eq!(
            MusicMsg::parse(r#"{"t":"bands","v":null}"#),
            Some(MusicMsg::Bands(None))
        );
        let Some(MusicMsg::Bands(Some(v))) = MusicMsg::parse(r#"{"t":"bands","v":[0.5,2,-1]}"#)
        else {
            panic!("bands expected");
        };
        assert_eq!(v, vec![0.5, 1.0, 0.0], "values are clamped to 0..1");
    }

    #[test]
    fn malformed_input_is_ignored() {
        assert_eq!(MusicMsg::parse(""), None);
        assert_eq!(MusicMsg::parse("not json"), None);
        assert_eq!(MusicMsg::parse(r#"{"t":"unknown"}"#), None);
        assert_eq!(MusicMsg::parse(r#"{"t":"auth"}"#), None, "missing field");
        assert_eq!(MusicMsg::parse(r#"{"t":"bands","v":"x"}"#), None);
        assert_eq!(MusicMsg::parse("[1,2,3]"), None);
    }

    #[test]
    fn state_defaults_and_guards() {
        let m = MusicMsg::parse(r#"{"t":"state","title":"  T  ","pos":-3,"art":"file:///x"}"#)
            .expect("valid");
        let MusicMsg::State {
            title,
            art,
            pos,
            dur,
            playing,
            ..
        } = m
        else {
            panic!("state expected");
        };
        assert_eq!(title, "T");
        assert_eq!(art, None, "only https artwork is accepted");
        assert_eq!((pos, dur, playing), (0.0, 0.0, false));
    }

    #[test]
    fn session_tracks_presence_and_auth() {
        let mut s = Session::default();
        assert!(!s.has_track());
        assert!(s.apply(MusicMsg::parse(SAMPLE).unwrap()));
        assert!(s.has_track());
        assert!(s.ui().has_track);
        // An empty title means the track is gone.
        s.apply(MusicMsg::parse(r#"{"t":"state","title":""}"#).unwrap());
        assert!(!s.has_track());
        s.apply(MusicMsg::Auth { signed_in: true });
        assert!(s.ui().signed_in);
    }

    #[test]
    fn bands_do_not_force_a_redraw_and_vu_needs_playing() {
        let mut s = Session::default();
        assert!(!s.apply(MusicMsg::Bands(Some(vec![0.1; 22]))));
        assert!(!s.vu_live(), "no track yet");
        s.apply(MusicMsg::parse(SAMPLE).unwrap());
        assert!(s.vu_live());
        s.apply(MusicMsg::Bands(None));
        assert!(!s.vu_live(), "analyser gave up: no VU");
    }

    #[test]
    fn vu_groups_average_evenly() {
        let bands: Vec<f32> = (0..22).map(|i| i as f32 / 21.0).collect();
        let g = vu_groups(&bands, 5);
        assert_eq!(g.len(), 5);
        assert!(g.windows(2).all(|w| w[0] < w[1]), "rising input stays rising");
        assert_eq!(vu_groups(&[], 5), vec![0.0; 5]);
        assert_eq!(vu_groups(&[0.5; 3], 5).len(), 5, "fewer bands than groups");
    }

    #[test]
    fn times_format_as_m_ss() {
        assert_eq!(fmt_time(0.0), "0:00");
        assert_eq!(fmt_time(65.9), "1:05");
        assert_eq!(fmt_time(-4.0), "0:00");
        assert_eq!(fmt_time(3599.0), "59:59");
    }

    #[test]
    fn hsl_roundtrip() {
        let c = Rgb::new(0.2, 0.5, 0.9);
        let (h, s, l) = rgb_to_hsl(c);
        let back = hsl_to_rgb(h, s, l);
        assert!((back.r - c.r).abs() < 1e-4);
        assert!((back.g - c.g).abs() < 1e-4);
        assert!((back.b - c.b).abs() < 1e-4);
    }

    #[test]
    fn cover_accent_is_clamped_to_saturation_and_lightness() {
        // A dark, fully saturated cover comes out lighter and less saturated.
        let out = cover_accent(Rgb::new(0.5, 0.0, 0.0), Rgb::new(0.0, 0.0, 0.0), 1.0);
        let (_, s, l) = rgb_to_hsl(out);
        assert!((0.45 - 1e-3..=0.65 + 1e-3).contains(&s), "s={s}");
        assert!((0.62 - 1e-3..=0.72 + 1e-3).contains(&l), "l={l}");
        // A near-white cover is pulled down into the lightness band.
        let out = cover_accent(Rgb::new(1.0, 1.0, 1.0), Rgb::new(0.0, 0.0, 0.0), 1.0);
        let (_, _, l2) = rgb_to_hsl(out);
        assert!((0.62 - 1e-3..=0.72 + 1e-3).contains(&l2), "l2={l2}");
    }

    #[test]
    fn cover_accent_amount_zero_keeps_theme_accent() {
        let base = Rgb::new(0.3, 0.6, 0.2);
        assert_eq!(cover_accent(Rgb::new(0.9, 0.1, 0.1), base, 0.0), base);
    }
}
