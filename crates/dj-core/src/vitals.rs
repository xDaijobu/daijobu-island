//! Vitals data model: the latest reading, its health, and the net history for the sparkline.
//! Pure data. The samplers live in `dj-vitals`.

use crate::health::{self, Health};
use std::collections::VecDeque;

/// Samples kept for the net sparkline (one per second while the panel is open).
pub const HISTORY: usize = 60;

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
    /// None when the machine has no battery (or the status is unknown).
    pub battery: Option<Battery>,
    pub uptime_secs: u64,
}

#[derive(Debug, Default)]
pub struct Vitals {
    latest: Option<Reading>,
    health: Option<Health>,
    net_down: VecDeque<f64>,
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

    /// Download rates, oldest first.
    pub fn net_history(&self) -> impl Iterator<Item = f64> + '_ {
        self.net_down.iter().copied()
    }
}
