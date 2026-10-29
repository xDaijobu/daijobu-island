use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    PROCESS_MEMORY_COUNTERS_EX2,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

#[derive(Clone, Debug)]
pub struct Process {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
    pub created: Option<u64>,
    pub working_set: Option<u64>,
    pub private_working_set: Option<u64>,
    pub private_commit: Option<u64>,
    pub peak_working_set: Option<u64>,
    pub status: String,
}

#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub processes: Vec<Process>,
    pub note: String,
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: owns one successfully opened native handle, closed exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn time(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

fn verified(p: u32, root: u32, rows: &HashMap<u32, Process>) -> bool {
    let mut current = p;
    let mut seen = HashSet::new();
    for _ in 0..64 {
        if !seen.insert(current) {
            return false;
        }
        let Some(child) = rows.get(&current) else {
            return false;
        };
        let Some(birth) = child.created else {
            return false;
        };
        if current == root {
            return true;
        }
        let Some(parent) = rows.get(&child.parent) else {
            return false;
        };
        if !parent.created.is_some_and(|t| t < birth) {
            return false;
        }
        current = child.parent;
    }
    false
}

pub fn sample(cancel: &std::sync::atomic::AtomicBool) -> Sample {
    use std::sync::atomic::Ordering;
    if cancel.load(Ordering::Acquire) {
        return Sample::default();
    }
    let root = std::process::id();
    // SAFETY: reads system clock without pointers or side effects.
    let snapshot_time =
        time(unsafe { windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime() });
    // SAFETY: creates read-only system snapshot, owned below.
    let snap = match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) } {
        Ok(h) => Handle(h),
        Err(e) => {
            return Sample {
                note: format!("Snapshot unavailable: {e}"),
                ..Sample::default()
            }
        }
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut entries = Vec::new();
    // SAFETY: snapshot valid, entry initialized with required size.
    let mut more = unsafe { Process32FirstW(snap.0, &mut entry) }.is_ok();
    while more && entries.len() < 32768 && !cancel.load(Ordering::Acquire) {
        let end = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.szExeFile.len());
        entries.push((
            entry.th32ProcessID,
            entry.th32ParentProcessID,
            String::from_utf16_lossy(&entry.szExeFile[..end]),
        ));
        // SAFETY: same valid snapshot and writable initialized entry.
        more = unsafe { Process32NextW(snap.0, &mut entry) }.is_ok();
    }
    let mut candidates = HashSet::from([root]);
    for _ in 0..64 {
        let before = candidates.len();
        for (pid, parent, _) in &entries {
            if candidates.contains(parent) && candidates.len() < 256 {
                candidates.insert(*pid);
            }
        }
        if before == candidates.len() {
            break;
        }
    }
    let mut rows = HashMap::new();
    let mut handles = Vec::new();
    for (pid, parent, name) in entries
        .iter()
        .filter(|(pid, _, _)| candidates.contains(pid))
    {
        if cancel.load(Ordering::Acquire) {
            return Sample::default();
        }
        let mut row = Process {
            pid: *pid,
            parent: *parent,
            name: name.clone(),
            created: None,
            working_set: None,
            private_working_set: None,
            private_commit: None,
            peak_working_set: None,
            status: String::new(),
        };
        // SAFETY: limited read-only query access; no mutation rights requested.
        match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, *pid) } {
            Err(e) => row.status = format!("Unavailable (denied/exited): {e}"),
            Ok(raw) => {
                let h = Handle(raw);
                let (mut birth, mut exit, mut kernel, mut user) = (
                    FILETIME::default(),
                    FILETIME::default(),
                    FILETIME::default(),
                    FILETIME::default(),
                );
                // SAFETY: handle remains owned; output pointers are valid locals.
                if unsafe { GetProcessTimes(h.0, &mut birth, &mut exit, &mut kernel, &mut user) }
                    .is_ok()
                    && time(exit) == 0
                    && time(birth) <= snapshot_time
                {
                    row.created = Some(time(birth));
                    let mut mem = PROCESS_MEMORY_COUNTERS_EX2 {
                        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32,
                        ..Default::default()
                    };
                    // SAFETY: EX2 is native repr(C) extended counters, size matches allocation.
                    if unsafe {
                        K32GetProcessMemoryInfo(
                            h.0,
                            (&mut mem as *mut PROCESS_MEMORY_COUNTERS_EX2)
                                .cast::<PROCESS_MEMORY_COUNTERS>(),
                            mem.cb,
                        )
                    }
                    .as_bool()
                    {
                        row.working_set = Some(mem.WorkingSetSize as u64);
                        row.private_working_set = Some(mem.PrivateWorkingSetSize as u64);
                        row.private_commit = Some(mem.PrivateUsage as u64);
                        row.peak_working_set = Some(mem.PeakWorkingSetSize as u64);
                    } else {
                        let mut mem = PROCESS_MEMORY_COUNTERS_EX {
                            cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
                            ..Default::default()
                        };
                        // SAFETY: EX fallback uses matching native structure size and live handle.
                        if unsafe {
                            K32GetProcessMemoryInfo(
                                h.0,
                                (&mut mem as *mut PROCESS_MEMORY_COUNTERS_EX)
                                    .cast::<PROCESS_MEMORY_COUNTERS>(),
                                mem.cb,
                            )
                        }
                        .as_bool()
                        {
                            row.working_set = Some(mem.WorkingSetSize as u64);
                            row.private_commit = Some(mem.PrivateUsage as u64);
                            row.peak_working_set = Some(mem.PeakWorkingSetSize as u64);
                            row.status = "Private WS unavailable".into();
                        } else {
                            row.status = "Memory query unavailable".into();
                        }
                    }
                } else {
                    row.status = "Identity unavailable / exited".into();
                }
                handles.push(h);
            }
        }
        rows.insert(*pid, row);
    }
    let mut processes: Vec<_> = rows
        .values()
        .filter(|r| r.pid == root || verified(r.pid, root, &rows))
        .cloned()
        .collect();
    processes.sort_by_key(|p| (p.pid != root, p.pid));
    let omitted = rows.len().saturating_sub(processes.len());
    Sample { processes, note: format!("{omitted} candidate(s) omitted: ancestry unavailable/reused/exited. Limits: 256 candidates, 64 generations; snapshot {} entries{}.", entries.len(), if more { " (truncated)" } else { "" }) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(pid: u32, parent: u32, created: Option<u64>) -> Process {
        Process {
            pid,
            parent,
            name: String::new(),
            created,
            working_set: None,
            private_working_set: None,
            private_commit: None,
            peak_working_set: None,
            status: String::new(),
        }
    }
    #[test]
    fn ancestry_rejects_reused_denied_and_cycles() {
        let mut rows = HashMap::from([
            (1, row(1, 0, Some(10))),
            (2, row(2, 1, Some(20))),
            (3, row(3, 2, Some(30))),
        ]);
        assert!(verified(3, 1, &rows));
        rows.get_mut(&2).unwrap().created = Some(40);
        assert!(!verified(3, 1, &rows));
        rows.get_mut(&2).unwrap().created = None;
        assert!(!verified(3, 1, &rows));
        rows.get_mut(&2).unwrap().parent = 3;
        assert!(!verified(3, 1, &rows));
    }
    #[test]
    fn bounded_read_only_native_sample() {
        let result = sample(&std::sync::atomic::AtomicBool::new(false));
        assert!(result.processes.len() <= 256);
        let app = result
            .processes
            .iter()
            .find(|p| p.pid == std::process::id())
            .expect("own process readable");
        assert!(app.created.is_some());
        assert!(app.working_set.is_some_and(|n| n > 0));
        assert!(app.private_commit.is_some());
    }
}
