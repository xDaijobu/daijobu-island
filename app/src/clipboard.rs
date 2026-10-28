//! UI-thread coordinator. Native acquisition, storage and transfer preparation are workers.
pub use crate::clipboard_transfer::Target;
use crate::clipboard_transfer::{self, Transfer, TransferEvent};
use dj_clip::{
    monitor::Monitor, store::Store, Generation, GenerationGate, MonitorEvent, MonitorPolicy,
    ReplayTracker, StoreCommand, StoreReply, StoreRequest, StoreResult, Wake,
};
use dj_core::clipboard::{Filter, ItemId, Payload};
use dj_core::clipboard_ui::{Action, View};
use dj_core::config::ClipboardConfig;
use dj_core::term_tabs::TabId;
use dj_win::{
    drag_drop::{DragOutcome, DragPayload},
    HWND,
};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const MAX_REQUESTS: usize = 12;
const MAX_EVENTS: usize = 16;
const DRAG_NAMESPACE: u64 = 1 << 63;

pub enum Event {
    Redraw,
    Close {
        restore: Option<Target>,
    },
    /// Release the panel/keyboard before calling tick again. Do not call close here.
    HideForPaste,
    /// Show the status panel without activating it or taking keyboard focus.
    ShowFeedback,
    TerminalPaste {
        tab: TabId,
        text: String,
    },
    PolicyChanged(ClipboardConfig),
    /// Owned data for the outer OLE loop; head must recheck the physical drag button.
    DragReady {
        request: u64,
        payload: DragPayload,
    },
}

// Small adapters keep controller tests entirely synthetic, including construction.
trait History {
    fn send(&self, request: StoreRequest) -> Result<(), String>;
    fn drain(&mut self) -> Vec<StoreReply>;
    fn shutdown(&mut self);
}
impl History for Store {
    fn send(&self, request: StoreRequest) -> Result<(), String> {
        self.send(request)
    }
    fn drain(&mut self) -> Vec<StoreReply> {
        self.drain()
    }
    fn shutdown(&mut self) {
        self.shutdown();
    }
}
trait Listener {
    fn configure(&self, policy: MonitorPolicy) -> Result<(), String>;
    fn drain(&mut self) -> Vec<MonitorEvent>;
    fn shutdown(&mut self);
}
impl Listener for Monitor {
    fn configure(&self, policy: MonitorPolicy) -> Result<(), String> {
        self.configure(policy)
    }
    fn drain(&mut self) -> Vec<MonitorEvent> {
        self.drain()
    }
    fn shutdown(&mut self) {
        self.shutdown();
    }
}
trait Transfers {
    fn begin(
        &mut self,
        payload: Arc<Payload>,
        plain: bool,
        paste: bool,
        target: Option<Target>,
        now: Instant,
    ) -> Result<(), String>;
    fn tick(&mut self, now: Instant) -> Vec<TransferEvent>;
    fn is_pending(&self) -> bool;
    fn cancel(&mut self);
    fn shutdown(&mut self);
}
impl Transfers for Transfer {
    fn begin(
        &mut self,
        payload: Arc<Payload>,
        plain: bool,
        paste: bool,
        target: Option<Target>,
        now: Instant,
    ) -> Result<(), String> {
        self.begin(payload, plain, paste, target, now)
    }
    fn tick(&mut self, now: Instant) -> Vec<TransferEvent> {
        self.tick(now)
    }
    fn is_pending(&self) -> bool {
        self.is_pending()
    }
    fn cancel(&mut self) {
        self.cancel();
    }
    fn shutdown(&mut self) {
        self.shutdown();
    }
}

#[derive(Clone)]
struct Query {
    revision: u64,
    epoch: u64,
    filter: Filter,
    text: String,
}
enum Purpose {
    Preview,
    Transfer {
        plain: bool,
        paste: bool,
        target: Option<Target>,
    },
    Drag,
}
enum Pending {
    Query(Query),
    Fetch {
        id: ItemId,
        epoch: u64,
        purpose: Purpose,
    },
    Mutation,
    Clear,
    Insert,
    Prune,
}
struct Request {
    generation: u64,
    pending: Pending,
}
struct Invalidation {
    config: ClipboardConfig,
    clear: Option<bool>,
    advanced: bool,
    clear_request: Option<u64>,
    attempts: u16,
}

enum Preparation {
    Search,
    Drag(Arc<Payload>, PathBuf),
}
enum Prepared {
    Search(Option<String>),
    Drag(DragPayload),
}
struct JobResult {
    request: u64,
    generation: u64,
    epoch: u64,
    result: Result<Prepared, String>,
}
struct Job {
    request: u64,
    generation: u64,
    epoch: u64,
    preparation: Preparation,
}

/// A single worker and single outstanding job: cancellation cannot create more threads.
struct PreparationWorker {
    sender: Option<mpsc::SyncSender<Job>>,
    receiver: mpsc::Receiver<JobResult>,
    callback: Arc<Mutex<Option<Wake>>>,
    pending: Option<(u64, u64, u64)>,
}
impl PreparationWorker {
    fn new(wake: Wake) -> Result<Self, String> {
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (results, receiver) = mpsc::sync_channel(1);
        let callback = Arc::new(Mutex::new(Some(wake)));
        let worker_callback = callback.clone();
        std::thread::Builder::new()
            .name("clipboard-preparation".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let result = match job.preparation {
                        Preparation::Search => {
                            clipboard_transfer::read_search_text().map(Prepared::Search)
                        }
                        Preparation::Drag(payload, directory) => {
                            clipboard_transfer::prepare_drag(payload, directory).map(Prepared::Drag)
                        }
                    };
                    if results
                        .send(JobResult {
                            request: job.request,
                            generation: job.generation,
                            epoch: job.epoch,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                    // Teardown removes the callback under this same gate before returning.
                    let callback = worker_callback.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(wake) = callback.as_ref() {
                        wake();
                    } else {
                        break;
                    }
                }
            })
            .map_err(|_| "Clipboard preparation worker unavailable".to_string())?;
        Ok(Self {
            sender: Some(sender),
            receiver,
            callback,
            pending: None,
        })
    }
    fn send(&mut self, job: Job) -> Result<(), String> {
        if self.pending.is_some() {
            return Err("Clipboard preparation busy".into());
        }
        let identity = (job.request, job.generation, job.epoch);
        self.sender
            .as_ref()
            .ok_or("Clipboard preparation stopped")?
            .try_send(job)
            .map_err(|_| "Clipboard preparation busy")?;
        self.pending = Some(identity);
        Ok(())
    }
    fn drain(&mut self) -> Option<JobResult> {
        match self.receiver.try_recv() {
            Ok(result) => {
                self.pending = None;
                Some(result)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending
                    .take()
                    .map(|(request, generation, epoch)| JobResult {
                        request,
                        generation,
                        epoch,
                        result: Err("Clipboard preparation stopped".into()),
                    })
            }
        }
    }
    fn shutdown(&mut self) {
        self.sender = None;
        self.callback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        self.pending = None;
        // No join: a delayed third-party clipboard read must not hang the island.
    }
}
impl Drop for PreparationWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub struct Clipboard {
    pub view: View,
    store: Box<dyn History>,
    monitor: Box<dyn Listener>,
    transfer: Box<dyn Transfers>,
    preparation: PreparationWorker,
    generation: Generation,
    config: ClipboardConfig,
    export_dir: PathBuf,
    target: Option<Target>,
    opened: bool,
    epoch: u64,
    next_request: u64,
    pending: HashMap<u64, Request>,
    invalidation: Option<Invalidation>,
    accepting: bool,
    ready: bool,
    startup_failed: bool,
    revision: u64,
    query_dirty: bool,
    payload: Option<(ItemId, Arc<Payload>)>,
    active_action: Option<u64>,
    drag_request: Option<u64>,
    transfer_paste: bool,
    events: VecDeque<Event>,
    stopped: bool,
}

impl Clipboard {
    pub fn new(
        owner: HWND,
        dbpath: PathBuf,
        exportdir: PathBuf,
        config: ClipboardConfig,
        wake: Wake,
    ) -> Result<Self, String> {
        let generation = Arc::new(GenerationGate::new(1));
        let replay = ReplayTracker::default();
        let store = Store::new(dbpath, generation.clone(), wake.clone())?;
        let monitor = Monitor::new(generation.clone(), replay.clone(), wake.clone())?;
        let transfer = Transfer::new(owner, replay, wake.clone());
        Self::assemble(
            Box::new(store),
            Box::new(monitor),
            Box::new(transfer),
            PreparationWorker::new(wake)?,
            generation,
            exportdir,
            config,
        )
    }

    fn assemble(
        store: Box<dyn History>,
        monitor: Box<dyn Listener>,
        transfer: Box<dyn Transfers>,
        preparation: PreparationWorker,
        generation: Generation,
        export_dir: PathBuf,
        config: ClipboardConfig,
    ) -> Result<Self, String> {
        monitor
            .configure(policy(&config, false))
            .map_err(|_| "Clipboard monitor unavailable")?;
        let mut view = View::default();
        view.enabled = config.enabled;
        view.paused = config.paused;
        Ok(Self {
            view,
            store,
            monitor,
            transfer,
            preparation,
            generation,
            accepting: config.enabled && !config.paused,
            config,
            export_dir,
            target: None,
            opened: false,
            epoch: 1,
            next_request: 1,
            pending: HashMap::new(),
            invalidation: None,
            ready: false,
            startup_failed: false,
            revision: 0,
            query_dirty: false,
            payload: None,
            active_action: None,
            drag_request: None,
            transfer_paste: false,
            events: VecDeque::new(),
            stopped: false,
        })
    }

    pub fn view(&self) -> &View {
        &self.view
    }
    pub fn payload(&self) -> Option<&Payload> {
        self.payload
            .as_ref()
            .filter(|(id, _)| Some(*id) == self.view.selected)
            .map(|(_, p)| p.as_ref())
    }
    pub fn open(&mut self, target: Option<Target>) {
        if self.stopped {
            return;
        }
        self.close();
        self.opened = true;
        self.target = target;
        self.query_dirty = true;
        self.view.loading = self.config.enabled && !self.startup_failed;
        self.redraw();
    }
    pub fn close(&mut self) {
        self.opened = false;
        self.cancel_pending_actions();
        self.payload = None;
        self.view.preview = false;
    }

    /// Cancels a user transfer without changing the visible panel (e.g. before Snap).
    pub fn cancel_pending_actions(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.active_action = None;
        if let Some(request) = self.drag_request {
            self.cancel_drag_request(request);
        }
        self.transfer.cancel();
        self.transfer_paste = false;
        self.events
            .retain(|event| matches!(event, Event::PolicyChanged(_) | Event::Redraw));
    }

    pub fn pending_drag_request(&self) -> Option<u64> {
        self.drag_request
    }

    pub fn cancel_drag_request(&mut self, request: u64) {
        if self.drag_request != Some(request) {
            return;
        }
        self.drag_request = None;
        if self.active_action == Some(request) {
            self.active_action = None;
        }
        self.events.retain(
            |event| !matches!(event, Event::DragReady { request: id, .. } if *id == request),
        );
        self.status("Drag cancelled; exported files are retained");
    }

    pub fn finish_drag(&mut self, request: u64, result: Result<DragOutcome, String>) {
        if self.stopped || self.drag_request != Some(request) {
            return;
        }
        self.drag_request = None;
        self.status(match result {
            Ok(DragOutcome::DroppedCopy) => "Files copied by drag",
            Ok(DragOutcome::Cancelled) => "Drag cancelled; exported files are retained",
            Err(_) => "Clipboard drag failed; exported files are retained",
        });
    }

    /// Head may report native failures without exposing OS error text or paths.
    pub fn set_status(&mut self, message: String) {
        if !self.stopped {
            self.status(match message.as_str() {
                "Ctrl+Alt+V is unavailable; open Clipboard from Home or the tray." => {
                    "Ctrl+Alt+V is unavailable; open Clipboard from Home or the tray."
                }
                "Copied; the original Terminal tab is no longer available."
                | "Terminal target is no longer live" => {
                    "Copied; the original Terminal tab is no longer available."
                }
                _ => "Clipboard operation could not be completed",
            });
            self.emit(Event::ShowFeedback);
        }
    }

    /// Applies a persisted Settings choice too; all transitions start from future events.
    pub fn configure(&mut self, config: ClipboardConfig) {
        if self.stopped {
            return;
        }
        if let Some(transition) = self.invalidation.as_mut() {
            transition.config = config;
        } else if config != self.config {
            self.invalidate(config, None);
        }
    }

    pub fn action(&mut self, action: Action) {
        if self.stopped {
            return;
        }
        if action == Action::Close {
            let restore = self.target.clone();
            self.close();
            self.emit(Event::Close { restore });
            return;
        }
        if self.invalidation.is_some() {
            return;
        }
        match action {
            Action::None => {}
            Action::Redraw => {
                self.drop_unselected_payload();
                self.redraw();
            }
            Action::SearchChanged => {
                self.revision = self.revision.wrapping_add(1);
                self.query_dirty = true;
                self.payload = None;
                self.redraw();
            }
            Action::Enable => {
                let mut config = self.config.clone();
                config.enabled = true;
                config.paused = false;
                self.invalidate(config, None);
            }
            Action::TogglePause => {
                let mut config = self.config.clone();
                config.paused = !config.paused;
                self.invalidate(config, None);
            }
            Action::ClearUnpinned => self.invalidate(self.config.clone(), Some(false)),
            Action::ConfirmClearAll => self.invalidate(self.config.clone(), Some(true)),
            Action::RequestClearAll | Action::CancelClearAll => self.redraw(),
            Action::Copy { plain } => self.fetch(Purpose::Transfer {
                plain,
                paste: false,
                target: None,
            }),
            Action::Paste { plain } => self.fetch(Purpose::Transfer {
                plain,
                paste: true,
                target: self.target.clone(),
            }),
            Action::Drag => self.fetch(Purpose::Drag),
            Action::Preview => {
                if self.view.preview {
                    self.fetch(Purpose::Preview);
                } else {
                    self.payload = None;
                    self.redraw();
                }
            }
            Action::TogglePin => {
                if let Some(row) = self.view.selected_row() {
                    self.submit(
                        StoreCommand::Pin {
                            id: row.id,
                            pinned: !row.pinned,
                        },
                        Pending::Mutation,
                    );
                }
            }
            Action::Delete => {
                if let Some(id) = self.view.selected {
                    self.submit(StoreCommand::Delete(id), Pending::Mutation);
                }
            }
            Action::PasteSearch => self.prepare(Preparation::Search),
            Action::Close => unreachable!(),
        }
    }

    fn invalidate(&mut self, config: ClipboardConfig, clear: Option<bool>) {
        self.accepting = false; // Stop admission even if storage currently holds its commit gate.
        self.payload = None;
        self.active_action = None;
        if let Some(request) = self.drag_request {
            self.cancel_drag_request(request);
        }
        self.epoch = self.epoch.wrapping_add(1);
        self.transfer.cancel();
        self.transfer_paste = false;
        self.invalidation = Some(Invalidation {
            config,
            clear,
            advanced: false,
            clear_request: None,
            attempts: 0,
        });
        self.status(if clear.is_some() {
            "Clearing history…"
        } else {
            "Pausing history…"
        });
    }

    fn advance_invalidation(&mut self) {
        let Some(mut transition) = self.invalidation.take() else {
            return;
        };
        if !transition.advanced {
            if self.generation.try_fetch_add(1, Ordering::SeqCst).is_none() {
                self.invalidation = Some(transition);
                return;
            }
            transition.advanced = true;
            self.pending.clear();
            self.query_dirty = true;
            if self
                .monitor
                .configure(policy(&transition.config, transition.clear.is_some()))
                .is_err()
            {
                self.status("Clipboard capture is stopped; policy could not be applied");
                // Keep admission closed; never claim this choice persisted.
                return;
            }
        }
        if let Some(include_pinned) = transition.clear {
            if transition.clear_request.is_none() {
                transition.attempts += 1;
                match self.send(StoreCommand::Clear { include_pinned }, Pending::Clear) {
                    Ok(request) => transition.clear_request = Some(request),
                    Err(_) if transition.attempts < 64 => {}
                    Err(_) => {
                        self.finish_invalidation(transition, false);
                        return;
                    }
                }
            }
            self.invalidation = Some(transition);
        } else {
            self.finish_invalidation(transition, true);
        }
    }

    fn finish_invalidation(&mut self, transition: Invalidation, success: bool) {
        if self
            .monitor
            .configure(policy(&transition.config, false))
            .is_err()
        {
            self.status("Clipboard capture is stopped; policy could not be applied");
            return;
        }
        let changed = self.config != transition.config;
        self.config = transition.config;
        self.view.enabled = self.config.enabled;
        self.view.paused = self.config.paused;
        self.accepting = self.config.enabled && !self.config.paused;
        self.view.confirm_clear_all = false;
        self.view.include_pinned = false;
        self.query_dirty = true;
        self.status(if !success {
            "History could not be cleared"
        } else if transition.clear.is_some() {
            "History cleared"
        } else if self.config.paused {
            "History paused"
        } else if self.config.enabled {
            "Recording future clipboard changes"
        } else {
            "History disabled"
        });
        if changed {
            self.emit(Event::PolicyChanged(self.config.clone()));
        }
    }

    fn id(&mut self) -> u64 {
        let id = self.next_request;
        self.next_request = self
            .next_request
            .checked_add(1)
            .expect("Clipboard request counter exhausted");
        id
    }
    fn send(&mut self, command: StoreCommand, pending: Pending) -> Result<u64, String> {
        if self.pending.len() >= MAX_REQUESTS {
            return Err("Clipboard busy".into());
        }
        let request = self.id()
            | if matches!(
                &pending,
                Pending::Fetch {
                    purpose: Purpose::Drag,
                    ..
                }
            ) {
                DRAG_NAMESPACE
            } else {
                0
            };
        let generation = self.generation.load(Ordering::SeqCst);
        self.store.send(StoreRequest {
            request,
            generation,
            command,
        })?;
        self.pending.insert(
            request,
            Request {
                generation,
                pending,
            },
        );
        Ok(request)
    }
    fn submit(&mut self, command: StoreCommand, pending: Pending) {
        if self.send(command, pending).is_err() {
            self.status("Clipboard operation unavailable or busy");
        }
    }
    fn fetch(&mut self, purpose: Purpose) {
        if !self.opened
            || self.active_action.is_some()
            || self.transfer.is_pending()
            || self.preparation.pending.is_some()
            || self.drag_request.is_some()
            || self
                .pending
                .values()
                .any(|r| matches!(r.pending, Pending::Fetch { .. }))
        {
            self.status("Clipboard operation already pending");
            return;
        }
        let Some(id) = self.view.selected_row().map(|row| row.id) else {
            self.status("Select an available history item");
            return;
        };
        self.payload = None; // C4 permits one lease, including transfer/preparation leases.
        match self.send(
            StoreCommand::Fetch(id),
            Pending::Fetch {
                id,
                epoch: self.epoch,
                purpose,
            },
        ) {
            Ok(request) => {
                self.active_action = Some(request);
                if request & DRAG_NAMESPACE != 0 {
                    self.drag_request = Some(request);
                }
            }
            Err(_) => self.status("Clipboard item unavailable or busy"),
        }
    }
    fn prepare(&mut self, preparation: Preparation) {
        if !self.opened || self.active_action.is_some() || self.transfer.is_pending() {
            self.status("Clipboard operation already pending");
            return;
        }
        self.payload = None;
        let request = match &preparation {
            Preparation::Drag(..) => match self.drag_request {
                Some(request) => request,
                None => return,
            },
            Preparation::Search => self.id(),
        };
        if self
            .preparation
            .send(Job {
                request,
                generation: self.generation.load(Ordering::SeqCst),
                epoch: self.epoch,
                preparation,
            })
            .is_err()
        {
            if request & DRAG_NAMESPACE != 0 {
                self.drag_request = None;
            }
            self.status("Clipboard preparation unavailable or busy");
        } else {
            self.active_action = Some(request);
        }
    }

    /// Optional head-owned coarse expiry scheduling. No continuous idle timer is needed.
    pub fn prune(&mut self, now_ms: u64) {
        if !self.stopped
            && self.ready
            && self.invalidation.is_none()
            && !self
                .pending
                .values()
                .any(|r| matches!(r.pending, Pending::Prune))
        {
            self.submit(StoreCommand::Prune { now_ms }, Pending::Prune);
        }
    }

    pub fn tick(&mut self, now: Instant) -> Vec<Event> {
        if self.stopped {
            return Vec::new();
        }
        self.advance_invalidation();
        for event in self.monitor.drain() {
            match event {
                MonitorEvent::Captured(capture)
                    if self.accepting
                        && self.invalidation.is_none()
                        && capture.generation == self.generation.load(Ordering::SeqCst) =>
                {
                    if self
                        .send(StoreCommand::Insert(capture), Pending::Insert)
                        .is_err()
                    {
                        self.status("Clipboard busy; a capture was skipped");
                    }
                }
                MonitorEvent::Captured(_) => {}
                MonitorEvent::Skipped { count, .. } => {
                    self.status(&format!("Clipboard skipped {count} captures"));
                }
                MonitorEvent::Error(_) => self.status("Clipboard capture unavailable"),
            }
        }
        for reply in self.store.drain() {
            self.reply(reply, now);
        }
        if let Some(result) = self.preparation.drain() {
            if self.active_action == Some(result.request) {
                self.active_action = None;
                if self.opened
                    && self.invalidation.is_none()
                    && result.epoch == self.epoch
                    && result.generation == self.generation.load(Ordering::SeqCst)
                {
                    match result.result {
                        Ok(Prepared::Search(Some(text))) => {
                            self.view.insert_search(&text);
                            self.action(Action::SearchChanged);
                        }
                        Ok(Prepared::Search(None)) => self.status("Clipboard has no search text"),
                        Ok(Prepared::Drag(payload))
                            if self.drag_request == Some(result.request) =>
                        {
                            self.emit(Event::DragReady {
                                request: result.request,
                                payload,
                            })
                        }
                        Ok(Prepared::Drag(_)) => {}
                        Err(_) => {
                            if self.drag_request == Some(result.request) {
                                self.drag_request = None;
                            }
                            self.status("Clipboard preparation failed");
                        }
                    }
                }
            }
        }
        // Copied is returned before the transfer's next activation tick. Head must
        // process HideForPaste before calling this controller again.
        for event in self.transfer.tick(now) {
            match event {
                TransferEvent::Copied => {
                    self.status("Copied");
                    if self.transfer_paste {
                        self.emit(Event::HideForPaste);
                    }
                }
                TransferEvent::TerminalPaste { tab, text } => {
                    self.emit(Event::TerminalPaste { tab, text })
                }
                TransferEvent::ManualPaste(reason) => {
                    self.status(safe_manual(&reason));
                    if self.transfer_paste {
                        self.emit(Event::ShowFeedback);
                    }
                }
                TransferEvent::Failed(_) => {
                    self.status("Clipboard transfer failed; automatic paste was not sent");
                    if self.transfer_paste {
                        self.emit(Event::ShowFeedback);
                    }
                }
            }
        }
        if !self.transfer.is_pending() {
            self.transfer_paste = false;
        }
        self.drop_unselected_payload();
        self.query();
        self.events.drain(..).collect()
    }

    fn reply(&mut self, reply: StoreReply, now: Instant) {
        if reply.request == 0 {
            match reply.result {
                Ok(StoreResult::Ready) => {
                    self.ready = true;
                    self.startup_failed = false;
                    self.query_dirty = true;
                }
                Ok(StoreResult::Changed { .. })
                    if reply.generation == self.generation.load(Ordering::SeqCst) =>
                {
                    self.query_dirty = true
                }
                Err(error) if reply.generation == self.generation.load(Ordering::SeqCst) => {
                    if !self.ready {
                        self.startup_failed = true;
                    }
                    self.view.loading = false;
                    self.status(storage_error(
                        &error,
                        "Clipboard history storage unavailable",
                    ));
                }
                _ => {}
            }
            return;
        }
        // A wrong-generation reply must never consume the real pending request.
        let Some(request) = self.pending.get(&reply.request) else {
            return;
        };
        if request.generation != reply.generation
            || reply.generation != self.generation.load(Ordering::SeqCst)
        {
            return;
        }
        let request = self
            .pending
            .remove(&reply.request)
            .expect("matched request");
        if matches!(request.pending, Pending::Clear) {
            if let Some(transition) = self.invalidation.take() {
                if transition.clear_request == Some(reply.request) {
                    let success = matches!(reply.result, Ok(StoreResult::Changed { .. }));
                    self.finish_invalidation(transition, success);
                } else {
                    self.invalidation = Some(transition);
                }
            }
            return;
        }
        match request.pending {
            Pending::Query(query) => {
                if !self.opened
                    || self.invalidation.is_some()
                    || query.epoch != self.epoch
                    || query.revision != self.revision
                    || query.filter != self.view.filter
                    || query.text != self.view.query
                {
                    self.query_dirty = true;
                    return;
                }
                match reply.result {
                    Ok(StoreResult::Rows { rows, corrupt, .. }) => {
                        self.view.set_rows(rows);
                        if self.view.rows.is_empty()
                            && self
                                .view
                                .status
                                .as_deref()
                                .is_some_and(informational_status)
                        {
                            self.view.status = None;
                        }
                        if corrupt > 0 {
                            self.status("Some history items are unavailable or corrupt");
                        }
                        self.redraw();
                    }
                    _ => {
                        self.view.loading = false;
                        self.status("Clipboard history could not be loaded");
                    }
                }
            }
            Pending::Fetch { id, epoch, purpose } => {
                if self.active_action != Some(reply.request) {
                    return;
                }
                self.active_action = None;
                if !self.opened || epoch != self.epoch || self.invalidation.is_some() {
                    return;
                }
                match reply.result {
                    Ok(StoreResult::Item { id: got, payload }) if got == id => match purpose {
                        Purpose::Preview if self.view.selected == Some(id) && self.view.preview => {
                            self.payload = Some((id, payload));
                            self.redraw();
                        }
                        Purpose::Preview => {}
                        Purpose::Transfer {
                            plain,
                            paste,
                            target,
                        } => match self.transfer.begin(payload, plain, paste, target, now) {
                            Ok(()) => self.transfer_paste = paste,
                            Err(_) => self.status("Clipboard transfer unavailable"),
                        },
                        Purpose::Drag => {
                            self.prepare(Preparation::Drag(payload, self.export_dir.clone()))
                        }
                    },
                    _ => {
                        if self.drag_request == Some(reply.request) {
                            self.drag_request = None;
                        }
                        self.status("Clipboard item is unavailable or corrupt");
                    }
                }
            }
            Pending::Mutation | Pending::Insert | Pending::Prune => match reply.result {
                Ok(StoreResult::Changed { .. }) => {
                    self.query_dirty = true;
                    self.payload = None;
                    self.redraw();
                }
                Ok(StoreResult::Skipped) => {}
                Err(error) => self.status(storage_error(&error, "Clipboard history change failed")),
                _ => self.status("Clipboard history change failed"),
            },
            Pending::Clear => unreachable!(),
        }
    }
    fn query(&mut self) {
        if !self.opened
            || !self.config.enabled
            || !self.ready
            || !self.query_dirty
            || self.invalidation.is_some()
            || self
                .pending
                .values()
                .any(|r| matches!(r.pending, Pending::Query(_)))
        {
            return;
        }
        let query = Query {
            revision: self.revision,
            epoch: self.epoch,
            filter: self.view.filter,
            text: self.view.query.clone(),
        };
        if self
            .send(
                StoreCommand::Query {
                    filter: query.filter,
                    query: query.text.clone(),
                    now_ms: wall_ms(),
                },
                Pending::Query(query),
            )
            .is_ok()
        {
            self.query_dirty = false;
            self.view.loading = true;
            self.redraw();
        } else {
            self.view.loading = false;
            self.query_dirty = false;
            self.status("Clipboard search unavailable or busy");
        }
    }
    pub fn needs_tick(&self) -> bool {
        !self.stopped
            && (self.invalidation.is_some()
                || self.transfer.is_pending()
                || !self.events.is_empty())
    }
    fn drop_unselected_payload(&mut self) {
        if self
            .payload
            .as_ref()
            .is_some_and(|(id, _)| Some(*id) != self.view.selected || !self.view.preview)
        {
            self.payload = None;
        }
    }
    fn emit(&mut self, event: Event) {
        if self.events.len() < MAX_EVENTS {
            self.events.push_back(event);
        }
    }
    fn redraw(&mut self) {
        if !self
            .events
            .iter()
            .any(|event| matches!(event, Event::Redraw))
        {
            self.emit(Event::Redraw);
        }
    }
    fn status(&mut self, text: &str) {
        self.view.status = Some(text.into());
        self.redraw();
    }
    pub fn shutdown(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.accepting = false;
        self.close();
        self.monitor.shutdown();
        self.store.shutdown();
        self.transfer.shutdown();
        self.preparation.shutdown();
        self.pending.clear();
        self.events.clear();
    }
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn policy(config: &ClipboardConfig, force_pause: bool) -> MonitorPolicy {
    MonitorPolicy {
        enabled: config.enabled,
        paused: config.paused || force_pause,
        excluded_sources: config.excluded_sources.clone(),
    }
}

fn safe_manual(message: &str) -> &'static str {
    match message {
        "Copied; this item cannot be pasted into Terminal as text." => {
            "Copied; this item cannot be pasted into Terminal as text."
        }
        "Copied; focus changed before target activation. Paste manually." => {
            "Copied; focus changed before target activation. Paste manually."
        }
        "Copied; image-only items cannot be pasted into Terminal." => {
            "Copied; image-only items cannot be pasted into Terminal."
        }
        "Copied; no paste target is available." => "Copied; no paste target is available.",
        "Copied; automatic paste timed out. Paste manually." => {
            "Copied; automatic paste timed out. Paste manually."
        }
        "Copied; the clipboard changed before paste. Paste manually." => {
            "The clipboard changed before paste; automatic paste was cancelled."
        }
        "Copied; the target closed or changed identity. Paste manually." => {
            "Copied; the target closed or changed identity. Paste manually."
        }
        "Copied; the target does not allow automatic input. Paste manually." => {
            "Copied; the target does not allow automatic input. Paste manually."
        }
        "Copied; the target could not be activated. Paste manually." => {
            "Copied; the target could not be activated. Paste manually."
        }
        "Copied; focus changed before paste. Paste manually." => {
            "Copied; focus changed before paste. Paste manually."
        }
        "Copied; selection or shortcut keys are still held. Paste manually." => {
            "Copied; selection or shortcut keys are still held. Paste manually."
        }
        "Copied; the target changed before paste. Paste manually." => {
            "Copied; the target changed before paste. Paste manually."
        }
        "Copied; automatic input was refused or incomplete. Paste manually." => {
            "Copied; automatic input was refused or incomplete. Check the target before retrying."
        }
        _ => "Copied; use the target application's paste command",
    }
}

fn informational_status(message: &str) -> bool {
    matches!(
        message,
        "Copied"
            | "Recording future clipboard changes"
            | "History cleared"
            | "History paused"
            | "History disabled"
            | "Files copied by drag"
            | "Drag cancelled; exported files are retained"
    )
}

fn storage_error(message: &str, fallback: &'static str) -> &'static str {
    match message {
        "Clipboard history full" => "History full; delete an item or unpin older entries",
        "Clipboard pin limit reached" => "Pin limit reached; unpin an item before pinning another",
        "Clipboard item no longer exists" => "The history item is no longer available",
        "Unsupported Clipboard database version" => {
            "This history database needs a newer version of Daijobu Island"
        }
        "Clipboard database is corrupt" | "Invalid Clipboard database schema" => {
            "Clipboard history database is damaged; it has been kept unchanged"
        }
        "Clipboard preview memory limit" => "Clipboard preview memory limit reached",
        "Clipboard store busy" => "Clipboard history is busy; try again",
        _ => fallback,
    }
}
fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_transfer_categories_keep_actionable_manual_feedback() {
        for text in [
            "Copied; this item cannot be pasted into Terminal as text.",
            "Copied; focus changed before target activation. Paste manually.",
        ] {
            assert_eq!(safe_manual(text), text);
        }
    }
    use dj_core::clipboard::{Content, Kind, Row};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct StoreState {
        sent: VecDeque<StoreRequest>,
        replies: Vec<StoreReply>,
        stopped: bool,
        fail: bool,
    }
    struct FakeStore(Rc<RefCell<StoreState>>);
    impl History for FakeStore {
        fn send(&self, request: StoreRequest) -> Result<(), String> {
            let mut state = self.0.borrow_mut();
            if state.fail {
                return Err("synthetic sensitive /private/path".into());
            }
            state.sent.push_back(request);
            Ok(())
        }
        fn drain(&mut self) -> Vec<StoreReply> {
            std::mem::take(&mut self.0.borrow_mut().replies)
        }
        fn shutdown(&mut self) {
            self.0.borrow_mut().stopped = true;
        }
    }
    #[derive(Default)]
    struct MonitorState {
        policies: Vec<MonitorPolicy>,
        events: Vec<MonitorEvent>,
        stopped: bool,
    }
    struct FakeMonitor(Rc<RefCell<MonitorState>>);
    impl Listener for FakeMonitor {
        fn configure(&self, policy: MonitorPolicy) -> Result<(), String> {
            self.0.borrow_mut().policies.push(policy);
            Ok(())
        }
        fn drain(&mut self) -> Vec<MonitorEvent> {
            std::mem::take(&mut self.0.borrow_mut().events)
        }
        fn shutdown(&mut self) {
            self.0.borrow_mut().stopped = true;
        }
    }
    struct Began {
        payload: Arc<Payload>,
        plain: bool,
        paste: bool,
        target: Option<Target>,
    }
    #[derive(Default)]
    struct TransferState {
        began: Vec<Began>,
        pending: bool,
        ticks: usize,
        events: Vec<TransferEvent>,
        stopped: bool,
    }
    struct FakeTransfer(Rc<RefCell<TransferState>>);
    impl Transfers for FakeTransfer {
        fn begin(
            &mut self,
            payload: Arc<Payload>,
            plain: bool,
            paste: bool,
            target: Option<Target>,
            _: Instant,
        ) -> Result<(), String> {
            let mut state = self.0.borrow_mut();
            state.began.push(Began {
                payload,
                plain,
                paste,
                target,
            });
            state.pending = true;
            Ok(())
        }
        fn tick(&mut self, _: Instant) -> Vec<TransferEvent> {
            let mut state = self.0.borrow_mut();
            state.ticks += 1;
            std::mem::take(&mut state.events)
        }
        fn is_pending(&self) -> bool {
            self.0.borrow().pending
        }
        fn cancel(&mut self) {
            self.0.borrow_mut().pending = false;
        }
        fn shutdown(&mut self) {
            self.0.borrow_mut().stopped = true;
        }
    }
    struct Rig {
        clipboard: Clipboard,
        store: Rc<RefCell<StoreState>>,
        monitor: Rc<RefCell<MonitorState>>,
        transfer: Rc<RefCell<TransferState>>,
        jobs: mpsc::Receiver<Job>,
        results: mpsc::SyncSender<JobResult>,
    }
    impl Rig {
        fn new(enabled: bool) -> Self {
            let store = Rc::new(RefCell::new(StoreState::default()));
            let monitor = Rc::new(RefCell::new(MonitorState::default()));
            let transfer = Rc::new(RefCell::new(TransferState::default()));
            let (sender, jobs) = mpsc::sync_channel(1);
            let (results, receiver) = mpsc::sync_channel(1);
            let preparation = PreparationWorker {
                sender: Some(sender),
                receiver,
                callback: Arc::new(Mutex::new(None)),
                pending: None,
            };
            let mut clipboard = Clipboard::assemble(
                Box::new(FakeStore(store.clone())),
                Box::new(FakeMonitor(monitor.clone())),
                Box::new(FakeTransfer(transfer.clone())),
                preparation,
                Arc::new(GenerationGate::new(1)),
                PathBuf::from("synthetic-export"),
                ClipboardConfig {
                    enabled,
                    ..ClipboardConfig::default()
                },
            )
            .unwrap();
            clipboard.ready = true;
            clipboard.opened = true;
            clipboard.view.set_rows(vec![row(11), row(22)]);
            Self {
                clipboard,
                store,
                monitor,
                transfer,
                jobs,
                results,
            }
        }
        fn tick(&mut self) -> Vec<Event> {
            self.clipboard.tick(Instant::now())
        }
        fn request(&self) -> StoreRequest {
            self.store
                .borrow_mut()
                .sent
                .pop_front()
                .expect("expected store request")
        }
        fn reply(&self, request: &StoreRequest, result: StoreResult) {
            self.store.borrow_mut().replies.push(StoreReply {
                request: request.request,
                generation: request.generation,
                result: Ok(result),
            });
        }
        fn capture(&self, generation: u64) {
            self.monitor
                .borrow_mut()
                .events
                .push(MonitorEvent::Captured(dj_clip::Captured {
                    sequence: 3,
                    generation,
                    at_ms: 1,
                    source: None,
                    payload: payload("synthetic capture"),
                }));
        }
        fn finish_job(&self, job: Job, result: Result<Prepared, String>) {
            self.results
                .send(JobResult {
                    request: job.request,
                    generation: job.generation,
                    epoch: job.epoch,
                    result,
                })
                .unwrap();
        }
    }
    fn payload(text: &str) -> Arc<Payload> {
        Arc::new(Payload {
            content: Content::Text {
                text: text.into(),
                html: None,
                rtf: None,
            },
            no_cloud: false,
        })
    }
    fn row(id: u64) -> Row {
        Row {
            id: ItemId(id),
            created_ms: 0,
            used_ms: 0,
            pinned: false,
            kind: Kind::Text,
            preview: "synthetic row".into(),
            source: None,
            thumbnail: None,
            bytes: 3,
            corrupt: false,
        }
    }
    fn drag() -> DragPayload {
        DragPayload {
            paths: vec![PathBuf::from("synthetic-retained.png")],
            image: None,
            ghost: None,
        }
    }

    #[test]
    fn clipboard_enable_starts_future_generation_without_import() {
        let mut rig = Rig::new(false);
        assert!(!rig.monitor.borrow().policies[0].enabled);
        assert!(rig.store.borrow().sent.is_empty());
        rig.capture(1);
        rig.clipboard.action(Action::Enable);
        assert!(!rig.clipboard.accepting);
        let events = rig.tick();
        assert!(events
            .iter()
            .any(|event| matches!(event,Event::PolicyChanged(c) if c.enabled && !c.paused)));
        assert!(!rig
            .store
            .borrow()
            .sent
            .iter()
            .any(|r| matches!(r.command, StoreCommand::Insert(_))));
        rig.capture(2);
        rig.tick();
        assert_eq!(
            rig.store
                .borrow()
                .sent
                .iter()
                .filter(|r| matches!(r.command, StoreCommand::Insert(_)))
                .count(),
            1
        );
    }

    #[test]
    fn clipboard_pause_defers_generation_without_waiting_on_commit() {
        let mut rig = Rig::new(true);
        let generation = rig.clipboard.generation.clone();
        let guard = generation.commit_guard();
        rig.clipboard.action(Action::TogglePause);
        rig.capture(1);
        let events = rig.tick(); // Deadlocks if a blocking fetch_add is accidentally used.
        assert!(!rig.clipboard.accepting);
        assert!(rig.clipboard.needs_tick());
        assert!(!rig.clipboard.view.paused);
        assert!(rig.store.borrow().sent.is_empty());
        assert!(!events
            .iter()
            .any(|event| matches!(event, Event::PolicyChanged(_))));
        drop(guard);
        let events = rig.tick();
        assert_eq!(generation.load(Ordering::SeqCst), 2);
        assert!(rig.clipboard.view.paused);
        assert!(events
            .iter()
            .any(|event| matches!(event,Event::PolicyChanged(c) if c.paused)));
        assert!(!rig.clipboard.needs_tick());
    }

    #[test]
    fn clipboard_clear_waits_for_exact_ack_and_stops_capture() {
        let mut rig = Rig::new(true);
        rig.clipboard.action(Action::ClearUnpinned);
        rig.tick();
        let clear = rig.request();
        assert!(matches!(
            clear.command,
            StoreCommand::Clear {
                include_pinned: false
            }
        ));
        assert_eq!(clear.generation, 2);
        assert_eq!(rig.clipboard.view.rows.len(), 2);
        assert!(rig.monitor.borrow().policies.last().unwrap().paused);
        rig.capture(2);
        rig.tick();
        assert!(rig.store.borrow().sent.is_empty());
        rig.store.borrow_mut().replies.push(StoreReply {
            request: clear.request,
            generation: 1,
            result: Ok(StoreResult::Changed { id: None }),
        });
        rig.tick();
        assert!(!rig.clipboard.accepting);
        assert!(rig.clipboard.pending.contains_key(&clear.request));
        rig.reply(&clear, StoreResult::Changed { id: None });
        rig.tick();
        assert!(rig.clipboard.accepting);
        assert!(!rig.monitor.borrow().policies.last().unwrap().paused);
        assert_eq!(
            rig.clipboard.view.status.as_deref(),
            Some("History cleared")
        );
        assert!(matches!(rig.request().command, StoreCommand::Query { .. }));
    }

    #[test]
    fn clipboard_failed_clear_preserves_rows_and_selection() {
        let mut rig = Rig::new(true);
        rig.clipboard.action(Action::ConfirmClearAll);
        rig.tick();
        let clear = rig.request();
        assert!(matches!(
            clear.command,
            StoreCommand::Clear {
                include_pinned: true
            }
        ));
        rig.store.borrow_mut().replies.push(StoreReply {
            request: clear.request,
            generation: clear.generation,
            result: Err("private filename and database details".into()),
        });
        rig.tick();
        assert_eq!(rig.clipboard.view.rows.len(), 2);
        assert_eq!(rig.clipboard.view.selected, Some(ItemId(11)));
        assert_eq!(
            rig.clipboard.view.status.as_deref(),
            Some("History could not be cleared")
        );
        assert!(rig.clipboard.accepting);
    }

    #[test]
    fn clipboard_fetch_matches_request_generation_and_exact_item() {
        let mut rig = Rig::new(true);
        rig.clipboard.action(Action::Copy { plain: false });
        let fetch = rig.request();
        rig.store.borrow_mut().replies.push(StoreReply {
            request: fetch.request,
            generation: 2,
            result: Ok(StoreResult::Item {
                id: ItemId(11),
                payload: payload("stale"),
            }),
        });
        rig.tick();
        assert!(rig.clipboard.pending.contains_key(&fetch.request));
        assert!(rig.transfer.borrow().began.is_empty());
        rig.reply(
            &fetch,
            StoreResult::Item {
                id: ItemId(22),
                payload: payload("wrong id"),
            },
        );
        rig.tick();
        assert!(rig.transfer.borrow().began.is_empty());
        assert!(rig.clipboard.active_action.is_none());
    }

    #[test]
    fn clipboard_paste_captures_item_and_exact_target_before_fetch() {
        let mut rig = Rig::new(true);
        rig.clipboard.target = Some(Target::Terminal(77));
        rig.clipboard.action(Action::Paste { plain: true });
        let fetch = rig.request();
        assert!(matches!(fetch.command, StoreCommand::Fetch(ItemId(11))));
        rig.clipboard.view.selected = Some(ItemId(22));
        rig.clipboard.target = Some(Target::Terminal(88));
        rig.reply(
            &fetch,
            StoreResult::Item {
                id: ItemId(11),
                payload: payload("exact full text"),
            },
        );
        rig.tick();
        let state = rig.transfer.borrow();
        assert_eq!(state.began.len(), 1);
        let began = &state.began[0];
        assert!(began.plain && began.paste);
        assert_eq!(began.target, Some(Target::Terminal(77)));
        assert_eq!(
            began.payload.plain_text().as_deref(),
            Some("exact full text")
        );
    }

    #[test]
    fn clipboard_query_coalesces_and_rejects_previous_query_and_open_epoch() {
        let mut rig = Rig::new(true);
        rig.clipboard.action(Action::SearchChanged);
        rig.tick();
        let first = rig.request();
        rig.clipboard.view.query = "日本語😀".into();
        rig.clipboard.action(Action::SearchChanged);
        rig.tick();
        assert!(rig.store.borrow().sent.is_empty());
        rig.reply(
            &first,
            StoreResult::Rows {
                rows: vec![row(99)],
                total: 1,
                encrypted_bytes: 0,
                corrupt: 0,
            },
        );
        rig.tick();
        assert_eq!(rig.clipboard.view.rows[0].id, ItemId(11));
        let latest = rig.request();
        assert!(matches!(&latest.command,StoreCommand::Query { query,.. } if query == "日本語😀"));
        rig.clipboard.close();
        rig.clipboard.open(None);
        rig.reply(
            &latest,
            StoreResult::Rows {
                rows: vec![row(88)],
                total: 1,
                encrypted_bytes: 0,
                corrupt: 0,
            },
        );
        rig.tick();
        assert_eq!(rig.clipboard.view.rows[0].id, ItemId(11));
        assert!(matches!(rig.request().command, StoreCommand::Query { .. }));
    }

    #[test]
    fn clipboard_background_changes_do_not_query_closed_panel() {
        let mut rig = Rig::new(true);
        rig.clipboard.close();
        rig.store.borrow_mut().replies.push(StoreReply {
            request: 0,
            generation: 1,
            result: Ok(StoreResult::Changed { id: None }),
        });
        rig.capture(1);
        rig.tick();
        let insert = rig.request();
        assert!(matches!(insert.command, StoreCommand::Insert(_)));
        rig.reply(
            &insert,
            StoreResult::Changed {
                id: Some(ItemId(22)),
            },
        );
        rig.tick();
        assert!(rig.store.borrow().sent.is_empty());
        assert!(!rig.clipboard.needs_tick());
    }

    #[test]
    fn clipboard_only_one_fetch_and_drops_preview_lease_before_fetch() {
        let mut rig = Rig::new(true);
        let old = payload("preview lease");
        rig.clipboard.payload = Some((ItemId(11), old.clone()));
        rig.clipboard.view.preview = true;
        rig.clipboard.action(Action::Copy { plain: false });
        assert_eq!(Arc::strong_count(&old), 1);
        rig.clipboard.action(Action::Paste { plain: false });
        assert_eq!(rig.store.borrow().sent.len(), 1);
        assert!(rig.clipboard.payload().is_none());
    }

    #[test]
    fn clipboard_drag_uses_one_high_bit_id_through_fetch_prepare_finish() {
        let mut rig = Rig::new(true);
        rig.clipboard.action(Action::Drag);
        let fetch = rig.request();
        assert_ne!(fetch.request & DRAG_NAMESPACE, 0);
        assert_eq!(rig.clipboard.pending_drag_request(), Some(fetch.request));
        rig.reply(
            &fetch,
            StoreResult::Item {
                id: ItemId(11),
                payload: payload("fake drag"),
            },
        );
        rig.tick();
        let job = rig.jobs.try_recv().unwrap();
        assert_eq!(job.request, fetch.request);
        assert_eq!(job.generation, fetch.generation);
        rig.finish_job(job, Ok(Prepared::Drag(drag())));
        let events = rig.tick();
        assert!(events.iter().any(
            |event| matches!(event,Event::DragReady { request,.. } if *request == fetch.request)
        ));
        rig.clipboard
            .finish_drag(fetch.request, Ok(DragOutcome::DroppedCopy));
        assert!(rig.clipboard.pending_drag_request().is_none());
        assert_eq!(rig.clipboard.view.selected, Some(ItemId(11)));
        assert_eq!(
            rig.clipboard.view.status.as_deref(),
            Some("Files copied by drag")
        );
    }

    #[test]
    fn clipboard_drag_cancel_during_fetch_ignores_late_payload() {
        let mut rig = Rig::new(true);
        rig.clipboard.action(Action::Drag);
        let fetch = rig.request();
        rig.clipboard.cancel_drag_request(fetch.request);
        rig.reply(
            &fetch,
            StoreResult::Item {
                id: ItemId(11),
                payload: payload("late"),
            },
        );
        let events = rig.tick();
        assert!(rig.jobs.try_recv().is_err());
        assert!(!events
            .iter()
            .any(|event| matches!(event, Event::DragReady { .. })));
        assert_eq!(rig.clipboard.view.selected, Some(ItemId(11)));
    }

    #[test]
    fn clipboard_drag_cancel_during_preparation_bounds_jobs_and_discards_ready() {
        let mut rig = Rig::new(true);
        rig.clipboard.action(Action::Drag);
        let fetch = rig.request();
        rig.reply(
            &fetch,
            StoreResult::Item {
                id: ItemId(11),
                payload: payload("fake"),
            },
        );
        rig.tick();
        let job = rig.jobs.try_recv().unwrap();
        rig.clipboard.cancel_drag_request(fetch.request);
        rig.clipboard.action(Action::Drag);
        assert!(rig.store.borrow().sent.is_empty());
        rig.finish_job(job, Ok(Prepared::Drag(drag())));
        assert!(!rig
            .tick()
            .iter()
            .any(|event| matches!(event, Event::DragReady { .. })));
        assert_eq!(rig.clipboard.view.selected, Some(ItemId(11)));
    }

    #[test]
    fn clipboard_explicit_search_paste_is_a_bounded_worker_job() {
        let mut rig = Rig::new(true);
        rig.clipboard.action(Action::PasteSearch);
        let job = rig.jobs.try_recv().unwrap();
        assert!(matches!(job.preparation, Preparation::Search));
        rig.clipboard.action(Action::PasteSearch);
        assert!(rig.jobs.try_recv().is_err());
        rig.finish_job(job, Ok(Prepared::Search(Some("日本語😀".into()))));
        rig.tick();
        assert_eq!(rig.clipboard.view.query, "日本語😀");
        assert!(matches!(rig.request().command, StoreCommand::Query { .. }));
    }

    #[test]
    fn clipboard_hide_precedes_external_activation_tick_and_exact_terminal_event() {
        let mut rig = Rig::new(true);
        rig.clipboard.transfer_paste = true;
        rig.transfer.borrow_mut().pending = true;
        rig.transfer.borrow_mut().events = vec![TransferEvent::Copied];
        let events = rig.tick();
        assert!(events
            .iter()
            .any(|event| matches!(event, Event::HideForPaste)));
        assert_eq!(rig.transfer.borrow().ticks, 1); // No second activation tick in this call.
        rig.transfer.borrow_mut().events = vec![TransferEvent::TerminalPaste {
            tab: 123,
            text: "unchanged\n".into(),
        }];
        let events = rig.tick();
        assert!(events.iter().any(
            |event| matches!(event,Event::TerminalPaste { tab:123,text } if text == "unchanged\n")
        ));
    }

    #[test]
    fn clipboard_shutdown_suppresses_queued_results_and_callbacks() {
        let mut rig = Rig::new(true);
        rig.clipboard.action(Action::PasteSearch);
        let job = rig.jobs.try_recv().unwrap();
        rig.clipboard.shutdown();
        rig.finish_job(job, Ok(Prepared::Search(Some("late".into()))));
        assert!(rig.tick().is_empty());
        assert!(
            rig.store.borrow().stopped
                && rig.monitor.borrow().stopped
                && rig.transfer.borrow().stopped
        );
        assert!(rig.clipboard.preparation.callback.lock().unwrap().is_none());
        assert!(rig.clipboard.view.query.is_empty());
        assert!(!rig.clipboard.needs_tick());
    }

    #[test]
    fn clipboard_queue_bounds_and_errors_never_expose_worker_details() {
        let mut rig = Rig::new(true);
        for _ in 0..50 {
            rig.capture(1);
        }
        rig.tick();
        assert_eq!(rig.store.borrow().sent.len(), MAX_REQUESTS);
        assert_eq!(rig.clipboard.pending.len(), MAX_REQUESTS);
        assert!(rig
            .clipboard
            .view
            .status
            .as_ref()
            .unwrap()
            .contains("skipped"));
        rig.clipboard
            .set_status("native sensitive path /private".into());
        assert!(!rig
            .clipboard
            .view
            .status
            .as_ref()
            .unwrap()
            .contains("private"));
        assert!(rig.clipboard.events.len() <= MAX_EVENTS);
    }

    #[test]
    fn clipboard_safe_feedback_is_scheduled_once_and_keeps_hotkey_notice() {
        let mut rig = Rig::new(true);
        rig.clipboard
            .set_status("Ctrl+Alt+V is unavailable; open Clipboard from Home or the tray.".into());
        assert!(rig.clipboard.needs_tick());
        assert!(rig
            .tick()
            .iter()
            .any(|event| matches!(event, Event::ShowFeedback)));
        assert!(!rig.clipboard.needs_tick());
        assert_eq!(
            rig.clipboard.view.status.as_deref(),
            Some("Ctrl+Alt+V is unavailable; open Clipboard from Home or the tray.")
        );
        rig.clipboard
            .set_status("Terminal target is no longer live".into());
        assert_eq!(
            rig.clipboard.view.status.as_deref(),
            Some("Copied; the original Terminal tab is no longer available.")
        );
    }

    #[test]
    fn clipboard_informational_status_does_not_mask_empty_or_no_results() {
        let mut rig = Rig::new(true);
        rig.clipboard.status("Recording future clipboard changes");
        rig.clipboard.action(Action::SearchChanged);
        rig.tick();
        let query = rig.request();
        rig.reply(
            &query,
            StoreResult::Rows {
                rows: vec![],
                total: 0,
                encrypted_bytes: 0,
                corrupt: 0,
            },
        );
        rig.tick();
        assert!(rig.clipboard.view.status.is_none());
        rig.clipboard.view.query = "missing".into();
        rig.clipboard.status("Copied");
        rig.clipboard.action(Action::SearchChanged);
        rig.tick();
        let query = rig.request();
        rig.reply(
            &query,
            StoreResult::Rows {
                rows: vec![],
                total: 0,
                encrypted_bytes: 0,
                corrupt: 0,
            },
        );
        rig.tick();
        assert!(rig.clipboard.view.status.is_none());
        assert_eq!(rig.clipboard.view.query, "missing");
    }

    #[test]
    fn clipboard_settings_choice_during_clear_is_applied_after_ack() {
        let mut rig = Rig::new(true);
        rig.clipboard.action(Action::ClearUnpinned);
        rig.tick();
        let clear = rig.request();
        rig.clipboard.configure(ClipboardConfig {
            enabled: true,
            paused: true,
            excluded_sources: vec!["synthetic.exe".into()],
        });
        rig.reply(&clear, StoreResult::Changed { id: None });
        let events = rig.tick();
        assert!(events.iter().any(|event| matches!(event,Event::PolicyChanged(c) if c.paused && c.excluded_sources == ["synthetic.exe"])));
        let monitor = rig.monitor.borrow();
        assert!(monitor.policies.last().unwrap().paused);
        assert_eq!(
            monitor.policies.last().unwrap().excluded_sources,
            ["synthetic.exe"]
        );
        assert!(!rig.clipboard.accepting);
    }

    #[test]
    fn clipboard_startup_error_ends_pending_open_loading_state() {
        let mut rig = Rig::new(true);
        rig.clipboard.ready = false;
        rig.clipboard.open(None);
        assert!(rig.clipboard.view.loading);
        rig.store.borrow_mut().replies.push(StoreReply {
            request: 0,
            generation: 1,
            result: Err("Unsupported Clipboard database version".into()),
        });
        rig.tick();
        assert!(!rig.clipboard.view.loading);
        assert!(!rig.clipboard.ready);
        assert_eq!(
            rig.clipboard.view.status.as_deref(),
            Some("This history database needs a newer version of Daijobu Island")
        );
        assert!(rig.store.borrow().sent.is_empty());
        rig.clipboard.close();
        rig.clipboard.open(None);
        rig.tick();
        assert!(!rig.clipboard.view.loading);
        assert!(rig.store.borrow().sent.is_empty());
    }

    #[test]
    fn clipboard_store_full_and_pin_limit_have_fixed_safe_feedback() {
        let mut rig = Rig::new(true);
        rig.clipboard.action(Action::TogglePin);
        let request = rig.request();
        rig.store.borrow_mut().replies.push(StoreReply {
            request: request.request,
            generation: request.generation,
            result: Err("Clipboard pin limit reached".into()),
        });
        rig.tick();
        assert_eq!(
            rig.clipboard.view.status.as_deref(),
            Some("Pin limit reached; unpin an item before pinning another")
        );
        assert!(!rig.clipboard.view.rows[0].pinned);
        rig.capture(1);
        rig.tick();
        let request = rig.request();
        rig.store.borrow_mut().replies.push(StoreReply {
            request: request.request,
            generation: request.generation,
            result: Err("Clipboard history full".into()),
        });
        rig.tick();
        assert_eq!(
            rig.clipboard.view.status.as_deref(),
            Some("History full; delete an item or unpin older entries")
        );
        assert_eq!(rig.clipboard.view.selected, Some(ItemId(11)));
    }
}
