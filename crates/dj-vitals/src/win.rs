//! Win32 reads for the samplers. Each wrapper returns `None` (or 0) on failure
//! instead of an error, so the sampler can fall back per field.

use crate::calc::{self, CpuTimes, NetTotals};
use dj_core::vitals::Battery;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, FILETIME};
use windows::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetIfTable2, IF_TYPE_SOFTWARE_LOOPBACK, MIB_IF_ROW2, MIB_IF_TABLE2,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue,
    PdhOpenQueryW, PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
};
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::{
    GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, GetSystemTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

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

/// PDH status for a valid sample, and for a valid first sample.
const PDH_CSTATUS_VALID_DATA: u32 = 0;
const PDH_CSTATUS_NEW_DATA: u32 = 1;

/// A PDH query for the physical-disk throughput of all disks. The handles are kept as
/// raw values so the sampler stays `Send`; only the thread that owns it uses them.
#[derive(Debug)]
pub struct DiskCounters {
    query: isize,
    read: isize,
    write: isize,
}

impl DiskCounters {
    /// Opens the query and takes the baseline sample. `None` if PDH or the counters are
    /// unavailable. English counter paths, so a localized Windows works too.
    pub fn open() -> Option<Self> {
        let mut query = PDH_HQUERY(std::ptr::null_mut());
        // SAFETY: `query` is a valid out-pointer; a null data source means live data.
        if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
            return None;
        }
        let mut me = Self {
            query: query.0 as isize,
            read: 0,
            write: 0,
        };
        let add = |path: PCWSTR| {
            let mut counter = PDH_HCOUNTER(std::ptr::null_mut());
            // SAFETY: `query` is the open query above, `path` is a static NUL-terminated
            // string and `counter` is a valid out-pointer.
            let status = unsafe { PdhAddEnglishCounterW(query, path, 0, &mut counter) };
            (status == 0).then_some(counter.0 as isize)
        };
        // On failure `me` is dropped, which closes the query.
        me.read = add(w!("\\PhysicalDisk(_Total)\\Disk Read Bytes/sec"))?;
        me.write = add(w!("\\PhysicalDisk(_Total)\\Disk Write Bytes/sec"))?;
        // SAFETY: the query is open. Rate counters need this first collection as a base.
        unsafe { PdhCollectQueryData(query) };
        Some(me)
    }

    /// `(read, write)` bytes per second since the previous call, or `None` on failure.
    pub fn read(&mut self) -> Option<(f64, f64)> {
        // SAFETY: the query handle stays open until Drop.
        if unsafe { PdhCollectQueryData(PDH_HQUERY(self.query as *mut _)) } != 0 {
            return None;
        }
        Some((value(self.read)?, value(self.write)?))
    }
}

fn value(counter: isize) -> Option<f64> {
    let mut v = PDH_FMT_COUNTERVALUE::default();
    // SAFETY: `counter` belongs to the open query and `v` is a live local out-value.
    let status = unsafe {
        PdhGetFormattedCounterValue(PDH_HCOUNTER(counter as *mut _), PDH_FMT_DOUBLE, None, &mut v)
    };
    if status != 0 || !matches!(v.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA) {
        return None;
    }
    // SAFETY: PDH_FMT_DOUBLE was requested, so the union holds `doubleValue`.
    let d = unsafe { v.Anonymous.doubleValue };
    d.is_finite().then_some(d.max(0.0))
}

impl Drop for DiskCounters {
    fn drop(&mut self) {
        // SAFETY: the query was opened in `open` and is closed exactly once here; this
        // also frees its counters.
        unsafe { PdhCloseQuery(PDH_HQUERY(self.query as *mut _)) };
    }
}

/// One running process. Times are 100 ns units.
#[derive(Clone, Debug)]
pub struct ProcSample {
    pub pid: u32,
    pub name: String,
    pub created: u64,
    pub cpu: u64,
    pub working_set: u64,
}

/// Every process the snapshot shows. Processes that refuse a limited query (protected
/// or elevated ones) are listed with zero CPU and memory.
pub fn processes() -> Vec<ProcSample> {
    let mut out = Vec::new();
    // SAFETY: a process snapshot takes no pointers; the handle is closed below.
    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return out;
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: `snap` is a live snapshot and `entry` has `dwSize` set, as required.
    let mut more = unsafe { Process32FirstW(snap, &mut entry) }.is_ok();
    while more {
        let pid = entry.th32ProcessID;
        if pid != 0 {
            let (created, cpu, working_set) = proc_usage(pid).unwrap_or((0, 0, 0));
            out.push(ProcSample {
                pid,
                name: calc::display_name(&entry.szExeFile),
                created,
                cpu,
                working_set,
            });
        }
        // SAFETY: as for Process32FirstW.
        more = unsafe { Process32NextW(snap, &mut entry) }.is_ok();
    }
    // SAFETY: `snap` came from CreateToolhelp32Snapshot and is closed once.
    let _ = unsafe { CloseHandle(snap) };
    out
}

/// `(creation time, kernel + user time, working set)` of one process.
fn proc_usage(pid: u32) -> Option<(u64, u64, u64)> {
    // SAFETY: plain call; a failure (no access, process gone) is an error result.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: `h` is a live process handle; the out-pointers refer to live locals.
    let times = unsafe { GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user) };
    let mut mem = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: `mem` is a live local whose `cb` matches the size passed in.
    let mem_ok = unsafe { K32GetProcessMemoryInfo(h, &mut mem, mem.cb) }.as_bool();
    // SAFETY: `h` came from OpenProcess and is closed once.
    let _ = unsafe { CloseHandle(h) };
    times.ok()?;
    let ws = if mem_ok { mem.WorkingSetSize as u64 } else { 0 };
    Some((
        combine(created),
        combine(kernel).saturating_add(combine(user)),
        ws,
    ))
}

/// Milliseconds since boot. Never fails.
pub fn tick_count_ms() -> u64 {
    // SAFETY: no pointers are involved.
    unsafe { GetTickCount64() }
}
