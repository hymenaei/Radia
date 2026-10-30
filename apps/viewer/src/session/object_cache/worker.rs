use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, mpsc as std_mpsc},
    thread::JoinHandle,
    time::Duration,
};

use network::SimulatorId;
use protocol::{
    lludp::{MAX_PACKET_RETAINED_CAPACITY_BYTES, Message, Packet},
    messages::{ObjectUpdate, ObjectUpdateCompressed},
};
use storage::sqlite::{SqliteBatch, SqliteDb, SqliteError};
use tokio::{
    runtime::Handle,
    sync::{mpsc::OwnedPermit, watch},
};

use super::storage::{make_delete_entries_batches, make_scope_delete_submission, make_write_batches};
use super::{
    CacheEntryVersions, CacheScope, EncodedEntry, MAX_OBJECT_CACHE_ENTRY_BYTES, MAX_PENDING_BYTES, MAX_PENDING_CACHE_WRITES,
    MAX_PENDING_INVALIDATION_IDS, MAX_PENDING_INVALIDATION_SCOPES, MAX_PENDING_SUBMISSIONS, MIN_OBJECT_CACHE_ENTRY_BYTES, ObjectCacheEvent,
    ObjectCacheSession, ScopeInvalidation,
};

pub(super) struct CacheWriteJob {
    pub(super) operation: CacheWriteOperation,
    pub(super) permit: Option<OwnedPermit<ObjectCacheEvent>>,
}

pub(super) enum CacheWriteOperation {
    Update {
        simulator_id: SimulatorId,
        scope: CacheScope,
        message: Message,
        versions: CacheEntryVersions,
        last_used: i64,
    },
    DeleteScope {
        scope: CacheScope,
        invalidation: Option<ScopeInvalidation>,
    },
    DeleteEntries {
        scope: CacheScope,
        entries: HashMap<(SimulatorId, u32), u64>,
    },
    #[cfg(test)]
    Panic,
    #[cfg(test)]
    WaitForRelease {
        started: std_mpsc::SyncSender<()>,
        release: std_mpsc::Receiver<()>,
    },
}

pub(in crate::session) struct CacheWriteWorker {
    pub(super) sender: std_mpsc::SyncSender<CacheWriteJob>,
    thread: JoinHandle<()>,
    stopped: watch::Receiver<bool>,
}

struct WorkerStopped(watch::Sender<bool>);

impl Drop for WorkerStopped {
    fn drop(&mut self) {
        self.0.send_replace(false);
    }
}

impl CacheWriteWorker {
    pub(super) fn start(db: Arc<SqliteDb>, runtime: Handle) -> std::io::Result<Self> {
        let (sender, receiver) = std_mpsc::sync_channel(MAX_PENDING_SUBMISSIONS);
        let (stopped_sender, stopped) = watch::channel(true);
        let thread = std::thread::Builder::new().name("object-cache-write".to_owned()).spawn(move || {
            let _stopped = WorkerStopped(stopped_sender);
            let mut invalidation_failed = false;
            while let Ok(job) = receiver.recv() {
                process_write_job(Arc::clone(&db), runtime.clone(), job, &mut invalidation_failed);
            }
        })?;
        Ok(Self { sender, thread, stopped })
    }

    pub(super) async fn stopped(&mut self) {
        while *self.stopped.borrow() {
            if self.stopped.changed().await.is_err() {
                return;
            }
        }
    }

    pub(super) fn shutdown(self) -> Result<(), String> {
        let Self { sender, thread, stopped: _ } = self;
        drop(sender);
        thread.join().map_err(|_| "object cache write worker stopped unexpectedly".to_owned())
    }
}

impl ObjectCacheSession {
    pub(super) fn queue_cache_write(&mut self, job: CacheWriteJob) -> bool {
        if self.pending_writes.is_empty() && self.write_worker.is_some() {
            match self.submit_to_write_worker(job) {
                Ok(accepted) => accepted,
                Err(job) => self.push_pending_write(*job),
            }
        } else {
            self.push_pending_write(job)
        }
    }

    pub(super) fn queue_invalidation(&mut self, job: CacheWriteJob) -> bool {
        if self.disabled {
            return false;
        }
        if self.pending_writes.is_empty() && self.write_worker.is_some() {
            match self.submit_to_write_worker(job) {
                Ok(accepted) => accepted,
                Err(job) => self.push_pending_invalidation(*job),
            }
        } else {
            self.push_pending_invalidation(job)
        }
    }

    fn push_pending_write(&mut self, job: CacheWriteJob) -> bool {
        let next_bytes = self.pending_write_bytes.saturating_add(MAX_PACKET_RETAINED_CAPACITY_BYTES);
        let update_count =
            self.pending_writes.iter().filter(|pending| matches!(&pending.operation, CacheWriteOperation::Update { .. })).count();
        if update_count >= MAX_PENDING_SUBMISSIONS
            || self.pending_writes.len() >= MAX_PENDING_CACHE_WRITES
            || next_bytes > MAX_PENDING_BYTES
        {
            self.warn("object cache write queue is full; some objects will be refetched");
            self.count_dropped_write();
            return false;
        }
        self.pending_write_bytes = next_bytes;
        self.pending_writes.push_back(job);
        true
    }

    fn push_pending_invalidation(&mut self, job: CacheWriteJob) -> bool {
        let Some(scope) = invalidation_scope(&job.operation).cloned() else { return false };
        let mut merge_index = None;
        let mut scope_needs_collapse = false;
        let mut crossed_update = false;
        for (index, pending) in self.pending_writes.iter().enumerate().rev() {
            match &pending.operation {
                CacheWriteOperation::Update { scope: pending_scope, .. } if pending_scope == &scope => crossed_update = true,
                operation if invalidation_scope(operation) == Some(&scope) => {
                    if crossed_update {
                        scope_needs_collapse = true;
                    } else {
                        merge_index = Some(index);
                    }
                    break;
                }
                _ => {}
            }
        }
        if scope_needs_collapse {
            self.collapse_pending_scope(&scope, job);
            return true;
        }
        if let Some(index) = merge_index {
            self.merge_pending_invalidation(index, job);
            return true;
        }

        let scopes = self.pending_writes.iter().filter_map(|pending| invalidation_scope(&pending.operation)).collect::<HashSet<_>>();
        if !scopes.contains(&&scope) && scopes.len() >= MAX_PENDING_INVALIDATION_SCOPES {
            self.warn("object cache invalidation scope queue is full; cache entries will be fetched normally");
            self.disable_after_invalidation_failure();
            return false;
        }

        if self.pending_writes.len() >= MAX_PENDING_CACHE_WRITES {
            if let Some(index) =
                self.pending_writes.iter().position(|pending| matches!(&pending.operation, CacheWriteOperation::Update { .. }))
            {
                if let Some(CacheWriteJob { operation: CacheWriteOperation::Update { simulator_id, versions, .. }, .. }) =
                    self.pending_writes.remove(index)
                {
                    self.clear_cache_versions(simulator_id, &versions);
                    self.pending_write_bytes = self.pending_write_bytes.saturating_sub(MAX_PACKET_RETAINED_CAPACITY_BYTES);
                    self.count_dropped_write();
                }
            } else {
                self.warn("object cache invalidation queue is full; cache entries will be fetched normally");
                self.disable_after_invalidation_failure();
                return false;
            }
        }
        self.pending_writes.push_back(job);
        true
    }

    fn collapse_pending_scope(&mut self, scope: &CacheScope, incoming: CacheWriteJob) {
        let mut simulator_ids = HashSet::new();
        let mut retained = VecDeque::with_capacity(self.pending_writes.len());
        while let Some(job) = self.pending_writes.pop_front() {
            if operation_scope(&job.operation) == Some(scope) {
                match job.operation {
                    CacheWriteOperation::Update { simulator_id, versions, .. } => {
                        simulator_ids.insert(simulator_id);
                        self.clear_cache_versions(simulator_id, &versions);
                        self.pending_write_bytes = self.pending_write_bytes.saturating_sub(MAX_PACKET_RETAINED_CAPACITY_BYTES);
                        self.count_dropped_write();
                    }
                    CacheWriteOperation::DeleteEntries { entries, .. } => {
                        simulator_ids.extend(entries.keys().map(|(simulator_id, _)| *simulator_id));
                    }
                    CacheWriteOperation::DeleteScope { invalidation, .. } => {
                        if let Some(invalidation) = invalidation {
                            simulator_ids.extend(invalidation.simulator_ids.iter().copied());
                        }
                    }
                    #[cfg(test)]
                    CacheWriteOperation::Panic | CacheWriteOperation::WaitForRelease { .. } => unreachable!(),
                }
            } else {
                retained.push_back(job);
            }
        }
        self.pending_writes = retained;
        let permit = incoming.permit;
        match incoming.operation {
            CacheWriteOperation::Update { .. } => unreachable!(),
            CacheWriteOperation::DeleteEntries { entries, .. } => {
                simulator_ids.extend(entries.keys().map(|(simulator_id, _)| *simulator_id));
            }
            CacheWriteOperation::DeleteScope { invalidation, .. } => {
                if let Some(invalidation) = invalidation {
                    simulator_ids.extend(invalidation.simulator_ids.iter().copied());
                }
            }
            #[cfg(test)]
            CacheWriteOperation::Panic | CacheWriteOperation::WaitForRelease { .. } => unreachable!(),
        }
        let invalidation = self.scope_invalidation(scope, simulator_ids, self.cache_generation_clock);
        self.invalidate_loaded_scope(scope, &invalidation);
        self.pending_writes.push_back(CacheWriteJob {
            operation: CacheWriteOperation::DeleteScope { scope: scope.clone(), invalidation: Some(invalidation) },
            permit,
        });
    }

    fn merge_pending_invalidation(&mut self, index: usize, incoming: CacheWriteJob) {
        let Some(mut existing) = self.pending_writes.remove(index) else { return };
        drop(incoming.permit);
        let (scope, entries, scope_delete, existing_invalidation, incoming_invalidation) = match (existing.operation, incoming.operation) {
            (
                CacheWriteOperation::DeleteEntries { scope, mut entries },
                CacheWriteOperation::DeleteEntries { scope: _, entries: incoming_entries },
            ) => {
                entries.extend(incoming_entries);
                (scope, entries, false, None, None)
            }
            (CacheWriteOperation::DeleteEntries { scope, entries }, CacheWriteOperation::DeleteScope { scope: _, invalidation }) => {
                (scope, entries, true, None, invalidation)
            }
            (CacheWriteOperation::DeleteScope { scope, invalidation }, CacheWriteOperation::DeleteEntries { scope: _, entries }) => {
                (scope, entries, true, invalidation, None)
            }
            (
                CacheWriteOperation::DeleteScope { scope, invalidation: existing_invalidation },
                CacheWriteOperation::DeleteScope { scope: _, invalidation: incoming_invalidation },
            ) => (scope, HashMap::new(), true, existing_invalidation, incoming_invalidation),
            _ => unreachable!(),
        };

        if scope_delete || entries.len() > MAX_PENDING_INVALIDATION_IDS {
            let mut simulator_ids = entries.keys().map(|(simulator_id, _)| *simulator_id).collect::<HashSet<_>>();
            if let Some(invalidation) = &existing_invalidation {
                simulator_ids.extend(invalidation.simulator_ids.iter().copied());
            }
            if let Some(invalidation) = &incoming_invalidation {
                simulator_ids.extend(invalidation.simulator_ids.iter().copied());
            }
            let invalidation = self.scope_invalidation(&scope, simulator_ids, self.cache_generation_clock);
            self.invalidate_loaded_scope(&scope, &invalidation);
            existing.operation = CacheWriteOperation::DeleteScope { scope, invalidation: Some(invalidation) };
        } else {
            existing.operation = CacheWriteOperation::DeleteEntries { scope, entries };
        }
        self.pending_writes.insert(index, existing);
    }

    fn scope_invalidation(
        &self,
        scope: &CacheScope,
        simulator_ids: impl IntoIterator<Item = SimulatorId>,
        generation: u64,
    ) -> ScopeInvalidation {
        let mut simulator_ids = simulator_ids.into_iter().collect::<HashSet<_>>();
        simulator_ids.extend(self.identities.iter().filter_map(|(simulator_id, current)| (current == scope).then_some(*simulator_id)));
        ScopeInvalidation { simulator_ids: simulator_ids.into_iter().collect::<Vec<_>>().into(), generation }
    }

    pub(super) fn invalidate_loaded_scope(&mut self, scope: &CacheScope, invalidation: &ScopeInvalidation) {
        for simulator_id in invalidation.simulator_ids.iter().copied() {
            self.cache_generations
                .retain(|(current_simulator, _), generation| *current_simulator != simulator_id || *generation > invalidation.generation);
            if self.identities.get(&simulator_id) == Some(scope) {
                self.remove_loaded_region(simulator_id);
            }
        }
        for ((_, load_scope), load) in &mut self.in_flight_loads {
            if load_scope == scope {
                load.invalidated_local_ids.clear();
                load.stale = true;
            }
        }
    }

    pub(super) fn flush_pending_writes(&mut self) {
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
                CacheWriteOperation::DeleteScope { .. } | CacheWriteOperation::DeleteEntries { .. } => self.pending_write_bytes,
                #[cfg(test)]
                CacheWriteOperation::Panic | CacheWriteOperation::WaitForRelease { .. } => self.pending_write_bytes,
            };
            match self.submit_to_write_worker(job) {
                Ok(_) => {}
                Err(job) => {
                    let job = *job;
                    self.pending_write_bytes = match &job.operation {
                        CacheWriteOperation::Update { .. } => self.pending_write_bytes.saturating_add(MAX_PACKET_RETAINED_CAPACITY_BYTES),
                        CacheWriteOperation::DeleteScope { .. } | CacheWriteOperation::DeleteEntries { .. } => self.pending_write_bytes,
                        #[cfg(test)]
                        CacheWriteOperation::Panic | CacheWriteOperation::WaitForRelease { .. } => self.pending_write_bytes,
                    };
                    self.pending_writes.push_front(job);
                    return;
                }
            }
        }
    }

    fn submit_to_write_worker(&mut self, mut job: CacheWriteJob) -> Result<bool, Box<CacheWriteJob>> {
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
                Ok(true)
            }
            Err(std_mpsc::TrySendError::Full(job)) => Err(Box::new(job)),
            Err(std_mpsc::TrySendError::Disconnected(job)) => {
                match job.operation {
                    CacheWriteOperation::Update { simulator_id, versions, .. } => {
                        self.clear_cache_versions(simulator_id, &versions);
                        self.count_dropped_write();
                    }
                    _ => self.disable_after_invalidation_failure(),
                }
                Ok(false)
            }
        }
    }
}

fn operation_scope(operation: &CacheWriteOperation) -> Option<&CacheScope> {
    match operation {
        CacheWriteOperation::Update { scope, .. }
        | CacheWriteOperation::DeleteScope { scope, .. }
        | CacheWriteOperation::DeleteEntries { scope, .. } => Some(scope),
        #[cfg(test)]
        CacheWriteOperation::Panic | CacheWriteOperation::WaitForRelease { .. } => None,
    }
}

fn invalidation_scope(operation: &CacheWriteOperation) -> Option<&CacheScope> {
    match operation {
        CacheWriteOperation::DeleteScope { scope, .. } | CacheWriteOperation::DeleteEntries { scope, .. } => Some(scope),
        CacheWriteOperation::Update { .. } => None,
        #[cfg(test)]
        CacheWriteOperation::Panic | CacheWriteOperation::WaitForRelease { .. } => None,
    }
}

fn process_write_job(db: Arc<SqliteDb>, runtime: Handle, job: CacheWriteJob, invalidation_failed: &mut bool) {
    let CacheWriteJob { operation, permit } = job;
    let Some(permit) = permit else { return };
    match operation {
        CacheWriteOperation::Update { simulator_id, scope, message, versions, last_used } => {
            if *invalidation_failed {
                drop(permit.send(ObjectCacheEvent::WriteDropped { simulator_id, versions }));
                return;
            }
            let entries: Arc<[EncodedEntry]> = encode_entries(&message).into();
            if entries.is_empty() {
                drop(permit.send(ObjectCacheEvent::WriteApplied { simulator_id, scope, versions, entries }));
                return;
            }
            let batches = match make_write_batches(scope.clone(), Arc::clone(&entries), last_used) {
                Ok(batches) => batches,
                Err(_) => {
                    drop(permit.send(ObjectCacheEvent::WriteDropped { simulator_id, versions }));
                    return;
                }
            };
            match db.submit_batches(batches) {
                Ok(receipt) => {
                    drop(runtime.spawn(async move {
                        match receipt.wait().await {
                            Ok(()) => drop(permit.send(ObjectCacheEvent::WriteApplied { simulator_id, scope, versions, entries })),
                            Err(_) => drop(permit.send(ObjectCacheEvent::WriteDropped { simulator_id, versions })),
                        }
                    }));
                }
                Err(_) => drop(permit.send(ObjectCacheEvent::WriteDropped { simulator_id, versions })),
            }
        }
        CacheWriteOperation::DeleteScope { scope, invalidation } => {
            let applied = submit_invalidation(&db, &runtime, || make_scope_delete_submission(scope.clone()).batches);
            if applied {
                match invalidation {
                    Some(invalidation) => drop(permit.send(ObjectCacheEvent::ScopeInvalidated { scope, invalidation })),
                    None => drop(permit.send(ObjectCacheEvent::InvalidationApplied)),
                }
            } else {
                *invalidation_failed = true;
                drop(permit.send(ObjectCacheEvent::InvalidationDropped));
            }
        }
        CacheWriteOperation::DeleteEntries { scope, entries } => {
            let mut local_ids = entries.keys().map(|(_, local_id)| *local_id).collect::<Vec<_>>();
            local_ids.sort_unstable();
            local_ids.dedup();
            let versions: super::InvalidatedEntryVersions = entries
                .into_iter()
                .map(|((simulator_id, local_id), generation)| (simulator_id, local_id, generation))
                .collect::<Vec<_>>()
                .into();
            let local_ids: Arc<[u32]> = local_ids.into();
            let applied = submit_invalidation(&db, &runtime, || make_delete_entries_batches(scope.clone(), Arc::clone(&local_ids)));
            if applied {
                drop(permit.send(ObjectCacheEvent::EntriesInvalidated { versions }));
            } else {
                *invalidation_failed = true;
                drop(permit.send(ObjectCacheEvent::InvalidationDropped));
            }
        }
        #[cfg(test)]
        CacheWriteOperation::Panic => panic!("test cache writer failure"),
        #[cfg(test)]
        CacheWriteOperation::WaitForRelease { started, release } => {
            started.send(()).expect("test observes stalled cache writer");
            release.recv().expect("test releases stalled cache writer");
            drop(permit.send(ObjectCacheEvent::InvalidationApplied));
        }
    }
}

fn submit_invalidation(db: &SqliteDb, runtime: &Handle, mut batches: impl FnMut() -> Vec<SqliteBatch>) -> bool {
    loop {
        let receipt = match db.submit_batches(batches()) {
            Ok(receipt) => receipt,
            Err(SqliteError::Busy | SqliteError::QueueFull) => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(_) => return false,
        };
        match runtime.block_on(receipt.wait()) {
            Ok(()) => return true,
            Err(SqliteError::Busy | SqliteError::QueueFull) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return false,
        }
    }
}

fn encode_entries(message: &Message) -> Vec<EncodedEntry> {
    match message {
        Message::ObjectUpdate(update) => update
            .objects
            .iter()
            .filter_map(|object| {
                let local_id = object.local_id;
                if local_id == 0 {
                    return None;
                }
                let payload =
                    Packet::new(0, Message::ObjectUpdate(ObjectUpdate { region_data: update.region_data, objects: vec![object.clone()] }))
                        .encode()
                        .ok()?;
                (payload.len() >= MIN_OBJECT_CACHE_ENTRY_BYTES && payload.len() <= MAX_OBJECT_CACHE_ENTRY_BYTES).then_some(EncodedEntry {
                    local_id,
                    crc: object.crc,
                    update_kind: 0,
                    update_flags: object.update_flags,
                    payload: Arc::from(payload),
                })
            })
            .collect(),
        Message::ObjectUpdateCompressed(update) => update
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
                (payload.len() >= MIN_OBJECT_CACHE_ENTRY_BYTES && payload.len() <= MAX_OBJECT_CACHE_ENTRY_BYTES).then_some(EncodedEntry {
                    local_id,
                    crc: object.crc,
                    update_kind: 1,
                    update_flags: object.update_flags,
                    payload: Arc::from(payload),
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use network::{Grid, SimulatorId};
    use protocol::lludp::Message;
    use tokio::runtime::Handle;

    use super::CacheWriteWorker;
    use crate::session::object_cache::storage::open_at;
    use crate::session::object_cache::test_support::*;
    use crate::session::object_cache::{CacheWriteJob, CacheWriteOperation, MAX_REGION_ENTRIES, ObjectCacheEvent, ObjectCacheSession};
    #[tokio::test]
    async fn region_reset_deletes_writes_already_queued_for_the_previous_scope() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let path = directory.path().join("objects.sqlite");
        let mut cache = open_cache(&path);
        cache.record_update(SIMULATOR, fixture_message(vectors::OBJECT_UPDATE));
        cache.set_region_identity(SIMULATOR, &region_handshake(NEXT_CACHE_ID));
        flush(&mut cache).await;
        cache.close().await;

        let (db, _) = open_at(&path).expect("reopen cache after region reset");
        assert_eq!(cache_count(&db).await, 0);
    }

    #[tokio::test]
    async fn pending_writes_are_bounded() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let path = directory.path().join("objects.sqlite");
        let (db, status) = open_at(&path).expect("open test database");
        let db = Arc::new(db);
        let writer = CacheWriteWorker::start(Arc::clone(&db), Handle::current()).expect("start cache writer");
        let mut cache = ObjectCacheSession::new(Grid::Agni);
        cache.set_region_identity(SIMULATOR, &region_handshake(CACHE_ID));

        for local_id in 1..=u32::try_from(super::MAX_PENDING_SUBMISSIONS + 1).expect("test write count fits u32") {
            let mut message = fixture_message(vectors::OBJECT_UPDATE);
            let Message::ObjectUpdate(update) = &mut message else {
                panic!("object update fixture should be a full update");
            };
            update.objects.truncate(1);
            update.objects[0].local_id = local_id;
            cache.record_update(SIMULATOR, message);
        }

        assert!(cache.handle_event(ObjectCacheEvent::Ready { db: Arc::clone(&db), status, writer }).is_none());
        flush(&mut cache).await;
        cache.close().await;
        assert_eq!(cache_count(&db).await, super::MAX_PENDING_SUBMISSIONS);
    }

    #[tokio::test]
    async fn stalled_overflow_preserves_other_scope() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let path = directory.path().join("objects.sqlite");
        let mut cache = open_cache(&path);
        let other_simulator = SimulatorId::new(78);
        cache.set_region_identity(other_simulator, &region_handshake(NEXT_CACHE_ID));

        let mut affected_update = fixture_message(vectors::OBJECT_UPDATE);
        let Message::ObjectUpdate(update) = &mut affected_update else { panic!("object fixture should be a full update") };
        update.objects.truncate(1);
        update.objects[0].local_id = 700_001;
        cache.record_update(SIMULATOR, affected_update);
        let mut other_update = fixture_message(vectors::OBJECT_UPDATE);
        let Message::ObjectUpdate(update) = &mut other_update else { panic!("object fixture should be a full update") };
        update.objects.truncate(1);
        update.objects[0].local_id = 700_002;
        cache.record_update(other_simulator, other_update);
        flush(&mut cache).await;

        let affected_scope = cache.identities.get(&SIMULATOR).expect("affected scope is set").clone();
        let other_scope = cache.identities.get(&other_simulator).expect("other scope is set").clone();
        let db = cache.db.as_ref().expect("cache database is open");
        assert_eq!(cache_scope_entry_count(db, &affected_scope).await, 1);
        assert_eq!(cache_scope_entry_count(db, &other_scope).await, 1);

        let (started_sender, started_receiver) = std::sync::mpsc::sync_channel(1);
        let (release_sender, release_receiver) = std::sync::mpsc::sync_channel(1);
        let permit = cache.event_sender.clone().try_reserve_owned().expect("reserve stalled writer completion");
        assert_eq!(
            cache
                .submit_to_write_worker(CacheWriteJob {
                    operation: CacheWriteOperation::WaitForRelease { started: started_sender, release: release_receiver },
                    permit: Some(permit),
                })
                .ok(),
            Some(true)
        );
        started_receiver.recv_timeout(Duration::from_secs(1)).expect("writer is stalled");

        let terse_template = fixture_message(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
        let Message::ImprovedTerseObjectUpdate(_) = &terse_template else { panic!("terse fixture should be a terse update") };
        let invalidation_count = MAX_REGION_ENTRIES + super::MAX_PENDING_SUBMISSIONS + 1;
        for local_id in 1..=u32::try_from(invalidation_count).expect("invalidation count fits a local ID") {
            let mut terse = terse_template.clone();
            let Message::ImprovedTerseObjectUpdate(update) = &mut terse else { panic!("terse fixture should be a terse update") };
            update.objects[0].as_decoded_mut().expect("terse fixture object is valid").local_id = local_id;
            cache.record_update(SIMULATOR, terse);
        }

        release_sender.send(()).expect("release stalled writer");
        flush(&mut cache).await;
        let db = cache.db.as_ref().expect("cache database is open");
        assert_eq!(cache_scope_entry_count(db, &affected_scope).await, 0);
        assert_eq!(cache_scope_entry_count(db, &other_scope).await, 1);
        cache.close().await;
    }

    #[tokio::test]
    async fn close_finishes_after_writer_panics() {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let mut cache = open_cache(directory.path().join("objects.sqlite"));
        let permit = cache.event_sender.clone().try_reserve_owned().expect("reserve writer completion slot");
        if cache.submit_to_write_worker(CacheWriteJob { operation: CacheWriteOperation::Panic, permit: Some(permit) }).is_err() {
            panic!("test writer should accept the injected panic job");
        }

        tokio::time::timeout(Duration::from_secs(1), cache.close()).await.expect("cache close completes after writer panic");
    }
}
