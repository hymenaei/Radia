use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use network::{Grid, SimulatorId};
use protocol::{
    lludp::{Message, Packet},
    messages::{
        CacheMissType::{CrcMismatch, Full},
        CachedObjectReference, ObjectUpdate, ObjectUpdateCached, ObjectUpdateCompressed, RegionHandshake,
        object::RegionData,
    },
};
use rusqlite::{Connection, Error as RusqliteError, OptionalExtension, Transaction, params};
use storage::{
    default_cache_dir,
    sqlite::{MigrationError, OpenStatus, SqliteBatch, SqliteDb, SqliteError},
};
use tokio::{
    runtime::Handle,
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
    time::timeout,
};
use uuid::Uuid;
use world::{OrderEvidence, Region, UpdateSource};

use crate::session::inbound::{ObjectBatch, ObjectMediaRequest, ObjectOperation, decode_cached_object, object_media_requests};

const SCHEMA_VERSION: i64 = 2;
const SQLITE_PAGE_SIZE: i64 = 4096;
const MAX_DATABASE_BYTES: usize = 256 * 1024 * 1024;
const MAX_DATABASE_PAGES: i64 = (MAX_DATABASE_BYTES as i64) / SQLITE_PAGE_SIZE;
const MAX_OBJECT_CACHE_ENTRY_BYTES: usize = 10_000;
const MIN_OBJECT_CACHE_ENTRY_BYTES: usize = 64;
const MAX_REGION_ENTRIES: usize = 10_000;
const MAX_REGION_BYTES: usize = 32 * 1024 * 1024;
const MAX_TOTAL_ENTRIES: usize = 50_000;
const MAX_TOTAL_BYTES: usize = 192 * 1024 * 1024;
const MAX_GLOBAL_PRUNE_ROWS: usize = 255;
const MAX_REGION_DELETE_BATCHES: usize = MAX_REGION_ENTRIES.div_ceil(255);
const MAX_PENDING_SUBMISSIONS: usize = 16;
const MAX_PENDING_ROWS: usize = 65_536;
const MAX_PENDING_BYTES: usize = 16 * 1024 * 1024;
pub(super) const HANDSHAKE_CACHE_LOAD_TIMEOUT: Duration = Duration::from_millis(50);
const MAX_PENDING_REGION_LOADS: usize = 2;
const CACHE_EVENT_CAPACITY: usize = 16;
const CACHE_SUPPORT_FLAG: u32 = 1;
const EMPTY_CACHE_FLAG: u32 = 2;
const SELF_APPEARANCE_FLAG: u32 = 4;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct CacheScope {
    grid_id: String,
    region_id: Uuid,
    cache_id: Uuid,
}

#[derive(Clone)]
pub(super) struct StoredEntry {
    local_id: i64,
    crc: i64,
    update_kind: i64,
    update_flags: i64,
    payload_bytes: i64,
    payload: Option<Vec<u8>>,
}

struct LoadedRegion {
    scope: CacheScope,
    entries: HashMap<u32, StoredEntry>,
    bytes: usize,
    last_used: u64,
    complete: bool,
}

#[derive(Clone)]
struct EncodedEntry {
    local_id: u32,
    crc: u32,
    update_kind: i64,
    update_flags: u32,
    payload: Vec<u8>,
}

struct PendingSubmission {
    batches: Vec<SqliteBatch>,
    rows: usize,
    bytes: usize,
}

pub(super) enum ObjectCacheEvent {
    Ready { db: SqliteDb, status: OpenStatus },
    OpenFailed(String),
    RegionLoaded { simulator_id: SimulatorId, scope: CacheScope, result: RegionLoadResult, _permit: OwnedSemaphorePermit },
}

pub(super) enum RegionLoadResult {
    Loaded(HashMap<u32, StoredEntry>),
    Failed(String),
    TimedOut,
}

pub(super) struct ObjectCacheSession {
    grid_id: String,
    identities: HashMap<SimulatorId, CacheScope>,
    loaded_regions: HashMap<SimulatorId, LoadedRegion>,
    loaded_entries: usize,
    loaded_bytes: usize,
    use_clock: u64,
    db: Option<SqliteDb>,
    event_sender: mpsc::Sender<ObjectCacheEvent>,
    event_receiver: mpsc::Receiver<ObjectCacheEvent>,
    load_slots: Arc<Semaphore>,
    pending: Vec<PendingSubmission>,
    pending_rows: usize,
    pending_bytes: usize,
    started: bool,
    disabled: bool,
    warned: bool,
}

impl ObjectCacheSession {
    pub(super) fn new(grid: Grid) -> Self {
        let (event_sender, event_receiver) = mpsc::channel(CACHE_EVENT_CAPACITY);
        Self {
            grid_id: grid.id().to_owned(),
            identities: HashMap::new(),
            loaded_regions: HashMap::new(),
            loaded_entries: 0,
            loaded_bytes: 0,
            use_clock: 0,
            db: None,
            event_sender,
            event_receiver,
            load_slots: Arc::new(Semaphore::new(MAX_PENDING_REGION_LOADS)),
            pending: Vec::new(),
            pending_rows: 0,
            pending_bytes: 0,
            started: false,
            disabled: false,
            warned: false,
        }
    }

    pub(super) fn start(&mut self, runtime: &Handle) {
        if self.started || self.disabled {
            return;
        }
        self.started = true;
        let sender = self.event_sender.clone();
        drop(runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(open_default).await;
            let event = match result {
                Ok(Ok((db, status))) => ObjectCacheEvent::Ready { db, status },
                Ok(Err(error)) => ObjectCacheEvent::OpenFailed(error),
                Err(error) => ObjectCacheEvent::OpenFailed(error.to_string()),
            };
            if let Err(send_error) = sender.send(event).await {
                drop(send_error);
            }
        }));
    }

    pub(super) async fn next_event(&mut self) -> Option<ObjectCacheEvent> {
        self.event_receiver.recv().await
    }

    pub(super) fn handle_event(&mut self, event: ObjectCacheEvent) -> Option<(SimulatorId, u32)> {
        match event {
            ObjectCacheEvent::Ready { db, status } => {
                if matches!(status, OpenStatus::Recreated(_)) {
                    tracing::warn!(?status, "object cache database was recreated; cached objects will be refetched");
                }
                self.db = Some(db);
                let pending = std::mem::take(&mut self.pending);
                self.pending_rows = 0;
                self.pending_bytes = 0;
                for submission in pending {
                    self.submit_to_db(submission);
                }
                None
            }
            ObjectCacheEvent::OpenFailed(error) => {
                self.warn(&error);
                self.disabled = true;
                self.pending.clear();
                self.pending_rows = 0;
                self.pending_bytes = 0;
                None
            }
            ObjectCacheEvent::RegionLoaded { simulator_id, scope, result, _permit } => {
                match result {
                    RegionLoadResult::Loaded(mut entries) => {
                        if self.identities.get(&simulator_id) == Some(&scope) {
                            self.merge_partial_region(simulator_id, &scope, &mut entries);
                            self.install_loaded_region(simulator_id, scope.clone(), entries);
                        }
                    }
                    RegionLoadResult::Failed(error) => self.warn(&error),
                    RegionLoadResult::TimedOut => self.warn("region object cache load exceeded its handshake deadline"),
                }
                let flags = if self.identities.get(&simulator_id) == Some(&scope) {
                    self.region_handshake_flags(simulator_id)
                } else {
                    CACHE_SUPPORT_FLAG | SELF_APPEARANCE_FLAG | EMPTY_CACHE_FLAG
                };
                Some((simulator_id, flags))
            }
        }
    }

    pub(super) fn set_region_identity(&mut self, simulator_id: SimulatorId, handshake: &RegionHandshake) {
        let region_id = handshake.region_info2.as_ref().map(|info| info.region_id).filter(|id| !id.is_nil());
        let cache_id = (!handshake.region_info1.cache_id.is_nil()).then_some(handshake.region_info1.cache_id);
        let next = region_id.zip(cache_id).map(|(region_id, cache_id)| CacheScope { grid_id: self.grid_id.clone(), region_id, cache_id });
        let previous = self.identities.get(&simulator_id).cloned();
        if previous == next {
            return;
        }
        self.remove_loaded_region(simulator_id);
        if let Some(previous) = previous {
            self.enqueue(make_scope_delete_submission(previous));
        }
        match next {
            Some(scope) => {
                self.identities.insert(simulator_id, scope);
            }
            None => {
                self.identities.remove(&simulator_id);
            }
        }
    }

    pub(super) fn forget_simulator(&mut self, simulator_id: SimulatorId) {
        self.identities.remove(&simulator_id);
        self.remove_loaded_region(simulator_id);
    }

    pub(super) fn region_handshake_flags(&self, simulator_id: SimulatorId) -> u32 {
        let has_cache = self.db.is_some()
            && !self.disabled
            && self.identities.get(&simulator_id).is_some_and(|scope| {
                self.loaded_regions
                    .get(&simulator_id)
                    .is_some_and(|region| region.scope == *scope && region.complete && !region.entries.is_empty())
            });
        CACHE_SUPPORT_FLAG | SELF_APPEARANCE_FLAG | if has_cache { 0 } else { EMPTY_CACHE_FLAG }
    }

    pub(super) fn record_update(&mut self, simulator_id: SimulatorId, message: &Message) {
        let Some(scope) = self.identities.get(&simulator_id).cloned() else { return };
        let entries = encode_entries(message);
        if entries.is_empty() {
            return;
        }
        self.remember_entries(simulator_id, scope.clone(), &entries);
        if !self.disabled {
            self.enqueue(make_write_submission(scope, entries, now_millis()));
        }
    }

    pub(super) fn begin_region_load(&mut self, simulator_id: SimulatorId, runtime: &Handle) -> Option<u32> {
        let Some(scope) = self.identities.get(&simulator_id).cloned() else { return Some(self.region_handshake_flags(simulator_id)) };
        if self.loaded_regions.get(&simulator_id).is_some_and(|region| region.scope == scope && region.complete) {
            return Some(self.region_handshake_flags(simulator_id));
        }
        if self.disabled {
            return Some(self.region_handshake_flags(simulator_id));
        }
        let Some(db) = self.db.as_ref() else { return Some(self.region_handshake_flags(simulator_id)) };
        let permit = match Arc::clone(&self.load_slots).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                self.warn("region object cache load capacity is full");
                return Some(self.region_handshake_flags(simulator_id));
            }
        };
        let receipt = match queue_scope_load(db, scope.clone()) {
            Ok(receipt) => receipt,
            Err(error) => {
                self.warn(&error.to_string());
                return Some(self.region_handshake_flags(simulator_id));
            }
        };
        let sender = self.event_sender.clone();
        drop(runtime.spawn(async move {
            let result = match timeout(HANDSHAKE_CACHE_LOAD_TIMEOUT, receipt.wait()).await {
                Ok(Ok(entries)) => RegionLoadResult::Loaded(entries),
                Ok(Err(error)) => RegionLoadResult::Failed(error.to_string()),
                Err(_) => RegionLoadResult::TimedOut,
            };
            let event = ObjectCacheEvent::RegionLoaded { simulator_id, scope, result, _permit: permit };
            if let Err(send_error) = sender.send(event).await {
                drop(send_error);
            }
        }));
        None
    }

    pub(super) fn resolve_cached(
        &mut self,
        simulator_id: SimulatorId,
        region: Region,
        sequence: u32,
        order: OrderEvidence,
        update: &ObjectUpdateCached,
        mut batch: ObjectBatch,
    ) -> (ObjectBatch, Vec<ObjectMediaRequest>) {
        let scope = self.identities.get(&simulator_id).cloned();
        let mut stale = HashSet::new();
        let mut media_requests = Vec::new();
        let mut invalid_fields = 0;
        for (index, operation) in batch.blocks.iter_mut().enumerate() {
            let Some(ObjectOperation::Cached { source, local_id, .. }) = operation else { continue };
            let source = *source;
            let local_id = *local_id;
            let reference = update.objects.get(index).copied();
            let entry = self
                .loaded_regions
                .get(&simulator_id)
                .filter(|region| scope.as_ref() == Some(&region.scope))
                .and_then(|region| region.entries.get(&local_id.get()));
            let mut cache_miss_type = Full;
            let recovered = reference.zip(entry).and_then(|(reference, entry)| {
                let Some(crc) = u32::try_from(entry.crc).ok() else {
                    stale.insert(local_id.get());
                    return None;
                };
                if crc != reference.crc {
                    stale.insert(local_id.get());
                    cache_miss_type = CrcMismatch;
                    return None;
                }
                let Some(mut payload) = decode_entry(entry, reference, local_id.get()) else {
                    stale.insert(local_id.get());
                    return None;
                };
                set_region_data(&mut payload, update.region_data);
                let ordinal = u32::try_from(index).ok()?;
                let operation = decode_cached_object(region, sequence, order, ordinal, reference, &payload, &mut invalid_fields);
                if operation.is_some() {
                    for mut request in object_media_requests(region, sequence, order, &payload) {
                        request.source = UpdateSource::new(region, sequence, order, ordinal);
                        media_requests.push(request);
                    }
                }
                operation
            });
            *operation = match recovered {
                Some(operation) => Some(operation),
                None => Some(ObjectOperation::Cached { source, local_id, cache_miss_type }),
            };
        }
        batch.invalid_fields = batch.invalid_fields.saturating_add(invalid_fields);
        if let Some(scope) = scope
            && self.loaded_regions.get(&simulator_id).is_some_and(|region| region.scope == scope)
        {
            self.touch_loaded_region(simulator_id, &scope);
            self.remove_loaded_entries(simulator_id, &stale);
            self.enqueue(make_access_submission(scope, stale, now_millis()));
        }
        (batch, media_requests)
    }

    pub(super) async fn close(mut self) {
        while !self.pending.is_empty() && self.db.is_none() && !self.disabled {
            let Some(event) = self.event_receiver.recv().await else { break };
            assert!(self.handle_event(event).is_none());
        }
        let Some(db) = self.db.take() else { return };
        if db.had_write_failure() {
            self.warn("SQLite worker reported a failed object-cache write");
        }
        match tokio::task::spawn_blocking(move || db.shutdown()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => self.warn(&error.to_string()),
            Err(error) => self.warn(&error.to_string()),
        }
    }

    fn enqueue(&mut self, submission: PendingSubmission) -> bool {
        if self.disabled {
            return false;
        }
        if self.db.is_some() {
            return self.submit_to_db(submission);
        }
        let rows = self.pending_rows.saturating_add(submission.rows);
        let bytes = self.pending_bytes.saturating_add(submission.bytes);
        if self.pending.len() >= MAX_PENDING_SUBMISSIONS || rows > MAX_PENDING_ROWS || bytes > MAX_PENDING_BYTES {
            self.warn("object cache write queue is full; some objects will be refetched");
            return false;
        }
        self.pending_rows = rows;
        self.pending_bytes = bytes;
        self.pending.push(submission);
        true
    }

    fn submit_to_db(&mut self, submission: PendingSubmission) -> bool {
        let Some(db) = &self.db else { return false };
        match db.submit_batches(submission.batches) {
            Ok(receipt) => {
                drop(receipt);
                true
            }
            Err(error) => {
                self.warn(&error.to_string());
                false
            }
        }
    }

    fn warn(&mut self, error: &str) {
        if self.warned {
            return;
        }
        self.warned = true;
        tracing::warn!(%error, "object cache is unavailable; simulator updates will be fetched normally");
    }

    fn remember_entries(&mut self, simulator_id: SimulatorId, scope: CacheScope, entries: &[EncodedEntry]) {
        self.ensure_loaded_region(simulator_id, scope);
        for entry in entries {
            let current_bytes = self
                .loaded_regions
                .get(&simulator_id)
                .and_then(|region| region.entries.get(&entry.local_id))
                .map_or(0, |old| usize::try_from(old.payload_bytes).unwrap_or(usize::MAX));
            let replacing = self.loaded_regions.get(&simulator_id).is_some_and(|region| region.entries.contains_key(&entry.local_id));
            let Some(bytes_after) = self
                .loaded_regions
                .get(&simulator_id)
                .and_then(|region| region.bytes.checked_sub(current_bytes))
                .and_then(|bytes| bytes.checked_add(entry.payload.len()))
            else {
                continue;
            };
            let region_entries = self.loaded_regions.get(&simulator_id).map_or(0, |region| region.entries.len());
            if (!replacing && region_entries >= MAX_REGION_ENTRIES) || bytes_after > MAX_REGION_BYTES {
                continue;
            }
            let row_increment = usize::from(!replacing);
            let byte_increment = entry.payload.len().saturating_sub(current_bytes);
            self.make_loaded_room(simulator_id, row_increment, byte_increment);
            let rows_after = self.loaded_entries.saturating_add(row_increment);
            let bytes_after_global = self.loaded_bytes.saturating_sub(current_bytes).saturating_add(entry.payload.len());
            if rows_after > MAX_TOTAL_ENTRIES || bytes_after_global > MAX_TOTAL_BYTES {
                continue;
            }
            let last_used = self.next_use();
            if let Some(region) = self.loaded_regions.get_mut(&simulator_id) {
                region.bytes = bytes_after;
                region.last_used = last_used;
                region.entries.insert(entry.local_id, stored_entry(entry));
            }
            self.loaded_entries = rows_after;
            self.loaded_bytes = bytes_after_global;
        }
    }

    fn ensure_loaded_region(&mut self, simulator_id: SimulatorId, scope: CacheScope) {
        if self.loaded_regions.get(&simulator_id).is_some_and(|region| region.scope == scope) {
            return;
        }
        self.remove_loaded_region(simulator_id);
        let last_used = self.next_use();
        self.loaded_regions.insert(simulator_id, LoadedRegion { scope, entries: HashMap::new(), bytes: 0, last_used, complete: false });
    }

    fn merge_partial_region(&mut self, simulator_id: SimulatorId, scope: &CacheScope, entries: &mut HashMap<u32, StoredEntry>) {
        let Some(partial) = self.loaded_regions.remove(&simulator_id) else { return };
        self.loaded_entries = self.loaded_entries.saturating_sub(partial.entries.len());
        self.loaded_bytes = self.loaded_bytes.saturating_sub(partial.bytes);
        if partial.scope != *scope {
            return;
        }
        let mut bytes = entries
            .values()
            .map(|entry| usize::try_from(entry.payload_bytes).unwrap_or(usize::MAX))
            .try_fold(0_usize, usize::checked_add)
            .unwrap_or(usize::MAX);
        for (local_id, latest) in partial.entries {
            let previous = entries.remove(&local_id);
            let previous_bytes = previous.as_ref().map_or(0, |entry| usize::try_from(entry.payload_bytes).unwrap_or(usize::MAX));
            let latest_bytes = usize::try_from(latest.payload_bytes).unwrap_or(usize::MAX);
            let Some(bytes_after) = bytes.checked_sub(previous_bytes).and_then(|bytes| bytes.checked_add(latest_bytes)) else { continue };
            if (previous.is_some() || entries.len() < MAX_REGION_ENTRIES) && bytes_after <= MAX_REGION_BYTES {
                entries.insert(local_id, latest);
                bytes = bytes_after;
            } else if let Some(previous) = previous {
                entries.insert(local_id, previous);
            }
        }
    }

    fn install_loaded_region(&mut self, simulator_id: SimulatorId, scope: CacheScope, entries: HashMap<u32, StoredEntry>) {
        if entries.len() > MAX_REGION_ENTRIES {
            return;
        }
        let bytes =
            entries.values().map(|entry| usize::try_from(entry.payload_bytes).unwrap_or(usize::MAX)).try_fold(0_usize, usize::checked_add);
        let Some(bytes) = bytes.filter(|bytes| *bytes <= MAX_REGION_BYTES) else { return };
        self.make_loaded_room(simulator_id, entries.len(), bytes);
        if entries.len() > MAX_TOTAL_ENTRIES
            || bytes > MAX_TOTAL_BYTES
            || self.loaded_entries.saturating_add(entries.len()) > MAX_TOTAL_ENTRIES
            || self.loaded_bytes.saturating_add(bytes) > MAX_TOTAL_BYTES
        {
            return;
        }
        let last_used = self.next_use();
        self.loaded_entries += entries.len();
        self.loaded_bytes += bytes;
        self.loaded_regions.insert(simulator_id, LoadedRegion { scope, entries, bytes, last_used, complete: true });
    }

    fn remove_loaded_region(&mut self, simulator_id: SimulatorId) {
        if let Some(region) = self.loaded_regions.remove(&simulator_id) {
            self.loaded_entries = self.loaded_entries.saturating_sub(region.entries.len());
            self.loaded_bytes = self.loaded_bytes.saturating_sub(region.bytes);
        }
    }

    fn remove_loaded_entries(&mut self, simulator_id: SimulatorId, local_ids: &HashSet<u32>) {
        let Some(region) = self.loaded_regions.get_mut(&simulator_id) else { return };
        for local_id in local_ids {
            if let Some(entry) = region.entries.remove(local_id) {
                let bytes = usize::try_from(entry.payload_bytes).unwrap_or(usize::MAX);
                region.bytes = region.bytes.saturating_sub(bytes);
                self.loaded_entries = self.loaded_entries.saturating_sub(1);
                self.loaded_bytes = self.loaded_bytes.saturating_sub(bytes);
            }
        }
    }

    fn touch_loaded_region(&mut self, simulator_id: SimulatorId, scope: &CacheScope) {
        let last_used = self.next_use();
        if let Some(region) = self.loaded_regions.get_mut(&simulator_id).filter(|region| &region.scope == scope) {
            region.last_used = last_used;
        }
    }

    fn make_loaded_room(&mut self, current: SimulatorId, rows: usize, bytes: usize) {
        while (self.loaded_entries.saturating_add(rows) > MAX_TOTAL_ENTRIES || self.loaded_bytes.saturating_add(bytes) > MAX_TOTAL_BYTES)
            && let Some(oldest) = self
                .loaded_regions
                .iter()
                .filter(|(simulator_id, _)| **simulator_id != current)
                .min_by_key(|(_, region)| region.last_used)
                .map(|(simulator_id, _)| *simulator_id)
        {
            self.remove_loaded_region(oldest);
        }
    }

    fn next_use(&mut self) -> u64 {
        self.use_clock = self.use_clock.saturating_add(1);
        self.use_clock
    }
}

fn open_default() -> Result<(SqliteDb, OpenStatus), String> {
    let root = default_cache_dir().map_err(|error| error.to_string())?;
    open_at(root.join("objects.sqlite")).map_err(|error| error.to_string())
}

fn open_at(path: impl AsRef<Path>) -> Result<(SqliteDb, OpenStatus), SqliteError> {
    SqliteDb::open(path, migrate)
}

fn migrate(connection: &mut Connection) -> Result<(), MigrationError> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        0 => {
            connection.execute_batch(
                "CREATE TABLE region_scopes (
                    grid_id TEXT NOT NULL,
                    region_id BLOB NOT NULL CHECK(length(region_id) = 16),
                    cache_id BLOB NOT NULL CHECK(length(cache_id) = 16),
                    last_used INTEGER NOT NULL,
                    PRIMARY KEY (grid_id, region_id, cache_id)
                ) WITHOUT ROWID;
                CREATE TABLE object_entries (
                    grid_id TEXT NOT NULL,
                    region_id BLOB NOT NULL CHECK(length(region_id) = 16),
                    cache_id BLOB NOT NULL CHECK(length(cache_id) = 16),
                    local_id INTEGER NOT NULL CHECK(local_id > 0 AND local_id <= 4294967295),
                    crc INTEGER NOT NULL CHECK(crc >= 0 AND crc <= 4294967295),
                    update_kind INTEGER NOT NULL CHECK(update_kind IN (0, 1)),
                    update_flags INTEGER NOT NULL CHECK(update_flags >= 0 AND update_flags <= 4294967295),
                    payload BLOB NOT NULL CHECK(length(payload) >= 64 AND length(payload) <= 10000),
                    PRIMARY KEY (grid_id, region_id, cache_id, local_id)
                ) WITHOUT ROWID;
                ",
            )?;
            install_global_stats(connection, false)?;
            connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        1 => {
            install_global_stats(connection, true)?;
            connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        SCHEMA_VERSION => {}
        _ => return Err(MigrationError::SchemaMismatch),
    }
    connection.pragma_update(None, "max_page_count", MAX_DATABASE_PAGES)?;
    Ok(())
}

fn install_global_stats(connection: &Connection, backfill: bool) -> Result<(), MigrationError> {
    connection.execute_batch(
        "CREATE TABLE object_cache_totals (
            singleton INTEGER NOT NULL PRIMARY KEY CHECK(singleton = 1),
            entry_count INTEGER NOT NULL CHECK(entry_count >= 0),
            payload_bytes INTEGER NOT NULL CHECK(payload_bytes >= 0)
        ) WITHOUT ROWID;",
    )?;
    if backfill {
        connection.execute(
            "INSERT INTO object_cache_totals (singleton, entry_count, payload_bytes)
             SELECT 1, COUNT(*), COALESCE(SUM(length(payload)), 0) FROM object_entries",
            [],
        )?;
    } else {
        connection.execute("INSERT INTO object_cache_totals (singleton, entry_count, payload_bytes) VALUES (1, 0, 0)", [])?;
    }
    connection.execute_batch(
        "CREATE TRIGGER object_entries_totals_insert AFTER INSERT ON object_entries BEGIN
            UPDATE object_cache_totals SET entry_count = entry_count + 1, payload_bytes = payload_bytes + length(NEW.payload)
            WHERE singleton = 1;
         END;
         CREATE TRIGGER object_entries_totals_delete AFTER DELETE ON object_entries BEGIN
            UPDATE object_cache_totals SET entry_count = entry_count - 1, payload_bytes = payload_bytes - length(OLD.payload)
            WHERE singleton = 1;
         END;
         CREATE TRIGGER object_entries_totals_update AFTER UPDATE OF payload ON object_entries
         WHEN length(OLD.payload) != length(NEW.payload) BEGIN
            UPDATE object_cache_totals SET payload_bytes = payload_bytes + length(NEW.payload) - length(OLD.payload)
            WHERE singleton = 1;
         END;",
    )?;
    Ok(())
}

#[cfg(test)]
pub(super) fn open_for_test(path: impl AsRef<Path>, grid: Grid) -> ObjectCacheSession {
    let (db, status) = open_at(path).expect("open isolated object cache database");
    let mut cache = ObjectCacheSession::new(grid);
    assert!(cache.handle_event(ObjectCacheEvent::Ready { db, status }).is_none());
    cache
}

fn queue_scope_load(db: &SqliteDb, scope: CacheScope) -> Result<storage::sqlite::SqliteReceipt<HashMap<u32, StoredEntry>>, SqliteError> {
    db.read(0, move |transaction| {
        let (row_count, bytes) = scope_stats(transaction, &scope)?;
        if row_count > MAX_REGION_ENTRIES || bytes > MAX_REGION_BYTES {
            return Ok(HashMap::new());
        }
        let mut statement = transaction.prepare(
            "SELECT local_id, crc, update_kind, update_flags, length(payload),
                    CASE WHEN length(payload) <= ?1 THEN payload ELSE NULL END
             FROM object_entries WHERE grid_id = ?2 AND region_id = ?3 AND cache_id = ?4 ORDER BY local_id",
        )?;
        let row_iter = statement.query_map(
            params![
                MAX_OBJECT_CACHE_ENTRY_BYTES as i64,
                scope.grid_id,
                scope.region_id.as_bytes().as_slice(),
                scope.cache_id.as_bytes().as_slice()
            ],
            |row| {
                Ok(StoredEntry {
                    local_id: row.get(0)?,
                    crc: row.get(1)?,
                    update_kind: row.get(2)?,
                    update_flags: row.get(3)?,
                    payload_bytes: row.get(4)?,
                    payload: row.get(5)?,
                })
            },
        )?;
        let mut entries = HashMap::with_capacity(row_count);
        for row in row_iter {
            let entry = row?;
            if let Ok(local_id) = u32::try_from(entry.local_id) {
                entries.insert(local_id, entry);
            }
        }
        Ok(entries)
    })
}

fn encode_entries(message: &Message) -> Vec<EncodedEntry> {
    match message {
        Message::ObjectUpdate(update) => {
            update
                .objects
                .iter()
                .filter_map(|object| {
                    let local_id = object.local_id;
                    if local_id == 0 {
                        return None;
                    }
                    let payload = Packet::new(
                        0,
                        Message::ObjectUpdate(ObjectUpdate { region_data: update.region_data, objects: vec![object.clone()] }),
                    )
                    .encode()
                    .ok()?;
                    (payload.len() >= MIN_OBJECT_CACHE_ENTRY_BYTES && payload.len() <= MAX_OBJECT_CACHE_ENTRY_BYTES)
                        .then_some(EncodedEntry { local_id, crc: object.crc, update_kind: 0, update_flags: object.update_flags, payload })
                })
                .collect()
        }
        Message::ObjectUpdateCompressed(update) => {
            update
                .objects
                .iter()
                .filter_map(|object| {
                    let local_id = object.local_id;
                    if local_id == 0 {
                        return None;
                    }
                    let payload = Packet::new(
                        0,
                        Message::ObjectUpdateCompressed(ObjectUpdateCompressed {
                            region_data: update.region_data,
                            objects: vec![object.clone()],
                        }),
                    )
                    .encode()
                    .ok()?;
                    (payload.len() >= MIN_OBJECT_CACHE_ENTRY_BYTES && payload.len() <= MAX_OBJECT_CACHE_ENTRY_BYTES)
                        .then_some(EncodedEntry { local_id, crc: object.crc, update_kind: 1, update_flags: object.update_flags, payload })
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

fn make_write_submission(scope: CacheScope, entries: Vec<EncodedEntry>, last_used: i64) -> PendingSubmission {
    let payload_bytes = entries.iter().map(|entry| entry.payload.len()).sum::<usize>();
    let submission_rows = entries.len().saturating_add(1 + 256 + MAX_GLOBAL_PRUNE_ROWS * 2);
    let submission_bytes = payload_bytes.saturating_add(entries.len().saturating_mul(64)).saturating_add(512);
    let scope_for_write = scope.clone();
    let scope_batch = SqliteBatch::new(1, 64, move |transaction| upsert_scope(transaction, &scope_for_write, last_used))
        .expect("scope batch fits SQLite limits");
    let scope_for_entries = scope.clone();
    let entries_batch = SqliteBatch::new(entries.len(), payload_bytes, move |transaction| {
        for entry in entries {
            transaction.execute(
                "INSERT INTO object_entries (grid_id, region_id, cache_id, local_id, crc, update_kind, update_flags, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (grid_id, region_id, cache_id, local_id) DO UPDATE SET
                    crc = excluded.crc,
                    update_kind = excluded.update_kind,
                    update_flags = excluded.update_flags,
                    payload = excluded.payload",
                params![
                    scope_for_entries.grid_id.clone(),
                    scope_for_entries.region_id.as_bytes().as_slice(),
                    scope_for_entries.cache_id.as_bytes().as_slice(),
                    i64::from(entry.local_id),
                    i64::from(entry.crc),
                    entry.update_kind,
                    i64::from(entry.update_flags),
                    entry.payload,
                ],
            )?;
        }
        Ok(())
    })
    .expect("object entries batch fits SQLite limits");
    let region_prune =
        SqliteBatch::new(256, 256, move |transaction| prune_scope(transaction, &scope, MAX_REGION_ENTRIES, MAX_REGION_BYTES))
            .expect("region prune batch fits SQLite limits");
    let global_prune_first = SqliteBatch::new(256, 256, prune_global_chunk).expect("global prune batch fits SQLite limits");
    let global_prune_second = SqliteBatch::new(256, 256, prune_global_chunk).expect("global prune batch fits SQLite limits");
    PendingSubmission {
        batches: vec![scope_batch, entries_batch, region_prune, global_prune_first, global_prune_second],
        rows: submission_rows,
        bytes: submission_bytes,
    }
}

fn make_scope_delete_submission(scope: CacheScope) -> PendingSubmission {
    let mut batches = Vec::with_capacity(MAX_REGION_DELETE_BATCHES);
    for _ in 0..MAX_REGION_DELETE_BATCHES {
        let scope = scope.clone();
        let batch = SqliteBatch::new(256, 256, move |transaction| delete_scope_chunk(transaction, &scope))
            .expect("scope delete batch fits SQLite limits");
        batches.push(batch);
    }
    PendingSubmission { batches, rows: MAX_REGION_DELETE_BATCHES * 256, bytes: MAX_REGION_DELETE_BATCHES * 256 }
}

fn make_access_submission(scope: CacheScope, stale: HashSet<u32>, last_used: i64) -> PendingSubmission {
    let rows = stale.len().saturating_add(1).min(256);
    let bytes = stale.len().saturating_mul(4).saturating_add(64);
    let batch = SqliteBatch::new(rows, bytes, move |transaction| {
        for local_id in stale {
            transaction.execute(
                "DELETE FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3 AND local_id = ?4",
                params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice(), i64::from(local_id)],
            )?;
        }
        let has_entries = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3)",
            params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice()],
            |row| row.get::<_, bool>(0),
        )?;
        if has_entries {
            transaction.execute(
                "UPDATE region_scopes SET last_used = MAX(last_used, ?4) WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3",
                params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice(), last_used],
            )?;
        } else {
            transaction.execute(
                "DELETE FROM region_scopes WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3",
                params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice()],
            )?;
        }
        Ok(())
    })
    .expect("cache access batch fits SQLite limits");
    PendingSubmission { batches: vec![batch], rows, bytes }
}

fn upsert_scope(transaction: &Transaction<'_>, scope: &CacheScope, last_used: i64) -> Result<(), RusqliteError> {
    transaction.execute(
        "INSERT INTO region_scopes (grid_id, region_id, cache_id, last_used) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (grid_id, region_id, cache_id) DO UPDATE SET last_used = MAX(last_used, excluded.last_used)",
        params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice(), last_used],
    )?;
    Ok(())
}

fn prune_scope(transaction: &Transaction<'_>, scope: &CacheScope, max_rows: usize, max_bytes: usize) -> Result<(), RusqliteError> {
    let (rows, bytes) = scope_stats(transaction, scope)?;
    if rows <= max_rows && bytes <= max_bytes {
        return Ok(());
    }
    let mut statement = transaction.prepare(
        "SELECT local_id FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3
         ORDER BY length(payload) DESC, local_id LIMIT 255",
    )?;
    let local_ids = statement
        .query_map(params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice()], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for local_id in local_ids {
        transaction.execute(
            "DELETE FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3 AND local_id = ?4",
            params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice(), local_id],
        )?;
    }
    remove_empty_scope(transaction, scope)?;
    Ok(())
}

fn prune_global_chunk(transaction: &Transaction<'_>) -> Result<(), RusqliteError> {
    let (rows, bytes) = global_stats(transaction)?;
    let rows = i64::try_from(rows).unwrap_or(i64::MAX);
    let bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
    if rows <= MAX_TOTAL_ENTRIES as i64 && bytes <= MAX_TOTAL_BYTES as i64 {
        return Ok(());
    }
    let mut remaining_rows = usize::try_from(rows).unwrap_or(usize::MAX);
    let mut remaining_bytes = usize::try_from(bytes).unwrap_or(usize::MAX);
    let mut deleted = 0;
    while (remaining_rows > MAX_TOTAL_ENTRIES || remaining_bytes > MAX_TOTAL_BYTES) && deleted < MAX_GLOBAL_PRUNE_ROWS {
        let oldest = transaction
            .query_row(
                "SELECT grid_id, region_id, cache_id FROM region_scopes
                 WHERE EXISTS (
                    SELECT 1 FROM object_entries e
                    WHERE e.grid_id = region_scopes.grid_id AND e.region_id = region_scopes.region_id AND e.cache_id = region_scopes.cache_id
                 )
                 ORDER BY last_used, grid_id, region_id, cache_id LIMIT 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, Vec<u8>>(2)?)),
            )
            .optional()?;
        let Some((grid_id, region_id, cache_id)) = oldest else { break };
        let limit = i64::try_from(MAX_GLOBAL_PRUNE_ROWS - deleted).unwrap_or(i64::MAX);
        let mut statement = transaction.prepare(
            "SELECT local_id, length(payload) FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3
             ORDER BY length(payload) DESC, local_id LIMIT ?4",
        )?;
        let local_ids = statement
            .query_map(params![grid_id, region_id, cache_id, limit], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if local_ids.is_empty() {
            break;
        }
        for (local_id, payload_bytes) in &local_ids {
            transaction.execute(
                "DELETE FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3 AND local_id = ?4",
                params![grid_id, region_id, cache_id, local_id],
            )?;
            remaining_rows = remaining_rows.saturating_sub(1);
            remaining_bytes = remaining_bytes.saturating_sub(usize::try_from(*payload_bytes).unwrap_or(usize::MAX));
        }
        deleted += local_ids.len();
        if !transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3)",
            params![grid_id, region_id, cache_id],
            |row| row.get::<_, bool>(0),
        )? {
            transaction.execute(
                "DELETE FROM region_scopes WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3",
                params![grid_id, region_id, cache_id],
            )?;
        }
    }
    Ok(())
}

fn global_stats(transaction: &Transaction<'_>) -> Result<(usize, usize), RusqliteError> {
    let (rows, bytes): (i64, i64) =
        transaction.query_row("SELECT entry_count, payload_bytes FROM object_cache_totals WHERE singleton = 1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    Ok((usize::try_from(rows).unwrap_or(usize::MAX), usize::try_from(bytes).unwrap_or(usize::MAX)))
}

fn delete_scope_chunk(transaction: &Transaction<'_>, scope: &CacheScope) -> Result<(), RusqliteError> {
    let mut statement = transaction.prepare(
        "SELECT local_id FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3 ORDER BY local_id LIMIT 255",
    )?;
    let local_ids = statement
        .query_map(params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice()], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for local_id in local_ids {
        transaction.execute(
            "DELETE FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3 AND local_id = ?4",
            params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice(), local_id],
        )?;
    }
    remove_empty_scope(transaction, scope)
}

fn remove_empty_scope(transaction: &Transaction<'_>, scope: &CacheScope) -> Result<(), RusqliteError> {
    transaction.execute(
        "DELETE FROM region_scopes WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3
         AND NOT EXISTS (
            SELECT 1 FROM object_entries e
            WHERE e.grid_id = region_scopes.grid_id AND e.region_id = region_scopes.region_id AND e.cache_id = region_scopes.cache_id
         )",
        params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice()],
    )?;
    Ok(())
}

fn scope_stats(transaction: &Transaction<'_>, scope: &CacheScope) -> Result<(usize, usize), RusqliteError> {
    let (rows, bytes): (i64, i64) = transaction.query_row(
        "SELECT COUNT(*), COALESCE(SUM(length(payload)), 0) FROM object_entries
         WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3",
        params![scope.grid_id, scope.region_id.as_bytes().as_slice(), scope.cache_id.as_bytes().as_slice()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok((usize::try_from(rows).unwrap_or(usize::MAX), usize::try_from(bytes).unwrap_or(usize::MAX)))
}

fn stored_entry(entry: &EncodedEntry) -> StoredEntry {
    StoredEntry {
        local_id: i64::from(entry.local_id),
        crc: i64::from(entry.crc),
        update_kind: entry.update_kind,
        update_flags: i64::from(entry.update_flags),
        payload_bytes: i64::try_from(entry.payload.len()).unwrap_or(i64::MAX),
        payload: Some(entry.payload.clone()),
    }
}

fn decode_entry(entry: &StoredEntry, reference: CachedObjectReference, local_id: u32) -> Option<Message> {
    let payload_bytes = usize::try_from(entry.payload_bytes).ok()?;
    if entry.local_id != i64::from(local_id)
        || u32::try_from(entry.crc).ok()? != reference.crc
        || u32::try_from(entry.update_flags).is_err()
        || !(MIN_OBJECT_CACHE_ENTRY_BYTES..=MAX_OBJECT_CACHE_ENTRY_BYTES).contains(&payload_bytes)
    {
        return None;
    }
    let payload = entry.payload.as_ref()?;
    if payload.len() != payload_bytes {
        return None;
    }
    let packet = Packet::decode(payload).ok()?;
    if !packet.extra_header().is_empty() || !packet.trailing_message_bytes().is_empty() || !packet.appended_acks().is_empty() {
        return None;
    }
    let message = packet.into_message();
    let (kind, object_local_id, crc, update_flags) = match &message {
        Message::ObjectUpdate(update) if update.objects.len() == 1 => {
            let object = &update.objects[0];
            (0, object.local_id, object.crc, object.update_flags)
        }
        Message::ObjectUpdateCompressed(update) if update.objects.len() == 1 => {
            let object = &update.objects[0];
            (1, object.local_id, object.crc, object.update_flags)
        }
        _ => return None,
    };
    (entry.update_kind == kind && object_local_id == local_id && crc == reference.crc && i64::from(update_flags) == entry.update_flags)
        .then_some(message)
}

fn set_region_data(message: &mut Message, region_data: RegionData) {
    match message {
        Message::ObjectUpdate(update) => update.region_data = region_data,
        Message::ObjectUpdateCompressed(update) => update.region_data = region_data,
        _ => {}
    }
}

fn now_millis() -> i64 {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

#[cfg(test)]
#[path = "object_cache_tests.rs"]
mod tests;
