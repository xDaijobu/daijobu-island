//! Pure helpers for the Vitals panel: rate text, sparkline points and ring sweep.
//! No Skia here, so these run in plain unit tests.

use dj_core::vitals::HISTORY;

/// The sparkline never scales below this, so a quiet link draws as a flat line.
const SPARK_FLOOR_BPS: f64 = 64.0 * 1024.0;

/// Formats bytes per second with a 1024 base: "B/s", "KB/s", "MB/s" or "GB/s".
/// One decimal below 10 units, none from 10 up. Negative and non-finite input shows as 0.
pub fn format_rate(bps: f64) -> String {
    const UNITS: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut v = if bps.is_finite() { bps.max(0.0) } else { 0.0 };
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if v < 9.95 {
        format!("{v:.1} {}", UNITS[unit])
    } else {
        format!("{v:.0} {}", UNITS[unit])
    }
}

/// Normalised sparkline points in a `w` x `h` box, oldest first, with y growing downwards.
/// Only the last `HISTORY` samples are used. The scale is the window max, floored at 64 KiB/s.
pub fn spark_points(samples: &[f64], w: f32, h: f32) -> Vec<(f32, f32)> {
    let window = &samples[samples.len().saturating_sub(HISTORY)..];
    if window.is_empty() {
        return Vec::new();
    }
    let max = window
        .iter()
        .map(|v| v.max(0.0))
        .fold(0.0_f64, f64::max)
        .max(SPARK_FLOOR_BPS);
    let n = window.len();
    window
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = if n == 1 {
                w / 2.0
            } else {
                w * i as f32 / (n - 1) as f32
            };
            let t = (v.max(0.0) / max).min(1.0) as f32;
            (x, h - t * h)
        })
        .collect()
}

/// Sweep of a ring arc in degrees: `pct` clamped to 0..=100 mapped to 0..=360.
pub fn ring_sweep(pct: f32) -> f32 {
    if pct.is_nan() {
        return 0.0;
    }
    pct.clamp(0.0, 100.0) / 100.0 * 360.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_units_and_decimals() {
        assert_eq!(format_rate(0.0), "0.0 B/s");
        assert_eq!(format_rate(512.0), "512 B/s");
        assert_eq!(format_rate(1024.0), "1.0 KB/s");
        assert_eq!(format_rate(1536.0), "1.5 KB/s");
        assert_eq!(format_rate(80.0 * 1024.0), "80 KB/s");
        assert_eq!(format_rate(1.2 * 1024.0 * 1024.0), "1.2 MB/s");
        assert_eq!(format_rate(4.2 * 1024.0 * 1024.0 * 1024.0), "4.2 GB/s");
    }

    #[test]
    fn rate_handles_huge_and_bad_input() {
        let big = format_rate(1e30);
        assert!(big.ends_with("GB/s"), "{big}");
        assert!(!big.contains('.'), "{big}");
        assert_eq!(format_rate(-5.0), "0.0 B/s");
        assert_eq!(format_rate(f64::NAN), "0.0 B/s");
        assert_eq!(format_rate(f64::INFINITY), "0.0 B/s");
    }

    #[test]
    fn spark_empty_gives_no_points() {
        assert!(spark_points(&[], 144.0, 24.0).is_empty());
    }

    #[test]
    fn spark_floor_keeps_quiet_link_flat() {
        let pts = spark_points(&[0.0, 1000.0, 2000.0], 100.0, 20.0);
        assert_eq!(pts.len(), 3);
        // Far below the 64 KiB/s floor, so every point sits within 1 px of the bottom.
        for (_, y) in &pts {
            assert!(*y > 19.0, "y = {y}");
        }
        assert_eq!(pts[0].0, 0.0);
        assert!((pts[2].0 - 100.0).abs() < 1e-4);
    }

    #[test]
    fn spark_scales_to_window_max_above_floor() {
        let big = 10.0 * 1024.0 * 1024.0;
        let pts = spark_points(&[0.0, big], 100.0, 20.0);
        assert!((pts[0].1 - 20.0).abs() < 1e-4);
        assert!(pts[1].1.abs() < 1e-4);
    }

    #[test]
    fn spark_caps_at_history_length() {
        let samples: Vec<f64> = (0..100).map(|i| i as f64).collect();
        let pts = spark_points(&samples, 100.0, 20.0);
        assert_eq!(pts.len(), HISTORY);
        assert_eq!(pts.len(), 60);
    }

    #[test]
    fn ring_sweep_clamps() {
        assert_eq!(ring_sweep(-5.0), 0.0);
        assert_eq!(ring_sweep(0.0), 0.0);
        assert_eq!(ring_sweep(50.0), 180.0);
        assert_eq!(ring_sweep(100.0), 360.0);
        assert_eq!(ring_sweep(150.0), 360.0);
        assert_eq!(ring_sweep(f32::NAN), 0.0);
    }
}
