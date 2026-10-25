//! Health score: 100 minus penalties, mapped to a band, with reason tags.
//! Pure function of a `Reading`. The weights are ours (see `task/progress.md`, Stage 4).

use crate::vitals::Reading;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    /// 85+
    Daijobu,
    /// 65+
    Ok,
    /// 45+
    Hmm,
    /// below 45
    Dame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    HighCpu,
    HighRam,
    LowDisk,
    RestartSoon,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Health {
    /// 0..=100.
    pub score: u8,
    pub band: Band,
    /// Reasons with a penalty above zero, largest penalty first.
    pub reasons: Vec<Reason>,
}

impl Band {
    /// Short lowercase label shown in the panel.
    pub fn label(self) -> &'static str {
        match self {
            Band::Daijobu => "daijobu",
            Band::Ok => "ok",
            Band::Hmm => "hmm",
            Band::Dame => "dame",
        }
    }

    /// True for the bands that should draw the eye (`Hmm` and `Dame`).
    pub fn is_warn(self) -> bool {
        matches!(self, Band::Hmm | Band::Dame)
    }
}

impl Reason {
    /// Short tag shown next to the health band.
    pub fn tag(self) -> &'static str {
        match self {
            Reason::HighCpu => "High CPU",
            Reason::HighRam => "High RAM",
            Reason::LowDisk => "Low disk",
            Reason::RestartSoon => "Restart soon",
        }
    }
}

/// Scores a reading. Penalties are linear between the breakpoints in the contract.
pub fn health(r: &Reading) -> Health {
    let cpu = cpu_penalty(pct(r.cpu_pct));
    let ram = ram_penalty(pct(r.ram_pct));
    let disk = disk_penalty(pct(r.disk_pct));
    let uptime = f64::from(uptime_penalty(r.uptime_secs));

    let total = cpu + ram + disk + uptime;
    // `round` rounds halves away from zero, so 84.5 becomes 85.
    let score = (100.0 - total).round().clamp(0.0, 100.0) as u8;

    // Stable sort: equal penalties keep the CPU, RAM, Disk, Uptime order.
    let mut ranked: Vec<(f64, Reason)> = [
        (cpu, Reason::HighCpu),
        (ram, Reason::HighRam),
        (disk, Reason::LowDisk),
        (uptime, Reason::RestartSoon),
    ]
    .into_iter()
    .filter(|(p, _)| *p > 0.0)
    .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));

    Health {
        score,
        band: band_for(score),
        reasons: ranked.into_iter().map(|(_, reason)| reason).collect(),
    }
}

/// Clamps a percentage to at most 100. A non-finite value counts as 0.
fn pct(x: f32) -> f64 {
    if x.is_finite() {
        f64::from(x).min(100.0)
    } else {
        0.0
    }
}

/// Linear interpolation between two points.
fn lerp(x: f64, x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    y0 + (y1 - y0) * (x - x0) / (x1 - x0)
}

fn cpu_penalty(x: f64) -> f64 {
    if x <= 50.0 {
        0.0
    } else if x <= 85.0 {
        lerp(x, 50.0, 0.0, 85.0, 15.0)
    } else {
        lerp(x, 85.0, 15.0, 100.0, 30.0)
    }
}

fn ram_penalty(x: f64) -> f64 {
    if x <= 70.0 {
        0.0
    } else if x <= 88.0 {
        lerp(x, 70.0, 0.0, 88.0, 12.0)
    } else {
        lerp(x, 88.0, 12.0, 100.0, 25.0)
    }
}

fn disk_penalty(x: f64) -> f64 {
    if x <= 80.0 {
        0.0
    } else if x <= 93.0 {
        lerp(x, 80.0, 0.0, 93.0, 10.0)
    } else {
        lerp(x, 93.0, 10.0, 100.0, 20.0)
    }
}

/// Not cumulative: 0 up to 7 days, 1 up to 14 days, 3 beyond that.
fn uptime_penalty(secs: u64) -> u8 {
    const WEEK: u64 = 7 * 24 * 60 * 60;
    if secs <= WEEK {
        0
    } else if secs <= 2 * WEEK {
        1
    } else {
        3
    }
}

fn band_for(score: u8) -> Band {
    match score {
        85..=u8::MAX => Band::Daijobu,
        65..=84 => Band::Ok,
        45..=64 => Band::Hmm,
        _ => Band::Dame,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vitals::{Battery, Reading};

    fn reading(cpu: f32, ram: f32, disk: f32, uptime_secs: u64) -> Reading {
        Reading {
            cpu_pct: cpu,
            ram_pct: ram,
            disk_pct: disk,
            uptime_secs,
            ..Default::default()
        }
    }

    fn score_of(r: &Reading) -> u8 {
        health(r).score
    }

    #[test]
    fn zero_reading_scores_100_daijobu_without_reasons() {
        let h = health(&Reading::default());
        assert_eq!(h.score, 100);
        assert_eq!(h.band, Band::Daijobu);
        assert!(h.reasons.is_empty());
    }

    #[test]
    fn cpu_breakpoints() {
        assert_eq!(score_of(&reading(50.0, 0.0, 0.0, 0)), 100);
        assert_eq!(score_of(&reading(85.0, 0.0, 0.0, 0)), 85);
        assert_eq!(score_of(&reading(100.0, 0.0, 0.0, 0)), 70);
    }

    #[test]
    fn ram_breakpoints() {
        assert_eq!(score_of(&reading(0.0, 70.0, 0.0, 0)), 100);
        assert_eq!(score_of(&reading(0.0, 88.0, 0.0, 0)), 88);
        assert_eq!(score_of(&reading(0.0, 100.0, 0.0, 0)), 75);
    }

    #[test]
    fn disk_breakpoints() {
        assert_eq!(score_of(&reading(0.0, 0.0, 80.0, 0)), 100);
        assert_eq!(score_of(&reading(0.0, 0.0, 93.0, 0)), 90);
        assert_eq!(score_of(&reading(0.0, 0.0, 100.0, 0)), 80);
    }

    #[test]
    fn linear_midpoint_penalty() {
        // Midway between 50 (0) and 85 (15) the CPU penalty is 7.5.
        assert!((cpu_penalty(67.5) - 7.5).abs() < 1e-9);
        // 100 - 7.5 = 92.5, which rounds half away from zero to 93.
        assert_eq!(score_of(&reading(67.5, 0.0, 0.0, 0)), 93);
    }

    #[test]
    fn uptime_boundaries() {
        let week = 604_800;
        let two_weeks = 1_209_600;
        assert_eq!(score_of(&reading(0.0, 0.0, 0.0, week)), 100);
        assert_eq!(score_of(&reading(0.0, 0.0, 0.0, week + 1)), 99);
        assert_eq!(score_of(&reading(0.0, 0.0, 0.0, two_weeks)), 99);
        assert_eq!(score_of(&reading(0.0, 0.0, 0.0, two_weeks + 1)), 97);
    }

    #[test]
    fn uptime_reason_is_restart_soon() {
        let h = health(&reading(0.0, 0.0, 0.0, 1_209_601));
        assert_eq!(h.reasons, vec![Reason::RestartSoon]);
    }

    #[test]
    fn band_boundaries() {
        assert_eq!(band_for(85), Band::Daijobu);
        assert_eq!(band_for(84), Band::Ok);
        assert_eq!(band_for(65), Band::Ok);
        assert_eq!(band_for(64), Band::Hmm);
        assert_eq!(band_for(45), Band::Hmm);
        assert_eq!(band_for(44), Band::Dame);
        assert_eq!(band_for(0), Band::Dame);
    }

    #[test]
    fn band_boundaries_through_health() {
        // CPU 85 -> score 85; CPU 86 -> penalty 16 -> score 84.
        assert_eq!(health(&reading(85.0, 0.0, 0.0, 0)).band, Band::Daijobu);
        assert_eq!(health(&reading(86.0, 0.0, 0.0, 0)).band, Band::Ok);
        // RAM 100 (25) + Disk 93 (10) -> 65; plus uptime at 14 days (1) -> 64.
        assert_eq!(health(&reading(0.0, 100.0, 93.0, 0)).band, Band::Ok);
        assert_eq!(
            health(&reading(0.0, 100.0, 93.0, 1_209_600)).band,
            Band::Hmm
        );
        // CPU 100 (30) + RAM 100 (25) -> 45; plus uptime at 14 days (1) -> 44.
        assert_eq!(health(&reading(100.0, 100.0, 0.0, 0)).band, Band::Hmm);
        assert_eq!(
            health(&reading(100.0, 100.0, 0.0, 1_209_600)).band,
            Band::Dame
        );
    }

    #[test]
    fn extreme_reading_stays_within_bounds() {
        let max = reading(100.0, 100.0, 100.0, u64::MAX);
        let h = health(&max);
        // Largest possible sum is 30 + 25 + 20 + 3 = 78, so the floor is 22.
        assert_eq!(h.score, 22);
        assert_eq!(h.band, Band::Dame);
        assert_eq!(
            h.reasons,
            vec![
                Reason::HighCpu,
                Reason::HighRam,
                Reason::LowDisk,
                Reason::RestartSoon
            ]
        );
        // Values beyond 100 clamp to 100 and give the same score.
        let beyond = reading(f32::MAX, f32::MAX, f32::MAX, u64::MAX);
        assert_eq!(health(&beyond).score, 22);
    }

    #[test]
    fn reasons_sorted_largest_penalty_first() {
        // Disk 100 (20), CPU 85 (15), uptime over 14 days (3).
        let h = health(&reading(85.0, 0.0, 100.0, 1_209_601));
        assert_eq!(
            h.reasons,
            vec![Reason::LowDisk, Reason::HighCpu, Reason::RestartSoon]
        );
    }

    #[test]
    fn reason_ties_keep_cpu_ram_disk_uptime_order() {
        // CPU 85 (15) and Disk 96.5 (15) tie; CPU comes first.
        let h = health(&reading(85.0, 0.0, 96.5, 0));
        assert_eq!(h.reasons, vec![Reason::HighCpu, Reason::LowDisk]);
    }

    #[test]
    fn no_reasons_when_all_penalties_are_zero() {
        // Every metric sits exactly on a zero-penalty edge.
        let h = health(&reading(50.0, 70.0, 80.0, 604_800));
        assert_eq!(h.score, 100);
        assert!(h.reasons.is_empty());
    }

    #[test]
    fn nan_and_infinite_inputs_count_as_zero() {
        let h = health(&reading(f32::NAN, f32::NAN, f32::NAN, 0));
        assert_eq!(h.score, 100);
        assert_eq!(h.band, Band::Daijobu);
        assert!(h.reasons.is_empty());

        let inf = health(&reading(f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, 0));
        assert_eq!(inf.score, 100);
        assert!(inf.reasons.is_empty());
    }

    #[test]
    fn battery_does_not_affect_score() {
        let r = Reading {
            battery: Some(Battery {
                percent: 0,
                charging: false,
            }),
            ..Default::default()
        };
        assert_eq!(health(&r).score, 100);
    }

    #[test]
    fn labels_tags_and_warn_flags() {
        assert_eq!(Band::Daijobu.label(), "daijobu");
        assert_eq!(Band::Ok.label(), "ok");
        assert_eq!(Band::Hmm.label(), "hmm");
        assert_eq!(Band::Dame.label(), "dame");

        assert!(!Band::Daijobu.is_warn());
        assert!(!Band::Ok.is_warn());
        assert!(Band::Hmm.is_warn());
        assert!(Band::Dame.is_warn());

        assert_eq!(Reason::HighCpu.tag(), "High CPU");
        assert_eq!(Reason::HighRam.tag(), "High RAM");
        assert_eq!(Reason::LowDisk.tag(), "Low disk");
        assert_eq!(Reason::RestartSoon.tag(), "Restart soon");
    }
}
