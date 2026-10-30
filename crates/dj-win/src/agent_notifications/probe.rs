//! A short-lived, job-bound --version probe; descendants cannot outlive it or hold stdout open.
use super::{owned, raw};
use dj_core::agent_notifications::Provider;
use std::io::Read;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::thread;
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{
    OpenThread, ResumeThread, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED,
    THREAD_SUSPEND_RESUME,
};

pub fn provider_version(provider: Provider) -> Result<String, String> {
    let program = provider.agent().command();
    let script = format!("$c=Get-Command {program}.cmd,{program}.exe -ErrorAction SilentlyContinue | Select-Object -First 1; if ($null -eq $c) {{ exit 1 }}; & $c.Source --version");
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &script,
    ]);
    run(command, 3000)
}

fn job() -> Result<OwnedHandle, String> {
    // SAFETY: unnamed, noninheritable job handle with no input pointers.
    let job = owned(unsafe { CreateJobObjectW(None, None) }.map_err(|e| e.to_string())?)?;
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: valid job handle and initialized native information buffer.
    unsafe {
        SetInformationJobObject(
            raw(&job),
            JobObjectExtendedLimitInformation,
            (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of_val(&info) as u32,
        )
    }
    .map_err(|e| e.to_string())?;
    Ok(job)
}

fn assign_and_resume(job: &OwnedHandle, child: &Child) -> Result<(), String> {
    // SAFETY: the child is suspended before any user code runs. Assigning it first
    // guarantees every descendant belongs to the kill-on-close job.
    unsafe { AssignProcessToJobObject(raw(job), HANDLE(child.as_raw_handle())) }
        .map_err(|e| e.to_string())?;
    // std::process retains the process handle, but not its initial thread handle.
    // Enumerate only this still-live, suspended child's threads to resume its primary thread.
    // SAFETY: snapshot is an owned kernel handle; no caller buffers are passed.
    let snapshot = owned(
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.map_err(|e| e.to_string())?,
    )?;
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    // SAFETY: entry has the required size and remains live for both enumeration calls.
    let mut present = unsafe { Thread32First(raw(&snapshot), &mut entry) }.is_ok();
    while present {
        if entry.th32OwnerProcessID == child.id() {
            // SAFETY: this thread belongs to our suspended live child; handle is noninheritable.
            let thread = owned(
                unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) }
                    .map_err(|e| e.to_string())?,
            )?;
            // SAFETY: valid thread handle on the primary thread created suspended above.
            if unsafe { ResumeThread(raw(&thread)) } == u32::MAX {
                return Err("cannot resume version probe".into());
            }
            return Ok(());
        }
        // SAFETY: same live snapshot and sized output buffer.
        present = unsafe { Thread32Next(raw(&snapshot), &mut entry) }.is_ok();
    }
    Err("version probe thread unavailable".into())
}

fn run(mut command: Command, timeout_ms: u32) -> Result<String, String> {
    let job = job()?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_SUSPENDED.0 | CREATE_NO_WINDOW.0)
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Err(error) = assign_and_resume(&job, &child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let stdout = child.stdout.take().ok_or("provider stdout unavailable")?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(1025).read_to_end(&mut bytes).map(|_| bytes)
    });
    // SAFETY: the owned child process handle stays live through this bounded wait.
    let finished =
        unsafe { WaitForSingleObject(HANDLE(child.as_raw_handle()), timeout_ms) } == WAIT_OBJECT_0;
    // Kill all remaining descendants before joining the reader, even when the root
    // exited normally. No process in this tree can retain the stdout writer afterward.
    drop(job);
    let status = child.wait().map_err(|e| e.to_string())?;
    let bytes = reader
        .join()
        .map_err(|_| "version reader failed")?
        .map_err(|_| "version read failed")?;
    if !finished || !status.success() || bytes.len() > 1024 {
        return Err("provider version unavailable or timed out".into());
    }
    String::from_utf8(bytes).map_err(|_| "invalid version encoding".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn fake_descendant_cannot_keep_stdout_or_outlive_the_wrapper() {
        let script = "$s=New-Object Diagnostics.ProcessStartInfo; $s.FileName='powershell.exe'; $s.Arguments='-NoProfile -NonInteractive -Command Start-Sleep -Seconds 20'; $s.UseShellExecute=$false; $s.CreateNoWindow=$true; $p=[Diagnostics.Process]::Start($s); [Console]::Out.Write($p.Id)";
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ]);
        let start = Instant::now();
        let pid: u32 = run(command, 3000).unwrap().trim().parse().unwrap();
        assert!(start.elapsed() < Duration::from_secs(5));
        // SAFETY: read-only synchronization handle for our fixture's child, if still present.
        let handle = unsafe {
            windows::Win32::System::Threading::OpenProcess(
                windows::Win32::System::Threading::PROCESS_SYNCHRONIZE,
                false,
                pid,
            )
        };
        if let Ok(handle) = handle {
            let handle = owned(handle).unwrap();
            // SAFETY: valid owned process handle; bounded read-only wait.
            assert_eq!(
                unsafe { WaitForSingleObject(raw(&handle), 2000) },
                WAIT_OBJECT_0
            );
        }
    }

    #[test]
    fn fake_hung_version_probe_is_bounded() {
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 20",
        ]);
        let start = Instant::now();
        assert!(run(command, 150).is_err());
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
