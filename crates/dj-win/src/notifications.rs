//! UserNotificationListener adapter. Consent starts on the UI thread only after
//! a user action. A coalesced event wakes a sleeping MTA snapshot worker; there is
//! no polling timer, content logging, disk storage, or notification URI execution.

use crate::window;
use dj_core::notifications::{
    bounded_text, Access, Activation, NotificationInput, NotificationKey, HISTORY_LIMIT,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use windows::core::{HSTRING, PCWSTR};
use windows::Foundation::TypedEventHandler;
use windows::Win32::Foundation::{APPMODEL_ERROR_NO_PACKAGE, ERROR_INSUFFICIENT_BUFFER};
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::WinRT::{
    RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED, RO_INIT_SINGLETHREADED,
};
use windows::Win32::UI::Shell::{
    ApplicationActivationManager, IApplicationActivationManager, AO_NONE,
};
use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus as Status,
};
use windows::UI::Notifications::{NotificationKinds, UserNotification};
use windows_future::{AsyncOperationCompletedHandler, IAsyncOperation};

pub(super) struct Apartment;
impl Apartment {
    pub(super) fn init(ui: bool) -> windows::core::Result<Self> {
        // SAFETY: called on the owning thread, balanced by Drop on that thread.
        unsafe {
            RoInitialize(if ui {
                RO_INIT_SINGLETHREADED
            } else {
                RO_INIT_MULTITHREADED
            })?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: balances the successful RoInitialize on this thread.
        unsafe {
            RoUninitialize();
        }
    }
}

#[derive(Clone)]
pub struct NativeNotification {
    pub input: NotificationInput,
    /// OS AppInfo identity, never text extracted from the toast.
    app_id: String,
    created: i64,
}

impl NativeNotification {
    pub fn created(&self) -> i64 {
        self.created
    }
}

#[derive(Default)]
pub struct Update {
    pub access: Option<Access>,
    pub snapshot: Option<Vec<NativeNotification>>,
}

struct Worker {
    stopped: Arc<AtomicBool>,
    tx: SyncSender<()>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.tx.try_send(());
        // Never block the UI waiting for an OS async query. The thread revokes
        // its handler after that query returns; its old mailbox is discarded.
    }
}

pub struct Listener {
    hwnd: isize,
    enabled: bool,
    access: Access,
    worker: Option<Worker>,
    pending: Option<IAsyncOperation<Status>>,
    mailbox: Arc<Mutex<Update>>,
    // Keep this last so COM objects above it are released first.
    apartment: Option<Apartment>,
}

impl Listener {
    pub fn new(hwnd: isize) -> Self {
        Self {
            hwnd,
            enabled: false,
            access: Access::Disabled,
            worker: None,
            pending: None,
            mailbox: Arc::default(),
            apartment: None,
        }
    }

    pub fn access(&self) -> Access {
        self.access
    }

    pub fn configure(&mut self, enabled: bool) {
        self.worker = None;
        // Discard stale permission/query completions from an older opt-in session.
        *self.mailbox.lock().unwrap_or_else(|e| e.into_inner()) = Update::default();
        self.mailbox = Arc::default();
        self.pending = None;
        self.enabled = enabled;
        self.access = if enabled {
            self.probe()
        } else {
            Access::Disabled
        };
        if self.access == Access::Listening {
            self.start();
        }
    }

    fn probe(&mut self) -> Access {
        let mut length = 0;
        // SAFETY: length points to writable storage; a null name buffer queries
        // identity presence/required length and never grants or installs identity.
        let identity = unsafe { GetCurrentPackageFullName(&mut length, None) };
        if identity == APPMODEL_ERROR_NO_PACKAGE {
            return Access::MissingIdentity;
        }
        if identity != ERROR_INSUFFICIENT_BUFFER {
            return Access::Unavailable;
        }
        if self.apartment.is_none() {
            self.apartment = Apartment::init(true).ok();
        }
        if self.apartment.is_none() {
            return Access::Unavailable;
        }
        UserNotificationListener::Current()
            .and_then(|l| l.GetAccessStatus())
            .map(map_status)
            .unwrap_or(Access::Unavailable)
    }

    /// Recheck at user interactions, before reads and actions. Windows can revoke
    /// permission without throwing; no idle polling is used to detect that.
    pub fn refresh_access(&mut self) -> bool {
        if !self.enabled || matches!(self.access, Access::Requesting | Access::Connecting) {
            return false;
        }
        let access = self.probe();
        if access == self.access {
            return false;
        }
        self.access = access;
        self.worker = None;
        if access == Access::Listening {
            self.start();
        }
        true
    }

    /// Must be invoked by the owning UI thread after an explicit Allow access click.
    pub fn request_access(&mut self) {
        if !self.enabled || self.access != Access::NeedsConsent {
            return;
        }
        let result = UserNotificationListener::Current()
            .and_then(|l| l.RequestAccessAsync())
            .and_then(|op| {
                let mailbox = self.mailbox.clone();
                let hwnd = self.hwnd;
                op.SetCompleted(&AsyncOperationCompletedHandler::new(move |op, _| {
                    let access = op
                        .as_ref()
                        .and_then(|o| o.GetResults().ok())
                        .map(map_status)
                        .unwrap_or(Access::Unavailable);
                    mailbox.lock().unwrap_or_else(|e| e.into_inner()).access = Some(access);
                    window::wake(hwnd);
                    Ok(())
                }))?;
                Ok(op)
            });
        match result {
            Ok(op) => {
                self.pending = Some(op);
                self.access = Access::Requesting;
            }
            Err(_) => self.access = Access::Unavailable,
        }
    }

    fn start(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let signal = tx.clone();
        let mailbox = self.mailbox.clone();
        let hwnd = self.hwnd;
        let thread = std::thread::Builder::new()
            .name("notification-listener".into())
            .spawn(move || {
                let publish = |update: Update| {
                    let mut pending = mailbox.lock().unwrap_or_else(|e| e.into_inner());
                    if !stop.load(Ordering::Acquire) {
                        *pending = update;
                        drop(pending);
                        window::wake(hwnd);
                    }
                };
                let run = || -> windows::core::Result<()> {
                    let _apartment = Apartment::init(false)?;
                    let listener = UserNotificationListener::Current()?;
                    let token =
                        listener.NotificationChanged(&TypedEventHandler::new(move |_, _| {
                            // Capacity one coalesces a burst; the worker always resyncs all
                            // current toasts, so added and removed events cannot be lost.
                            let _ = signal.try_send(());
                            Ok(())
                        }))?;
                    loop {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        let access = listener
                            .GetAccessStatus()
                            .map(map_status)
                            .unwrap_or(Access::Unavailable);
                        if access != Access::Listening {
                            publish(Update {
                                access: Some(access),
                                snapshot: Some(Vec::new()),
                            });
                            break;
                        }
                        let result = snapshot(&listener);
                        match result {
                            Ok(items) => {
                                let after = listener
                                    .GetAccessStatus()
                                    .map(map_status)
                                    .unwrap_or(Access::Unavailable);
                                publish(Update {
                                    access: Some(after),
                                    snapshot: Some(if after == Access::Listening {
                                        items
                                    } else {
                                        Vec::new()
                                    }),
                                });
                                if after != Access::Listening {
                                    break;
                                }
                            }
                            Err(_) => {
                                publish(Update {
                                    access: Some(Access::Unavailable),
                                    snapshot: Some(Vec::new()),
                                });
                                break;
                            }
                        }
                        if rx.recv().is_err() {
                            break;
                        }
                    }
                    listener.RemoveNotificationChanged(token)?;
                    Ok(())
                };
                if run().is_err() {
                    publish(Update {
                        access: Some(Access::Unavailable),
                        snapshot: Some(Vec::new()),
                    });
                }
            });
        match thread {
            Ok(_) => {
                self.worker = Some(Worker { stopped, tx });
                self.access = Access::Connecting;
            }
            Err(_) => self.access = Access::Unavailable,
        }
    }

    pub fn drain(&mut self) -> Update {
        let update = std::mem::take(&mut *self.mailbox.lock().unwrap_or_else(|e| e.into_inner()));
        if let Some(access) = update.access {
            self.access = access;
            self.pending = None;
            if access == Access::Listening {
                self.start();
            } else {
                self.worker = None;
            }
        }
        update
    }

    pub fn dismiss(&mut self, id: u32) -> bool {
        self.refresh_access();
        self.access == Access::Listening
            && UserNotificationListener::Current()
                .and_then(|l| l.RemoveNotification(id))
                .is_ok()
    }

    /// Opens the app's default view with no notification arguments. Re-resolves
    /// the OS notification and matches its AppInfo identity before activation.
    pub fn open_source(&mut self, item: &NativeNotification) -> bool {
        self.refresh_access();
        let NotificationKey::Windows(id) = item.input.key else {
            return false;
        };
        if self.access != Access::Listening || !safe_app_id(&item.app_id) {
            return false;
        }
        let result = (|| -> windows::core::Result<()> {
            let n = UserNotificationListener::Current()?.GetNotification(id)?;
            if n.AppInfo()?.AppUserModelId()?.to_string_lossy() != item.app_id {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x80070005u32 as i32,
                )));
            }
            let app_id: Vec<u16> = item.app_id.encode_utf16().chain(Some(0)).collect();
            // SAFETY: apartment is initialized; OS-derived validated AUMID is
            // NUL-terminated and alive. Arguments are explicitly null/empty.
            unsafe {
                let manager: IApplicationActivationManager =
                    CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_INPROC_SERVER)?;
                manager.ActivateApplication(PCWSTR(app_id.as_ptr()), PCWSTR::null(), AO_NONE)?;
            }
            Ok(())
        })();
        result.is_ok()
    }
}

fn map_status(status: Status) -> Access {
    match status {
        Status::Allowed => Access::Listening,
        Status::Denied => Access::Denied,
        Status::Unspecified => Access::NeedsConsent,
        _ => Access::Unavailable,
    }
}

fn safe_app_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '!'))
}

fn convert(n: &UserNotification) -> windows::core::Result<NativeNotification> {
    let id = n.Id()?;
    let app = n.AppInfo()?;
    let app_id = app.AppUserModelId()?.to_string_lossy();
    let source = bounded_text(&app.DisplayInfo()?.DisplayName()?.to_string_lossy(), 96);
    let binding = n
        .Notification()?
        .Visual()?
        .GetBinding(&HSTRING::from("ToastGeneric"))?;
    let texts = binding.GetTextElements()?;
    let mut title = String::new();
    let mut body = String::new();
    for i in 0..texts.Size()?.min(8) {
        let text = bounded_text(&texts.GetAt(i)?.Text()?.to_string_lossy(), 1024);
        if i == 0 {
            title = bounded_text(&text, 256);
        } else if body.chars().count() < 1024 {
            body.push(' ');
            body.push_str(&text);
        }
    }
    let activation = if safe_app_id(&app_id) {
        Activation::OpenSourceApp
    } else {
        Activation::None
    };
    Ok(NativeNotification {
        input: NotificationInput {
            key: NotificationKey::Windows(id),
            source,
            title,
            body: bounded_text(&body, 1024),
            activation,
        },
        app_id,
        created: n.CreationTime()?.UniversalTime,
    })
}

fn snapshot(listener: &UserNotificationListener) -> windows::core::Result<Vec<NativeNotification>> {
    let all = listener
        .GetNotificationsAsync(NotificationKinds::Toast)?
        .join()?;
    let mut newest = Vec::with_capacity(HISTORY_LIMIT + 1);
    for i in 0..all.Size()? {
        let Ok(n) = all.GetAt(i).and_then(|n| convert(&n)) else {
            continue;
        };
        newest.push(n);
        newest.sort_by_key(|n| n.created);
        if newest.len() > HISTORY_LIMIT {
            newest.remove(0);
        }
    }
    // Access may have been revoked while the async query ran.
    if listener.GetAccessStatus()? != Status::Allowed {
        newest.clear();
    }
    Ok(newest)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notification_text_cannot_become_an_app_identity_or_command() {
        for bad in [
            "",
            "https://example.org",
            "cmd /c calc",
            "a\0b",
            "shell:AppsFolder",
            "../app.exe",
        ] {
            assert!(!safe_app_id(bad));
        }
        assert!(safe_app_id("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"));
    }
    #[test]
    fn unpackaged_native_probe_never_requests_consent() {
        let mut listener = Listener::new(0);
        assert_eq!(listener.access(), Access::Disabled);
        listener.configure(true);
        // Read-only probe: this test runner carries no package identity.
        assert_eq!(listener.access(), Access::MissingIdentity);
        listener.request_access();
        assert!(listener.pending.is_none());
        assert!(listener.worker.is_none());
        listener.configure(false);
        assert_eq!(listener.access(), Access::Disabled);
    }
}
