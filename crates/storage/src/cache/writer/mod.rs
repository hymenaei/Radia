mod disk;
mod queue;
mod support;

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs::{self, File, ReadDir},
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
};

use sha2::{Digest, Sha256};
use tempfile::{Builder, NamedTempFile};

#[cfg(test)]
use std::sync::{
    Mutex,
    atomic::AtomicUsize,
    mpsc::{self, Receiver, SyncSender},
};

use super::{
    DirectoryId, ENTRY_INVALID, ENTRY_REPLACING, ENTRY_VALID, Entry, Key, PendingTouch, ProtectionRegistry, ReadState, StorageError,
    UnavailableReason, index::Index,
};
use crate::{
    EvictionCandidate, EvictionPolicy, EvictionReason,
    cache::eviction::{Lru, OrderedScan},
    sqlite::{MAX_BATCH_ROWS, SqliteError},
};
use disk::{DATA_TEMP_PREFIX, delete_data, fail_temporary, persist_with_retry, remove_path};
use support::{
    account_size, clone_error, effective_entry, is_index_file, is_simulated_crash, lock, mutations, read_map, remove_new_candidate,
    simulated_crash_error, sort_changes, write_map,
};

pub(in crate::cache) use queue::{Command, WriteQueue, writer_loop};

const QUEUE_SWEEP_CHUNK: usize = 64;
const MAX_SWEEP_DEPTH: usize = 32;
const MAX_EVICTIONS_PER_BATCH: usize = 32;
const MAX_EVICTION_CANDIDATES: usize = 128;
const MAX_EVICTION_SCAN_PASSES: usize = 4;

#[repr(usize)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WritePoint {
    DataTempCreated = 1,
    DataWriteFailure,
    #[cfg(test)]
    DataWritePartial,
    DataWritten,
    DataRenamed,
    IndexBatchStarted,
    #[cfg(test)]
    IndexBatchFailure,
    IndexBatchCommitted,
    #[cfg(test)]
    LookupEntrySelected,
    #[cfg(test)]
    LookupHashMismatch,
    #[cfg(test)]
    IndexTouchBusy,
    #[cfg(test)]
    PutQueued,
    #[cfg(test)]
    WriterReady,
}

#[cfg(test)]
struct TestPause {
    point: WritePoint,
    started: SyncSender<()>,
    resume: Receiver<()>,
}

#[derive(Default)]
pub(super) struct FaultHooks {
    #[cfg(test)]
    fail_at: AtomicUsize,
    #[cfg(test)]
    fail_after: AtomicUsize,
    #[cfg(test)]
    pause: Mutex<Vec<TestPause>>,
}

impl FaultHooks {
    fn write_data_payload(&self, file: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
        #[cfg(test)]
        if self.fail_at.load(Ordering::Acquire) == WritePoint::DataWritePartial as usize {
            let split = bytes.len() / 2;
            let prefix = bytes.get(..split).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid payload split"))?;
            file.write_all(prefix)?;
            return self.check(WritePoint::DataWritePartial);
        }
        file.write_all(bytes)
    }

    pub(super) fn check(&self, point: WritePoint) -> io::Result<()> {
        #[cfg(test)]
        {
            let mut pauses = match self.pause.lock() {
                Ok(pauses) => pauses,
                Err(poisoned) => poisoned.into_inner(),
            };
            let selected = pauses.iter().position(|pause| pause.point == point);
            let pause = selected.map(|index| pauses.remove(index));
            drop(pauses);
            if let Some(pause) = pause {
                pause.started.send(()).map_err(|_| io::Error::other("test pause receiver stopped"))?;
                pause.resume.recv().map_err(|_| io::Error::other("test pause was released"))?;
            }
            let should_fail = if self.fail_at.load(Ordering::Acquire) == point as usize {
                let remaining = self.fail_after.fetch_sub(1, Ordering::AcqRel);
                if remaining <= 1 {
                    self.fail_at.store(0, Ordering::Release);
                    self.fail_after.store(0, Ordering::Release);
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if should_fail {
                let (kind, message) = match point {
                    WritePoint::DataWriteFailure => (io::ErrorKind::StorageFull, "simulated full disk"),
                    #[cfg(test)]
                    WritePoint::IndexBatchFailure => (io::ErrorKind::StorageFull, "simulated full disk"),
                    _ => (io::ErrorKind::Interrupted, "simulated cache crash"),
                };
                return Err(io::Error::new(kind, message));
            }
        }
        #[cfg(not(test))]
        let _ = point;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn arm(&self, point: WritePoint) {
        self.arm_after(point, 1);
    }

    #[cfg(test)]
    pub(super) fn arm_after(&self, point: WritePoint, hits: usize) {
        self.fail_after.store(hits.max(1), Ordering::Release);
        self.fail_at.store(point as usize, Ordering::Release);
    }

    #[cfg(test)]
    pub(super) fn pause_at(&self, point: WritePoint) -> (Receiver<()>, SyncSender<()>) {
        let (started, started_receiver) = mpsc::sync_channel(0);
        let (resume_sender, resume) = mpsc::sync_channel(0);
        let pause = TestPause { point, started, resume };
        match self.pause.lock() {
            Ok(mut current) => current.push(pause),
            Err(poisoned) => poisoned.into_inner().push(pause),
        }
        (started_receiver, resume_sender)
    }

    #[cfg(test)]
    pub(super) fn pause_many_at(&self, point: WritePoint, count: usize) -> (Receiver<()>, Vec<SyncSender<()>>) {
        let (started, started_receiver) = mpsc::sync_channel(count);
        let mut pauses = Vec::with_capacity(count);
        let mut resumes = Vec::with_capacity(count);
        for _ in 0..count {
            let (resume_sender, resume) = mpsc::sync_channel(1);
            pauses.push(TestPause { point, started: started.clone(), resume });
            resumes.push(resume_sender);
        }
        match self.pause.lock() {
            Ok(mut current) => current.extend(pauses),
            Err(poisoned) => poisoned.into_inner().extend(pauses),
        }
        (started_receiver, resumes)
    }
}

struct Plan {
    changes: HashMap<Key, Option<Arc<Entry>>>,
    directory_deltas: HashMap<DirectoryId, i64>,
    eviction_reasons: HashMap<Key, EvictionReason>,
}

struct StagedData {
    entry: Arc<Entry>,
    temp: NamedTempFile,
}

struct PlannedChange {
    key: Key,
    previous: Option<Arc<Entry>>,
    next: Option<Arc<Entry>>,
    eviction_reason: Option<EvictionReason>,
}

struct CommitFailure {
    committed: HashSet<Key>,
    error: StorageError,
}

impl CommitFailure {
    fn before_commit(error: StorageError) -> Self {
        Self { committed: HashSet::new(), error }
    }
}

enum VictimSearch {
    Found(Key, EvictionReason),
    Exhausted,
    Deferred,
}

pub(super) struct WorkerState {
    root: PathBuf,
    _lock_file: File,
    index: Index,
    read_state: Arc<ReadState>,
    lru: Lru,
    eviction_scan_cursor: Option<(u64, Key)>,
    referenced_keys: HashSet<Key>,
    directory_counts: HashMap<DirectoryId, usize>,
    directory_count: u64,
    data_bytes: u64,
    entry_count: usize,
    size_limit: u64,
    blocked_protection_generation: Option<u64>,
    hooks: Arc<FaultHooks>,
    sweep: Vec<(PathBuf, ReadDir)>,
    sweep_complete: bool,
}

impl WorkerState {
    pub fn new(
        root: PathBuf,
        lock_file: File,
        read_state: Arc<ReadState>,
        index: Index,
        hooks: Arc<FaultHooks>,
    ) -> Result<Self, StorageError> {
        let entries = read_map(&read_state.entries);
        let mut lru = Lru::default();
        lru.try_reserve(entries.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let mut referenced_keys = HashSet::new();
        let mut directory_counts = HashMap::<DirectoryId, usize>::new();
        referenced_keys.try_reserve(entries.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        directory_counts
            .try_reserve(entries.len().min(read_state.namespaces.len().saturating_mul(256)).saturating_add(read_state.namespaces.len() * 8))
            .map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let mut data_bytes = 0u64;
        for entry in entries.values() {
            lru.insert(entry.key, entry.last_use.load(Ordering::Relaxed));
            referenced_keys.insert(entry.key);
            if let Some((prefixes, shard)) = read_state.namespaces.directories(entry.key) {
                for directory in prefixes.iter().copied().chain(std::iter::once(shard)) {
                    let count = directory_counts.entry(directory).or_default();
                    *count = count.checked_add(1).ok_or(StorageError::TooLarge)?;
                }
            }
            data_bytes = data_bytes.checked_add(entry.size).ok_or(StorageError::TooLarge)?;
        }
        let entry_count = entries.len();
        let directory_count = u64::try_from(directory_counts.len()).map_err(|_| StorageError::TooLarge)?;
        drop(entries);
        let state = Self {
            root,
            _lock_file: lock_file,
            index,
            size_limit: read_state.size_limit.load(Ordering::Relaxed),
            read_state,
            lru,
            eviction_scan_cursor: None,
            referenced_keys,
            directory_counts,
            directory_count,
            data_bytes,
            entry_count,
            blocked_protection_generation: None,
            hooks,
            sweep: Vec::new(),
            sweep_complete: false,
        };
        state.update_accounted_size();
        Ok(state)
    }

    pub fn accounted_size(&self) -> u64 {
        let index_bytes = self.index.estimated_bytes(self.entry_count).unwrap_or(u64::MAX);
        account_size(index_bytes, self.data_bytes, self.entry_count, self.directory_count).unwrap_or(u64::MAX)
    }

    fn put_batch(&mut self, puts: Vec<(Key, Vec<u8>)>) -> Vec<Result<(), StorageError>> {
        self.drain_touches_best_effort();
        let total = puts.len();
        let mut staged = Vec::<StagedData>::new();
        let mut staged_indices = Vec::new();
        if staged.try_reserve(total).is_err() || staged_indices.try_reserve(total).is_err() {
            return (0..total).map(|_| Err(StorageError::Unavailable(UnavailableReason::Allocation))).collect();
        }
        let mut results = std::iter::repeat_with(|| None).take(total).collect::<Vec<Option<Result<(), StorageError>>>>();
        for (index, (key, bytes)) in puts.into_iter().enumerate() {
            match self.write_data(key, &bytes) {
                Ok(data) => {
                    staged.push(data);
                    staged_indices.push(index);
                }
                Err(error) => {
                    if is_simulated_crash(&error) {
                        let failure = simulated_crash_error();
                        for result in &mut results {
                            if result.is_none() {
                                *result = Some(Err(clone_error(&failure)));
                            }
                        }
                        return results.into_iter().map(|result| result.unwrap_or(Ok(()))).collect();
                    }
                    results[index] = Some(Err(error));
                }
            }
        }
        if staged.is_empty() {
            return results.into_iter().map(|result| result.unwrap_or(Ok(()))).collect();
        }
        let mut staged_entries = Vec::new();
        if staged_entries.try_reserve_exact(staged.len()).is_err() {
            for index in staged_indices {
                if let Some(result) = results.get_mut(index) {
                    *result = Some(Err(StorageError::Unavailable(UnavailableReason::Allocation)));
                }
            }
            return results.into_iter().map(|result| result.unwrap_or(Ok(()))).collect();
        }
        staged_entries.extend(staged.iter().map(|data| Arc::clone(&data.entry)));
        let prepared = {
            let protection_shared = Arc::clone(&self.read_state.protection);
            let mut protection = lock(&protection_shared.state);
            self.plan_puts(&staged_entries, &protection).and_then(|plan| {
                let changes = self.prepare_changes(&plan, &mut protection)?;
                let eviction_marks = self.mark_evictions(&changes, &mut protection)?;
                Ok((changes, eviction_marks))
            })
        };
        let (changes, eviction_marks) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                for index in staged_indices {
                    if let Some(result) = results.get_mut(index) {
                        *result = Some(Err(clone_error(&error)));
                    }
                }
                return results.into_iter().map(|result| result.unwrap_or(Ok(()))).collect();
            }
        };
        let (committed, error) = match self.publish_and_commit(changes, staged, eviction_marks) {
            Ok(committed) => (committed, None),
            Err(failure) => (failure.committed, Some(failure.error)),
        };
        for (entry, index) in staged_entries.iter().zip(staged_indices) {
            if let Some(result) = results.get_mut(index) {
                *result = Some(match error.as_ref() {
                    None => Ok(()),
                    Some(_) if committed.contains(&entry.key) => Ok(()),
                    Some(error) => Err(clone_error(error)),
                });
            }
        }
        results.into_iter().map(|result| result.unwrap_or(Ok(()))).collect()
    }

    fn plan_puts(&mut self, staged: &[Arc<Entry>], protection: &ProtectionRegistry) -> Result<Plan, StorageError> {
        let entries = read_map(&self.read_state.entries);
        let mut changes = HashMap::new();
        changes.try_reserve(staged.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let mut eviction_reasons = HashMap::new();
        eviction_reasons.try_reserve(MAX_EVICTIONS_PER_BATCH).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let mut directory_deltas = HashMap::<DirectoryId, i64>::new();
        directory_deltas
            .try_reserve(staged.len().saturating_mul(3))
            .map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let mut new_order = BTreeSet::<(u64, Key)>::new();
        let mut new_stamps = HashMap::<Key, u64>::new();
        let mut data_bytes = self.data_bytes;
        let mut directory_count = self.directory_count;
        let mut entry_count = self.entry_count;
        let now = self.read_state.usage_clock.load(Ordering::Relaxed);

        for entry in staged {
            let key = entry.key;
            if let Some(previous) = effective_entry(&key, &changes, &entries) {
                self.adjust_directories(&key, -1, &mut directory_deltas, &mut directory_count)?;
                data_bytes = data_bytes.checked_sub(previous.size).ok_or(StorageError::TooLarge)?;
                entry_count = entry_count.saturating_sub(1);
            }
            remove_new_candidate(&key, &mut new_order, &mut new_stamps);
            self.adjust_directories(&key, 1, &mut directory_deltas, &mut directory_count)?;
            data_bytes = data_bytes.checked_add(entry.size).ok_or(StorageError::TooLarge)?;
            entry_count = entry_count.checked_add(1).ok_or(StorageError::TooLarge)?;
            changes.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
            changes.insert(key, Some(Arc::clone(entry)));
            let stamp = entry.last_use.load(Ordering::Relaxed);
            new_order.insert((stamp, key));
            new_stamps.insert(key, stamp);
            while !self.fits(data_bytes, entry_count, directory_count, self.size_limit)? {
                if eviction_reasons.len() >= MAX_EVICTIONS_PER_BATCH {
                    return Err(StorageError::TooLarge);
                }
                let VictimSearch::Found(victim, reason) = find_victim(
                    &self.lru,
                    &mut new_order,
                    &mut new_stamps,
                    Some(key),
                    &changes,
                    &entries,
                    protection,
                    self.read_state.policy.as_ref(),
                    now,
                    &mut self.eviction_scan_cursor,
                ) else {
                    return Err(StorageError::TooLarge);
                };
                let previous = effective_entry(&victim, &changes, &entries).ok_or(StorageError::TooLarge)?;
                self.adjust_directories(&victim, -1, &mut directory_deltas, &mut directory_count)?;
                data_bytes = data_bytes.checked_sub(previous.size).ok_or(StorageError::TooLarge)?;
                entry_count = entry_count.saturating_sub(1);
                changes.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
                changes.insert(victim, None);
                eviction_reasons.insert(victim, reason);
            }
        }
        Ok(Plan { changes, directory_deltas, eviction_reasons })
    }

    fn adjust_directories(
        &self,
        key: &Key,
        amount: i64,
        deltas: &mut HashMap<DirectoryId, i64>,
        directory_count: &mut u64,
    ) -> Result<(), StorageError> {
        let Some((prefixes, shard)) = self.read_state.namespaces.directories(*key) else {
            return Err(StorageError::UnknownNamespace);
        };
        for directory in prefixes.iter().copied().chain(std::iter::once(shard)) {
            let current_delta = deltas.get(&directory).copied().unwrap_or(0);
            let existing = self.directory_counts.get(&directory).copied().unwrap_or(0);
            let current =
                i64::try_from(existing).map_err(|_| StorageError::TooLarge)?.checked_add(current_delta).ok_or(StorageError::TooLarge)?;
            let next = current.checked_add(amount).ok_or(StorageError::TooLarge)?;
            if next < 0 {
                return Err(StorageError::TooLarge);
            }
            if current == 0 && next > 0 {
                *directory_count = directory_count.checked_add(1).ok_or(StorageError::TooLarge)?;
            } else if current > 0 && next == 0 {
                *directory_count = directory_count.checked_sub(1).ok_or(StorageError::TooLarge)?;
            }
            deltas.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
            deltas.insert(directory, current_delta.checked_add(amount).ok_or(StorageError::TooLarge)?);
        }
        Ok(())
    }

    fn write_data(&self, key: Key, bytes: &[u8]) -> Result<StagedData, StorageError> {
        let destination = self.read_state.namespaces.path(&self.root, key).ok_or(StorageError::UnknownNamespace)?;
        let parent = destination
            .parent()
            .ok_or_else(|| StorageError::Io(io::Error::new(io::ErrorKind::InvalidInput, "cache key has no shard directory")))?;
        fs::create_dir_all(parent)?;
        let mut temp = Builder::new().prefix(DATA_TEMP_PREFIX).tempfile_in(parent)?;
        if let Err(error) = self.hooks.check(WritePoint::DataTempCreated) {
            return Err(fail_temporary(temp, error));
        }
        self.hooks.check(WritePoint::DataWriteFailure).map_err(StorageError::Io)?;
        if let Err(error) = self.hooks.write_data_payload(&mut temp, bytes) {
            return Err(fail_temporary(temp, error));
        }
        if let Err(error) = self.hooks.check(WritePoint::DataWritten) {
            return Err(fail_temporary(temp, error));
        }
        let stamp = super::saturating_increment(&self.read_state.usage_clock);
        let size = u64::try_from(bytes.len()).map_err(|_| StorageError::TooLarge)?;
        let entry = Arc::new(Entry {
            key,
            size,
            hash: Sha256::digest(bytes).into(),
            last_use: std::sync::atomic::AtomicU64::new(stamp),
            use_count: std::sync::atomic::AtomicU64::new(0),
            persisted_use_count: std::sync::atomic::AtomicU64::new(0),
            state: std::sync::atomic::AtomicU8::new(ENTRY_VALID),
        });
        Ok(StagedData { entry, temp })
    }

    fn publish_and_commit(
        &mut self,
        changes: Vec<PlannedChange>,
        staged: Vec<StagedData>,
        eviction_marks: Vec<Arc<Entry>>,
    ) -> Result<HashSet<Key>, CommitFailure> {
        let mut staged_files = Vec::new();
        if staged_files.try_reserve_exact(staged.len()).is_err() {
            return Err(self.commit_failure_before_commit(&eviction_marks, StorageError::Unavailable(UnavailableReason::Allocation)));
        }
        for data in staged {
            let Some(destination) = self.read_state.namespaces.path(&self.root, data.entry.key) else {
                return Err(self.commit_failure_before_commit(&eviction_marks, StorageError::UnknownNamespace));
            };
            staged_files.push((data, destination));
        }

        let mut batches = Vec::new();
        if batches.try_reserve_exact(changes.len().div_ceil(MAX_BATCH_ROWS)).is_err() {
            return Err(self.commit_failure_before_commit(&eviction_marks, StorageError::Unavailable(UnavailableReason::Allocation)));
        }
        for chunk in changes.chunks(MAX_BATCH_ROWS) {
            match mutations(chunk) {
                Ok(batch) => batches.push(batch),
                Err(error) => return Err(self.commit_failure_before_commit(&eviction_marks, error)),
            }
        }

        let mut renamed = HashSet::new();
        if renamed.try_reserve(staged_files.len()).is_err() {
            return Err(self.commit_failure_before_commit(&eviction_marks, StorageError::Unavailable(UnavailableReason::Allocation)));
        }
        let mut committed = HashSet::new();
        if committed.try_reserve(changes.len()).is_err() {
            return Err(self.commit_failure_before_commit(&eviction_marks, StorageError::Unavailable(UnavailableReason::Allocation)));
        }
        let replacing = match self.begin_replacements(&changes) {
            Ok(replacing) => replacing,
            Err(error) => return Err(self.commit_failure_before_commit(&eviction_marks, error)),
        };
        for (data, destination) in staged_files {
            if let Err(error) = persist_with_retry(data.temp, &destination).map_err(StorageError::Io) {
                self.finish_failed_replacements(&replacing, &renamed, &changes, &committed, is_simulated_crash(&error), &eviction_marks);
                return Err(CommitFailure { committed, error });
            }
            renamed.insert(data.entry.key);
            if let Err(error) = self.hooks.check(WritePoint::DataRenamed).map_err(StorageError::Io) {
                self.finish_failed_replacements(&replacing, &renamed, &changes, &committed, is_simulated_crash(&error), &eviction_marks);
                return Err(CommitFailure { committed, error });
            }
        }

        for (chunk, mutations) in changes.chunks(MAX_BATCH_ROWS).zip(batches) {
            if let Err(error) = self.hooks.check(WritePoint::IndexBatchStarted).map_err(StorageError::Io) {
                self.finish_failed_replacements(&replacing, &renamed, &changes, &committed, is_simulated_crash(&error), &eviction_marks);
                return Err(CommitFailure { committed, error });
            }
            #[cfg(test)]
            if let Err(error) = self.hooks.check(WritePoint::IndexBatchFailure).map_err(StorageError::Io) {
                self.finish_failed_replacements(&replacing, &renamed, &changes, &committed, false, &eviction_marks);
                return Err(CommitFailure { committed, error });
            }
            if let Err(error) = self.index.apply(mutations) {
                let recreated = matches!(&error, SqliteError::Corrupt | SqliteError::SchemaMismatch);
                let storage_error = self.index_failure(error, &changes);
                if recreated {
                    committed.clear();
                }
                self.finish_failed_replacements(&replacing, &renamed, &changes, &committed, false, &eviction_marks);
                return Err(CommitFailure { committed, error: storage_error });
            }
            self.publish_changes(chunk);
            for change in chunk {
                committed.insert(change.key);
            }
            if let Err(error) = self.hooks.check(WritePoint::IndexBatchCommitted).map_err(StorageError::Io) {
                self.finish_failed_replacements(&replacing, &renamed, &changes, &committed, is_simulated_crash(&error), &eviction_marks);
                return Err(CommitFailure { committed, error });
            }
        }
        Ok(committed)
    }

    fn commit_failure_before_commit(&self, eviction_marks: &[Arc<Entry>], error: StorageError) -> CommitFailure {
        self.rollback_evictions(eviction_marks, &HashSet::new());
        CommitFailure::before_commit(error)
    }
    fn prepare_changes(&mut self, plan: &Plan, protection: &mut ProtectionRegistry) -> Result<Vec<PlannedChange>, StorageError> {
        let mut changes = Vec::new();
        changes.try_reserve_exact(plan.changes.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let entries = read_map(&self.read_state.entries);
        for (key, next) in &plan.changes {
            changes.push(PlannedChange {
                key: *key,
                previous: entries.get(key).cloned(),
                next: next.clone(),
                eviction_reason: plan.eviction_reasons.get(key).copied(),
            });
        }
        drop(entries);
        sort_changes(&mut changes);
        let additional = changes.iter().filter(|change| change.next.is_some()).count();
        let mut entries = write_map(&self.read_state.entries);
        entries.try_reserve(additional).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        drop(entries);
        self.referenced_keys.try_reserve(additional).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        self.directory_counts
            .try_reserve(plan.directory_deltas.len())
            .map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        self.lru.try_reserve(additional).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        protection.sizes.try_reserve(additional).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        Ok(changes)
    }

    fn begin_replacements(&self, changes: &[PlannedChange]) -> Result<Vec<Arc<Entry>>, StorageError> {
        let count = changes.iter().filter(|change| change.next.is_some() && change.previous.is_some()).count();
        let mut replacing = Vec::new();
        replacing.try_reserve_exact(count).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let mut invalid = lock(&self.read_state.invalid);
        invalid.try_reserve(count).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        drop(invalid);
        for change in changes {
            let Some(previous) = &change.previous else {
                continue;
            };
            if change.next.is_none() {
                continue;
            }
            match previous.state.compare_exchange(ENTRY_VALID, ENTRY_REPLACING, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => replacing.push(Arc::clone(previous)),
                Err(ENTRY_INVALID) => {}
                Err(_) => {
                    for entry in &replacing {
                        entry.state.store(ENTRY_VALID, Ordering::Release);
                    }
                    return Err(StorageError::Unavailable(UnavailableReason::Io));
                }
            }
        }
        Ok(replacing)
    }

    fn finish_failed_replacements(
        &self,
        replacing: &[Arc<Entry>],
        renamed: &HashSet<Key>,
        changes: &[PlannedChange],
        committed: &HashSet<Key>,
        crashed: bool,
        eviction_marks: &[Arc<Entry>],
    ) {
        let entries = read_map(&self.read_state.entries);
        let mut invalid = lock(&self.read_state.invalid);
        for entry in replacing {
            if committed.contains(&entry.key) || !entries.get(&entry.key).is_some_and(|current| Arc::ptr_eq(current, entry)) {
                continue;
            }
            if renamed.contains(&entry.key) {
                entry.state.store(ENTRY_INVALID, Ordering::Release);
                invalid.insert(entry.key, Arc::clone(entry));
            } else {
                entry.state.store(ENTRY_VALID, Ordering::Release);
            }
        }
        drop(invalid);
        drop(entries);
        self.rollback_evictions(eviction_marks, committed);
        if !crashed {
            for change in changes {
                if !committed.contains(&change.key) && change.next.is_some() && renamed.contains(&change.key) {
                    delete_data(&self.root, &self.read_state.namespaces, &change.key);
                }
            }
        }
    }

    fn rollback_evictions(&self, eviction_marks: &[Arc<Entry>], committed: &HashSet<Key>) {
        let protection_shared = Arc::clone(&self.read_state.protection);
        let mut protection = lock(&protection_shared.state);
        for entry in eviction_marks.iter().filter(|entry| !committed.contains(&entry.key)) {
            protection.restore_eviction(entry, &protection_shared.bytes);
        }
    }

    fn mark_evictions(&self, changes: &[PlannedChange], protection: &mut ProtectionRegistry) -> Result<Vec<Arc<Entry>>, StorageError> {
        let count = changes.iter().filter(|change| change.eviction_reason.is_some()).count();
        let mut marked: Vec<Arc<Entry>> = Vec::new();
        marked.try_reserve_exact(count).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        for change in changes.iter().filter(|change| change.eviction_reason.is_some()) {
            let Some(entry) = &change.previous else {
                continue;
            };
            if !protection.begin_eviction(entry) {
                for entry in &marked {
                    protection.restore_eviction(entry, &self.read_state.protection.bytes);
                }
                return Err(StorageError::Unavailable(UnavailableReason::Io));
            }
            marked.push(Arc::clone(entry));
        }
        Ok(marked)
    }

    fn publish_changes(&mut self, changes: &[PlannedChange]) {
        {
            let mut entries = write_map(&self.read_state.entries);
            for change in changes {
                if let Some(entry) = &change.next {
                    entries.insert(change.key, Arc::clone(entry));
                } else {
                    entries.remove(&change.key);
                }
            }
        }
        let protection_shared = Arc::clone(&self.read_state.protection);
        {
            let mut protection = lock(&protection_shared.state);
            for change in changes {
                protection.update_size(change.key, change.next.as_ref().map(|entry| entry.size), &protection_shared.bytes);
            }
        }
        for change in changes {
            if let Some(previous) = &change.previous {
                self.update_namespace_stats(change.key, true, false);
                self.lru.remove(&change.key);
                self.referenced_keys.remove(&change.key);
                self.data_bytes = self.data_bytes.saturating_sub(previous.size);
                self.entry_count = self.entry_count.saturating_sub(1);
                self.adjust_live_directories(&change.key, -1);
            }
            if let Some(entry) = &change.next {
                self.update_namespace_stats(change.key, false, true);
                self.lru.insert(change.key, entry.last_use.load(Ordering::Relaxed));
                self.referenced_keys.insert(change.key);
                self.data_bytes = self.data_bytes.saturating_add(entry.size);
                self.entry_count = self.entry_count.saturating_add(1);
                self.adjust_live_directories(&change.key, 1);
            } else {
                delete_data(&self.root, &self.read_state.namespaces, &change.key);
            }
            if let Some(reason) = change.eviction_reason {
                self.read_state.evictions.record(reason);
            }
        }
        self.update_accounted_size();
        self.blocked_protection_generation = None;
    }

    fn update_namespace_stats(&self, key: Key, previous: bool, next: bool) {
        let index = usize::from(key.namespace().0);
        if previous && let Some(count) = self.read_state.namespace_entry_counts.get(index) {
            count.fetch_sub(1, Ordering::Relaxed);
        }
        if next && let Some(count) = self.read_state.namespace_entry_counts.get(index) {
            count.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn adjust_live_directories(&mut self, key: &Key, amount: i8) {
        let Some((prefixes, shard)) = self.read_state.namespaces.directories(*key) else {
            tracing::error!("cache directory accounting invariant failed");
            return;
        };
        for directory in prefixes.iter().copied().chain(std::iter::once(shard)) {
            if amount > 0 {
                let count = self.directory_counts.entry(directory).or_default();
                *count = count.saturating_add(1);
            } else if let Some(count) = self.directory_counts.get_mut(&directory) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.directory_counts.remove(&directory);
                }
            }
        }
        self.directory_count = u64::try_from(self.directory_counts.len()).unwrap_or(u64::MAX);
    }

    fn remove(&mut self, key: Key) -> Result<bool, StorageError> {
        self.drain_touches_best_effort();
        let entry = read_map(&self.read_state.entries).get(&key).cloned();
        let Some(entry) = entry else {
            return Ok(false);
        };
        let change = PlannedChange { key, previous: Some(entry), next: None, eviction_reason: None };
        self.commit_changes(std::slice::from_ref(&change), &[])?;
        Ok(true)
    }

    fn remove_invalid(&mut self, key: Key, target: Arc<Entry>) -> Result<(), StorageError> {
        let current = read_map(&self.read_state.entries).get(&key).cloned();
        if !current.as_ref().is_some_and(|entry| Arc::ptr_eq(entry, &target)) {
            return Ok(());
        }
        let change = PlannedChange { key, previous: Some(target), next: None, eviction_reason: None };
        self.commit_changes(std::slice::from_ref(&change), &[])
    }

    fn commit_changes(&mut self, changes: &[PlannedChange], eviction_marks: &[Arc<Entry>]) -> Result<(), StorageError> {
        let mut committed = HashSet::new();
        if committed.try_reserve(changes.len()).is_err() {
            self.rollback_evictions(eviction_marks, &committed);
            return Err(StorageError::Unavailable(UnavailableReason::Allocation));
        }
        for chunk in changes.chunks(MAX_BATCH_ROWS) {
            let mutations = match mutations(chunk) {
                Ok(mutations) => mutations,
                Err(error) => {
                    self.rollback_evictions(eviction_marks, &committed);
                    return Err(error);
                }
            };
            if let Err(error) = self.index.apply(mutations) {
                let recreated = matches!(error, SqliteError::Corrupt | SqliteError::SchemaMismatch);
                let storage_error = self.index_failure(error, changes);
                if !recreated {
                    self.rollback_evictions(eviction_marks, &committed);
                }
                return Err(storage_error);
            }
            self.publish_changes(chunk);
            committed.extend(chunk.iter().map(|change| change.key));
        }
        Ok(())
    }

    fn index_failure(&mut self, error: SqliteError, changes: &[PlannedChange]) -> StorageError {
        let recreated = matches!(error, SqliteError::Corrupt | SqliteError::SchemaMismatch);
        let storage_error = super::index::map_sqlite_error(error);
        if recreated {
            self.reset_after_index_recreation(changes);
        }
        storage_error
    }

    fn reset_after_index_recreation(&mut self, changes: &[PlannedChange]) {
        let old = {
            let mut entries = write_map(&self.read_state.entries);
            std::mem::take(&mut *entries)
        };
        let protection_shared = Arc::clone(&self.read_state.protection);
        let mut protection = lock(&protection_shared.state);
        for key in old.keys() {
            self.update_namespace_stats(*key, true, false);
            protection.update_size(*key, None, &protection_shared.bytes);
        }
        drop(protection);
        self.lru.clear();
        self.referenced_keys.clear();
        self.directory_counts.clear();
        self.directory_count = 0;
        self.data_bytes = 0;
        self.entry_count = 0;
        lock(&self.read_state.invalid).clear();
        lock(&self.read_state.touches).clear();
        for key in old.keys().chain(changes.iter().map(|change| &change.key)) {
            delete_data(&self.root, &self.read_state.namespaces, key);
        }
        self.update_accounted_size();
    }

    fn clear(&mut self) -> Result<(), StorageError> {
        self.drain_touches_best_effort();
        if let Err(error) = self.index.clear() {
            return Err(self.index_failure(error, &[]));
        }
        let old = {
            let mut entries = write_map(&self.read_state.entries);
            std::mem::take(&mut *entries)
        };
        let protection_shared = Arc::clone(&self.read_state.protection);
        let mut protection = lock(&protection_shared.state);
        for key in old.keys() {
            self.update_namespace_stats(*key, true, false);
            protection.update_size(*key, None, &protection_shared.bytes);
        }
        drop(protection);
        self.lru.clear();
        self.referenced_keys.clear();
        self.directory_counts.clear();
        self.directory_count = 0;
        self.data_bytes = 0;
        self.entry_count = 0;
        lock(&self.read_state.invalid).clear();
        lock(&self.read_state.touches).clear();
        for key in old.keys() {
            delete_data(&self.root, &self.read_state.namespaces, key);
        }
        self.update_accounted_size();
        Ok(())
    }

    fn set_limit(&mut self, limit: u64) -> Result<(), StorageError> {
        if limit < self.read_state.minimum_size_limit {
            return Err(StorageError::LimitTooSmall);
        }
        self.drain_touches_best_effort();
        self.blocked_protection_generation = None;
        self.enforce_limit(limit)?;
        self.size_limit = limit;
        self.read_state.size_limit.store(limit, Ordering::Relaxed);
        Ok(())
    }

    fn enforce_limit(&mut self, limit: u64) -> Result<(), StorageError> {
        self.drain_touches_best_effort();
        if self.accounted_size() <= limit {
            self.blocked_protection_generation = None;
            return Ok(());
        }
        let prepared = {
            let protection_shared = Arc::clone(&self.read_state.protection);
            let mut protection = lock(&protection_shared.state);
            if self.blocked_protection_generation == Some(protection.generation) {
                return Ok(());
            }
            let entries = read_map(&self.read_state.entries);
            let mut changes = HashMap::<Key, Option<Arc<Entry>>>::new();
            let mut eviction_reasons = HashMap::new();
            eviction_reasons.try_reserve(MAX_EVICTIONS_PER_BATCH).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
            let mut directory_deltas = HashMap::<DirectoryId, i64>::new();
            let mut data_bytes = self.data_bytes;
            let mut directory_count = self.directory_count;
            let mut entry_count = self.entry_count;
            let mut no_new_order = BTreeSet::new();
            let mut no_new_stamps = HashMap::new();
            let now = self.read_state.usage_clock.load(Ordering::Relaxed);
            let mut scan_exhausted = false;
            while !self.fits(data_bytes, entry_count, directory_count, limit)? && eviction_reasons.len() < MAX_EVICTIONS_PER_BATCH {
                let (key, reason) = match find_victim(
                    &self.lru,
                    &mut no_new_order,
                    &mut no_new_stamps,
                    None,
                    &changes,
                    &entries,
                    &protection,
                    self.read_state.policy.as_ref(),
                    now,
                    &mut self.eviction_scan_cursor,
                ) {
                    VictimSearch::Found(key, reason) => (key, reason),
                    VictimSearch::Exhausted => {
                        scan_exhausted = true;
                        break;
                    }
                    VictimSearch::Deferred => break,
                };
                let entry = effective_entry(&key, &changes, &entries).ok_or(StorageError::TooLarge)?;
                self.adjust_directories(&key, -1, &mut directory_deltas, &mut directory_count)?;
                data_bytes = data_bytes.checked_sub(entry.size).ok_or(StorageError::TooLarge)?;
                entry_count = entry_count.saturating_sub(1);
                changes.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
                changes.insert(key, None);
                eviction_reasons.insert(key, reason);
            }
            drop(entries);
            if changes.is_empty() {
                self.blocked_protection_generation = (self.accounted_size() > limit && scan_exhausted).then_some(protection.generation);
                return Ok(());
            }
            let plan = Plan { changes, directory_deltas, eviction_reasons };
            let changes = self.prepare_changes(&plan, &mut protection)?;
            let eviction_marks = self.mark_evictions(&changes, &mut protection)?;
            self.blocked_protection_generation = None;
            Some((changes, eviction_marks))
        };
        if let Some((changes, eviction_marks)) = prepared { self.commit_changes(&changes, &eviction_marks) } else { Ok(()) }
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        self.drain_touches()?;
        Ok(())
    }

    fn shutdown(mut self) -> Result<(), StorageError> {
        self.drain_invalid();
        self.sweep_all();
        self.flush()?;
        self.index.shutdown().map_err(super::index::map_sqlite_error)
    }

    fn drain_touches_best_effort(&mut self) {
        if let Err(error) = self.drain_touches() {
            tracing::warn!(error = %error, "cache usage metadata could not be persisted");
        }
    }

    fn drain_touches(&mut self) -> Result<(), StorageError> {
        let touches = {
            let mut pending = lock(&self.read_state.touches);
            std::mem::take(&mut *pending)
        };
        if touches.is_empty() {
            return Ok(());
        }
        let entries = read_map(&self.read_state.entries);
        let mut pending_touches = Vec::new();
        if pending_touches.try_reserve_exact(touches.len()).is_err() {
            drop(entries);
            self.restore_touches(touches.into_values());
            return Err(StorageError::Unavailable(UnavailableReason::Allocation));
        }
        for (key, touched) in touches {
            if entries.get(&key).is_some_and(|entry| Arc::ptr_eq(entry, &touched.entry)) {
                self.lru.insert(key, touched.last_use);
                pending_touches.push(touched);
            }
        }
        drop(entries);
        let mut offset = 0;
        while offset < pending_touches.len() {
            let end = (offset + MAX_BATCH_ROWS).min(pending_touches.len());
            let mut batch = Vec::new();
            if batch.try_reserve_exact(end - offset).is_err() {
                self.restore_touches(pending_touches.drain(offset..));
                return Err(StorageError::Unavailable(UnavailableReason::Allocation));
            }
            batch.extend(pending_touches[offset..end].iter().map(PendingTouch::record));
            #[cfg(test)]
            let result =
                if self.hooks.check(WritePoint::IndexTouchBusy).is_err() { Err(SqliteError::Busy) } else { self.index.touch(batch) };
            #[cfg(not(test))]
            let result = self.index.touch(batch);
            if let Err(error) = result {
                self.restore_touches(pending_touches.drain(offset..));
                return Err(self.index_failure(error, &[]));
            }
            for touched in &pending_touches[offset..end] {
                touched.entry.persisted_use_count.store(touched.resulting_use_count(), Ordering::Relaxed);
            }
            offset = end;
        }
        self.update_accounted_size();
        Ok(())
    }

    fn restore_touches(&self, touches: impl IntoIterator<Item = PendingTouch>) {
        let entries = read_map(&self.read_state.entries);
        let mut pending = lock(&self.read_state.touches);
        for touched in touches {
            let key = touched.entry.key;
            if !entries.get(&key).is_some_and(|entry| Arc::ptr_eq(entry, &touched.entry)) {
                continue;
            }
            if let Some(current) = pending.get_mut(&key) {
                if Arc::ptr_eq(&current.entry, &touched.entry) {
                    current.last_use = current.last_use.max(touched.last_use);
                    current.use_count_delta = current.use_count_delta.saturating_add(touched.use_count_delta);
                } else {
                    *current = touched;
                }
            } else {
                pending.insert(key, touched);
            }
        }
        drop(pending);
        drop(entries);
    }

    fn drain_invalid(&mut self) {
        let invalid = {
            let mut pending = lock(&self.read_state.invalid);
            std::mem::take(&mut *pending)
        };
        for (key, entry) in invalid {
            if let Err(error) = self.remove_invalid(key, Arc::clone(&entry)) {
                tracing::warn!(error = %error, "failed to remove a corrupt cache entry");
                let current = read_map(&self.read_state.entries).get(&key).cloned();
                if current.as_ref().is_some_and(|current| Arc::ptr_eq(current, &entry))
                    && current.is_some_and(|current| current.state.load(Ordering::Acquire) == ENTRY_INVALID)
                {
                    lock(&self.read_state.invalid).insert(key, entry);
                }
            }
        }
    }

    fn fits(&self, data_bytes: u64, entry_count: usize, directory_count: u64, limit: u64) -> Result<bool, StorageError> {
        if entry_count > super::index::MAX_ENTRIES {
            return Ok(false);
        }
        let index_bytes = self.index.estimated_bytes(entry_count)?;
        Ok(account_size(index_bytes, data_bytes, entry_count, directory_count).ok_or(StorageError::TooLarge)? <= limit)
    }

    fn update_accounted_size(&self) {
        self.read_state.accounted_size.store(self.accounted_size(), Ordering::Relaxed);
    }

    fn sweep_step(&mut self, limit: usize) -> bool {
        if self.sweep_complete {
            return true;
        }
        if self.sweep.is_empty() {
            match fs::read_dir(&self.root) {
                Ok(read_dir) => self.sweep.push((self.root.clone(), read_dir)),
                Err(error) => {
                    tracing::warn!(error = %error, "cache orphan sweep could not open its root");
                    self.sweep_complete = true;
                    return true;
                }
            }
        }
        let mut processed = 0usize;
        while processed < limit {
            let next = self.sweep.last_mut().and_then(|(_, entries)| entries.next());
            match next {
                Some(Ok(item)) => {
                    processed += 1;
                    let path = item.path();
                    let file_type = match item.file_type() {
                        Ok(file_type) => file_type,
                        Err(error) => {
                            tracing::warn!(error = %error, "cache orphan sweep could not inspect a path");
                            continue;
                        }
                    };
                    if file_type.is_dir() {
                        if self.sweep.len() >= MAX_SWEEP_DEPTH {
                            tracing::warn!(path = %path.display(), "cache orphan sweep reached its directory depth limit");
                            continue;
                        }
                        match fs::read_dir(&path) {
                            Ok(read_dir) if self.sweep.try_reserve(1).is_ok() => self.sweep.push((path, read_dir)),
                            Ok(_) => tracing::warn!("cache orphan sweep could not grow its directory stack"),
                            Err(error) => tracing::warn!(error = %error, "cache orphan sweep could not open a directory"),
                        }
                    } else {
                        let is_index = is_index_file(&self.root, &path);
                        let referenced = path
                            .strip_prefix(&self.root)
                            .ok()
                            .and_then(|relative| self.read_state.namespaces.key_from_relative_path(relative))
                            .is_some_and(|key| self.referenced_keys.contains(&key));
                        if !is_index
                            && !referenced
                            && let Err(error) = remove_path(&path)
                        {
                            tracing::warn!(error = %error, "cache orphan sweep could not remove a file");
                        }
                    }
                }
                Some(Err(error)) => {
                    processed += 1;
                    tracing::warn!(error = %error, "cache orphan sweep could not read a directory entry");
                }
                None => {
                    let popped = self.sweep.pop();
                    if let Some((directory, _)) = popped
                        && directory != self.root
                    {
                        match fs::remove_dir(directory) {
                            Ok(()) => {}
                            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty) => {}
                            Err(error) => tracing::warn!(error = %error, "cache orphan sweep could not remove an empty directory"),
                        }
                    }
                    if self.sweep.is_empty() {
                        self.sweep_complete = true;
                        return true;
                    }
                }
            }
        }
        false
    }

    fn sweep_all(&mut self) {
        while !self.sweep_step(QUEUE_SWEEP_CHUNK) {}
    }
}
fn find_victim(
    lru: &Lru,
    new_order: &mut BTreeSet<(u64, Key)>,
    new_stamps: &mut HashMap<Key, u64>,
    excluded: Option<Key>,
    changes: &HashMap<Key, Option<Arc<Entry>>>,
    entries: &HashMap<Key, Arc<Entry>>,
    protection: &ProtectionRegistry,
    policy: &dyn EvictionPolicy,
    now: u64,
    scan_cursor: &mut Option<(u64, Key)>,
) -> VictimSearch {
    for _ in 0..MAX_EVICTION_SCAN_PASSES {
        match scan_victim(lru, new_order, new_stamps, excluded, changes, entries, protection, policy, now, scan_cursor) {
            VictimSearch::Found(key, reason) => return VictimSearch::Found(key, reason),
            VictimSearch::Exhausted => return VictimSearch::Exhausted,
            VictimSearch::Deferred => {}
        }
    }
    VictimSearch::Deferred
}

fn scan_victim(
    lru: &Lru,
    new_order: &mut BTreeSet<(u64, Key)>,
    new_stamps: &mut HashMap<Key, u64>,
    excluded: Option<Key>,
    changes: &HashMap<Key, Option<Arc<Entry>>>,
    entries: &HashMap<Key, Arc<Entry>>,
    protection: &ProtectionRegistry,
    policy: &dyn EvictionPolicy,
    now: u64,
    scan_cursor: &mut Option<(u64, Key)>,
) -> VictimSearch {
    let mut base_scan = lru.scan_after(*scan_cursor).peekable();
    let mut new_scan = OrderedScan::new(new_order, *scan_cursor).peekable();
    let mut selected: Option<(u128, u64, Key, EvictionReason, bool)> = None;
    let mut inspected = 0usize;
    while inspected < MAX_EVICTION_CANDIDATES {
        let next_base = base_scan.peek().copied();
        let next_new = new_scan.peek().copied();
        let (stamp, key, is_new) = match (next_base, next_new) {
            (Some(base_candidate), Some(new_candidate)) if base_candidate <= new_candidate => {
                base_scan.next();
                (base_candidate.0, base_candidate.1, false)
            }
            (_, Some(new_candidate)) => {
                new_scan.next();
                (new_candidate.0, new_candidate.1, true)
            }
            (Some(base_candidate), None) => {
                base_scan.next();
                (base_candidate.0, base_candidate.1, false)
            }
            (None, None) => break,
        };
        inspected += 1;
        *scan_cursor = Some((stamp, key));
        let stale = if is_new {
            changes.get(&key).is_some_and(Option::is_none) || effective_entry(&key, changes, entries).is_none()
        } else {
            changes.contains_key(&key) || effective_entry(&key, changes, entries).is_none()
        };
        if stale || Some(key) == excluded || protection.is_active(&key) {
            continue;
        }
        let Some(entry) = effective_entry(&key, changes, entries) else {
            continue;
        };
        let candidate = EvictionCandidate {
            key,
            size: entry.size,
            last_use: entry.last_use.load(Ordering::Relaxed),
            use_count: entry.use_count.load(Ordering::Relaxed),
        };
        let score = policy.score(candidate, now);
        let rank = (score.value, stamp, key, score.reason, is_new);
        if selected.is_none_or(|current| (rank.0, rank.1, rank.2) < (current.0, current.1, current.2)) {
            selected = Some(rank);
        }
    }
    let has_more = base_scan.peek().is_some() || new_scan.peek().is_some();
    let Some((_, _, key, reason, is_new)) = selected else {
        return if has_more { VictimSearch::Deferred } else { VictimSearch::Exhausted };
    };
    drop(new_scan);
    drop(base_scan);
    if is_new {
        remove_new_candidate(&key, new_order, new_stamps);
    }
    VictimSearch::Found(key, reason)
}
