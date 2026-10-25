//! Vitals data model: the latest reading, its health, the net history for the sparkline
//! and the top processes. Pure data. The samplers live in `dj-vitals`.

use crate::health::{self, Health};
use std::collections::VecDeque;

/// Samples kept for the net sparkline (one per second while the panel is open).
pub const HISTORY: usize = 60;

/// Rows in each top-process list.
pub const TOP_N: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Battery {
    /// 0..=100.
    pub percent: u8,
    pub charging: bool,
}

/// One sampler pass. Percentages are 0..=100. Rates are bytes per second.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reading {
    pub cpu_pct: f32,
    pub ram_pct: f32,
    /// Used space on the system drive.
    pub disk_pct: f32,
    pub net_down_bps: f64,
    pub net_up_bps: f64,
    /// Physical disk throughput summed over all disks.
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    /// None when the machine has no battery (or the status is unknown).
    pub battery: Option<Battery>,
    pub uptime_secs: u64,
}

impl Reading {
    /// Read plus write throughput. A non-finite or negative part counts as 0.
    pub fn disk_io_bps(&self) -> f64 {
        let part = |v: f64| if v.is_finite() { v.max(0.0) } else { 0.0 };
        part(self.disk_read_bps) + part(self.disk_write_bps)
    }
}

/// One row of a top list. Processes with the same image name are summed, so
/// "chrome" is one row instead of twenty.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcUsage {
    /// Image name without the `.exe` suffix.
    pub name: String,
    /// Share of the whole machine's CPU over the sample interval, 0..=100.
    pub cpu_pct: f32,
    /// Working set in bytes.
    pub ram_bytes: u64,
}

/// The heaviest processes. Only sampled while the Vitals panel is open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TopProcs {
    /// Largest CPU share first, at most `TOP_N`. Idle processes are left out.
    pub cpu: Vec<ProcUsage>,
    /// Largest working set first, at most `TOP_N`.
    pub ram: Vec<ProcUsage>,
}

/// Picks the top `n` by CPU and the top `n` by RAM. Entries with the same name
/// (ASCII case-insensitive) are merged first, and a merged CPU share is capped at 100.
/// Ties keep the order in which the names first appeared.
pub fn top_procs(entries: &[ProcUsage], n: usize) -> TopProcs {
    let mut merged: Vec<ProcUsage> = Vec::new();
    for e in entries {
        let cpu = finite(e.cpu_pct);
        match merged
            .iter_mut()
            .find(|m| m.name.eq_ignore_ascii_case(&e.name))
        {
            Some(m) => {
                m.cpu_pct += cpu;
                m.ram_bytes = m.ram_bytes.saturating_add(e.ram_bytes);
            }
            None => merged.push(ProcUsage {
                cpu_pct: cpu,
                ..e.clone()
            }),
        }
    }
    for m in &mut merged {
        m.cpu_pct = m.cpu_pct.min(100.0);
    }
    let mut cpu: Vec<ProcUsage> = merged
        .iter()
        .filter(|m| m.cpu_pct > 0.0)
        .cloned()
        .collect();
    cpu.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
    cpu.truncate(n);
    let mut ram: Vec<ProcUsage> = merged.into_iter().filter(|m| m.ram_bytes > 0).collect();
    ram.sort_by(|a, b| b.ram_bytes.cmp(&a.ram_bytes));
    ram.truncate(n);
    TopProcs { cpu, ram }
}

fn finite(v: f32) -> f32 {
    if v.is_finite() {
        v.max(0.0)
    } else {
        0.0
    }
}

#[derive(Debug, Default)]
pub struct Vitals {
    latest: Option<Reading>,
    health: Option<Health>,
    net_down: VecDeque<f64>,
    top: Option<TopProcs>,
}

impl Vitals {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores a reading, recomputes its health and appends its download rate to the history.
    pub fn push(&mut self, r: Reading) {
        self.health = Some(health::health(&r));
        if self.net_down.len() == HISTORY {
            self.net_down.pop_front();
        }
        self.net_down.push_back(r.net_down_bps);
        self.latest = Some(r);
    }

    pub fn latest(&self) -> Option<&Reading> {
        self.latest.as_ref()
    }

    pub fn health(&self) -> Option<&Health> {
        self.health.as_ref()
    }

    /// Replaces the top-process lists.
    pub fn set_top(&mut self, top: TopProcs) {
        self.top = Some(top);
    }

    pub fn top(&self) -> Option<&TopProcs> {
        self.top.as_ref()
    }

    /// Download rates, oldest first.
    pub fn net_history(&self) -> impl Iterator<Item = f64> + '_ {
        self.net_down.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str, cpu: f32, ram: u64) -> ProcUsage {
        ProcUsage {
            name: name.into(),
            cpu_pct: cpu,
            ram_bytes: ram,
        }
    }

    fn names(v: &[ProcUsage]) -> Vec<&str> {
        v.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn top_n_picks_largest_cpu_and_ram_separately() {
        let e = [
            p("a", 1.0, 900),
            p("b", 30.0, 100),
            p("c", 12.0, 500),
            p("d", 5.0, 700),
            p("e", 0.0, 50),
        ];
        let t = top_procs(&e, 3);
        assert_eq!(names(&t.cpu), ["b", "c", "d"]);
        assert_eq!(names(&t.ram), ["a", "d", "c"]);
    }

    #[test]
    fn top_n_merges_same_name_case_insensitively() {
        let e = [
            p("chrome", 4.0, 300),
            p("code", 6.0, 200),
            p("Chrome", 4.0, 300),
        ];
        let t = top_procs(&e, 3);
        assert_eq!(names(&t.cpu), ["chrome", "code"]);
        assert_eq!(t.cpu[0].cpu_pct, 8.0);
        assert_eq!(names(&t.ram), ["chrome", "code"]);
        assert_eq!(t.ram[0].ram_bytes, 600);
    }

    #[test]
    fn top_n_skips_idle_and_bad_values_and_caps_cpu() {
        let e = [
            p("idle", 0.0, 0),
            p("nan", f32::NAN, 10),
            p("big", 80.0, 1),
            p("big", 80.0, 1),
        ];
        let t = top_procs(&e, 3);
        assert_eq!(names(&t.cpu), ["big"]);
        assert_eq!(t.cpu[0].cpu_pct, 100.0);
        assert_eq!(names(&t.ram), ["nan", "big"]);
    }

    #[test]
    fn top_n_ties_keep_first_appearance_order() {
        let e = [p("x", 2.0, 5), p("y", 2.0, 5), p("z", 2.0, 5)];
        let t = top_procs(&e, 2);
        assert_eq!(names(&t.cpu), ["x", "y"]);
        assert_eq!(names(&t.ram), ["x", "y"]);
    }

    #[test]
    fn top_n_handles_empty_input_and_zero_n() {
        assert_eq!(top_procs(&[], 3), TopProcs::default());
        let t = top_procs(&[p("x", 1.0, 1)], 0);
        assert!(t.cpu.is_empty() && t.ram.is_empty());
    }

    #[test]
    fn disk_io_sums_read_and_write() {
        let r = Reading {
            disk_read_bps: 10.0,
            disk_write_bps: 5.0,
            ..Default::default()
        };
        assert_eq!(r.disk_io_bps(), 15.0);
        let bad = Reading {
            disk_read_bps: f64::NAN,
            disk_write_bps: -3.0,
            ..Default::default()
        };
        assert_eq!(bad.disk_io_bps(), 0.0);
    }

    #[test]
    fn top_lists_are_kept_until_replaced() {
        let mut v = Vitals::new();
        assert!(v.top().is_none());
        v.set_top(top_procs(&[p("a", 1.0, 1)], TOP_N));
        assert_eq!(v.top().map(|t| t.cpu.len()), Some(1));
        v.push(Reading::default());
        assert!(v.top().is_some());
    }
}
