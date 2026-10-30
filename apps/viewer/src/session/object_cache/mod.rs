mod resolution;
mod storage;
mod validation;
mod worker;

#[cfg(test)]
use std::path::Path;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ::storage::sqlite::MAX_BATCH_BYTES;
use ::storage::sqlite::{OpenStatus, SqliteDb};
use network::{Grid, SimulatorId};
use protocol::{lludp::Message, messages::RegionHandshake};
use tokio::{
    runtime::Handle,
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
    time::timeout,
};
use uuid::Uuid;

#[cfg(test)]
use self::storage::open_for_test as open_for_test_storage;
use self::storage::{PendingSubmission, make_access_submission, open_default, queue_scope_load};
use self::worker::{CacheWriteJob, CacheWriteOperation, CacheWriteWorker};
const SCHEMA_VERSION: i64 = 2;
const SQLITE_PAGE_SIZE: i64 = 4096;
const MAX_DATABASE_BYTES: usize = 256 * 1024 * 1024;
const MAX_DATABASE_PAGES: i64 = (MAX_DATABASE_BYTES as i64) / SQLITE_PAGE_SIZE;
const MAX_OBJECT_CACHE_ENTRY_BYTES: usize = 10_000;
const MIN_OBJECT_CACHE_ENTRY_BYTES: usize = 64;
const MAX_ENTRY_BATCH_ROWS: usize = MAX_BATCH_BYTES / (MAX_OBJECT_CACHE_ENTRY_BYTES + 64);
const MAX_REGION_ENTRIES: usize = 10_000;
const MAX_REGION_BYTES: usize = 32 * 1024 * 1024;
const MAX_TOTAL_ENTRIES: usize = 50_000;
const MAX_TOTAL_BYTES: usize = 192 * 1024 * 1024;
const MAX_GLOBAL_PRUNE_ROWS: usize = 255;
const MAX_REGION_DELETE_BATCHES: usize = MAX_REGION_ENTRIES.div_ceil(255);
const MAX_PENDING_SUBMISSIONS: usize = 10;
const MAX_PENDING_INVALIDATION_SCOPES: usize = 65;
const MAX_PENDING_CACHE_WRITES: usize = MAX_PENDING_SUBMISSIONS + MAX_PENDING_INVALIDATION_SCOPES;
const MAX_PENDING_INVALIDATION_IDS: usize = MAX_REGION_ENTRIES;
const MAX_PENDING_ROWS: usize = 65_536;
const MAX_PENDING_BYTES: usize = 32 * 1024 * 1024;
pub(super) const HANDSHAKE_CACHE_LOAD_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_PENDING_REGION_LOADS: usize = 2;
const CACHE_EVENT_CAPACITY: usize = MAX_PENDING_SUBMISSIONS + MAX_PENDING_REGION_LOADS + 1;
const CACHE_SUPPORT_FLAG: u32 = 1;
const EMPTY_CACHE_FLAG: u32 = 2;
const SELF_APPEARANCE_FLAG: u32 = 4;
pub(super) type CacheEntryVersions = Arc<[(u32, u64)]>;
type InvalidatedEntryVersions = Arc<[(SimulatorId, u32, u64)]>;

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
    payload: Option<Arc<[u8]>>,
}

struct LoadedRegion {
    scope: CacheScope,
    entries: HashMap<u32, StoredEntry>,
    bytes: usize,
    last_used: u64,
    complete: bool,
}

#[derive(Default)]
struct InFlightRegionLoad {
    invalidated_local_ids: HashSet<u32>,
    stale: bool,
}

#[derive(Clone)]
pub(super) struct ScopeInvalidation {
    simulator_ids: Arc<[SimulatorId]>,
    generation: u64,
}

#[derive(Clone)]
pub(super) struct EncodedEntry {
    local_id: u32,
    crc: u32,
    update_kind: i64,
    update_flags: u32,
    payload: Arc<[u8]>,
}

pub(super) enum ObjectCacheEvent {
    Ready { db: Arc<SqliteDb>, status: OpenStatus, writer: CacheWriteWorker },
    OpenFailed(String),
    RegionLoaded { simulator_id: SimulatorId, scope: CacheScope, result: RegionLoadResult, _permit: OwnedSemaphorePermit },
    WriteApplied { simulator_id: SimulatorId, scope: CacheScope, versions: CacheEntryVersions, entries: Arc<[EncodedEntry]> },
    WriteDropped { simulator_id: SimulatorId, versions: CacheEntryVersions },
    InvalidationApplied,
    EntriesInvalidated { versions: InvalidatedEntryVersions },
    ScopeInvalidated { scope: CacheScope, invalidation: ScopeInvalidation },
    InvalidationDropped,
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
    in_flight_loads: HashMap<(SimulatorId, CacheScope), InFlightRegionLoad>,
    loaded_entries: usize,
    loaded_bytes: usize,
    use_clock: u64,
    cache_generation_clock: u64,
    cache_generations: HashMap<(SimulatorId, u32), u64>,
    db: Option<Arc<SqliteDb>>,
    write_worker: Option<CacheWriteWorker>,
    event_sender: mpsc::Sender<ObjectCacheEvent>,
    event_receiver: mpsc::Receiver<ObjectCacheEvent>,
    load_slots: Arc<Semaphore>,
    pending: Vec<PendingSubmission>,
    pending_rows: usize,
    pending_bytes: usize,
    pending_writes: VecDeque<CacheWriteJob>,
    pending_write_bytes: usize,
    outstanding_writes: usize,
    started: bool,
    disabled: bool,
    warned: bool,
    dropped_writes: u64,
}

impl ObjectCacheSession {
    pub(super) fn new(grid: Grid) -> Self {
        let (event_sender, event_receiver) = mpsc::channel(CACHE_EVENT_CAPACITY);
        Self {
            grid_id: grid.id().to_owned(),
            identities: HashMap::new(),
            loaded_regions: HashMap::new(),
            in_flight_loads: HashMap::new(),
            loaded_entries: 0,
            loaded_bytes: 0,
            use_clock: 0,
            cache_generation_clock: 0,
            cache_generations: HashMap::new(),
            db: None,
            write_worker: None,
            event_sender,
            event_receiver,
            load_slots: Arc::new(Semaphore::new(MAX_PENDING_REGION_LOADS)),
            pending: Vec::new(),
            pending_rows: 0,
            pending_bytes: 0,
            pending_writes: VecDeque::new(),
            pending_write_bytes: 0,
            outstanding_writes: 0,
            started: false,
            disabled: false,
            warned: false,
            dropped_writes: 0,
        }
    }

    pub(super) fn start(&mut self, runtime: &Handle) {
        if self.started || self.disabled {
            return;
        }
        self.started = true;
        let sender = self.event_sender.clone();
        let task_runtime = runtime.clone();
        drop(runtime.spawn(async move {
            let open_runtime = task_runtime.clone();
            let result = tokio::task::spawn_blocking(move || {
                let (db, status) = open_default()?;
                let db = Arc::new(db);
                let writer = CacheWriteWorker::start(Arc::clone(&db), open_runtime).map_err(|error| error.to_string())?;
                Ok::<_, String>((db, status, writer))
            })
            .await;
            let event = match result {
                Ok(Ok((db, status, writer))) => ObjectCacheEvent::Ready { db, status, writer },
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
            ObjectCacheEvent::Ready { db, status, writer } => {
                if matches!(status, OpenStatus::Recreated(_)) {
                    tracing::warn!(?status, "object cache database was recreated; cached objects will be refetched");
                }
                self.db = Some(db);
                self.write_worker = Some(writer);
                let pending = std::mem::take(&mut self.pending);
                self.pending_rows = 0;
                self.pending_bytes = 0;
                for submission in pending {
                    self.submit_to_db(submission);
                }
                self.flush_pending_writes();
                None
            }
            ObjectCacheEvent::OpenFailed(error) => {
                self.warn(&error);
                self.disabled = true;
                self.pending.clear();
                self.pending_rows = 0;
                self.pending_bytes = 0;
                self.dropped_writes = self.dropped_writes.saturating_add(
                    u64::try_from(
                        self.pending_writes.iter().filter(|job| matches!(&job.operation, CacheWriteOperation::Update { .. })).count(),
                    )
                    .unwrap_or(u64::MAX),
                );
                self.pending_writes.clear();
                self.pending_write_bytes = 0;
                None
            }
            ObjectCacheEvent::RegionLoaded { simulator_id, scope, result, _permit } => {
                let in_flight = self.in_flight_loads.remove(&(simulator_id, scope.clone()));
                match result {
                    RegionLoadResult::Loaded(mut entries) => {
                        if self.identities.get(&simulator_id) == Some(&scope) {
                            if let Some(in_flight) = in_flight {
                                if in_flight.stale {
                                    entries.clear();
                                } else {
                                    entries.retain(|local_id, _| !in_flight.invalidated_local_ids.contains(local_id));
                                }
                            }
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
                drop(_permit);
                self.flush_pending_writes();
                Some((simulator_id, flags))
            }
            ObjectCacheEvent::WriteApplied { simulator_id, scope, versions, entries } => {
                self.outstanding_writes = self.outstanding_writes.saturating_sub(1);
                let latest_local_ids = versions
                    .iter()
                    .filter_map(|(local_id, generation)| {
                        (self.cache_generations.get(&(simulator_id, *local_id)) == Some(generation)).then_some(*local_id)
                    })
                    .collect::<HashSet<_>>();
                if !self.disabled && self.identities.get(&simulator_id) == Some(&scope) {
                    let latest_entries =
                        entries.iter().filter(|entry| latest_local_ids.contains(&entry.local_id)).cloned().collect::<Vec<_>>();
                    self.remember_entries(simulator_id, scope, &latest_entries);
                }
                self.clear_cache_versions(simulator_id, &versions);
                self.flush_pending_writes();
                None
            }
            ObjectCacheEvent::WriteDropped { simulator_id, versions } => {
                self.outstanding_writes = self.outstanding_writes.saturating_sub(1);
                self.clear_cache_versions(simulator_id, &versions);
                self.count_dropped_write();
                self.flush_pending_writes();
                None
            }
            ObjectCacheEvent::InvalidationApplied => {
                self.outstanding_writes = self.outstanding_writes.saturating_sub(1);
                self.flush_pending_writes();
                None
            }
            ObjectCacheEvent::EntriesInvalidated { versions } => {
                self.outstanding_writes = self.outstanding_writes.saturating_sub(1);
                self.clear_invalidated_versions(&versions);
                self.flush_pending_writes();
                None
            }
            ObjectCacheEvent::ScopeInvalidated { scope, invalidation } => {
                self.outstanding_writes = self.outstanding_writes.saturating_sub(1);
                self.invalidate_loaded_scope(&scope, &invalidation);
                self.flush_pending_writes();
                None
            }
            ObjectCacheEvent::InvalidationDropped => {
                self.outstanding_writes = self.outstanding_writes.saturating_sub(1);
                self.disable_after_invalidation_failure();
                self.flush_pending_writes();
                None
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
        self.cache_generations.retain(|(id, _), _| *id != simulator_id);
        self.remove_loaded_region(simulator_id);
        if let Some(previous) = previous {
            self.queue_invalidation(CacheWriteJob {
                operation: CacheWriteOperation::DeleteScope { scope: previous, invalidation: None },
                permit: None,
            });
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
        self.cache_generations.retain(|(id, _), _| *id != simulator_id);
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

    pub(super) fn record_update(&mut self, simulator_id: SimulatorId, message: Message) {
        let Some(scope) = self.identities.get(&simulator_id).cloned() else { return };
        if self.disabled {
            return;
        }
        if let Message::ImprovedTerseObjectUpdate(update) = &message {
            let mut local_ids = update
                .objects
                .iter()
                .filter_map(|block| block.as_decoded().map(|object| object.local_id))
                .filter(|local_id| *local_id != 0)
                .collect::<Vec<_>>();
            local_ids.sort_unstable();
            local_ids.dedup();
            if local_ids.is_empty() {
                return;
            }
            let generation = self.next_cache_generation();
            let versions: CacheEntryVersions = local_ids.iter().map(|local_id| (*local_id, generation)).collect::<Vec<_>>().into();
            let local_id_set = local_ids.iter().copied().collect::<HashSet<_>>();
            if let Some(load) = self.in_flight_loads.get_mut(&(simulator_id, scope.clone()))
                && !load.stale
            {
                let additional = local_id_set.iter().filter(|local_id| !load.invalidated_local_ids.contains(local_id)).count();
                if load.invalidated_local_ids.len().saturating_add(additional) > MAX_REGION_ENTRIES {
                    load.invalidated_local_ids.clear();
                    load.stale = true;
                } else {
                    load.invalidated_local_ids.extend(local_id_set.iter().copied());
                }
            }
            self.remove_loaded_entries(simulator_id, &local_id_set);
            let queued = self.queue_invalidation(CacheWriteJob {
                operation: CacheWriteOperation::DeleteEntries {
                    scope,
                    entries: versions.iter().map(|(local_id, generation)| ((simulator_id, *local_id), *generation)).collect(),
                },
                permit: None,
            });
            if queued {
                self.remember_cache_versions(simulator_id, &versions);
            }
            return;
        }
        let row_count = match &message {
            Message::ObjectUpdate(update) => update.objects.len(),
            Message::ObjectUpdateCompressed(update) => update.objects.len(),
            _ => 0,
        };
        if row_count == 0 {
            return;
        }
        let permit = match self.event_sender.clone().try_reserve_owned() {
            Ok(permit) => permit,
            Err(_) => {
                self.count_dropped_write();
                return;
            }
        };
        let mut local_ids = match &message {
            Message::ObjectUpdate(update) => update.objects.iter().map(|object| object.local_id).collect::<Vec<_>>(),
            Message::ObjectUpdateCompressed(update) => update.objects.iter().map(|object| object.local_id).collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        local_ids.retain(|local_id| *local_id != 0);
        local_ids.sort_unstable();
        local_ids.dedup();
        let generation = self.next_cache_generation();
        let versions: CacheEntryVersions = local_ids.iter().map(|local_id| (*local_id, generation)).collect::<Vec<_>>().into();
        let job = CacheWriteJob {
            operation: CacheWriteOperation::Update { simulator_id, scope, message, versions: versions.clone(), last_used: now_millis() },
            permit: Some(permit),
        };
        if self.queue_cache_write(job) {
            self.remember_cache_versions(simulator_id, &versions);
        }
    }

    pub(super) fn begin_region_load(&mut self, simulator_id: SimulatorId, runtime: &Handle) -> Option<u32> {
        let Some(scope) = self.identities.get(&simulator_id).cloned() else { return Some(self.region_handshake_flags(simulator_id)) };
        if !self.disabled && self.db.is_some() {
            self.enqueue(make_access_submission(scope.clone(), HashSet::new(), now_millis()));
        }
        if self.loaded_regions.get(&simulator_id).is_some_and(|region| region.scope == scope && region.complete) {
            return Some(self.region_handshake_flags(simulator_id));
        }
        if self.in_flight_loads.contains_key(&(simulator_id, scope.clone())) {
            return None;
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
        self.in_flight_loads.insert((simulator_id, scope.clone()), InFlightRegionLoad::default());
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

    pub(super) async fn close(mut self) {
        while (!self.pending.is_empty() || !self.pending_writes.is_empty()) && self.db.is_none() && !self.disabled {
            let Some(event) = self.event_receiver.recv().await else { break };
            let _ = self.handle_event(event);
        }
        let Some(db) = self.db.take() else { return };
        self.flush_pending_writes();
        while !self.pending_writes.is_empty() {
            if !self.receive_worker_event().await {
                self.abandon_writer_work();
                break;
            }
        }
        if let Some(writer) = self.write_worker.take() {
            let shutdown = tokio::task::spawn_blocking(move || writer.shutdown()).await;
            match shutdown {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    self.warn(&error);
                    self.abandon_writer_work();
                }
                Err(error) => {
                    self.warn(&error.to_string());
                    self.abandon_writer_work();
                }
            }
        }
        while self.outstanding_writes > 0 {
            let Some(event) = self.event_receiver.recv().await else { break };
            let _ = self.handle_event(event);
        }
        if db.had_write_failure() {
            self.warn("SQLite worker reported a failed object-cache write");
        }
        let shutdown = tokio::task::spawn_blocking(move || {
            Arc::try_unwrap(db)
                .map_err(|_| "object cache writer still owns the SQLite worker".to_owned())?
                .shutdown()
                .map_err(|error| error.to_string())
        })
        .await;
        match shutdown {
            Ok(Ok(())) => {}
            Ok(Err(error)) => self.warn(&error),
            Err(error) => self.warn(&error.to_string()),
        }
        if self.dropped_writes > 0 {
            tracing::warn!(dropped = self.dropped_writes, "object cache writes were dropped under bounded queue pressure");
        }
    }

    async fn receive_worker_event(&mut self) -> bool {
        let event = {
            let Self { event_receiver, write_worker, .. } = self;
            let Some(writer) = write_worker.as_mut() else { return false };
            tokio::select! {
                event = event_receiver.recv() => event,
                _ = writer.stopped() => None,
            }
        };
        if let Some(event) = event {
            let _ = self.handle_event(event);
            true
        } else {
            false
        }
    }

    fn abandon_writer_work(&mut self) {
        let dropped_updates = self.pending_writes.iter().filter(|job| matches!(&job.operation, CacheWriteOperation::Update { .. })).count();
        self.dropped_writes = self.dropped_writes.saturating_add(u64::try_from(dropped_updates).unwrap_or(u64::MAX));
        self.pending_writes.clear();
        self.pending_write_bytes = 0;
        self.outstanding_writes = 0;
        self.disable_after_invalidation_failure();
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

    fn count_dropped_write(&mut self) {
        self.dropped_writes = self.dropped_writes.saturating_add(1);
    }

    fn next_cache_generation(&mut self) -> u64 {
        self.cache_generation_clock = self.cache_generation_clock.saturating_add(1);
        self.cache_generation_clock
    }

    fn remember_cache_versions(&mut self, simulator_id: SimulatorId, versions: &CacheEntryVersions) {
        let additional = versions.iter().filter(|(local_id, _)| !self.cache_generations.contains_key(&(simulator_id, *local_id))).count();
        if self.cache_generations.len().saturating_add(additional) > MAX_TOTAL_ENTRIES {
            self.cache_generations.clear();
        }
        for (local_id, generation) in versions.iter() {
            self.cache_generations.insert((simulator_id, *local_id), *generation);
        }
    }

    fn clear_cache_versions(&mut self, simulator_id: SimulatorId, versions: &CacheEntryVersions) {
        for (local_id, generation) in versions.iter() {
            if self.cache_generations.get(&(simulator_id, *local_id)) == Some(generation) {
                self.cache_generations.remove(&(simulator_id, *local_id));
            }
        }
    }

    fn clear_invalidated_versions(&mut self, versions: &InvalidatedEntryVersions) {
        for (simulator_id, local_id, generation) in versions.iter() {
            if self.cache_generations.get(&(*simulator_id, *local_id)) == Some(generation) {
                self.cache_generations.remove(&(*simulator_id, *local_id));
            }
        }
    }

    fn disable_after_invalidation_failure(&mut self) {
        self.warn("object cache invalidation failed; cached objects will be refetched");
        self.disabled = true;
        self.cache_generations.clear();
        let simulators: Vec<_> = self.loaded_regions.keys().copied().collect();
        for simulator_id in simulators {
            self.remove_loaded_region(simulator_id);
        }
    }
}

fn now_millis() -> i64 {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

#[cfg(test)]
pub(super) fn open_for_test(path: impl AsRef<Path>, grid: Grid) -> ObjectCacheSession {
    open_for_test_storage(path, grid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::object_cache::test_support::*;
    #[tokio::test]
    async fn unavailable_cache_still_advertises_support_and_empty_state() {
        let mut cache = ObjectCacheSession::new(Grid::Agni);
        assert!(cache.handle_event(ObjectCacheEvent::OpenFailed("test failure".into())).is_none());
        cache.set_region_identity(SIMULATOR, &region_handshake(CACHE_ID));
        cache.record_update(SIMULATOR, fixture_message(vectors::OBJECT_UPDATE));
        assert_eq!(cache.region_handshake_flags(SIMULATOR), 7);
        assert_eq!(cache.begin_region_load(SIMULATOR, &Handle::current()), Some(7));
        cache.close().await;
    }

    #[tokio::test]
    async fn timed_out_region_load_returns_empty_handshake_flags() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let mut cache = open_cache(directory.path().join("objects.sqlite"));
        let scope = cache.identities.get(&SIMULATOR).expect("simulator has a cache scope").clone();
        let permit = cache.load_slots.clone().try_acquire_owned().expect("region load has an admission permit");
        let reply = cache.handle_event(ObjectCacheEvent::RegionLoaded {
            simulator_id: SIMULATOR,
            scope,
            result: RegionLoadResult::TimedOut,
            _permit: permit,
        });
        assert_eq!(reply, Some((SIMULATOR, 7)));
        cache.close().await;
    }

    #[tokio::test]
    async fn terse_update_during_region_load_stays_missing() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let mut cache = open_cache(directory.path().join("objects.sqlite"));
        let mut full = fixture_message(vectors::OBJECT_UPDATE);
        let Message::ObjectUpdate(update) = &mut full else { panic!("object fixture should be a full update") };
        update.objects.truncate(1);
        let local_id = update.objects[0].local_id;
        let (region_data, references) = cached_references(&full);
        cache.record_update(SIMULATOR, full);
        flush(&mut cache).await;
        cache.remove_loaded_region(SIMULATOR);

        assert_eq!(cache.begin_region_load(SIMULATOR, &Handle::current()), None);
        let event = cache.next_event().await.expect("scope load returns its snapshot");
        let ObjectCacheEvent::RegionLoaded { scope, .. } = &event else { panic!("scope load returns RegionLoaded") };
        assert_eq!(cache.identities.get(&SIMULATOR), Some(scope));

        let mut terse = fixture_message(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
        let Message::ImprovedTerseObjectUpdate(update) = &mut terse else { panic!("terse fixture should be a terse update") };
        update.region_data.region_handle = region_data.region_handle;
        update.objects[0].as_decoded_mut().expect("terse fixture object is valid").local_id = local_id;
        cache.record_update(SIMULATOR, terse);
        assert_eq!(cache.handle_event(event), Some((SIMULATOR, 7)));
        flush(&mut cache).await;

        assert_eq!(cache_count(cache.db.as_ref().expect("cache database is open")).await, 0);
        let resolved = resolve(&mut cache, &cached_message(region_data, references), 80).await;
        std::assert_matches!(
            resolved.blocks[0],
            Some(crate::session::inbound::ObjectOperation::Cached { cache_miss_type: protocol::messages::CacheMissType::Full, .. })
        );
        cache.close().await;
    }
}

#[cfg(test)]
pub(super) mod test_support;
