//! Windows Clipboard acquisition, normalization and encrypted local storage.
pub mod crypto;
pub mod formats;
pub mod image;
pub mod monitor;
pub mod native;
pub mod store;

use dj_core::clipboard::{Filter, ItemId, Payload, Row};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

pub type Wake = Arc<dyn Fn() + Send + Sync>;
pub type Generation = Arc<GenerationGate>;

/// Serializes invalidation with the final storage commit, without making the UI wait.
pub struct GenerationGate {
    value: AtomicU64,
    commit: Mutex<()>,
}
impl GenerationGate {
    pub fn new(value: u64) -> Self {
        Self {
            value: AtomicU64::new(value),
            commit: Mutex::new(()),
        }
    }
    pub fn load(&self, order: Ordering) -> u64 {
        self.value.load(order)
    }
    pub fn fetch_add(&self, value: u64, order: Ordering) -> u64 {
        let _guard = self.commit_guard();
        self.value.fetch_add(value, order)
    }
    /// UI callers defer their action until this succeeds; they never wait for disk IO.
    pub fn try_fetch_add(&self, value: u64, order: Ordering) -> Option<u64> {
        let _guard = match self.commit.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        Some(self.value.fetch_add(value, order))
    }
    pub fn commit_guard(&self) -> MutexGuard<'_, ()> {
        self.commit
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}
pub const REPLAY_FORMAT: &str = "DaijobuIsland.ClipboardReplay.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RawFormat {
    Unicode,
    Html,
    Rtf,
    Png,
    DibV5,
    Dib,
    Files,
}

#[derive(Clone, Debug)]
pub struct RawPart {
    pub format: RawFormat,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct RawSnapshot {
    pub sequence: u32,
    pub generation: u64,
    pub at_ms: u64,
    pub source: Option<String>,
    pub no_cloud: bool,
    pub parts: Vec<RawPart>,
}

#[derive(Clone, Debug)]
pub struct Captured {
    pub sequence: u32,
    pub generation: u64,
    pub at_ms: u64,
    pub source: Option<String>,
    pub payload: Arc<Payload>,
}

#[derive(Clone, Debug, Default)]
pub struct MonitorPolicy {
    pub enabled: bool,
    pub paused: bool,
    pub excluded_sources: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum MonitorEvent {
    Captured(Captured),
    Skipped { reason: String, count: u64 },
    Error(String),
}

/// Suppresses only exact private replay operations, including the CloseClipboard race.
#[derive(Clone, Default)]
pub struct ReplayTracker(Arc<Mutex<VecDeque<Replay>>>);
#[derive(Clone)]
struct Replay {
    token: [u8; 16],
    sequence: Option<u32>,
}

impl ReplayTracker {
    pub fn reserve(&self, token: [u8; 16]) {
        let mut queue = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if queue.len() >= 16 {
            queue.pop_front();
        }
        queue.push_back(Replay {
            token,
            sequence: None,
        });
    }

    pub fn complete(&self, token: [u8; 16], sequence: u32) {
        let mut queue = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(op) = queue.iter_mut().find(|op| op.token == token) {
            op.sequence = Some(sequence);
        }
    }

    pub fn cancel(&self, token: [u8; 16]) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|op| op.token != token);
    }

    /// Call only after the owner PID was verified as this process.
    pub fn matches(&self, token: &[u8], sequence: u32) -> bool {
        let mut queue = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(op) = queue.iter_mut().find(|op| op.token.as_slice() == token) {
            if op.sequence.is_none_or(|s| s == sequence) {
                op.sequence = Some(sequence);
                return true;
            }
        }
        false
    }
}

pub struct StoreRequest {
    pub request: u64,
    pub generation: u64,
    pub command: StoreCommand,
}
pub enum StoreCommand {
    Insert(Captured),
    Query {
        filter: Filter,
        query: String,
        now_ms: u64,
    },
    Fetch(ItemId),
    Pin {
        id: ItemId,
        pinned: bool,
    },
    Delete(ItemId),
    Clear {
        include_pinned: bool,
    },
    Prune {
        now_ms: u64,
    },
}

pub struct StoreReply {
    pub request: u64,
    pub generation: u64,
    pub result: Result<StoreResult, String>,
}

pub enum StoreResult {
    Ready,
    Rows {
        rows: Vec<Row>,
        total: usize,
        encrypted_bytes: u64,
        corrupt: usize,
    },
    Item {
        id: ItemId,
        payload: Arc<Payload>,
    },
    Changed {
        id: Option<ItemId>,
    },
    Skipped,
}
