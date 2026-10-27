use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
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
use rusqlite::{Connection, Error as RusqliteError, OptionalExtension, Transaction, params, params_from_iter, types::Value as SqlValue};
use storage::{
    default_cache_dir,
    sqlite::{MigrationError, OpenStatus, SqliteBatch, SqliteDb, SqliteError},
};
use tokio::{runtime::Handle, sync::mpsc};
use uuid::Uuid;
use world::{OrderEvidence, Region};

use crate::session::inbound::{ObjectBatch, ObjectOperation, decode_cached_object};

const SCHEMA_VERSION: i64 = 1;
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
const MAX_CACHE_READ_ENTRIES: usize = 255;
const MAX_CACHE_READ_BYTES: usize = MAX_OBJECT_CACHE_ENTRY_BYTES * MAX_CACHE_READ_ENTRIES;
const MAX_PENDING_SUBMISSIONS: usize = 16;
const MAX_PENDING_ROWS: usize = 65_536;
const MAX_PENDING_BYTES: usize = 16 * 1024 * 1024;
const MAX_SCOPE_METADATA_BYTES: usize = 8 * 1024 * 1024;
const MAX_CACHED_SCOPES: usize = MAX_TOTAL_ENTRIES;
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
struct StoredEntry {
    local_id: i64,
    crc: i64,
    update_kind: i64,
    update_flags: i64,
    payload_bytes: i64,
    payload: Option<Vec<u8>>,
}

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
    populated_scope: Option<CacheScope>,
}

pub(super) enum ObjectCacheEvent {
    Ready { db: SqliteDb, status: OpenStatus, populated_scopes: HashSet<CacheScope> },
    OpenFailed(String),
}

pub(super) struct ObjectCacheSession {
    grid_id: String,
    identities: HashMap<SimulatorId, CacheScope>,
    populated_scopes: HashSet<CacheScope>,
    db: Option<SqliteDb>,
    event_sender: mpsc::UnboundedSender<ObjectCacheEvent>,
    event_receiver: mpsc::UnboundedReceiver<ObjectCacheEvent>,
    pending: Vec<PendingSubmission>,
    pending_rows: usize,
    pending_bytes: usize,
    started: bool,
    disabled: bool,
    warned: bool,
}

impl ObjectCacheSession {
    pub(super) fn new(grid: Grid) -> Self {
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        Self {
            grid_id: grid.id().to_owned(),
            identities: HashMap::new(),
            populated_scopes: HashSet::new(),
            db: None,
            event_sender,
            event_receiver,
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
        let grid_id = self.grid_id.clone();
        drop(runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || open_default(&grid_id)).await;
            let event = match result {
                Ok(Ok((db, status, populated_scopes))) => ObjectCacheEvent::Ready { db, status, populated_scopes },
                Ok(Err(error)) => ObjectCacheEvent::OpenFailed(error),
                Err(error) => ObjectCacheEvent::OpenFailed(error.to_string()),
            };
            if let Err(send_error) = sender.send(event) {
                drop(send_error);
            }
        }));
    }

    pub(super) async fn next_event(&mut self) -> Option<ObjectCacheEvent> {
        self.event_receiver.recv().await
    }

    pub(super) fn handle_event(&mut self, event: ObjectCacheEvent) {
        match event {
            ObjectCacheEvent::Ready { db, status, populated_scopes } => {
                if matches!(status, OpenStatus::Recreated(_)) {
                    tracing::warn!(?status, "object cache database was recreated; cached objects will be refetched");
                }
                self.db = Some(db);
                self.populated_scopes = populated_scopes;
                let pending = std::mem::take(&mut self.pending);
                self.pending_rows = 0;
                self.pending_bytes = 0;
                for submission in pending {
                    let populated_scope = submission.populated_scope.clone();
                    if self.submit_to_db(submission)
                        && let Some(scope) = populated_scope
                    {
                        self.populated_scopes.insert(scope);
                    }
                }
            }
            ObjectCacheEvent::OpenFailed(error) => {
                self.warn(&error);
                self.disabled = true;
                self.pending.clear();
                self.pending_rows = 0;
                self.pending_bytes = 0;
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
        if let Some(previous) = previous {
            self.populated_scopes.remove(&previous);
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
    }

    pub(super) fn region_handshake_flags(&self, simulator_id: SimulatorId) -> u32 {
        let has_cache = self.db.is_some()
            && !self.disabled
            && self.identities.get(&simulator_id).is_some_and(|scope| self.populated_scopes.contains(scope));
        CACHE_SUPPORT_FLAG | SELF_APPEARANCE_FLAG | if has_cache { 0 } else { EMPTY_CACHE_FLAG }
    }

    pub(super) fn record_update(&mut self, simulator_id: SimulatorId, message: &Message) {
        if self.disabled {
            return;
        }
        let Some(scope) = self.identities.get(&simulator_id).cloned() else { return };
        let entries = encode_entries(message);
        if entries.is_empty() {
            return;
        }
        let submission = make_write_submission(scope.clone(), entries, now_millis());
        if self.enqueue(submission) && self.db.is_some() {
            self.populated_scopes.insert(scope);
        }
    }

    pub(super) async fn resolve_cached(
        &mut self,
        simulator_id: SimulatorId,
        region: Region,
        sequence: u32,
        order: OrderEvidence,
        update: &ObjectUpdateCached,
        mut batch: ObjectBatch,
    ) -> ObjectBatch {
        let scope = self.identities.get(&simulator_id).cloned();
        let local_ids = batch
            .blocks
            .iter()
            .filter_map(|operation| match operation {
                Some(ObjectOperation::Cached { local_id, .. }) => Some(local_id.get()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let entries = match (self.db.as_ref(), scope.as_ref(), local_ids.is_empty()) {
            (Some(db), Some(scope), false) => match load_entries(db, scope.clone(), local_ids).await {
                Ok(entries) => entries,
                Err(error) => {
                    self.warn(&error.to_string());
                    HashMap::new()
                }
            },
            _ => HashMap::new(),
        };
        let mut stale = HashSet::new();
        let mut invalid_fields = 0;
        for (index, operation) in batch.blocks.iter_mut().enumerate() {
            let Some(ObjectOperation::Cached { source, local_id, .. }) = operation else { continue };
            let source = *source;
            let local_id = *local_id;
            let reference = update.objects.get(index).copied();
            let entry = entries.get(&local_id.get());
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
                decode_cached_object(region, sequence, order, ordinal, reference, &payload, &mut invalid_fields)
            });
            *operation = match recovered {
                Some(operation) => Some(operation),
                None => Some(ObjectOperation::Cached { source, local_id, cache_miss_type }),
            };
        }
        batch.invalid_fields = batch.invalid_fields.saturating_add(invalid_fields);
        if let Some(scope) = scope {
            self.enqueue(make_access_submission(scope, stale, now_millis()));
        }
        batch
    }

    pub(super) async fn close(mut self) {
        while !self.pending.is_empty() && self.db.is_none() && !self.disabled {
            let Some(event) = self.event_receiver.recv().await else { break };
            self.handle_event(event);
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
}

fn open_default(grid_id: &str) -> Result<(SqliteDb, OpenStatus, HashSet<CacheScope>), String> {
    let root = default_cache_dir().map_err(|error| error.to_string())?;
    let (db, status) = open_at(root.join("objects.sqlite")).map_err(|error| error.to_string())?;
    let populated_scopes = match load_populated_scopes(&db, grid_id) {
        Ok(scopes) => scopes,
        Err(error) => {
            tracing::warn!(%error, "could not load object cache scope metadata; regions will be treated as empty until updated");
            HashSet::new()
        }
    };
    Ok((db, status, populated_scopes))
}

fn open_at(path: impl AsRef<Path>) -> Result<(SqliteDb, OpenStatus), SqliteError> {
    SqliteDb::open(path, migrate)
}

fn load_populated_scopes(db: &SqliteDb, grid_id: &str) -> Result<HashSet<CacheScope>, SqliteError> {
    let grid_id = grid_id.to_owned();
    let receipt = db.read(MAX_SCOPE_METADATA_BYTES, move |transaction| {
        let mut statement = transaction.prepare(
            "SELECT s.grid_id, s.region_id, s.cache_id FROM region_scopes s
             WHERE s.grid_id = ?1 AND EXISTS (
                SELECT 1 FROM object_entries e
                WHERE e.grid_id = s.grid_id AND e.region_id = s.region_id AND e.cache_id = s.cache_id
             )
             ORDER BY s.last_used DESC, s.region_id, s.cache_id LIMIT ?2",
        )?;
        let rows = statement.query_map(params![grid_id, MAX_CACHED_SCOPES as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, Vec<u8>>(2)?))
        })?;
        let mut scopes = HashSet::new();
        for row in rows {
            let (grid_id, region_id, cache_id) = row?;
            let (Ok(region_id), Ok(cache_id)) = (Uuid::from_slice(&region_id), Uuid::from_slice(&cache_id)) else { continue };
            scopes.insert(CacheScope { grid_id, region_id, cache_id });
        }
        Ok(scopes)
    })?;
    futures::executor::block_on(receipt.wait())
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
            connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        SCHEMA_VERSION => {}
        _ => return Err(MigrationError::SchemaMismatch),
    }
    connection.pragma_update(None, "max_page_count", MAX_DATABASE_PAGES)?;
    Ok(())
}

#[cfg(test)]
pub(super) fn open_for_test(path: impl AsRef<Path>, grid: Grid) -> ObjectCacheSession {
    let (db, status) = open_at(path).expect("open isolated object cache database");
    let populated_scopes = load_populated_scopes(&db, grid.id()).expect("load isolated object cache metadata");
    let mut cache = ObjectCacheSession::new(grid);
    cache.handle_event(ObjectCacheEvent::Ready { db, status, populated_scopes });
    cache
}

async fn load_entries(db: &SqliteDb, scope: CacheScope, local_ids: Vec<u32>) -> Result<HashMap<u32, StoredEntry>, SqliteError> {
    let mut unique_ids = local_ids;
    unique_ids.sort_unstable();
    unique_ids.dedup();
    unique_ids.truncate(MAX_CACHE_READ_ENTRIES);
    let weight = unique_ids.len().saturating_mul(MAX_OBJECT_CACHE_ENTRY_BYTES);
    let receipt = db.read(weight.min(MAX_CACHE_READ_BYTES), move |transaction| {
        let placeholders = (0..unique_ids.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT local_id, crc, update_kind, update_flags, length(payload),
                    CASE WHEN length(payload) <= ? THEN payload ELSE NULL END
             FROM object_entries
             WHERE grid_id = ? AND region_id = ? AND cache_id = ? AND local_id IN ({placeholders})"
        );
        let mut values = Vec::with_capacity(unique_ids.len() + 4);
        values.push(SqlValue::Integer(MAX_OBJECT_CACHE_ENTRY_BYTES as i64));
        values.push(SqlValue::Text(scope.grid_id));
        values.push(SqlValue::Blob(scope.region_id.as_bytes().to_vec()));
        values.push(SqlValue::Blob(scope.cache_id.as_bytes().to_vec()));
        values.extend(unique_ids.into_iter().map(|local_id| SqlValue::Integer(i64::from(local_id))));
        let mut statement = transaction.prepare(&query)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok(StoredEntry {
                local_id: row.get(0)?,
                crc: row.get(1)?,
                update_kind: row.get(2)?,
                update_flags: row.get(3)?,
                payload_bytes: row.get(4)?,
                payload: row.get(5)?,
            })
        })?;
        let mut entries = HashMap::new();
        for row in rows {
            let entry = row?;
            if let Ok(local_id) = u32::try_from(entry.local_id) {
                entries.insert(local_id, entry);
            }
        }
        Ok(entries)
    })?;
    receipt.wait().await
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
    let scope_for_region_prune = scope.clone();
    let region_prune = SqliteBatch::new(256, 256, move |transaction| {
        prune_scope(transaction, &scope_for_region_prune, MAX_REGION_ENTRIES, MAX_REGION_BYTES)
    })
    .expect("region prune batch fits SQLite limits");
    let global_prune_first = SqliteBatch::new(256, 256, prune_global_chunk).expect("global prune batch fits SQLite limits");
    let global_prune_second = SqliteBatch::new(256, 256, prune_global_chunk).expect("global prune batch fits SQLite limits");
    PendingSubmission {
        batches: vec![scope_batch, entries_batch, region_prune, global_prune_first, global_prune_second],
        rows: submission_rows,
        bytes: submission_bytes,
        populated_scope: Some(scope),
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
    PendingSubmission { batches, rows: MAX_REGION_DELETE_BATCHES * 256, bytes: MAX_REGION_DELETE_BATCHES * 256, populated_scope: None }
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
    PendingSubmission { batches: vec![batch], rows, bytes, populated_scope: None }
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
    let (rows, bytes): (i64, i64) =
        transaction.query_row("SELECT COUNT(*), COALESCE(SUM(length(payload)), 0) FROM object_entries", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
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
