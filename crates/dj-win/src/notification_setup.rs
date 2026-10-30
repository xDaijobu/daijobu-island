//! Explicit per-user identity deployment. No shell, SDK, signing or trust changes.
use super::notifications::Apartment;
use crate::window;
use dj_core::notification_setup::Failure;
use dj_core::notifications::bounded_text;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Foundation::Uri;
use windows::Management::Deployment::{
    AddPackageOptions, DeploymentResult, PackageManager, RegisterPackageOptions,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
use windows::Win32::System::Threading::{
    CreateEventW, OpenEventW, OpenProcess, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
    PROCESS_SYNCHRONIZE, SYNCHRONIZATION_SYNCHRONIZE,
};
use windows_future::AsyncStatus;

pub const RESTART_SWITCH: &str = "--notification-restart-helper";
pub const OPEN_SWITCH: &str = "--notifications";
const MEDIA_RESTART_SWITCH: &str = "--media-restart-helper";
pub const OPEN_MEDIA_SWITCH: &str = "--music";
const TIMEOUT: Duration = Duration::from_secs(120);

pub enum Completion {
    Registered(Result<(), Failure>),
    Restart(Result<RestartTicket, Failure>),
}

pub struct Job {
    rx: mpsc::Receiver<Completion>,
    cancel: Arc<AtomicBool>,
    restart: bool,
}
impl Job {
    pub fn take(&self) -> Option<Completion> {
        match self.rx.try_recv() {
            Ok(completion) => Some(completion),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                let error = failure(
                    "The setup worker stopped unexpectedly. Retry the operation.".into(),
                    true,
                );
                Some(if self.restart {
                    Completion::Restart(Err(error))
                } else {
                    Completion::Registered(Err(error))
                })
            }
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

fn launch(
    hwnd: isize,
    restart: bool,
    work: impl FnOnce(Arc<AtomicBool>) -> Completion + Send + 'static,
) -> Result<Job, Failure> {
    let (tx, rx) = mpsc::sync_channel(1);
    let cancel = Arc::new(AtomicBool::new(false));
    let stopped = cancel.clone();
    std::thread::Builder::new()
        .name("notification-setup".into())
        .spawn(move || {
            let result = work(stopped.clone());
            if !stopped.load(Ordering::Acquire) && tx.send(result).is_ok() {
                window::wake(hwnd);
            }
        })
        .map_err(|e| failure(format!("Could not start setup worker: {e}"), true))?;
    Ok(Job {
        rx,
        cancel,
        restart,
    })
}

/// All filesystem preparation and WinRT deployment happen on the MTA worker.
pub fn register(
    hwnd: isize,
    prepare: impl FnOnce() -> Result<(PathBuf, PathBuf, bool), Failure> + Send + 'static,
) -> Result<Job, Failure> {
    launch(hwnd, false, move |cancel| {
        Completion::Registered((|| {
            let (payload, external, signed) = prepare()?;
            deploy(&payload, &external, signed, &cancel)
        })())
    })
}

pub fn developer_mode() -> bool {
    let mut value = 0u32;
    let mut size = 4;
    // SAFETY: read-only HKLM query into a DWORD-sized buffer. Never enables mode.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\AppModelUnlock"),
            w!("AllowDevelopmentWithoutDevLicense"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
        .is_ok()
            && value == 1
    }
}

fn deploy(
    payload: &Path,
    external: &Path,
    signed: bool,
    cancel: &AtomicBool,
) -> Result<(), Failure> {
    if !signed && !developer_mode() {
        return Err(failure("This build has no signed setup payload. Use a signed distribution build, or enable Developer Mode yourself in Windows Settings for local development. The app never changes Developer Mode or certificate trust.".into(), true));
    }
    let _apartment = Apartment::init(false).map_err(os_failure)?;
    let manager = PackageManager::new().map_err(os_failure)?;
    let external = file_uri(external)?;
    let payload = file_uri(payload)?;
    let op = if signed {
        let options = AddPackageOptions::new().map_err(os_failure)?;
        options
            .SetExternalLocationUri(&external)
            .map_err(os_failure)?;
        // No AllowUnsigned, force shutdown, elevation or developer fallback.
        manager.AddPackageByUriAsync(&payload, &options)
    } else {
        let options = RegisterPackageOptions::new().map_err(os_failure)?;
        options
            .SetExternalLocationUri(&external)
            .map_err(os_failure)?;
        options.SetDeveloperMode(true).map_err(os_failure)?;
        manager.RegisterPackageByUriAsync(&payload, &options)
    }
    .map_err(os_failure)?;
    let start = Instant::now();
    while op.Status().map_err(os_failure)? == AsyncStatus::Started {
        if cancel.load(Ordering::Acquire) || start.elapsed() >= TIMEOUT {
            let _ = op.Cancel();
            // Cancellation is best-effort. Do not authorize another concurrent
            // registration if Windows has not acknowledged terminal completion.
            let deadline = Instant::now() + Duration::from_secs(5);
            while op.Status().ok() == Some(AsyncStatus::Started) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            return Err(failure("Windows setup timed out or was cancelled. Close and reopen the app before trying again; Windows may still finish registration.".into(), false));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let result = op.GetResults().map_err(os_failure)?;
    deployment_result(result)
}

fn deployment_result(result: DeploymentResult) -> Result<(), Failure> {
    let code = result.ExtendedErrorCode().map_err(os_failure)?;
    if code.is_err() {
        return Err(failure(
            format!(
                "Windows setup failed ({:#010X}): {}. Retry after resolving the Windows error.",
                code.0 as u32,
                bounded_text(&result.ErrorText().unwrap_or_default().to_string(), 400)
            ),
            true,
        ));
    }
    Ok(())
}

fn file_uri(path: &Path) -> Result<Uri, Failure> {
    // Escape every non-path-safe UTF-8 byte, including %, #, ? and apostrophes;
    // never interpolate a path into PowerShell or a command string.
    let path = path
        .canonicalize()
        .map_err(|e| failure(format!("Setup path unavailable: {e}"), true))?;
    let path = path
        .to_str()
        .ok_or_else(|| failure("Setup path is not valid Unicode".into(), true))?;
    let path = path
        .strip_prefix(r"\\?\")
        .unwrap_or(path)
        .replace('\\', "/");
    if path.starts_with("UNC/") || path.starts_with("//") {
        return Err(failure(
            "Move the app to a local disk before setting up notifications.".into(),
            true,
        ));
    }
    Uri::CreateUri(&HSTRING::from(format!("file:///{}", encode_path(&path)))).map_err(os_failure)
}

fn encode_path(path: &str) -> String {
    let mut result = String::new();
    use std::fmt::Write;
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~:".contains(&byte) {
            result.push(byte as char);
        } else {
            let _ = write!(result, "%{byte:02X}");
        }
    }
    result
}

fn failure(message: String, retryable: bool) -> Failure {
    Failure {
        message: bounded_text(&message, 600),
        retryable,
    }
}
fn os_failure(error: windows::core::Error) -> Failure {
    let hint = match error.code().0 as u32 {
        0x800B0100 | 0x800B0109 | 0x800B010A => "The package signature is missing or not trusted. Obtain a correctly signed build; the app will not add certificate trust.",
        0x80073D02 => "Windows reports package resources in use. Close other copies and retry.",
        0x80070005 => "Windows denied setup. Check deployment policy or existing Developer Mode for a development build.",
        _ => "Resolve the Windows deployment error, then retry setup.",
    };
    failure(
        format!(
            "Setup failed ({:#010X}). {hint} {}",
            error.code().0 as u32,
            error.message()
        ),
        true,
    )
}

// Native handles are transferred as integer values, with exactly one owner.
struct Handle(isize);
impl Handle {
    fn raw(&self) -> HANDLE {
        HANDLE(self.0 as *mut _)
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: this owner closes its successfully created/opened handle once.
        unsafe {
            let _ = CloseHandle(self.raw());
        }
    }
}

pub struct RestartTicket {
    go: Handle,
    helper: std::process::Child,
}
impl RestartTicket {
    /// Authorize the waiting helper immediately before normal app shutdown.
    pub fn commit(mut self) -> Result<(), Failure> {
        if self
            .helper
            .try_wait()
            .map_err(|e| failure(e.to_string(), true))?
            .is_some()
        {
            return Err(failure("The restart helper exited before shutdown. Retry restart or close and reopen manually.".into(), true));
        }
        // SAFETY: this is the owned commit event, still open for this call.
        unsafe { SetEvent(self.go.raw()).map_err(os_failure) }
    }
}

pub fn prepare_restart(hwnd: isize) -> Result<Job, Failure> {
    launch(hwnd, true, move |_| {
        Completion::Restart(restart_ticket(false))
    })
}

pub fn prepare_media_restart(hwnd: isize) -> Result<Job, Failure> {
    launch(hwnd, true, move |_| {
        Completion::Restart(restart_ticket(true))
    })
}

fn restart_ticket(media: bool) -> Result<RestartTicket, Failure> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    let exe = std::env::current_exe().map_err(|e| failure(e.to_string(), true))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let base = format!(
        "Local\\DaijobuIsland.Restart.{}.{stamp}",
        std::process::id()
    );
    let ready_name = HSTRING::from(format!("{base}.ready"));
    let go_name = HSTRING::from(format!("{base}.go"));
    // SAFETY: local, initially nonsignaled manual-reset events; no inherited handles.
    let ready = Handle(
        unsafe { CreateEventW(None, true, false, PCWSTR(ready_name.as_ptr())) }
            .map_err(os_failure)?
            .0 as isize,
    );
    // SAFETY: as above; both names are unique to this attempt.
    let go = Handle(
        unsafe { CreateEventW(None, true, false, PCWSTR(go_name.as_ptr())) }
            .map_err(os_failure)?
            .0 as isize,
    );
    let helper = Command::new(exe)
        .arg(if media {
            MEDIA_RESTART_SWITCH
        } else {
            RESTART_SWITCH
        })
        .arg(std::process::id().to_string())
        .arg(&base)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|e| {
            failure(
                format!("Could not start restart helper: {e}. Close and reopen the app manually."),
                true,
            )
        })?;
    // SAFETY: the event remains valid for the bounded wait. Runs off the UI thread.
    if unsafe { WaitForSingleObject(ready.raw(), 10_000) } != WAIT_OBJECT_0 {
        return Err(failure("Restart helper did not become ready. Close and reopen the app manually, or retry restart.".into(), true));
    }
    Ok(RestartTicket { go, helper })
}

/// Early headless route. It never loads config, acquires the app mutex or sets up
/// packages. The ready handshake proves we hold the old process before it exits.
pub fn handle_restart_cli(args: &[OsString]) -> bool {
    let media = args.first().is_some_and(|arg| arg == MEDIA_RESTART_SWITCH);
    if !media && args.first().is_none_or(|arg| arg != RESTART_SWITCH) {
        return false;
    }
    if args.len() == 3 {
        if let (Some(pid), Some(base)) = (
            args[1].to_str().and_then(|s| s.parse::<u32>().ok()),
            args[2].to_str(),
        ) {
            if base.starts_with(&format!("Local\\DaijobuIsland.Restart.{pid}.")) && base.len() < 160
            {
                let _ = restart_helper(pid, base, media);
            }
        }
    }
    true
}

fn restart_helper(pid: u32, base: &str, media: bool) -> Result<(), Failure> {
    let ready_name = HSTRING::from(format!("{base}.ready"));
    let go_name = HSTRING::from(format!("{base}.go"));
    // SAFETY: only synchronization access; never kills or manipulates the parent.
    let parent = Handle(
        unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }
            .map_err(os_failure)?
            .0 as isize,
    );
    // SAFETY: fixed-prefix local named events supplied by our parent process.
    let ready = Handle(
        unsafe { OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(ready_name.as_ptr())) }
            .map_err(os_failure)?
            .0 as isize,
    );
    // SAFETY: synchronization-only event access, kept alive through its wait.
    let go = Handle(
        unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(go_name.as_ptr())) }
            .map_err(os_failure)?
            .0 as isize,
    );
    // SAFETY: opened ready event; signal only after acquiring the parent handle.
    unsafe {
        SetEvent(ready.raw()).map_err(os_failure)?;
    }
    restart_after_exit(
        || {
            // SAFETY: owned commit event; bounded wait with no kill fallback.
            unsafe { WaitForSingleObject(go.raw(), 60_000) == WAIT_OBJECT_0 }
        },
        || {
            // SAFETY: owned parent handle signals after process exit/mutex release.
            unsafe { WaitForSingleObject(parent.raw(), 60_000) == WAIT_OBJECT_0 }
        },
        || {
            use std::os::windows::process::CommandExt;
            let exe = std::env::current_exe().map_err(|e| failure(e.to_string(), true))?;
            std::process::Command::new(exe)
                .arg(if media {
                    OPEN_MEDIA_SWITCH
                } else {
                    OPEN_SWITCH
                })
                .creation_flags(0x08000000)
                .spawn()
                .map_err(|e| failure(e.to_string(), true))?;
            Ok(())
        },
    )
}

fn restart_after_exit(
    wait_commit: impl FnOnce() -> bool,
    wait_exit: impl FnOnce() -> bool,
    launch: impl FnOnce() -> Result<(), Failure>,
) -> Result<(), Failure> {
    if wait_commit() && wait_exit() {
        launch()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_preparation_is_off_thread_and_failure_does_not_deploy() {
        let ui = std::thread::current().id();
        let (tx, rx) = mpsc::channel();
        let job = register(0, move || {
            tx.send(std::thread::current().id()).unwrap();
            Err(failure("fixture: unavailable payload".into(), true))
        })
        .unwrap();
        assert_ne!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), ui);
        let result = job.rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            matches!(result, Completion::Registered(Err(Failure { retryable: true, message })) if message == "fixture: unavailable payload")
        );
    }

    #[test]
    fn abandoned_worker_observes_cancel_and_disconnected_jobs_are_actionable() {
        let (started_tx, started_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let job = launch(0, false, move |cancel| {
            started_tx.send(()).unwrap();
            resume_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            done_tx.send(cancel.load(Ordering::Acquire)).unwrap();
            Completion::Registered(Ok(()))
        })
        .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(job);
        resume_tx.send(()).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_secs(2)).unwrap());
        for restart in [false, true] {
            let (tx, rx) = mpsc::sync_channel(1);
            drop(tx);
            let job = Job {
                rx,
                cancel: Arc::default(),
                restart,
            };
            match job.take().unwrap() {
                Completion::Registered(Err(error)) if !restart => assert!(error.retryable),
                Completion::Restart(Err(error)) if restart => assert!(error.retryable),
                _ => panic!("disconnected worker must not leave the UI busy forever"),
            }
        }
    }

    #[test]
    fn restart_requires_commit_and_actual_exit_in_order_and_preserves_launch_errors() {
        use std::cell::RefCell;
        for (commit, exited) in [(false, false), (true, false), (true, true)] {
            let order = RefCell::new(Vec::new());
            restart_after_exit(
                || {
                    order.borrow_mut().push("commit");
                    commit
                },
                || {
                    order.borrow_mut().push("exit");
                    exited
                },
                || {
                    order.borrow_mut().push("launch");
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(
                *order.borrow(),
                if !commit {
                    vec!["commit"]
                } else if !exited {
                    vec!["commit", "exit"]
                } else {
                    vec!["commit", "exit", "launch"]
                }
            );
        }
        assert!(restart_after_exit(
            || true,
            || true,
            || Err(failure("launch failed".into(), true))
        )
        .is_err());
    }
    #[test]
    fn paths_and_os_errors_are_bounded_and_never_shell_commands() {
        assert_eq!(
            encode_path("C:/it's #1% ?/字.xml"),
            "C:/it%27s%20%231%25%20%3F/%E5%AD%97.xml"
        );
        for code in [0x800B0100u32, 0x800B0109, 0x800B010A] {
            let error = os_failure(windows::core::Error::from_hresult(windows::core::HRESULT(
                code as i32,
            )));
            assert!(error.message.contains("will not add certificate trust"));
            assert!(error.retryable);
        }
        assert_eq!(failure("x".repeat(900), true).message.len(), 600);
    }
    #[test]
    fn unrelated_headless_commands_never_start_setup_or_restart() {
        for args in [
            vec![],
            vec![OsString::from("--agent-notification-hook")],
            vec![OsString::from(OPEN_SWITCH)],
            vec![OsString::from(OPEN_MEDIA_SWITCH)],
        ] {
            assert!(!handle_restart_cli(&args));
        }
        assert!(handle_restart_cli(&[OsString::from(RESTART_SWITCH)]));
        assert!(handle_restart_cli(&[OsString::from(MEDIA_RESTART_SWITCH)]));
        assert!(handle_restart_cli(&[
            OsString::from(RESTART_SWITCH),
            "not-a-pid".into(),
            "bad".into()
        ]));
    }
}
