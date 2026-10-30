mod disk;
mod eviction;
mod queue;
mod support;

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, ReadDir},
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    time::Instant,
};

use sha2::{Digest, Sha256};
use tempfile::{Builder, NamedTempFile};
use utils::persist_atomic;

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
    EvictionReason,
    cache::eviction::Lru,
    sqlite::{MAX_BATCH_ROWS, SqliteError},
};
use disk::{DATA_TEMP_PREFIX, delete_data, fail_temporary, remove_path};
use support::{clone_error, is_index_file, is_simulated_crash, lock, mutations, read_map, simulated_crash_error, write_map};

pub(in crate::cache) use queue::{Command, WriteQueue, writer_loop};

const QUEUE_SWEEP_CHUNK: usize = 64;
const MAX_SWEEP_DEPTH: usize = 32;

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
    SetLimitQueued,
    #[cfg(test)]
    WriterReady,
    #[cfg(test)]
    SetLimitReplied,
    #[cfg(test)]
    MaintenanceStarted,
    #[cfg(test)]
    MaintenanceFinished,
    #[cfg(test)]
    MaintenanceCycleComplete,
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
    time: Mutex<Option<Instant>>,
    #[cfg(test)]
    fail_at: AtomicUsize,
    #[cfg(test)]
    fail_after: AtomicUsize,
    #[cfg(test)]
    pause: Mutex<Vec<TestPause>>,
}

impl FaultHooks {
    fn now(&self) -> Instant {
        #[cfg(test)]
        if let Some(now) = *lock(&self.time) {
            return now;
        }
        Instant::now()
    }

    #[cfg(test)]
    pub(super) fn set_time(&self, now: Instant) {
        *lock(&self.time) = Some(now);
    }

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
    evictions_this_batch: usize,
    size_limit: u64,
    blocked_protection_generation: Option<u64>,
    hooks: Arc<FaultHooks>,
    sweep: Vec<(PathBuf, ReadDir)>,
    sweep_complete: bool,
    invalid_cleanup_failures: usize,
    touch_flush_failures: usize,
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
            evictions_this_batch: 0,
            blocked_protection_generation: None,
            hooks,
            sweep: Vec::new(),
            sweep_complete: false,
            invalid_cleanup_failures: 0,
            touch_flush_failures: 0,
        };
        state.update_accounted_size();
        Ok(state)
    }

    fn put_batch<'a>(&mut self, total: usize, puts: impl Iterator<Item = (Key, &'a [u8])>) -> Vec<Result<(), StorageError>> {
        self.drain_touches_best_effort();
        let mut staged = Vec::<StagedData>::new();
        let mut staged_indices = Vec::new();
        if staged.try_reserve(total).is_err() || staged_indices.try_reserve(total).is_err() {
            return (0..total).map(|_| Err(StorageError::Unavailable(UnavailableReason::Allocation))).collect();
        }
        let mut results = std::iter::repeat_with(|| None).take(total).collect::<Vec<Option<Result<(), StorageError>>>>();
        for (index, (key, bytes)) in puts.enumerate() {
            match self.single_put_fits_empty(key, bytes.len()) {
                Ok(true) => {}
                Ok(false) => {
                    results[index] = Some(Err(StorageError::TooLarge));
                    continue;
                }
                Err(error) => {
                    results[index] = Some(Err(error));
                    continue;
                }
            }
            match self.write_data(key, bytes) {
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
        let mut accepted_staged = Vec::new();
        let mut accepted_entries = Vec::new();
        let mut accepted_indices = Vec::new();
        if accepted_staged.try_reserve_exact(staged.len()).is_err()
            || accepted_entries.try_reserve_exact(staged.len()).is_err()
            || accepted_indices.try_reserve_exact(staged.len()).is_err()
        {
            for index in staged_indices {
                if let Some(result) = results.get_mut(index) {
                    *result = Some(Err(StorageError::Unavailable(UnavailableReason::Allocation)));
                }
            }
            return results.into_iter().map(|result| result.unwrap_or(Ok(()))).collect();
        }
        let prepared = {
            let protection_shared = Arc::clone(&self.read_state.protection);
            let mut protection = lock(&protection_shared.state);
            self.plan_puts(&staged_entries, &protection).and_then(|put_plan| {
                for index in &put_plan.rejected {
                    if let Some(result) = results.get_mut(staged_indices[*index]) {
                        *result = Some(Err(StorageError::TooLarge));
                    }
                }
                let accepted = put_plan.accepted;
                for index in &accepted {
                    accepted_entries.push(Arc::clone(&staged_entries[*index]));
                    accepted_indices.push(staged_indices[*index]);
                }
                let Some(plan) = put_plan.plan else {
                    return Ok(None);
                };
                let changes = self.prepare_changes(&plan, &mut protection)?;
                let eviction_marks = self.mark_evictions(&changes, &mut protection)?;
                Ok(Some((changes, eviction_marks, accepted)))
            })
        };
        let (changes, eviction_marks, accepted_stage_indices) = match prepared {
            Ok(Some(prepared)) => prepared,
            Ok(None) => return results.into_iter().map(|result| result.unwrap_or(Ok(()))).collect(),
            Err(error) => {
                for index in staged_indices {
                    if let Some(result) = results.get_mut(index)
                        && result.is_none()
                    {
                        *result = Some(Err(clone_error(&error)));
                    }
                }
                return results.into_iter().map(|result| result.unwrap_or(Ok(()))).collect();
            }
        };
        let mut accepted_cursor = 0;
        for (index, data) in staged.into_iter().enumerate() {
            if accepted_stage_indices.get(accepted_cursor) == Some(&index) {
                accepted_staged.push(data);
                accepted_cursor += 1;
            }
        }
        let (committed, error) = match self.publish_and_commit(changes, accepted_staged, eviction_marks) {
            Ok(committed) => (committed, None),
            Err(failure) => (failure.committed, Some(failure.error)),
        };
        for (entry, index) in accepted_entries.iter().zip(accepted_indices.iter()) {
            if let Some(result) = results.get_mut(*index) {
                *result = Some(match error.as_ref() {
                    None => Ok(()),
                    Some(_) if committed.contains(&entry.key) => Ok(()),
                    Some(error) => Err(clone_error(error)),
                });
            }
        }
        results.into_iter().map(|result| result.unwrap_or(Ok(()))).collect()
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
            if let Err(error) = persist_atomic(data.temp, &destination).map_err(StorageError::Io) {
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
            #[cfg(test)]
            if let Err(error) = self.hooks.check(WritePoint::IndexBatchFailure).map_err(StorageError::Io) {
                self.rollback_evictions(eviction_marks, &committed);
                return Err(error);
            }
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
            #[cfg(test)]
            if let Err(error) = self.hooks.check(WritePoint::IndexBatchCommitted).map_err(StorageError::Io) {
                self.rollback_evictions(eviction_marks, &committed);
                return Err(error);
            }
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
        match self.drain_touches() {
            Err(error) => {
                if self.touch_flush_failures == 0 {
                    tracing::warn!(error = %error, "cache usage metadata could not be persisted; retrying");
                }
                self.touch_flush_failures = self.touch_flush_failures.saturating_add(1);
            }
            Ok(()) if self.touch_flush_failures != 0 => {
                tracing::debug!(failed_attempts = self.touch_flush_failures, "cache usage metadata persistence recovered");
                self.touch_flush_failures = 0;
            }
            Ok(()) => {}
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
        let mut failures = 0usize;
        let mut first_error = None;
        for (key, entry) in invalid {
            if let Err(error) = self.remove_invalid(key, Arc::clone(&entry)) {
                failures = failures.saturating_add(1);
                first_error.get_or_insert(error);
                let current = read_map(&self.read_state.entries).get(&key).cloned();
                if current.as_ref().is_some_and(|current| Arc::ptr_eq(current, &entry))
                    && current.is_some_and(|current| current.state.load(Ordering::Acquire) == ENTRY_INVALID)
                {
                    lock(&self.read_state.invalid).insert(key, entry);
                }
            }
        }
        if let Some(error) = first_error {
            if self.invalid_cleanup_failures == 0 {
                tracing::warn!(failed_removals = failures, error = %error, "corrupt cache entry cleanup failed; retrying");
            }
            self.invalid_cleanup_failures = self.invalid_cleanup_failures.saturating_add(failures);
        } else if self.invalid_cleanup_failures != 0 {
            tracing::debug!(failed_attempts = self.invalid_cleanup_failures, "corrupt cache entry cleanup recovered");
            self.invalid_cleanup_failures = 0;
        }
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
