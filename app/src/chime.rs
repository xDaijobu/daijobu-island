//! The session-end chime: a short, soft bell synthesised in code (two sine partials
//! with an exponential decay), built once into an in-memory WAV and played through
//! `dj_win::sound`. No audio asset ships with the app.

use std::f32::consts::TAU;
use std::sync::OnceLock;

/// 44.1 kHz mono 16-bit PCM.
pub const RATE: u32 = 44_100;
/// About 0.8 s, by which point the bell has decayed to -60 dB.
pub const SECS: f32 = 0.8;
/// Peak level after normalising, well below full scale.
pub const PEAK: f32 = 0.6;
const ATTACK_SECS: f32 = 0.005;
/// A short linear fade at the very end, so the cut is silent.
const TAIL_SECS: f32 = 0.01;
/// (frequency Hz, amplitude, decay speed). The upper partial dies away faster, which
/// is what makes it sound like a bell rather than a beep.
const PARTIALS: [(f32, f32, f32); 2] = [(880.0, 1.0, 1.0), (1320.0, 0.45, 1.6)];
/// -60 dB as a natural-log amplitude ratio.
const LN_MINUS_60_DB: f32 = -6.907_755;

/// Plays the chime. The app talks to this trait so tests can count plays.
pub trait Chime {
    fn play(&mut self);
}

/// The real chime: `PlaySound` on the WAV below.
pub struct SystemChime;

impl Chime for SystemChime {
    fn play(&mut self) {
        static WAV: OnceLock<Vec<u8>> = OnceLock::new();
        let wav: &'static [u8] = WAV.get_or_init(wav);
        if !dj_win::sound::play_wav(wav) {
            crate::storage::log("timer: cannot play the chime");
        }
    }
}

/// Number of samples in the chime.
pub fn sample_count() -> usize {
    (RATE as f32 * SECS) as usize
}

/// The chime as floats in -PEAK..=PEAK.
pub fn samples() -> Vec<f32> {
    let n = sample_count();
    let raw: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let attack = (t / ATTACK_SECS).min(1.0);
            let tail = ((SECS - t) / TAIL_SECS).clamp(0.0, 1.0);
            let bell: f32 = PARTIALS
                .iter()
                .map(|&(hz, amp, speed)| {
                    amp * (LN_MINUS_60_DB * speed * t / SECS).exp() * (TAU * hz * t).sin()
                })
                .sum();
            bell * attack * tail
        })
        .collect();
    let max = raw.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
    let k = if max > 0.0 { PEAK / max } else { 0.0 };
    raw.into_iter().map(|s| s * k).collect()
}

/// A RIFF/WAVE file holding the chime (PCM, mono, 16-bit, 44.1 kHz).
pub fn wav() -> Vec<u8> {
    let pcm: Vec<i16> = samples()
        .into_iter()
        .map(|s| (s * f32::from(i16::MAX)).round() as i16)
        .collect();
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(b: &[u8], i: usize) -> u16 {
        u16::from_le_bytes([b[i], b[i + 1]])
    }

    fn u32_at(b: &[u8], i: usize) -> u32 {
        u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
    }

    fn pcm(b: &[u8]) -> Vec<i16> {
        b[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect()
    }

    #[test]
    fn header_is_pcm_mono_16_bit_44k() {
        let w = wav();
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32_at(&w, 4) as usize, w.len() - 8);
        assert_eq!(&w[8..12], b"WAVE");
        assert_eq!(&w[12..16], b"fmt ");
        assert_eq!(u32_at(&w, 16), 16);
        assert_eq!(u16_at(&w, 20), 1, "PCM");
        assert_eq!(u16_at(&w, 22), 1, "mono");
        assert_eq!(u32_at(&w, 24), 44_100);
        assert_eq!(u32_at(&w, 28), 88_200);
        assert_eq!(u16_at(&w, 32), 2);
        assert_eq!(u16_at(&w, 34), 16);
        assert_eq!(&w[36..40], b"data");
        assert_eq!(u32_at(&w, 40) as usize, w.len() - 44);
    }

    #[test]
    fn length_is_about_point_eight_seconds() {
        let w = wav();
        assert_eq!(sample_count(), 35_280);
        assert_eq!(w.len(), 44 + 2 * 35_280);
    }

    #[test]
    fn never_clips_and_peaks_at_the_set_level() {
        let s = pcm(&wav());
        let peak = s.iter().map(|v| i32::from(*v).abs()).max().unwrap();
        let limit = (PEAK * f32::from(i16::MAX)).round() as i32;
        assert!(peak <= limit, "peak {peak} > {limit}");
        assert!(peak >= limit - 1, "peak {peak} is too quiet");
        assert!(samples().iter().all(|v| v.is_finite()));
    }

    #[test]
    fn starts_and_ends_silent_and_decays() {
        let s = pcm(&wav());
        assert_eq!(s[0], 0);
        assert_eq!(*s.last().unwrap(), 0);
        let rms = |part: &[i16]| {
            (part.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / part.len() as f64).sqrt()
        };
        let n = s.len();
        let (head, tail) = (rms(&s[..n / 8]), rms(&s[n - n / 8..]));
        assert!(tail * 100.0 < head, "head {head:.0}, tail {tail:.0}");
    }
}
