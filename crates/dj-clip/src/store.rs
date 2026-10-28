//! One bounded asynchronous SQLite owner. Only nonsensitive envelope fields reach SQL.
use crate::crypto::{Crypto, Dpapi};
use crate::{Captured, Generation, StoreCommand, StoreReply, StoreRequest, StoreResult, Wake};
use dj_core::clipboard::{Content, Filter, ImageData, ItemId, Kind, Payload, Row, Thumbnail};
use dj_core::clipboard_types::{MAX_PINNED, MAX_RAM_BYTES, MAX_STORED_BYTES, MAX_UNPINNED, TTL_MS};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const VERSION: i64 = 1;
const APPLICATION: i64 = 0x444a434c;
const MAX_PAYLOAD: usize = 98 * 1024 * 1024;
const MAX_META: usize = 2 * 1024 * 1024;
const MAX_ROWS: usize = MAX_UNPINNED + MAX_PINNED;
const MAX_REQUESTS: usize = 16;
const QUERY_RESERVE: usize = 40 * 1024 * 1024;
const DB_ERROR: &str = "Clipboard database operation failed";
const CORRUPT: &str = "Clipboard item is corrupt or unavailable";
const INVALIDATED: &str = "Clipboard operation invalidated";

// SQLite stores signed 64-bit integers. rusqlite 0.41 intentionally does not
// accept u64/usize, so conversions are checked at the boundary.
struct SqlNumber(u64);
impl rusqlite::types::ToSql for SqlNumber {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        let number = i64::try_from(self.0).map_err(|_| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
                "Clipboard integer range",
            )))
        })?;
        Ok(rusqlite::types::ToSqlOutput::Owned(number.into()))
    }
}
fn unsigned(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(column)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, value))
}
fn count(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<usize> {
    let value = unsigned(row, column)?;
    usize::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, i64::MAX))
}

fn db<T>(value: rusqlite::Result<T>) -> Result<T, String> {
    value.map_err(|_| DB_ERROR.into())
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as u64
}

struct Queued {
    request: StoreRequest,
    reserved: usize,
}
struct Completed {
    reply: StoreReply,
    reserved: usize,
}
impl Completed {
    fn lifetime_notice(&self) -> bool {
        self.reserved == 0
            && self.reply.request == 0
            && matches!(&self.reply.result, Ok(StoreResult::Ready) | Err(_))
    }
}
#[derive(Default)]
struct Mail {
    requests: VecDeque<Queued>,
    replies: VecDeque<Completed>,
    outstanding: usize,
    bytes: usize,
}
struct Shared {
    mail: Mutex<Mail>,
    changed: Condvar,
    stopped: AtomicBool,
    callback: Mutex<Option<Wake>>,
}
impl Shared {
    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    fn publish(&self, reply: StoreReply, reserved: usize) {
        {
            let mut mail = self.mail.lock().unwrap_or_else(|e| e.into_inner());
            if self.stopped() {
                return;
            }
            if reserved == 0 && !matches!(&reply.result, Ok(StoreResult::Ready)) {
                if let Some(existing) = mail.replies.iter_mut().find(|done| {
                    done.reserved == 0
                        && !matches!(&done.reply.result, Ok(StoreResult::Ready))
                        && done.reply.result.is_err() == reply.result.is_err()
                }) {
                    existing.reply = reply;
                    return;
                }
            }
            mail.replies.push_back(Completed { reply, reserved });
        }
        // Shutdown takes the same gate before removing the callback. A completed
        // shutdown therefore guarantees no future callback can target a dead HWND.
        let callback = self.callback.lock().unwrap_or_else(|e| e.into_inner());
        if !self.stopped() {
            if let Some(wake) = callback.as_ref() {
                wake();
            }
        }
    }
}

/// Requests and their replies retain an admission reservation until drain().
/// Shutdown detaches an unfinished OS/SQLite call rather than joining on the UI.
pub struct Store {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    generation: Generation,
}
impl Store {
    pub fn new(path: PathBuf, generation: Generation, wake: Wake) -> Result<Self, String> {
        Self::spawn(path, generation, wake, Box::new(Dpapi))
    }
    fn spawn(
        path: PathBuf,
        generation: Generation,
        wake: Wake,
        crypto: Box<dyn Crypto>,
    ) -> Result<Self, String> {
        Self::spawn_with_open(generation, wake, move || Database::open(&path, crypto))
    }
    fn spawn_with_open(
        generation: Generation,
        wake: Wake,
        open: impl FnOnce() -> Result<Database, String> + Send + 'static,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            mail: Mutex::new(Mail::default()),
            changed: Condvar::new(),
            stopped: AtomicBool::new(false),
            callback: Mutex::new(Some(wake)),
        });
        let thread_shared = shared.clone();
        let thread_generation = generation.clone();
        let worker = thread::Builder::new()
            .name("clipboard-store".into())
            .spawn(move || {
                run(open, thread_generation, thread_shared);
            })
            .map_err(|_| "Clipboard store worker unavailable".to_string())?;
        Ok(Self {
            shared,
            worker: Some(worker),
            generation,
        })
    }
    pub fn send(&self, request: StoreRequest) -> Result<(), String> {
        if self.shared.stopped() {
            return Err("Clipboard store stopped".into());
        }
        if request.generation != self.generation.load(Ordering::Acquire) {
            return Err(INVALIDATED.into());
        }
        let reserved = match &request.command {
            StoreCommand::Insert(capture) => {
                capture
                    .payload
                    .validate()
                    .map_err(|_| "Invalid Clipboard payload".to_string())?;
                if capture.source.as_ref().is_some_and(|s| s.len() > 4096) {
                    return Err("Clipboard source size limit".into());
                }
                capture.payload.retained_bytes().saturating_add(MAX_META)
            }
            StoreCommand::Query { query, .. } => {
                if query.len() > 4096 {
                    return Err("Clipboard search size limit".into());
                }
                QUERY_RESERVE
            }
            StoreCommand::Fetch(_) => MAX_PAYLOAD,
            _ => 1024,
        };
        let mut mail = self.shared.mail.lock().unwrap_or_else(|e| e.into_inner());
        if mail.outstanding >= MAX_REQUESTS || mail.bytes.saturating_add(reserved) > MAX_RAM_BYTES {
            return Err("Clipboard store busy".into());
        }
        if self.shared.stopped() {
            return Err("Clipboard store stopped".into());
        }
        mail.outstanding += 1;
        mail.bytes += reserved;
        mail.requests.push_back(Queued { request, reserved });
        self.shared.changed.notify_one();
        Ok(())
    }
    pub fn drain(&mut self) -> Vec<StoreReply> {
        let generation = self.generation.load(Ordering::Acquire);
        let mut mail = self.shared.mail.lock().unwrap_or_else(|e| e.into_inner());
        let mut replies = Vec::new();
        while let Some(mut done) = mail.replies.pop_front() {
            if done.reserved != 0 {
                mail.outstanding = mail.outstanding.saturating_sub(1);
                mail.bytes = mail.bytes.saturating_sub(done.reserved);
            }
            // Ready and unsolicited storage errors describe this store's lifetime,
            // not captured data. Rebase at consumption, including notices queued
            // before Enable/Pause/Clear changed the generation. Request replies and
            // maintenance changes retain their original invalidation semantics.
            if done.lifetime_notice() {
                done.reply.generation = generation;
            }
            if done.reply.generation == generation {
                replies.push(done.reply);
            }
        }
        replies
    }
    pub fn shutdown(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        self.shared.changed.notify_all();
        self.shared
            .callback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let mut mail = self.shared.mail.lock().unwrap_or_else(|e| e.into_inner());
        mail.requests.clear();
        mail.replies.clear();
        mail.bytes = 0;
        mail.outstanding = 0;
        drop(mail);
        if let Some(worker) = self.worker.take() {
            if worker.is_finished() {
                let _ = worker.join();
            }
            // Otherwise dropping JoinHandle detaches; Shared remains owned by worker.
        }
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(
    open: impl FnOnce() -> Result<Database, String>,
    generation: Generation,
    shared: Arc<Shared>,
) {
    let mut database = match open() {
        Ok(database) => database,
        Err(error) => {
            shared.publish(
                StoreReply {
                    request: 0,
                    generation: generation.load(Ordering::Acquire),
                    result: Err(error),
                },
                0,
            );
            shared.stopped.store(true, Ordering::Release);
            return;
        }
    };
    let Some(initial) = prune_current(&generation, &shared.stopped, |current| {
        database.prune(now_ms(), current, &generation, &shared.stopped)
    }) else {
        return;
    };
    shared.publish(
        StoreReply {
            request: 0,
            generation: generation.load(Ordering::Acquire),
            result: initial.map(|_| StoreResult::Ready),
        },
        0,
    );
    let mut expiry = std::time::Instant::now() + Duration::from_secs(60 * 60);
    loop {
        let queued = {
            let mut mail = shared.mail.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if shared.stopped() {
                    return;
                }
                let until_expiry = expiry.saturating_duration_since(std::time::Instant::now());
                if until_expiry.is_zero() {
                    break None;
                }
                if let Some(request) = mail.requests.pop_front() {
                    break Some(request);
                }
                let waited = shared.changed.wait_timeout(mail, until_expiry);
                let (next, _) = waited.unwrap_or_else(|e| e.into_inner());
                mail = next;
            }
        };
        let Some(queued) = queued else {
            let current = generation.load(Ordering::Acquire);
            let result = database.execute(
                StoreCommand::Prune { now_ms: now_ms() },
                current,
                &generation,
                &shared.stopped,
            );
            if !matches!(&result, Ok(StoreResult::Skipped))
                && !matches!(&result, Err(error) if error == INVALIDATED)
            {
                shared.publish(
                    StoreReply {
                        request: 0,
                        generation: current,
                        result,
                    },
                    0,
                );
            }
            expiry = std::time::Instant::now() + Duration::from_secs(60 * 60);
            continue;
        };
        let request = queued.request;
        let result = database.execute(
            request.command,
            request.generation,
            &generation,
            &shared.stopped,
        );
        shared.publish(
            StoreReply {
                request: request.request,
                generation: request.generation,
                result,
            },
            queued.reserved,
        );
    }
}

// Initialization is independent of capture invalidation. A generation change
// rolls back the stale prune through commit(), then retries at the latest value.
// Call prune directly: execute() also uses Skipped for an invalidated request,
// which cannot be distinguished from a successfully committed no-op prune.
fn prune_current(
    generation: &Generation,
    stopped: &AtomicBool,
    mut prune: impl FnMut(u64) -> Result<StoreResult, String>,
) -> Option<Result<StoreResult, String>> {
    loop {
        if stopped.load(Ordering::Acquire) {
            return None;
        }
        let result = prune(generation.load(Ordering::Acquire));
        if matches!(&result, Err(error) if error == INVALIDATED) {
            continue;
        }
        return Some(result);
    }
}

struct Metadata {
    search: String,
    preview: String,
    source: Option<String>,
    thumbnail: Option<Thumbnail>,
}
struct Database {
    connection: Connection,
    crypto: Box<dyn Crypto>,
    lease: Weak<Payload>,
    limits: Limits,
    hashes: HashMap<u64, u64>,
}
#[derive(Clone, Copy)]
struct Limits {
    unpinned: usize,
    pinned: usize,
    bytes: u64,
    ttl: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            unpinned: MAX_UNPINNED,
            pinned: MAX_PINNED,
            bytes: MAX_STORED_BYTES,
            ttl: TTL_MS,
        }
    }
}
impl Database {
    fn open(path: &Path, crypto: Box<dyn Crypto>) -> Result<Self, String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|_| DB_ERROR.to_string())?;
        }
        let connection = db(Connection::open(path))?;
        db(connection.busy_timeout(Duration::from_millis(250)))?;
        let version: i64 = db(connection.pragma_query_value(None, "user_version", |r| r.get(0)))?;
        let application: i64 =
            db(connection.pragma_query_value(None, "application_id", |r| r.get(0)))?;
        if version > VERSION {
            return Err("Unsupported Clipboard database version".into());
        }
        if version != 0 && (version != VERSION || application != APPLICATION) {
            return Err("Invalid Clipboard database schema".into());
        }
        if version == 0 {
            let tables: i64 = db(connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            ))?;
            if tables != 0 || (application != 0 && application != APPLICATION) {
                return Err("Invalid Clipboard database schema".into());
            }
            db(connection.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE items(id INTEGER PRIMARY KEY AUTOINCREMENT, created INTEGER NOT NULL,
                used INTEGER NOT NULL, kind INTEGER NOT NULL, pinned INTEGER NOT NULL CHECK(pinned IN (0,1)),
                bytes INTEGER NOT NULL CHECK(bytes>=0), payload BLOB NOT NULL, metadata BLOB NOT NULL);
                CREATE INDEX items_age ON items(pinned,used,id);
                PRAGMA application_id=1145717580; PRAGMA user_version=1; COMMIT;"))?;
        }
        let check: String = db(connection.query_row("PRAGMA quick_check(1)", [], |r| r.get(0)))?;
        if check != "ok" {
            return Err("Clipboard database is corrupt".into());
        }
        db(connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            PRAGMA temp_store=MEMORY; PRAGMA secure_delete=ON; PRAGMA cache_size=-4096;
            PRAGMA wal_autocheckpoint=256; PRAGMA journal_size_limit=8388608;",
        ))?;
        // Validate expected columns without reading any user payload.
        db(connection.prepare(
            "SELECT id,created,used,kind,pinned,bytes,payload,metadata FROM items LIMIT 0",
        ))?;
        Ok(Self {
            connection,
            crypto,
            lease: Weak::new(),
            limits: Limits::default(),
            hashes: HashMap::new(),
        })
    }
    fn execute(
        &mut self,
        command: StoreCommand,
        expected: u64,
        generation: &Generation,
        stopped: &AtomicBool,
    ) -> Result<StoreResult, String> {
        if stopped.load(Ordering::Acquire) || generation.load(Ordering::Acquire) != expected {
            return Ok(StoreResult::Skipped);
        }
        match command {
            StoreCommand::Insert(capture) => self.insert(capture, expected, generation, stopped),
            StoreCommand::Query {
                filter,
                query,
                now_ms,
            } => {
                self.prune(now_ms, expected, generation, stopped)?;
                self.query(filter, &query)
            }
            StoreCommand::Fetch(id) => self.fetch(id),
            StoreCommand::Pin { id, pinned } => {
                let tx = db(self.connection.transaction())?;
                if pinned {
                    let count: usize = db(tx.query_row(
                        "SELECT count(*) FROM items WHERE pinned=1 AND id<>?",
                        [SqlNumber(id.0)],
                        |r| count(r, 0),
                    ))?;
                    if count >= self.limits.pinned {
                        return Err("Clipboard pin limit reached".into());
                    }
                }
                if db(tx.execute(
                    "UPDATE items SET pinned=? WHERE id=?",
                    params![pinned, SqlNumber(id.0)],
                ))? == 0
                {
                    return Err("Clipboard item no longer exists".into());
                }
                prune_tx(&tx, now_ms(), self.limits)?;
                commit(tx, expected, generation, stopped)?;
                Ok(StoreResult::Changed { id: Some(id) })
            }
            StoreCommand::Delete(id) => {
                let tx = db(self.connection.transaction())?;
                if db(tx.execute("DELETE FROM items WHERE id=?", [SqlNumber(id.0)]))? == 0 {
                    return Err("Clipboard item no longer exists".into());
                }
                commit(tx, expected, generation, stopped)?;
                Ok(StoreResult::Changed { id: Some(id) })
            }
            StoreCommand::Clear { include_pinned } => {
                let tx = db(self.connection.transaction())?;
                db(tx.execute(
                    if include_pinned {
                        "DELETE FROM items"
                    } else {
                        "DELETE FROM items WHERE pinned=0"
                    },
                    [],
                ))?;
                commit(tx, expected, generation, stopped)?;
                Ok(StoreResult::Changed { id: None })
            }
            StoreCommand::Prune { now_ms } => self.prune(now_ms, expected, generation, stopped),
        }
    }
    fn prune(
        &mut self,
        now: u64,
        expected: u64,
        generation: &Generation,
        stopped: &AtomicBool,
    ) -> Result<StoreResult, String> {
        let tx = db(self.connection.transaction())?;
        let before: usize = db(tx.query_row("SELECT count(*) FROM items", [], |r| count(r, 0)))?;
        prune_tx(&tx, now, self.limits)?;
        let after: usize = db(tx.query_row("SELECT count(*) FROM items", [], |r| count(r, 0)))?;
        commit(tx, expected, generation, stopped)?;
        Ok(if before == after {
            StoreResult::Skipped
        } else {
            StoreResult::Changed { id: None }
        })
    }
    fn insert(
        &mut self,
        capture: Captured,
        expected: u64,
        generation: &Generation,
        stopped: &AtomicBool,
    ) -> Result<StoreResult, String> {
        if capture.generation != expected {
            return Ok(StoreResult::Skipped);
        }
        capture
            .payload
            .validate()
            .map_err(|_| "Invalid Clipboard payload".to_string())?;
        if capture.at_ms > i64::MAX as u64
            || capture.source.as_ref().is_some_and(|s| s.len() > 4096)
        {
            return Err("Invalid Clipboard metadata".into());
        }
        let plain = encode(&capture.payload)?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        plain.hash(&mut hasher);
        let hash = hasher.finish();
        let metadata = metadata(&capture);
        let meta_plain = encode_meta(&metadata);
        if meta_plain.len() > MAX_META {
            return Err("Clipboard metadata size limit".into());
        }
        // Encrypt everything before beginning a transaction; failures cannot evict rows.
        let encrypted = self.crypto.encrypt(&plain)?;
        let meta_encrypted = self.crypto.encrypt(&meta_plain)?;
        if encrypted.len() > MAX_PAYLOAD || meta_encrypted.len() > MAX_META {
            return Err("Clipboard encrypted size limit".into());
        }
        let bytes = (encrypted.len() + meta_encrypted.len()) as u64;
        let tx = db(self.connection.transaction())?;
        let mut duplicate = None;
        {
            let mut live = HashSet::new();
            let mut stmt = db(tx.prepare(
                "SELECT id,payload FROM items WHERE typeof(payload)='blob' AND length(payload)<=? ORDER BY used DESC,id DESC",
            ))?;
            let mut rows = db(stmt.query([MAX_PAYLOAD as i64]))?;
            while let Some(row) = db(rows.next())? {
                let id: u64 = db(unsigned(row, 0))?;
                live.insert(id);
                if self.hashes.get(&id).is_some_and(|known| *known != hash) {
                    continue;
                }
                let blob = db(row.get_ref(1))?
                    .as_blob()
                    .map_err(|_| CORRUPT.to_string())?;
                let Ok(candidate) = self.crypto.decrypt(blob) else {
                    continue;
                };
                let mut candidate_hasher = std::collections::hash_map::DefaultHasher::new();
                candidate.hash(&mut candidate_hasher);
                let candidate_hash = candidate_hasher.finish();
                self.hashes.insert(id, candidate_hash);
                if candidate_hash == hash && candidate == plain {
                    duplicate = Some(id);
                }
                if live.len() > MAX_ROWS {
                    return Err("Clipboard history limits exceeded".into());
                }
            }
            self.hashes.retain(|id, _| live.contains(id));
        }
        let id = if let Some(id) = duplicate {
            db(tx.execute(
                "UPDATE items SET used=?,kind=?,bytes=?,payload=?,metadata=? WHERE id=?",
                params![
                    SqlNumber(capture.at_ms),
                    kind_code(capture.payload.kind()),
                    SqlNumber(bytes),
                    encrypted,
                    meta_encrypted,
                    SqlNumber(id)
                ],
            ))?;
            ItemId(id)
        } else {
            db(tx.execute("INSERT INTO items(created,used,kind,pinned,bytes,payload,metadata) VALUES(?,?,?,0,?,?,?)",
                params![SqlNumber(capture.at_ms),SqlNumber(capture.at_ms),kind_code(capture.payload.kind()),SqlNumber(bytes),encrypted,meta_encrypted]))?;
            ItemId(db(tx.query_row("SELECT last_insert_rowid()", [], |r| {
                unsigned(r, 0)
            }))?)
        };
        prune_tx(&tx, capture.at_ms, self.limits)?;
        let present: bool = db(tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM items WHERE id=?)",
            [SqlNumber(id.0)],
            |r| r.get(0),
        ))?;
        if !present {
            return Err("Clipboard history full".into());
        }
        commit(tx, expected, generation, stopped)?;
        self.hashes.insert(id.0, hash);
        Ok(StoreResult::Changed { id: Some(id) })
    }
    fn query(&self, filter: Filter, query: &str) -> Result<StoreResult, String> {
        if query.len() > 4096 {
            return Err("Clipboard search size limit".into());
        }
        let needle = query.to_lowercase();
        let (total, bytes): (usize, u64) = db(self.connection.query_row(
            "SELECT count(*),coalesce(sum(length(payload)+length(metadata)),0) FROM items",
            [],
            |r| Ok((count(r, 0)?, unsigned(r, 1)?)),
        ))?;
        if total > MAX_ROWS {
            return Err("Clipboard history limits exceeded".into());
        }
        let mut stmt = db(self.connection.prepare("SELECT id,created,used,kind,pinned,bytes,CASE WHEN typeof(metadata)='blob' AND length(metadata)<=? THEN metadata ELSE NULL END FROM items ORDER BY used DESC,id DESC"))?;
        let mut cursor = db(stmt.query([MAX_META as i64]))?;
        let mut rows = Vec::new();
        let mut corrupt = 0;
        let mut retained = 0usize;
        while let Some(item) = db(cursor.next())? {
            let pinned: bool = db(item.get(4))?;
            let parsed_kind = read_kind(db(item.get(3))?);
            let invalid_kind = parsed_kind.is_err();
            let kind = parsed_kind.unwrap_or(Kind::Text);
            if !matches_filter(filter, kind, pinned) {
                continue;
            }
            let meta = if invalid_kind {
                None
            } else {
                db(item.get_ref(6))?
                    .as_blob()
                    .ok()
                    .and_then(|blob| read_meta(self.crypto.as_ref(), blob).ok())
            };
            if meta.is_none() {
                corrupt += 1;
            }
            if !needle.is_empty()
                && meta
                    .as_ref()
                    .is_none_or(|m| !m.search.to_lowercase().contains(&needle))
            {
                continue;
            }
            let broken = meta.is_none();
            let (preview, source, thumbnail) = meta
                .map(|m| (m.preview, m.source, m.thumbnail))
                .unwrap_or_else(|| ("Unavailable item".into(), None, None));
            retained = retained.saturating_add(
                preview.len()
                    + source.as_ref().map_or(0, String::len)
                    + thumbnail.as_ref().map_or(0, |t| t.bgra.len())
                    + 256,
            );
            if retained > QUERY_RESERVE {
                return Err("Clipboard preview memory limit".into());
            }
            rows.push(Row {
                id: ItemId(db(unsigned(item, 0))?),
                created_ms: db(unsigned(item, 1))?,
                used_ms: db(unsigned(item, 2))?,
                pinned,
                kind,
                preview,
                source,
                thumbnail,
                bytes: db(unsigned(item, 5))?,
                corrupt: broken,
            });
        }
        Ok(StoreResult::Rows {
            rows,
            total,
            encrypted_bytes: bytes,
            corrupt,
        })
    }
    fn fetch(&mut self, id: ItemId) -> Result<StoreResult, String> {
        if self.lease.strong_count() != 0 {
            return Err("Release the previous Clipboard payload first".into());
        }
        let bytes: Option<Vec<u8>> = db(self
            .connection
            .query_row(
                "SELECT payload FROM items WHERE id=? AND typeof(payload)='blob' AND length(payload)<=?",
                params![SqlNumber(id.0), MAX_PAYLOAD as i64],
                |r| r.get(0),
            )
            .optional())?;
        let bytes = bytes.ok_or(CORRUPT)?;
        let plain = self
            .crypto
            .decrypt(&bytes)
            .map_err(|_| CORRUPT.to_string())?;
        let payload = Arc::new(decode(&plain)?);
        self.lease = Arc::downgrade(&payload);
        Ok(StoreResult::Item { id, payload })
    }
}

fn commit(
    tx: Transaction<'_>,
    expected: u64,
    generation: &Generation,
    stopped: &AtomicBool,
) -> Result<(), String> {
    // Invalidation takes the same gate. Whichever acquires it first determines
    // the transaction's ordering relative to pause/disable/clear.
    let _commit_guard = generation.commit_guard();
    if stopped.load(Ordering::Acquire) || generation.load(Ordering::Acquire) != expected {
        return Err(INVALIDATED.into());
    }
    db(tx.commit())
}
fn prune_tx(tx: &Transaction<'_>, now: u64, limits: Limits) -> Result<(), String> {
    if now > i64::MAX as u64 {
        return Err("Invalid Clipboard timestamp".into());
    }
    let pins: usize = db(
        tx.query_row("SELECT count(*) FROM items WHERE pinned=1", [], |r| {
            count(r, 0)
        }),
    )?;
    if pins > limits.pinned {
        return Err("Clipboard pin limit reached".into());
    }
    if now >= limits.ttl {
        db(tx.execute(
            "DELETE FROM items WHERE pinned=0 AND used<=?",
            [SqlNumber(now - limits.ttl)],
        ))?;
    }
    db(tx.execute("DELETE FROM items WHERE id IN (SELECT id FROM items WHERE pinned=0 ORDER BY used DESC,id DESC LIMIT -1 OFFSET ?)", [SqlNumber(limits.unpinned as u64)]))?;
    let mut bytes: u64 = db(tx.query_row(
        "SELECT coalesce(sum(length(payload)+length(metadata)),0) FROM items",
        [],
        |r| unsigned(r, 0),
    ))?;
    while bytes > limits.bytes {
        let oldest: Option<(u64, u64)> = db(tx
            .query_row(
                "SELECT id,length(payload)+length(metadata) FROM items WHERE pinned=0 ORDER BY used,id LIMIT 1",
                [],
                |r| Ok((unsigned(r,0)?, unsigned(r,1)?)),
            )
            .optional())?;
        let Some((id, size)) = oldest else {
            return Err("Clipboard history full".into());
        };
        db(tx.execute("DELETE FROM items WHERE id=?", [SqlNumber(id)]))?;
        bytes = bytes.saturating_sub(size);
    }
    Ok(())
}
fn kind_code(kind: Kind) -> u8 {
    match kind {
        Kind::Text => 0,
        Kind::Link => 1,
        Kind::Colour => 2,
        Kind::Image => 3,
        Kind::Files => 4,
    }
}
fn read_kind(value: i64) -> Result<Kind, String> {
    match value {
        0 => Ok(Kind::Text),
        1 => Ok(Kind::Link),
        2 => Ok(Kind::Colour),
        3 => Ok(Kind::Image),
        4 => Ok(Kind::Files),
        _ => Err(CORRUPT.into()),
    }
}
fn matches_filter(filter: Filter, kind: Kind, pinned: bool) -> bool {
    match filter {
        Filter::All => true,
        Filter::Text => matches!(kind, Kind::Text | Kind::Link | Kind::Colour),
        Filter::Images => kind == Kind::Image,
        Filter::Files => kind == Kind::Files,
        Filter::Pinned => pinned,
    }
}
fn truncate(value: &str, limit: usize) -> String {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].into()
}
fn metadata(capture: &Captured) -> Metadata {
    let payload = &capture.payload;
    let search = match &payload.content {
        Content::Text { text, .. } => text.clone(),
        Content::Files(paths) => paths.join("\n"),
        Content::Image(_) => String::new(),
    };
    Metadata {
        search,
        preview: truncate(&payload.summary(), 4096),
        source: capture.source.clone(),
        thumbnail: match &payload.content {
            Content::Image(image) => Some(image.thumbnail.clone()),
            _ => None,
        },
    }
}
fn read_meta(crypto: &dyn Crypto, blob: &[u8]) -> Result<Metadata, String> {
    if blob.len() > MAX_META {
        return Err(CORRUPT.into());
    }
    let plain = crypto.decrypt(blob)?;
    if plain.len() > MAX_META {
        return Err(CORRUPT.into());
    }
    let mut reader = Reader(&plain);
    if reader.take(5)? != b"DJCM\x01" {
        return Err("Unsupported Clipboard metadata version".into());
    }
    let search = reader.text(1024 * 1024)?;
    let preview = reader.text(4096)?;
    let source = match reader.byte()? {
        0 => None,
        1 => Some(reader.text(4096)?),
        _ => return Err(CORRUPT.into()),
    };
    let thumbnail = match reader.byte()? {
        0 => None,
        1 => Some(Thumbnail {
            width: reader.number()?,
            height: reader.number()?,
            bgra: reader.bytes(160 * 90 * 4)?,
        }),
        _ => return Err(CORRUPT.into()),
    };
    let meta = Metadata {
        search,
        preview,
        source,
        thumbnail,
    };
    if !reader.0.is_empty()
        || meta
            .thumbnail
            .as_ref()
            .is_some_and(|t| !dj_core::clipboard::valid_thumbnail(t))
    {
        return Err(CORRUPT.into());
    }
    Ok(meta)
}

fn encode_meta(meta: &Metadata) -> Vec<u8> {
    let mut out = b"DJCM\x01".to_vec();
    put(&mut out, meta.search.as_bytes());
    put(&mut out, meta.preview.as_bytes());
    out.push(u8::from(meta.source.is_some()));
    if let Some(source) = &meta.source {
        put(&mut out, source.as_bytes());
    }
    out.push(u8::from(meta.thumbnail.is_some()));
    if let Some(thumbnail) = &meta.thumbnail {
        out.extend_from_slice(&thumbnail.width.to_le_bytes());
        out.extend_from_slice(&thumbnail.height.to_le_bytes());
        put(&mut out, &thumbnail.bgra);
    }
    out
}

// Length-delimited binary avoids JSON's manyfold expansion of large pixel arrays.
fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}
fn optional(out: &mut Vec<u8>, value: &Option<Vec<u8>>) {
    out.push(u8::from(value.is_some()));
    if let Some(value) = value {
        put(out, value);
    }
}
fn encode(payload: &Payload) -> Result<Vec<u8>, String> {
    let mut out = b"DJCL\x01".to_vec();
    out.push(u8::from(payload.no_cloud));
    match &payload.content {
        Content::Text { text, html, rtf } => {
            out.push(0);
            put(&mut out, text.as_bytes());
            optional(&mut out, html);
            optional(&mut out, rtf);
        }
        Content::Image(image) => {
            out.push(1);
            out.extend_from_slice(&image.width.to_le_bytes());
            out.extend_from_slice(&image.height.to_le_bytes());
            put(&mut out, &image.bgra);
            put(&mut out, &image.png);
            out.extend_from_slice(&image.thumbnail.width.to_le_bytes());
            out.extend_from_slice(&image.thumbnail.height.to_le_bytes());
            put(&mut out, &image.thumbnail.bgra);
        }
        Content::Files(paths) => {
            out.push(2);
            out.extend_from_slice(&(paths.len() as u32).to_le_bytes());
            for path in paths {
                put(&mut out, path.as_bytes());
            }
        }
    }
    if out.len() > MAX_PAYLOAD {
        return Err("Clipboard payload size limit".into());
    }
    Ok(out)
}
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let value = self.0.get(..len).ok_or(CORRUPT)?;
        self.0 = &self.0[len..];
        Ok(value)
    }
    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn number(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| CORRUPT.to_string())?,
        ))
    }
    fn bytes(&mut self, limit: usize) -> Result<Vec<u8>, String> {
        let len = self.number()? as usize;
        if len > limit {
            return Err(CORRUPT.into());
        }
        Ok(self.take(len)?.to_vec())
    }
    fn text(&mut self, limit: usize) -> Result<String, String> {
        String::from_utf8(self.bytes(limit)?).map_err(|_| CORRUPT.into())
    }
    fn rich(&mut self) -> Result<Option<Vec<u8>>, String> {
        match self.byte()? {
            0 => Ok(None),
            1 => self.bytes(4 * 1024 * 1024).map(Some),
            _ => Err(CORRUPT.into()),
        }
    }
}
fn decode(bytes: &[u8]) -> Result<Payload, String> {
    if bytes.len() > MAX_PAYLOAD {
        return Err(CORRUPT.into());
    }
    let mut reader = Reader(bytes);
    if reader.take(5)? != b"DJCL\x01" {
        return Err("Unsupported Clipboard item version".into());
    }
    let no_cloud = match reader.byte()? {
        0 => false,
        1 => true,
        _ => return Err(CORRUPT.into()),
    };
    let content = match reader.byte()? {
        0 => Content::Text {
            text: reader.text(1024 * 1024)?,
            html: reader.rich()?,
            rtf: reader.rich()?,
        },
        1 => Content::Image(ImageData {
            width: reader.number()?,
            height: reader.number()?,
            bgra: reader.bytes(64 * 1024 * 1024)?,
            png: reader.bytes(32 * 1024 * 1024)?,
            thumbnail: Thumbnail {
                width: reader.number()?,
                height: reader.number()?,
                bgra: reader.bytes(160 * 90 * 4)?,
            },
        }),
        2 => {
            let count = reader.number()?;
            if count > 128 {
                return Err(CORRUPT.into());
            }
            let mut paths = Vec::new();
            for _ in 0..count {
                paths.push(reader.text(256 * 1024)?);
            }
            Content::Files(paths)
        }
        _ => return Err(CORRUPT.into()),
    };
    let payload = Payload { content, no_cloud };
    if !reader.0.is_empty() {
        return Err(CORRUPT.into());
    }
    payload.validate().map_err(|_| CORRUPT.to_string())?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::FakeCrypto;
    use std::sync::atomic::AtomicU64;
    use tempfile::TempDir;

    fn generation() -> Generation {
        Arc::new(crate::GenerationGate::new(1))
    }
    fn fresh() -> (TempDir, Database, Generation) {
        let temp = tempfile::tempdir().unwrap();
        let database =
            Database::open(&temp.path().join("history.sqlite3"), Box::new(FakeCrypto)).unwrap();
        (temp, database, generation())
    }
    fn text(value: &str) -> Payload {
        Payload {
            content: Content::Text {
                text: value.into(),
                html: None,
                rtf: None,
            },
            no_cloud: false,
        }
    }
    fn capture(payload: Payload, at_ms: u64) -> Captured {
        Captured {
            sequence: 1,
            generation: 1,
            at_ms,
            source: Some("synthetic-source-marker.exe".into()),
            payload: Arc::new(payload),
        }
    }
    fn perform(
        database: &mut Database,
        command: StoreCommand,
        generation: &Generation,
    ) -> Result<StoreResult, String> {
        database.execute(
            command,
            generation.load(Ordering::Acquire),
            generation,
            &AtomicBool::new(false),
        )
    }
    fn insert(
        database: &mut Database,
        payload: Payload,
        at: u64,
        generation: &Generation,
    ) -> ItemId {
        match perform(
            database,
            StoreCommand::Insert(capture(payload, at)),
            generation,
        )
        .unwrap()
        {
            StoreResult::Changed { id: Some(id) } => id,
            _ => panic!("unexpected store result"),
        }
    }
    fn rows(database: &Database) -> Vec<Row> {
        match database.query(Filter::All, "").unwrap() {
            StoreResult::Rows { rows, .. } => rows,
            _ => panic!("unexpected result"),
        }
    }

    #[test]
    fn binary_roundtrip_preserves_rich_privacy_files_and_image() {
        let payloads = [
            Payload {
                content: Content::Text {
                    text: "synthetic 日本語 😀".into(),
                    html: Some(b"<b>synthetic</b>".to_vec()),
                    rtf: Some(br"{\rtf1 synthetic}".to_vec()),
                },
                no_cloud: true,
            },
            Payload {
                content: Content::Files(vec![
                    r"C:\synthetic\b.txt".into(),
                    r"C:\synthetic\a.txt".into(),
                ]),
                no_cloud: true,
            },
            Payload {
                content: Content::Image(ImageData {
                    width: 1,
                    height: 1,
                    bgra: vec![1, 2, 3, 255],
                    png: vec![137, 80, 78, 71],
                    thumbnail: Thumbnail {
                        width: 1,
                        height: 1,
                        bgra: vec![1, 2, 3, 255],
                    },
                }),
                no_cloud: false,
            },
        ];
        for payload in payloads {
            assert!(decode(&encode(&payload).unwrap()).is_ok_and(|p| p == payload));
        }
        assert!(decode(b"DJCL\x02").is_err());
        let mut malformed = encode(&text("fixture")).unwrap();
        malformed.push(0);
        assert!(decode(&malformed).is_err());
        assert!(decode(b"DJCL\x01\x00\x00\xff\xff\xff\xff").is_err());
    }

    #[test]
    fn database_and_wal_never_contain_sensitive_markers() {
        let (temp, mut database, generation) = fresh();
        let at = now_ms();
        let payload = Payload {
            content: Content::Text {
                text: "synthetic-secret-text-marker".into(),
                html: Some(b"synthetic-rich-marker".to_vec()),
                rtf: None,
            },
            no_cloud: true,
        };
        insert(&mut database, payload, at, &generation);
        insert(
            &mut database,
            Payload {
                content: Content::Files(vec![r"C:\synthetic\private-path-marker.txt".into()]),
                no_cloud: false,
            },
            at + 1,
            &generation,
        );
        insert(
            &mut database,
            Payload {
                content: Content::Image(ImageData {
                    width: 1,
                    height: 1,
                    bgra: vec![1, 2, 3, 255],
                    png: b"synthetic-image-marker".to_vec(),
                    thumbnail: Thumbnail {
                        width: 1,
                        height: 1,
                        bgra: vec![1, 2, 3, 255],
                    },
                }),
                no_cloud: false,
            },
            at + 2,
            &generation,
        );
        for file in std::fs::read_dir(temp.path()).unwrap() {
            let bytes = std::fs::read(file.unwrap().path()).unwrap();
            for marker in [
                b"synthetic-secret-text-marker".as_slice(),
                b"synthetic-rich-marker",
                b"synthetic-source-marker",
                b"private-path-marker",
                b"synthetic-image-marker",
            ] {
                assert!(
                    !bytes.windows(marker.len()).any(|part| part == marker),
                    "sensitive fixture persisted in plaintext"
                );
            }
        }
        let columns: Vec<String> = database
            .connection
            .prepare("PRAGMA table_info(items)")
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(!columns.iter().any(|name| name.contains("hash")
            || name.contains("search")
            || name.contains("source")));
    }

    #[test]
    fn exact_dedupe_retains_id_pin_rich_alternatives_and_restart() {
        let (temp, mut database, generation) = fresh();
        let at = now_ms();
        let original = text("same synthetic plain fallback");
        let id = insert(&mut database, original.clone(), at, &generation);
        perform(
            &mut database,
            StoreCommand::Pin { id, pinned: true },
            &generation,
        )
        .unwrap();
        assert_eq!(
            id,
            insert(&mut database, original.clone(), at + 10, &generation)
        );
        let mut rich = original.clone();
        if let Content::Text { html, .. } = &mut rich.content {
            *html = Some(b"<b>synthetic</b>".to_vec());
        }
        let rich_id = insert(&mut database, rich, at + 20, &generation);
        assert_ne!(rich_id, id);
        let mut private = original;
        private.no_cloud = true;
        assert_ne!(insert(&mut database, private, at + 30, &generation), id);
        let pinned = rows(&database).into_iter().find(|r| r.id == id).unwrap();
        assert!(pinned.pinned);
        assert_eq!(pinned.created_ms, at);
        assert_eq!(pinned.used_ms, at + 10);
        drop(database);
        let database =
            Database::open(&temp.path().join("history.sqlite3"), Box::new(FakeCrypto)).unwrap();
        assert_eq!(rows(&database).len(), 3);
        assert!(rows(&database).iter().any(|r| r.id == id && r.pinned));
    }

    #[test]
    fn search_reads_full_unicode_text_beyond_preview_and_files_without_io() {
        let (_temp, mut database, generation) = fresh();
        let at = now_ms();
        insert(
            &mut database,
            text(&format!(
                "{} 日本語 😀 needle-after-preview",
                "x".repeat(5000)
            )),
            at,
            &generation,
        );
        match database
            .query(Filter::Text, "日本語 😀 NEEDLE-AFTER-PREVIEW")
            .unwrap()
        {
            StoreResult::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert!(rows[0].preview.len() <= 4096);
                assert!(!rows[0].preview.contains("needle-after-preview"));
            }
            _ => panic!("unexpected result"),
        }
        insert(
            &mut database,
            Payload {
                content: Content::Files(vec![
                    r"\\does-not-exist.invalid\share\synthetic-file".into()
                ]),
                no_cloud: false,
            },
            at + 1,
            &generation,
        );
        match database.query(Filter::Files, "synthetic-file").unwrap() {
            StoreResult::Rows { rows, .. } => assert_eq!(rows.len(), 1),
            _ => panic!("unexpected result"),
        }
    }

    #[test]
    fn full_size_control_text_and_ram_only_hashes_survive_restart_dedupe() {
        let (temp, mut database, generation) = fresh();
        let at = now_ms();
        let payload = text(&"\u{1}".repeat(1024 * 1024));
        let id = insert(&mut database, payload.clone(), at, &generation);
        let blob: Vec<u8> = database
            .connection
            .query_row(
                "SELECT metadata FROM items WHERE id=?",
                [SqlNumber(id.0)],
                |r| r.get(0),
            )
            .unwrap();
        let metadata = FakeCrypto.decrypt(&blob).unwrap();
        // Header is followed immediately by full search text, never a dedupe hash.
        assert!(metadata.starts_with(b"DJCM\x01\x00\x00\x10\x00"));
        assert_eq!(database.hashes.len(), 1);
        drop(database);
        let mut database =
            Database::open(&temp.path().join("history.sqlite3"), Box::new(FakeCrypto)).unwrap();
        assert!(database.hashes.is_empty());
        assert_eq!(id, insert(&mut database, payload, at + 1, &generation));
        assert_eq!(rows(&database).len(), 1);
    }

    #[test]
    fn count_ttl_pin_cap_and_byte_budget_are_transactional() {
        let (_temp, mut database, generation) = fresh();
        let at = now_ms() + 100_000;
        database.limits.unpinned = 2;
        database.limits.pinned = 1;
        let pin = insert(&mut database, text("pin fixture"), at, &generation);
        perform(
            &mut database,
            StoreCommand::Pin {
                id: pin,
                pinned: true,
            },
            &generation,
        )
        .unwrap();
        let old = insert(&mut database, text("old fixture"), at + 1, &generation);
        let middle = insert(&mut database, text("middle fixture"), at + 2, &generation);
        let latest = insert(&mut database, text("latest fixture"), at + 3, &generation);
        assert!(!rows(&database).iter().any(|r| r.id == old));
        assert_eq!(rows(&database).len(), 3);
        assert!(perform(
            &mut database,
            StoreCommand::Pin {
                id: middle,
                pinned: true
            },
            &generation
        )
        .is_err());
        database.limits.ttl = 10;
        perform(
            &mut database,
            StoreCommand::Prune { now_ms: at + 13 },
            &generation,
        )
        .unwrap();
        assert_eq!(rows(&database).len(), 1);
        assert_eq!(rows(&database)[0].id, pin);
        let pinned_bytes = rows(&database)[0].bytes;
        database.limits.bytes = pinned_bytes;
        assert!(perform(
            &mut database,
            StoreCommand::Insert(capture(text("refused fixture"), at + 14)),
            &generation
        )
        .is_err());
        assert_eq!(rows(&database).len(), 1);
        assert_eq!(rows(&database)[0].id, pin);
        assert!(perform(&mut database, StoreCommand::Fetch(latest), &generation).is_err());
    }

    #[test]
    fn encrypted_budget_evicts_oldest_unpinned_only() {
        let (_temp, mut database, generation) = fresh();
        let at = now_ms();
        let first = insert(&mut database, text("first fixture"), at, &generation);
        database.limits.bytes = rows(&database)[0].bytes + 20;
        let second = insert(&mut database, text("other fixture"), at + 1, &generation);
        assert_eq!(rows(&database).len(), 1);
        assert_eq!(rows(&database)[0].id, second);
        assert_ne!(first, second);
    }

    #[test]
    fn sqlite_failure_rolls_back_insertion_and_evictions() {
        let (_temp, mut database, generation) = fresh();
        let at = now_ms();
        database.limits.unpinned = 1;
        let original = insert(&mut database, text("original fixture"), at, &generation);
        database.connection.execute_batch("CREATE TRIGGER reject_prune BEFORE DELETE ON items BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert!(perform(
            &mut database,
            StoreCommand::Insert(capture(text("replacement fixture"), at + 1)),
            &generation
        )
        .is_err());
        assert_eq!(rows(&database).len(), 1);
        assert_eq!(rows(&database)[0].id, original);
        assert!(perform(&mut database, StoreCommand::Delete(original), &generation).is_err());
        assert_eq!(rows(&database).len(), 1);
    }

    struct FailingCrypto;
    impl Crypto for FailingCrypto {
        fn encrypt(&self, _: &[u8]) -> Result<Vec<u8>, String> {
            Err("Synthetic encryption failure".into())
        }
        fn decrypt(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
            FakeCrypto.decrypt(bytes)
        }
    }
    #[test]
    fn encryption_failure_never_falls_back_or_prunes() {
        let (_temp, mut database, generation) = fresh();
        let at = now_ms();
        let id = insert(&mut database, text("existing fixture"), at, &generation);
        database.crypto = Box::new(FailingCrypto);
        database.limits.unpinned = 1;
        assert!(perform(
            &mut database,
            StoreCommand::Insert(capture(text("failed fixture"), at + 1)),
            &generation
        )
        .is_err());
        assert_eq!(rows(&database).len(), 1);
        assert_eq!(rows(&database)[0].id, id);
    }

    struct InvalidatingCrypto(Generation);
    impl Crypto for InvalidatingCrypto {
        fn encrypt(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
            self.0.fetch_add(1, Ordering::AcqRel);
            FakeCrypto.encrypt(bytes)
        }
        fn decrypt(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
            FakeCrypto.decrypt(bytes)
        }
    }
    #[test]
    fn generation_changes_during_encryption_rollback_the_whole_write() {
        let (_temp, mut database, generation) = fresh();
        let at = now_ms();
        let original = insert(&mut database, text("original fixture"), at, &generation);
        database.limits.unpinned = 1;
        database.crypto = Box::new(InvalidatingCrypto(generation.clone()));
        let result = database.execute(
            StoreCommand::Insert(capture(text("stale fixture"), at + 1)),
            1,
            &generation,
            &AtomicBool::new(false),
        );
        assert!(result.is_err());
        assert_eq!(rows(&database).len(), 1);
        assert_eq!(rows(&database)[0].id, original);
        assert!(matches!(
            database
                .execute(
                    StoreCommand::Clear {
                        include_pinned: true
                    },
                    1,
                    &generation,
                    &AtomicBool::new(false)
                )
                .unwrap(),
            StoreResult::Skipped
        ));
    }

    #[test]
    fn invalidation_after_sql_changes_but_before_commit_rolls_back() {
        let (_temp, mut database, generation) = fresh();
        let id = insert(
            &mut database,
            text("boundary fixture"),
            now_ms(),
            &generation,
        );
        let tx = database.connection.transaction().unwrap();
        tx.execute("DELETE FROM items WHERE id=?", [SqlNumber(id.0)])
            .unwrap();
        generation.fetch_add(1, Ordering::AcqRel);
        assert!(commit(tx, 1, &generation, &AtomicBool::new(false)).is_err());
        assert_eq!(rows(&database).len(), 1);
    }

    #[test]
    fn final_sqlite_commit_holds_invalidation_gate_and_ui_can_defer() {
        let (temp, mut database, generation) = fresh();
        let id = insert(
            &mut database,
            text("commit race fixture"),
            now_ms(),
            &generation,
        );
        database
            .connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")
            .unwrap();
        database
            .connection
            .busy_timeout(Duration::from_secs(3))
            .unwrap();
        let reader = Connection::open(temp.path().join("history.sqlite3")).unwrap();
        reader.execute_batch("BEGIN;").unwrap();
        reader
            .query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
            .unwrap();
        let (staged_tx, staged_rx) = std::sync::mpsc::channel();
        let worker_generation = generation.clone();
        let worker = thread::spawn(move || {
            let tx = database.connection.transaction().unwrap();
            tx.execute("UPDATE items SET used=used+1 WHERE id=?", [SqlNumber(id.0)])
                .unwrap();
            staged_tx.send(()).unwrap();
            commit(tx, 1, &worker_generation, &AtomicBool::new(false))
        });
        staged_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(1);
        while generation.try_fetch_add(0, Ordering::AcqRel).is_some() {
            assert!(std::time::Instant::now() < until, "commit gate deadline");
            thread::yield_now();
        }
        // Reader keeps SQLite inside COMMIT; invalidation must remain nonblocking
        // and leave the generation unchanged until that earlier commit completes.
        assert_eq!(generation.try_fetch_add(1, Ordering::AcqRel), None);
        assert_eq!(generation.load(Ordering::Acquire), 1);
        reader.execute_batch("ROLLBACK;").unwrap();
        assert!(worker.join().unwrap().is_ok());
        assert_eq!(generation.try_fetch_add(1, Ordering::AcqRel), Some(1));
        assert_eq!(generation.load(Ordering::Acquire), 2);
    }

    #[test]
    fn one_owned_lease_survives_delete_and_ids_are_never_reused() {
        let (_temp, mut database, generation) = fresh();
        let at = now_ms();
        let id = insert(&mut database, text("leased fixture"), at, &generation);
        let lease = match database.fetch(id).unwrap() {
            StoreResult::Item { payload, .. } => payload,
            _ => panic!("unexpected result"),
        };
        perform(
            &mut database,
            StoreCommand::Clear {
                include_pinned: true,
            },
            &generation,
        )
        .unwrap();
        let next = insert(&mut database, text("new fixture"), at + 1, &generation);
        assert_ne!(next, id);
        assert!(database.fetch(next).is_err());
        assert!(lease.plain_text().is_some_and(|s| s == "leased fixture"));
        drop(lease);
        assert!(database.fetch(next).is_ok());
        assert!(database.fetch(id).is_err());
    }

    #[test]
    fn corrupt_items_are_visible_and_fetch_errors_without_resetting_history() {
        let (_temp, mut database, generation) = fresh();
        let at = now_ms();
        let id = insert(&mut database, text("corrupt fixture"), at, &generation);
        database
            .connection
            .execute(
                "UPDATE items SET metadata=X'00',payload=X'00' WHERE id=?",
                [SqlNumber(id.0)],
            )
            .unwrap();
        assert!(rows(&database)[0].corrupt);
        assert!(database.fetch(id).is_err());
        assert_eq!(rows(&database).len(), 1);
        assert!(matches!(
            database.query(Filter::All, "unmatched").unwrap(),
            StoreResult::Rows { corrupt: 1, .. }
        ));
    }

    #[test]
    fn unsupported_and_unrecognized_databases_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("newer.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "user_version", VERSION + 1)
            .unwrap();
        drop(connection);
        assert!(Database::open(&path, Box::new(FakeCrypto)).is_err());
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            VERSION + 1
        );
        let path = temp.path().join("foreign.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("CREATE TABLE unrelated(marker INTEGER)", [])
            .unwrap();
        drop(connection);
        assert!(Database::open(&path, Box::new(FakeCrypto)).is_err());
    }

    fn wait_replies(store: &mut Store, expected: usize) -> Vec<StoreReply> {
        let until = std::time::Instant::now() + Duration::from_secs(5);
        let mut replies = Vec::new();
        while replies.len() < expected {
            replies.extend(store.drain());
            assert!(std::time::Instant::now() < until, "store reply deadline");
            thread::sleep(Duration::from_millis(2));
        }
        replies
    }
    #[test]
    fn slow_initial_open_prunes_current_generation_and_queued_ready_is_rebased() {
        let (temp, mut database, generation) = fresh();
        let expired = insert(
            &mut database,
            text("expired startup fixture"),
            now_ms(),
            &generation,
        );
        database
            .connection
            .execute("UPDATE items SET used=0 WHERE id=?", [SqlNumber(expired.0)])
            .unwrap();
        drop(database);
        let path = temp.path().join("history.sqlite3");
        let open_path = path.clone();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let (wake_tx, wake_rx) = std::sync::mpsc::channel();
        let mut store = Store::spawn_with_open(
            generation.clone(),
            Arc::new(move || {
                let _ = wake_tx.send(());
            }),
            move || {
                entered_tx.send(()).unwrap();
                resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                Database::open(&open_path, Box::new(FakeCrypto))
            },
        )
        .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        generation.fetch_add(1, Ordering::AcqRel);
        resume_tx.send(()).unwrap();
        wake_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // A second invalidation after Ready was queued must also be acknowledged.
        generation.fetch_add(1, Ordering::AcqRel);
        let replies = store.drain();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].request, 0);
        assert_eq!(replies[0].generation, 3);
        assert!(matches!(replies[0].result, Ok(StoreResult::Ready)));
        let database = Database::open(&path, Box::new(FakeCrypto)).unwrap();
        assert!(
            rows(&database).is_empty(),
            "startup pruning must commit for the new generation"
        );
        store.shutdown();
    }

    #[test]
    fn slow_initial_prune_rolls_back_and_retries_after_generation_changes() {
        let (_temp, mut database, generation) = fresh();
        let expired = insert(
            &mut database,
            text("prune race fixture"),
            now_ms(),
            &generation,
        );
        database
            .connection
            .execute("UPDATE items SET used=0 WHERE id=?", [SqlNumber(expired.0)])
            .unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let worker_generation = generation.clone();
        let worker = thread::spawn(move || {
            let stopped = AtomicBool::new(false);
            let mut attempts = 0;
            let result = prune_current(&worker_generation, &stopped, |expected| {
                attempts += 1;
                if attempts == 1 {
                    let tx = database.connection.transaction().unwrap();
                    prune_tx(&tx, now_ms(), database.limits).unwrap();
                    entered_tx.send(()).unwrap();
                    resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    commit(tx, expected, &worker_generation, &stopped)?;
                    return Ok(StoreResult::Changed { id: None });
                }
                assert_eq!(expected, 3);
                assert_eq!(rows(&database).len(), 1, "invalidated prune must roll back");
                database.prune(now_ms(), expected, &worker_generation, &stopped)
            })
            .unwrap()
            .unwrap();
            assert!(matches!(result, StoreResult::Changed { id: None }));
            assert!(rows(&database).is_empty());
            attempts
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        generation.fetch_add(1, Ordering::AcqRel);
        generation.fetch_add(1, Ordering::AcqRel);
        resume_tx.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), 2);
    }

    #[test]
    fn initial_prune_retry_stops_at_shutdown_and_keeps_real_errors() {
        let generation = generation();
        let stopped = AtomicBool::new(false);
        let mut attempts = 0;
        let result = prune_current(&generation, &stopped, |_| {
            attempts += 1;
            stopped.store(true, Ordering::Release);
            Err(INVALIDATED.into())
        });
        assert!(result.is_none());
        assert_eq!(attempts, 1);
        stopped.store(false, Ordering::Release);
        let result = prune_current(&generation, &stopped, |_| {
            generation.fetch_add(1, Ordering::AcqRel);
            Err(DB_ERROR.into())
        })
        .unwrap();
        assert!(matches!(result, Err(error) if error == DB_ERROR));
    }

    #[test]
    fn fatal_open_error_survives_generation_changes_and_worker_stop() {
        let generation = generation();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let (wake_tx, wake_rx) = std::sync::mpsc::channel();
        let mut store = Store::spawn_with_open(
            generation.clone(),
            Arc::new(move || {
                let _ = wake_tx.send(());
            }),
            move || {
                entered_tx.send(()).unwrap();
                resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                Err(DB_ERROR.into())
            },
        )
        .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        generation.fetch_add(1, Ordering::AcqRel);
        resume_tx.send(()).unwrap();
        wake_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while !store.shared.stopped() {
            assert!(
                std::time::Instant::now() < until,
                "failed worker stop deadline"
            );
            thread::yield_now();
        }
        // The failure is still pending after the worker shuts itself down.
        generation.fetch_add(1, Ordering::AcqRel);
        let replies = store.drain();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].request, 0);
        assert_eq!(replies[0].generation, 3);
        assert!(matches!(&replies[0].result, Err(error) if error == DB_ERROR));
        assert!(store
            .send(StoreRequest {
                request: 1,
                generation: 3,
                command: StoreCommand::Prune { now_ms: now_ms() }
            })
            .is_err());
        store.shutdown();
        assert!(store.drain().is_empty());
    }

    #[test]
    fn startup_prune_error_is_current_and_not_replaced_by_maintenance_change() {
        let (temp, mut database, generation) = fresh();
        insert(
            &mut database,
            text("startup failure fixture"),
            now_ms(),
            &generation,
        );
        database.connection.execute_batch("UPDATE items SET used=0; CREATE TRIGGER reject_startup_prune BEFORE DELETE ON items BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        drop(database);
        let (wake_tx, wake_rx) = std::sync::mpsc::channel();
        let mut store = Store::spawn(
            temp.path().join("history.sqlite3"),
            generation.clone(),
            Arc::new(move || {
                let _ = wake_tx.send(());
            }),
            Box::new(FakeCrypto),
        )
        .unwrap();
        wake_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        store.shared.publish(
            StoreReply {
                request: 0,
                generation: 1,
                result: Ok(StoreResult::Changed { id: None }),
            },
            0,
        );
        for _ in 0..MAX_REQUESTS * 2 {
            store.shared.publish(
                StoreReply {
                    request: 0,
                    generation: 1,
                    result: Err(DB_ERROR.into()),
                },
                0,
            );
            store.shared.publish(
                StoreReply {
                    request: 0,
                    generation: 1,
                    result: Ok(StoreResult::Changed { id: None }),
                },
                0,
            );
        }
        assert_eq!(
            store.shared.mail.lock().unwrap().replies.len(),
            2,
            "error and maintenance notices must coalesce separately"
        );
        generation.fetch_add(1, Ordering::AcqRel);
        let replies = store.drain();
        assert_eq!(replies.len(), 1, "stale maintenance change must be dropped");
        assert_eq!(replies[0].generation, 2);
        assert!(matches!(&replies[0].result, Err(error) if error == DB_ERROR));
        let database =
            Database::open(&temp.path().join("history.sqlite3"), Box::new(FakeCrypto)).unwrap();
        assert_eq!(
            rows(&database).len(),
            1,
            "failed startup prune must roll back"
        );
        store.shutdown();
    }

    #[test]
    fn request_bound_errors_remain_generation_scoped_even_with_request_zero() {
        let temp = tempfile::tempdir().unwrap();
        let generation = generation();
        let (wake_tx, wake_rx) = std::sync::mpsc::channel();
        let mut store = Store::spawn(
            temp.path().join("history.sqlite3"),
            generation.clone(),
            Arc::new(move || {
                let _ = wake_tx.send(());
            }),
            Box::new(FakeCrypto),
        )
        .unwrap();
        wake_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        wait_replies(&mut store, 1);
        store
            .send(StoreRequest {
                request: 0,
                generation: 1,
                command: StoreCommand::Fetch(ItemId(999)),
            })
            .unwrap();
        wake_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        generation.fetch_add(1, Ordering::AcqRel);
        assert!(store.drain().is_empty());
        let mail = store.shared.mail.lock().unwrap();
        assert_eq!(mail.outstanding, 0);
        assert_eq!(mail.bytes, 0);
    }

    #[test]
    fn asynchronous_protocol_acknowledges_persistence_and_bounds_admission() {
        let temp = tempfile::tempdir().unwrap();
        let generation = generation();
        let mut store = Store::spawn(
            temp.path().join("async.sqlite3"),
            generation.clone(),
            Arc::new(|| {}),
            Box::new(FakeCrypto),
        )
        .unwrap();
        assert!(matches!(
            wait_replies(&mut store, 1).pop().unwrap().result,
            Ok(StoreResult::Ready)
        ));
        for request in 1..=MAX_REQUESTS {
            store
                .send(StoreRequest {
                    request: request as u64,
                    generation: 1,
                    command: StoreCommand::Prune { now_ms: now_ms() },
                })
                .unwrap();
        }
        assert!(store
            .send(StoreRequest {
                request: 99,
                generation: 1,
                command: StoreCommand::Prune { now_ms: now_ms() }
            })
            .is_err());
        assert_eq!(wait_replies(&mut store, MAX_REQUESTS).len(), MAX_REQUESTS);
        // Even quickly completed operations reserve bytes until their reply is
        // drained. Large fetches cannot bypass admission through the worker.
        store
            .send(StoreRequest {
                request: 97,
                generation: 1,
                command: StoreCommand::Fetch(ItemId(999)),
            })
            .unwrap();
        assert!(store
            .send(StoreRequest {
                request: 98,
                generation: 1,
                command: StoreCommand::Fetch(ItemId(999))
            })
            .is_err());
        assert!(store
            .send(StoreRequest {
                request: 99,
                generation: 1,
                command: StoreCommand::Query {
                    filter: Filter::All,
                    query: String::new(),
                    now_ms: now_ms()
                }
            })
            .is_err());
        assert!(wait_replies(&mut store, 1).pop().unwrap().result.is_err());
        store
            .send(StoreRequest {
                request: 100,
                generation: 1,
                command: StoreCommand::Insert(capture(text("async fixture"), now_ms())),
            })
            .unwrap();
        let reply = wait_replies(&mut store, 1).pop().unwrap();
        assert!(matches!(
            reply.result,
            Ok(StoreResult::Changed { id: Some(_) })
        ));
        let connection = Connection::open(temp.path().join("async.sqlite3")).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        generation.fetch_add(1, Ordering::AcqRel);
        assert!(store
            .send(StoreRequest {
                request: 101,
                generation: 1,
                command: StoreCommand::Clear {
                    include_pinned: true
                }
            })
            .is_err());
        store.shutdown();
        assert!(store
            .send(StoreRequest {
                request: 102,
                generation: 2,
                command: StoreCommand::Fetch(ItemId(1))
            })
            .is_err());
    }

    struct BlockingCrypto {
        entered: std::sync::mpsc::SyncSender<()>,
        resume: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
        finished: std::sync::mpsc::Sender<()>,
    }
    impl Crypto for BlockingCrypto {
        fn encrypt(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
            if let Some(resume) = self.resume.lock().unwrap().take() {
                self.entered.send(()).unwrap();
                resume.recv().unwrap();
            }
            FakeCrypto.encrypt(bytes)
        }
        fn decrypt(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
            FakeCrypto.decrypt(bytes)
        }
    }
    impl Drop for BlockingCrypto {
        fn drop(&mut self) {
            let _ = self.finished.send(());
        }
    }
    #[test]
    fn shutdown_detaches_blocked_crypto_and_late_work_cannot_commit_or_wake() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("blocked.sqlite3");
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let wakes = Arc::new(AtomicU64::new(0));
        let wake_counter = wakes.clone();
        let mut store = Store::spawn(
            path.clone(),
            generation(),
            Arc::new(move || {
                wake_counter.fetch_add(1, Ordering::AcqRel);
            }),
            Box::new(BlockingCrypto {
                entered: entered_tx,
                resume: Mutex::new(Some(resume_rx)),
                finished: finished_tx,
            }),
        )
        .unwrap();
        wait_replies(&mut store, 1);
        store
            .send(StoreRequest {
                request: 1,
                generation: 1,
                command: StoreCommand::Insert(capture(text("blocked fixture"), now_ms())),
            })
            .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let start = std::time::Instant::now();
        store.shutdown();
        assert!(start.elapsed() < Duration::from_millis(200));
        let before = wakes.load(Ordering::Acquire);
        resume_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(wakes.load(Ordering::Acquire), before);
        assert!(store.drain().is_empty());
        let database = Database::open(&path, Box::new(FakeCrypto)).unwrap();
        assert!(rows(&database).is_empty());
    }
}
