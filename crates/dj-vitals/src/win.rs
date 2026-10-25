//! Win32 reads for the samplers. Each wrapper returns `None` (or 0) on failure
//! instead of an error, so the sampler can fall back per field.

use crate::calc::{self, CpuTimes, NetTotals};
use dj_core::vitals::Battery;
use windows::core::PCWSTR;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetIfTable2, IF_TYPE_SOFTWARE_LOOPBACK, MIB_IF_ROW2, MIB_IF_TABLE2,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::{
    GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX,
};
use windows::Win32::System::Threading::GetSystemTimes;

fn combine(ft: FILETIME) -> u64 {
    calc::filetime_to_u64(ft.dwLowDateTime, ft.dwHighDateTime)
}

/// Cumulative CPU times, or `None` if the call fails.
pub fn cpu_times() -> Option<CpuTimes> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: all three out-pointers refer to live local FILETIME values.
    let ok = unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) };
    ok.ok()?;
    Some(CpuTimes {
        idle: combine(idle),
        kernel: combine(kernel),
        user: combine(user),
    })
}

/// Physical memory `(total, available)` in bytes, or `None` if the call fails.
pub fn memory_totals() -> Option<(u64, u64)> {
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `status` is a live local with `dwLength` set to its own size, as the API requires.
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some((status.ullTotalPhys, status.ullAvailPhys))
}

/// Total and free bytes of the volume at `root` (for example `C:\`), or `None`.
pub fn disk_totals(root: &str) -> Option<(u64, u64)> {
    let wide = calc::to_wide_nul(root);
    let mut total = 0u64;
    let mut free = 0u64;
    // SAFETY: `wide` is NUL-terminated and lives across the call; the out-pointers
    // refer to live locals. The free-to-caller output is not requested.
    unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(wide.as_ptr()),
            None,
            Some(&mut total),
            Some(&mut free),
        )
    }
    .ok()?;
    Some((total, free))
}

/// Sums the octet counters of the up, non-loopback interfaces, or `None` on failure.
pub fn net_totals() -> Option<NetTotals> {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: `table` is a valid out-pointer. On success the system allocates the table,
    // which is released with FreeMibTable below on every path.
    let status = unsafe { GetIfTable2(&mut table) };

    let totals = if status.is_ok() && !table.is_null() {
        // SAFETY: on success `table` points to a table holding `NumEntries` rows that are
        // contiguous from the `Table` field. The memory stays valid until FreeMibTable.
        // The slice is only read while `table` is still alive.
        unsafe {
            let n = (*table).NumEntries as usize;
            let first = std::ptr::addr_of!((*table).Table) as *const MIB_IF_ROW2;
            let rows = std::slice::from_raw_parts(first, n);
            let mut sum = NetTotals::default();
            for row in rows {
                if row.OperStatus == IfOperStatusUp && row.Type != IF_TYPE_SOFTWARE_LOOPBACK {
                    sum.in_octets = sum.in_octets.saturating_add(row.InOctets);
                    sum.out_octets = sum.out_octets.saturating_add(row.OutOctets);
                }
            }
            Some(sum)
        }
    } else {
        None
    };

    if !table.is_null() {
        // SAFETY: `table` was returned by GetIfTable2 and is freed exactly once here.
        unsafe { FreeMibTable(table as *const core::ffi::c_void) };
    }
    totals
}

/// Battery state from GetSystemPowerStatus. `None` when there is no battery or the state is unknown.
pub fn battery() -> Option<Battery> {
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: `status` is a live local that the API fills in.
    unsafe { GetSystemPowerStatus(&mut status) }.ok()?;
    calc::battery_from_status(status.BatteryFlag, status.BatteryLifePercent)
}

/// Milliseconds since boot. Never fails.
pub fn tick_count_ms() -> u64 {
    // SAFETY: no pointers are involved.
    unsafe { GetTickCount64() }
}
