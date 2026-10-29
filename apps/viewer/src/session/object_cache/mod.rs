mod resolution;
mod storage;
mod validation;
mod worker;

#[cfg(test)]
pub(super) mod test_support;

#[cfg(test)]
use std::path::Path;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, mpsc as std_mpsc},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ::storage::sqlite::MAX_BATCH_BYTES;
use ::storage::sqlite::{OpenStatus, SqliteDb};
use network::{Grid, SimulatorId};
use protocol::{
    lludp::{MAX_PACKET_RETAINED_CAPACITY_BYTES, Message},
    messages::RegionHandshake,
};
use tokio::{
    runtime::Handle,
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
    time::timeout,
};
use uuid::Uuid;

#[cfg(test)]
use self::storage::open_for_test as open_for_test_storage;
use self::storage::{PendingSubmission, make_access_submission, open_default, queue_scope_load};
use self::worker::CacheWriteWorker;
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
const MAX_PENDING_ROWS: usize = 65_536;
const MAX_PENDING_BYTES: usize = 32 * 1024 * 1024;
pub(super) const HANDSHAKE_CACHE_LOAD_TIMEOUT: Duration = Duration::from_millis(50);
const MAX_PENDING_REGION_LOADS: usize = 2;
const CACHE_EVENT_CAPACITY: usize = MAX_PENDING_SUBMISSIONS + MAX_PENDING_REGION_LOADS + 1;
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
    payload: Option<Arc<[u8]>>,
}

struct LoadedRegion {
    scope: CacheScope,
    entries: HashMap<u32, StoredEntry>,
    bytes: usize,
    last_used: u64,
    complete: bool,
}

#[derive(Clone)]
pub(super) struct EncodedEntry {
    local_id: u32,
    crc: u32,
    update_kind: i64,
    update_flags: u32,
    payload: Arc<[u8]>,
}

struct CacheWriteJob {
    operation: CacheWriteOperation,
    permit: Option<mpsc::OwnedPermit<ObjectCacheEvent>>,
}

enum CacheWriteOperation {
    Update { simulator_id: SimulatorId, scope: CacheScope, message: Message, last_used: i64 },
    DeleteScope { scope: CacheScope },
    ClearAll,
}

pub(super) enum ObjectCacheEvent {
    Ready { db: Arc<SqliteDb>, status: OpenStatus, writer: CacheWriteWorker },
    OpenFailed(String),
    RegionLoaded { simulator_id: SimulatorId, scope: CacheScope, result: RegionLoadResult, _permit: OwnedSemaphorePermit },
    WriteApplied { simulator_id: SimulatorId, scope: CacheScope, entries: Arc<[EncodedEntry]> },
    WriteDropped,
    InvalidationApplied,
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
    loaded_entries: usize,
    loaded_bytes: usize,
    use_clock: u64,
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
            loaded_entries: 0,
            loaded_bytes: 0,
            use_clock: 0,
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
                drop(_permit);
                self.flush_pending_writes();
                Some((simulator_id, flags))
            }
            ObjectCacheEvent::WriteApplied { simulator_id, scope, entries } => {
                self.outstanding_writes = self.outstanding_writes.saturating_sub(1);
                if self.identities.get(&simulator_id) == Some(&scope) {
                    self.remember_entries(simulator_id, scope, &entries);
                }
                self.flush_pending_writes();
                None
            }
            ObjectCacheEvent::WriteDropped => {
                self.outstanding_writes = self.outstanding_writes.saturating_sub(1);
                self.count_dropped_write();
                self.flush_pending_writes();
                None
            }
            ObjectCacheEvent::InvalidationApplied => {
                self.outstanding_writes = self.outstanding_writes.saturating_sub(1);
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
        self.remove_loaded_region(simulator_id);
        if let Some(previous) = previous {
            self.queue_invalidation(CacheWriteJob { operation: CacheWriteOperation::DeleteScope { scope: previous }, permit: None });
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

    pub(super) fn record_update(&mut self, simulator_id: SimulatorId, message: Message) {
        let Some(scope) = self.identities.get(&simulator_id).cloned() else { return };
        if self.disabled {
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
        let job = CacheWriteJob {
            operation: CacheWriteOperation::Update { simulator_id, scope, message, last_used: now_millis() },
            permit: Some(permit),
        };
        self.queue_cache_write(job);
    }

    pub(super) fn begin_region_load(&mut self, simulator_id: SimulatorId, runtime: &Handle) -> Option<u32> {
        let Some(scope) = self.identities.get(&simulator_id).cloned() else { return Some(self.region_handshake_flags(simulator_id)) };
        if !self.disabled && self.db.is_some() {
            self.enqueue(make_access_submission(scope.clone(), HashSet::new(), now_millis()));
        }
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

    pub(super) async fn close(mut self) {
        while (!self.pending.is_empty() || !self.pending_writes.is_empty()) && self.db.is_none() && !self.disabled {
            let Some(event) = self.event_receiver.recv().await else { break };
            let _ = self.handle_event(event);
        }
        let Some(db) = self.db.take() else { return };
        self.flush_pending_writes();
        while !self.pending_writes.is_empty() {
            let Some(event) = self.event_receiver.recv().await else { break };
            let _ = self.handle_event(event);
        }
        if let Some(writer) = self.write_worker.take() {
            match tokio::task::spawn_blocking(move || writer.shutdown()).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => self.warn(&error),
                Err(error) => self.warn(&error.to_string()),
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

    fn queue_cache_write(&mut self, job: CacheWriteJob) {
        if self.pending_writes.is_empty() && self.write_worker.is_some() {
            match self.submit_to_write_worker(job) {
                Ok(()) => {}
                Err(job) => self.push_pending_write(*job),
            }
        } else {
            self.push_pending_write(job);
        }
    }

    fn queue_invalidation(&mut self, job: CacheWriteJob) {
        if self.disabled {
            return;
        }
        if self.pending_writes.is_empty() && self.write_worker.is_some() {
            match self.submit_to_write_worker(job) {
                Ok(()) => {}
                Err(job) => self.push_pending_invalidation(*job),
            }
        } else {
            self.push_pending_invalidation(job);
        }
    }

    fn push_pending_write(&mut self, job: CacheWriteJob) {
        let next_bytes = self.pending_write_bytes.saturating_add(MAX_PACKET_RETAINED_CAPACITY_BYTES);
        if self.pending_writes.len() >= MAX_PENDING_SUBMISSIONS || next_bytes > MAX_PENDING_BYTES {
            self.warn("object cache write queue is full; some objects will be refetched");
            self.count_dropped_write();
            return;
        }
        self.pending_write_bytes = next_bytes;
        self.pending_writes.push_back(job);
    }

    fn push_pending_invalidation(&mut self, job: CacheWriteJob) {
        if self.pending_writes.len() >= MAX_PENDING_SUBMISSIONS {
            let dropped =
                self.pending_writes.iter().filter(|pending| matches!(&pending.operation, CacheWriteOperation::Update { .. })).count();
            self.dropped_writes = self.dropped_writes.saturating_add(u64::try_from(dropped).unwrap_or(u64::MAX));
            self.pending_writes.clear();
            self.pending_write_bytes = 0;
            self.pending_writes.push_back(CacheWriteJob { operation: CacheWriteOperation::ClearAll, permit: None });
            return;
        }
        self.pending_writes.push_back(job);
    }

    fn flush_pending_writes(&mut self) {
        loop {
            if self.pending_writes.is_empty() || self.write_worker.is_none() {
                return;
            }
            let mut job = self.pending_writes.pop_front().expect("pending cache write queue is nonempty");
            if job.permit.is_none() {
                match self.event_sender.clone().try_reserve_owned() {
                    Ok(permit) => job.permit = Some(permit),
                    Err(_) => {
                        self.pending_writes.push_front(job);
                        return;
                    }
                }
            }
            self.pending_write_bytes = match &job.operation {
                CacheWriteOperation::Update { .. } => self.pending_write_bytes.saturating_sub(MAX_PACKET_RETAINED_CAPACITY_BYTES),
                CacheWriteOperation::DeleteScope { .. } | CacheWriteOperation::ClearAll => self.pending_write_bytes,
            };
            match self.submit_to_write_worker(job) {
                Ok(()) => {}
                Err(job) => {
                    let job = *job;
                    self.pending_write_bytes = match &job.operation {
                        CacheWriteOperation::Update { .. } => self.pending_write_bytes.saturating_add(MAX_PACKET_RETAINED_CAPACITY_BYTES),
                        CacheWriteOperation::DeleteScope { .. } | CacheWriteOperation::ClearAll => self.pending_write_bytes,
                    };
                    self.pending_writes.push_front(job);
                    return;
                }
            }
        }
    }

    fn submit_to_write_worker(&mut self, mut job: CacheWriteJob) -> Result<(), Box<CacheWriteJob>> {
        let Some(writer) = self.write_worker.as_ref() else { return Err(Box::new(job)) };
        if job.permit.is_none() {
            match self.event_sender.clone().try_reserve_owned() {
                Ok(permit) => job.permit = Some(permit),
                Err(_) => return Err(Box::new(job)),
            }
        }
        match writer.sender.try_send(job) {
            Ok(()) => {
                self.outstanding_writes = self.outstanding_writes.saturating_add(1);
                Ok(())
            }
            Err(std_mpsc::TrySendError::Full(job)) => Err(Box::new(job)),
            Err(std_mpsc::TrySendError::Disconnected(job)) => {
                if matches!(job.operation, CacheWriteOperation::Update { .. }) {
                    self.count_dropped_write();
                } else {
                    self.disable_after_invalidation_failure();
                }
                drop(job);
                Ok(())
            }
        }
    }

    fn disable_after_invalidation_failure(&mut self) {
        self.warn("object cache invalidation failed; cached objects will be refetched");
        self.disabled = true;
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
}
