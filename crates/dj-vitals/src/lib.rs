//! System samplers for the Vitals panel: CPU, RAM, system drive, network, disk I/O,
//! battery, uptime and the top processes. Win32 only. Pure rate and delta helpers are
//! in `calc` and have no Win32 dependency.

pub mod calc;
mod win;

use calc::{CpuTimes, NetTotals};
use dj_core::vitals::{top_procs, ProcUsage, Reading, TopProcs, TOP_N};
use std::collections::HashMap;
use std::time::Instant;

/// A process CPU baseline older than this is dropped instead of averaged, so the first
/// list after the panel opens doesn't average over minutes.
const MAX_TOP_GAP_SECS: f64 = 5.0;

/// Takes one reading per call. Holds the counter baselines for CPU, network, disk I/O
/// and per-process CPU time. Moved into a worker thread, so it is `Send`.
///
/// Every field falls back on its own: a failed API gives 0 for that field, and
/// battery gives `None` when unknown. `sample` never panics and never errors.
#[derive(Debug)]
pub struct Sampler {
    cpu: Option<CpuTimes>,
    net: Option<(NetTotals, Instant)>,
    disk: Option<win::DiskCounters>,
    /// pid -> (creation time, CPU time) from the previous top-process pass.
    procs: HashMap<u32, (u64, u64)>,
    procs_at: Option<Instant>,
    ncpu: usize,
}

impl Sampler {
    /// Reads the baseline counters once so the first `sample` has a base for rates.
    pub fn new() -> Self {
        Self {
            cpu: win::cpu_times(),
            net: win::net_totals().map(|t| (t, Instant::now())),
            disk: win::DiskCounters::open(),
            procs: HashMap::new(),
            procs_at: None,
            ncpu: std::thread::available_parallelism().map_or(1, |n| n.get()),
        }
    }

    /// Reads all vitals now. Rates and CPU are measured since the previous call.
    pub fn sample(&mut self) -> Reading {
        let cpu_pct = match win::cpu_times() {
            Some(cur) => {
                let pct = self.cpu.map_or(0.0, |prev| calc::cpu_busy_pct(prev, cur));
                self.cpu = Some(cur);
                pct
            }
            // Keep the old baseline: the next good read averages over the gap.
            None => 0.0,
        };

        let ram_pct =
            win::memory_totals().map_or(0.0, |(total, avail)| calc::used_pct(total, avail));

        let root = system_drive_root();
        let disk_pct =
            win::disk_totals(&root).map_or(0.0, |(total, free)| calc::used_pct(total, free));

        let (net_down_bps, net_up_bps) = match win::net_totals() {
            Some(cur) => {
                let now = Instant::now();
                let rates = match self.net {
                    Some((prev, t0)) => {
                        let secs = now.duration_since(t0).as_secs_f64();
                        calc::net_rates(prev, cur, secs)
                    }
                    None => (0.0, 0.0),
                };
                self.net = Some((cur, now));
                rates
            }
            // Keep the old baseline and its Instant together, so the next rate is still correct.
            None => (0.0, 0.0),
        };

        let (disk_read_bps, disk_write_bps) = self
            .disk
            .as_mut()
            .and_then(win::DiskCounters::read)
            .unwrap_or((0.0, 0.0));

        Reading {
            cpu_pct,
            ram_pct,
            disk_pct,
            net_down_bps,
            net_up_bps,
            disk_read_bps,
            disk_write_bps,
            battery: win::battery(),
            uptime_secs: win::tick_count_ms() / 1000,
        }
    }

    /// The top processes by CPU (since the previous call) and by working set. The first
    /// call, or one after a long gap, only sets the CPU baseline and lists RAM.
    pub fn top(&mut self) -> TopProcs {
        let procs = win::processes();
        let now = Instant::now();
        let secs = self
            .procs_at
            .map(|t| now.duration_since(t).as_secs_f64())
            .filter(|s| *s <= MAX_TOP_GAP_SECS);
        let entries: Vec<ProcUsage> = procs
            .iter()
            .map(|p| {
                let cpu_pct = match (secs, self.procs.get(&p.pid)) {
                    // Same pid and creation time: the same process as last time.
                    (Some(secs), Some(&(created, cpu))) if created == p.created => {
                        calc::proc_cpu_pct(p.cpu.saturating_sub(cpu), secs, self.ncpu)
                    }
                    _ => 0.0,
                };
                ProcUsage {
                    name: p.name.clone(),
                    cpu_pct,
                    ram_bytes: p.working_set,
                }
            })
            .collect();
        self.procs = procs.iter().map(|p| (p.pid, (p.created, p.cpu))).collect();
        self.procs_at = Some(now);
        top_procs(&entries, TOP_N)
    }
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

/// Root of the system drive, from `SystemDrive` (for example `C:\`). Falls back to `C:\`.
fn system_drive_root() -> String {
    calc::system_drive_root(std::env::var("SystemDrive").ok().as_deref())
}

// Compile-time check that the sampler can move into a worker thread.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Sampler>();
};
