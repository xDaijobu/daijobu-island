//! Pure helpers for the samplers: counter deltas, rates and percentages.
//! No Win32 types and no unsafe code, so everything here is unit tested.

use dj_core::vitals::Battery;

/// `SYSTEM_POWER_STATUS::BatteryFlag` bit set when the machine has no system battery.
pub const BATTERY_FLAG_NO_BATTERY: u8 = 128;
/// `SYSTEM_POWER_STATUS::BatteryFlag` bit set while the battery is charging.
pub const BATTERY_FLAG_CHARGING: u8 = 8;
/// `SYSTEM_POWER_STATUS::BatteryLifePercent` value meaning "unknown".
pub const BATTERY_LIFE_UNKNOWN: u8 = 255;

/// Cumulative CPU times, already combined from FILETIME halves (100 ns units).
/// `kernel` includes `idle`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub idle: u64,
    pub kernel: u64,
    pub user: u64,
}

/// Cumulative octet counters summed over the counted interfaces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetTotals {
    pub in_octets: u64,
    pub out_octets: u64,
}

/// Joins the two halves of a FILETIME into one 64-bit value.
pub fn filetime_to_u64(low: u32, high: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

/// Growth of a cumulative counter between two samples.
///
/// Wrap policy: a backwards step is treated as 0, never as a jump. A 64-bit
/// octet counter wraps only after centuries at real link speeds, so a backwards
/// value practically means the counter source changed (for example an adapter
/// went away, or the table was rebuilt). The caller then takes the new value as
/// the next baseline. A wrap that did happen is therefore lost for one tick,
/// which is the intended trade-off.
pub fn counter_delta(prev: u64, cur: u64) -> u64 {
    cur.saturating_sub(prev)
}

/// Bytes per second over `secs`. A zero, negative or non-finite interval gives 0.
pub fn bytes_per_sec(delta_bytes: u64, secs: f64) -> f64 {
    if !secs.is_finite() || secs <= 0.0 {
        return 0.0;
    }
    delta_bytes as f64 / secs
}

/// Download and upload rates (bytes per second) between two net totals.
pub fn net_rates(prev: NetTotals, cur: NetTotals, secs: f64) -> (f64, f64) {
    let down = bytes_per_sec(counter_delta(prev.in_octets, cur.in_octets), secs);
    let up = bytes_per_sec(counter_delta(prev.out_octets, cur.out_octets), secs);
    (down, up)
}

/// CPU busy percentage between two cumulative readings:
/// `(Δkernel + Δuser − Δidle) / (Δkernel + Δuser) × 100`, clamped to 0..=100.
/// A zero total delta gives 0.
pub fn cpu_busy_pct(prev: CpuTimes, cur: CpuTimes) -> f32 {
    let idle = counter_delta(prev.idle, cur.idle);
    let kernel = counter_delta(prev.kernel, cur.kernel);
    let user = counter_delta(prev.user, cur.user);
    let total = kernel.saturating_add(user);
    if total == 0 {
        return 0.0;
    }
    let busy = total.saturating_sub(idle) as f64 / total as f64 * 100.0;
    clamp_pct(busy)
}

/// Used percentage: `(total − free) / total × 100`, clamped to 0..=100.
/// A zero total gives 0. `free` larger than `total` counts as 0 used.
pub fn used_pct(total: u64, free: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    let used = total.saturating_sub(free);
    clamp_pct(used as f64 / total as f64 * 100.0)
}

/// Clamps a percentage to 0..=100. NaN gives 0.
pub fn clamp_pct(v: f64) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, 100.0) as f32
    }
}

/// Maps the raw power status fields to a battery. `None` when there is no
/// system battery or the level is unknown. The percent is capped at 100.
pub fn battery_from_status(flag: u8, life_percent: u8) -> Option<Battery> {
    if flag & BATTERY_FLAG_NO_BATTERY != 0 || life_percent == BATTERY_LIFE_UNKNOWN {
        return None;
    }
    Some(Battery {
        percent: life_percent.min(100),
        charging: flag & BATTERY_FLAG_CHARGING != 0,
    })
}

/// Root of the system drive for `GetDiskFreeSpaceExW`, e.g. `"C:"` becomes `"C:\"`.
/// A missing or empty value falls back to `"C:\"`.
pub fn system_drive_root(system_drive: Option<&str>) -> String {
    let drive = system_drive
        .map(|s| s.trim().trim_end_matches('\\'))
        .unwrap_or("");
    if drive.is_empty() {
        "C:\\".to_owned()
    } else {
        format!("{drive}\\")
    }
}

/// One process's share of the whole machine over `secs`: CPU time used (100 ns units)
/// divided by the wall time times the logical CPU count, as a percentage clamped to
/// 0..=100. A bad interval or a zero CPU count gives 0.
pub fn proc_cpu_pct(delta_100ns: u64, secs: f64, ncpu: usize) -> f32 {
    if !secs.is_finite() || secs <= 0.0 || ncpu == 0 {
        return 0.0;
    }
    let busy_secs = delta_100ns as f64 / 10_000_000.0;
    clamp_pct(busy_secs / (secs * ncpu as f64) * 100.0)
}

/// A process image name for display: up to the first NUL, without a trailing `.exe`.
pub fn display_name(raw: &[u16]) -> String {
    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    let name = String::from_utf16_lossy(&raw[..end]);
    match name.len().checked_sub(4) {
        Some(cut) if name.is_char_boundary(cut) && name[cut..].eq_ignore_ascii_case(".exe") => {
            name[..cut].to_string()
        }
        _ => name,
    }
}

/// UTF-16 encoding with a trailing NUL, ready for a `PCWSTR`.
pub fn to_wide_nul(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu(idle: u64, kernel: u64, user: u64) -> CpuTimes {
        CpuTimes { idle, kernel, user }
    }

    #[test]
    fn filetime_halves_combine() {
        assert_eq!(filetime_to_u64(0, 0), 0);
        assert_eq!(filetime_to_u64(5, 0), 5);
        assert_eq!(filetime_to_u64(0, 1), 1 << 32);
        assert_eq!(filetime_to_u64(u32::MAX, u32::MAX), u64::MAX);
    }

    #[test]
    fn counter_delta_normal_step() {
        assert_eq!(counter_delta(100, 250), 150);
        assert_eq!(counter_delta(7, 7), 0);
    }

    #[test]
    fn counter_delta_backwards_is_zero() {
        assert_eq!(counter_delta(250, 100), 0);
        assert_eq!(counter_delta(u64::MAX, 0), 0);
    }

    #[test]
    fn counter_delta_wrap_is_treated_as_backwards() {
        // A u64 wrap shows up as a backwards step and must not become a huge jump.
        assert_eq!(counter_delta(u64::MAX - 10, 5), 0);
        assert_eq!(counter_delta(u64::MAX - 10, u64::MAX), 10);
    }

    #[test]
    fn bytes_per_sec_divides_by_seconds() {
        assert_eq!(bytes_per_sec(1000, 2.0), 500.0);
        assert_eq!(bytes_per_sec(0, 1.0), 0.0);
    }

    #[test]
    fn zero_or_bad_seconds_gives_zero_rate() {
        assert_eq!(bytes_per_sec(1000, 0.0), 0.0);
        assert_eq!(bytes_per_sec(1000, -1.0), 0.0);
        assert_eq!(bytes_per_sec(1000, f64::NAN), 0.0);
        assert_eq!(bytes_per_sec(1000, f64::INFINITY), 0.0);
    }

    #[test]
    fn net_rates_from_fixed_deltas() {
        let prev = NetTotals {
            in_octets: 1_000,
            out_octets: 5_000,
        };
        let cur = NetTotals {
            in_octets: 3_000,
            out_octets: 5_500,
        };
        let (down, up) = net_rates(prev, cur, 2.0);
        assert_eq!(down, 1_000.0);
        assert_eq!(up, 250.0);
    }

    #[test]
    fn net_rates_backwards_total_counts_as_zero_for_that_direction() {
        let prev = NetTotals {
            in_octets: 9_000,
            out_octets: 1_000,
        };
        let cur = NetTotals {
            in_octets: 100,
            out_octets: 1_400,
        };
        let (down, up) = net_rates(prev, cur, 1.0);
        assert_eq!(down, 0.0);
        assert_eq!(up, 400.0);
    }

    #[test]
    fn net_rates_zero_seconds_is_zero() {
        let prev = NetTotals {
            in_octets: 0,
            out_octets: 0,
        };
        let cur = NetTotals {
            in_octets: 10,
            out_octets: 10,
        };
        assert_eq!(net_rates(prev, cur, 0.0), (0.0, 0.0));
    }

    #[test]
    fn cpu_quarter_idle_is_75_percent_busy() {
        // Total delta 400: idle 100, kernel 200 (includes idle), user 200.
        let prev = cpu(0, 0, 0);
        let cur = cpu(100, 200, 200);
        assert_eq!(cpu_busy_pct(prev, cur), 75.0);
    }

    #[test]
    fn cpu_fully_idle_is_zero_busy() {
        let prev = cpu(0, 0, 0);
        let cur = cpu(400, 400, 0);
        assert_eq!(cpu_busy_pct(prev, cur), 0.0);
    }

    #[test]
    fn cpu_zero_total_delta_is_zero() {
        let t = cpu(10, 20, 30);
        assert_eq!(cpu_busy_pct(t, t), 0.0);
    }

    #[test]
    fn cpu_idle_above_total_clamps_to_zero() {
        // Inconsistent deltas (idle grew more than kernel + user) must not go negative.
        let prev = cpu(0, 0, 0);
        let cur = cpu(500, 100, 100);
        assert_eq!(cpu_busy_pct(prev, cur), 0.0);
    }

    #[test]
    fn used_pct_fixed_numbers() {
        assert_eq!(used_pct(1000, 250), 75.0);
        assert_eq!(used_pct(1000, 1000), 0.0);
        assert_eq!(used_pct(1000, 0), 100.0);
    }

    #[test]
    fn used_pct_zero_total_is_zero() {
        assert_eq!(used_pct(0, 0), 0.0);
        assert_eq!(used_pct(0, 50), 0.0);
    }

    #[test]
    fn used_pct_free_above_total_is_zero_used() {
        assert_eq!(used_pct(100, 150), 0.0);
    }

    #[test]
    fn clamp_pct_bounds_and_nan() {
        assert_eq!(clamp_pct(-5.0), 0.0);
        assert_eq!(clamp_pct(120.0), 100.0);
        assert_eq!(clamp_pct(42.5), 42.5);
        assert_eq!(clamp_pct(f64::NAN), 0.0);
    }

    #[test]
    fn battery_absent_or_unknown_is_none() {
        assert_eq!(battery_from_status(BATTERY_FLAG_NO_BATTERY, 50), None);
        assert_eq!(battery_from_status(0, BATTERY_LIFE_UNKNOWN), None);
    }

    #[test]
    fn battery_present_maps_percent_and_charging() {
        assert_eq!(
            battery_from_status(0, 80),
            Some(Battery {
                percent: 80,
                charging: false
            })
        );
        assert_eq!(
            battery_from_status(BATTERY_FLAG_CHARGING, 42),
            Some(Battery {
                percent: 42,
                charging: true
            })
        );
    }

    #[test]
    fn battery_percent_is_capped_at_100() {
        assert_eq!(
            battery_from_status(0, 100),
            Some(Battery {
                percent: 100,
                charging: false
            })
        );
        assert_eq!(
            battery_from_status(0, 254),
            Some(Battery {
                percent: 100,
                charging: false
            })
        );
    }

    #[test]
    fn system_drive_root_builds_root_path() {
        assert_eq!(system_drive_root(Some("C:")), "C:\\");
        assert_eq!(system_drive_root(Some("D:\\")), "D:\\");
        assert_eq!(system_drive_root(Some("  E: ")), "E:\\");
    }

    #[test]
    fn system_drive_root_falls_back_to_c() {
        assert_eq!(system_drive_root(None), "C:\\");
        assert_eq!(system_drive_root(Some("")), "C:\\");
        assert_eq!(system_drive_root(Some("   ")), "C:\\");
    }

    #[test]
    fn proc_cpu_share_of_the_machine() {
        // 0.5 s of CPU over 1 s on 4 logical CPUs is 12.5 % of the machine.
        assert_eq!(proc_cpu_pct(5_000_000, 1.0, 4), 12.5);
        // A process using all 8 CPUs for 2 s is 100 %.
        assert_eq!(proc_cpu_pct(160_000_000, 2.0, 8), 100.0);
        // Rounding noise above 100 is clamped.
        assert_eq!(proc_cpu_pct(20_000_000, 1.0, 1), 100.0);
    }

    #[test]
    fn proc_cpu_bad_inputs_give_zero() {
        assert_eq!(proc_cpu_pct(1_000, 0.0, 4), 0.0);
        assert_eq!(proc_cpu_pct(1_000, -1.0, 4), 0.0);
        assert_eq!(proc_cpu_pct(1_000, f64::NAN, 4), 0.0);
        assert_eq!(proc_cpu_pct(1_000, 1.0, 0), 0.0);
    }

    #[test]
    fn display_name_strips_exe_and_nul_padding() {
        let mut raw: Vec<u16> = "Chrome.EXE".encode_utf16().collect();
        raw.extend([0, b'x' as u16, 0]);
        assert_eq!(display_name(&raw), "Chrome");
        let plain: Vec<u16> = "Memory Compression".encode_utf16().collect();
        assert_eq!(display_name(&plain), "Memory Compression");
        let short: Vec<u16> = "exe".encode_utf16().collect();
        assert_eq!(display_name(&short), "exe");
        assert_eq!(display_name(&[0]), "");
    }

    #[test]
    fn wide_string_is_nul_terminated() {
        assert_eq!(
            to_wide_nul("C:\\"),
            vec![b'C' as u16, b':' as u16, b'\\' as u16, 0]
        );
        assert_eq!(to_wide_nul(""), vec![0]);
    }
}
