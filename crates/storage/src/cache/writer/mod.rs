mod disk;
mod queue;

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs::{self, File, ReadDir},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock, atomic::Ordering, mpsc::SyncSender},
};

use sha2::{Digest, Sha256};
use tempfile::{Builder, NamedTempFile};

#[cfg(test)]
use std::sync::{
    atomic::AtomicUsize,
    mpsc::{self, Receiver},
};

use super::{
    DIRECTORY_OVERHEAD, DirectoryId, ENTRY_INVALID, ENTRY_REPLACING, ENTRY_VALID, Entry, FILE_OVERHEAD, INDEX_FILE, INDEX_FILE_OVERHEAD,
    Key, ReadState, StorageError, UnavailableReason,
    index::{Index, Mutation},
};
use crate::{
    cache::eviction::Lru,
    sqlite::{MAX_BATCH_ROWS, SqliteError},
};
use disk::{DATA_TEMP_PREFIX, delete_data, fail_temporary, persist_with_retry, remove_path};

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
    IndexBatchCommitted,
    #[cfg(test)]
    LookupEntrySelected,
    #[cfg(test)]
    LookupHashMismatch,
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
                let (kind, message) = if point == WritePoint::DataWriteFailure {
                    (io::ErrorKind::StorageFull, "simulated full disk")
                } else {
                    (io::ErrorKind::Interrupted, "simulated cache crash")
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
}

struct Plan {
    changes: HashMap<Key, Option<Arc<Entry>>>,
    directory_deltas: HashMap<DirectoryId, i64>,
}

struct StagedData {
    entry: Arc<Entry>,
    temp: NamedTempFile,
}

struct PlannedChange {
    key: Key,
    previous: Option<Arc<Entry>>,
    next: Option<Arc<Entry>>,
}

pub(super) struct WorkerState {
    root: PathBuf,
    _lock_file: File,
    index: Index,
    read_state: Arc<ReadState>,
    lru: Lru,
    referenced_keys: HashSet<Key>,
    directory_counts: HashMap<DirectoryId, usize>,
    directory_count: u64,
    data_bytes: u64,
    entry_count: usize,
    size_limit: u64,
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
        let mut state = Self {
            root,
            _lock_file: lock_file,
            index,
            size_limit: read_state.size_limit.load(Ordering::Relaxed),
            read_state,
            lru,
            referenced_keys,
            directory_counts,
            directory_count,
            data_bytes,
            entry_count,
            hooks,
            sweep: Vec::new(),
            sweep_complete: false,
        };
        state.update_accounted_size();
        if state.accounted_size() > state.size_limit {
            state.enforce_limit(state.size_limit)?;
            if state.accounted_size() > state.size_limit {
                return Err(StorageError::TooLarge);
            }
        }
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
        let plan = match self.plan_puts(&staged_entries) {
            Ok(plan) => plan,
            Err(error) => {
                for index in staged_indices {
                    if let Some(result) = results.get_mut(index) {
                        *result = Some(Err(clone_error(&error)));
                    }
                }
                return results.into_iter().map(|result| result.unwrap_or(Ok(()))).collect();
            }
        };
        match self.publish_and_commit(&plan, staged) {
            Ok(()) => {
                for index in staged_indices {
                    if let Some(result) = results.get_mut(index) {
                        *result = Some(Ok(()));
                    }
                }
            }
            Err(error) => {
                for index in staged_indices {
                    if let Some(result) = results.get_mut(index) {
                        *result = Some(Err(clone_error(&error)));
                    }
                }
            }
        }
        results.into_iter().map(|result| result.unwrap_or(Ok(()))).collect()
    }

    fn plan_puts(&self, staged: &[Arc<Entry>]) -> Result<Plan, StorageError> {
        let entries = read_map(&self.read_state.entries);
        let mut changes = HashMap::new();
        changes.try_reserve(staged.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let mut directory_deltas = HashMap::<DirectoryId, i64>::new();
        directory_deltas
            .try_reserve(staged.len().saturating_mul(3))
            .map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let mut new_order = BTreeSet::<(u64, Key)>::new();
        let mut new_stamps = HashMap::<Key, u64>::new();
        let mut base_order = self.lru.oldest().peekable();
        let mut data_bytes = self.data_bytes;
        let mut directory_count = self.directory_count;
        let mut entry_count = self.entry_count;

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
                let victim = next_victim(&mut base_order, &mut new_order, &mut new_stamps, Some(key), &changes, &entries)
                    .ok_or(StorageError::TooLarge)?;
                let previous = effective_entry(&victim, &changes, &entries).ok_or(StorageError::TooLarge)?;
                self.adjust_directories(&victim, -1, &mut directory_deltas, &mut directory_count)?;
                data_bytes = data_bytes.checked_sub(previous.size).ok_or(StorageError::TooLarge)?;
                entry_count = entry_count.saturating_sub(1);
                changes.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
                changes.insert(victim, None);
            }
        }
        Ok(Plan { changes, directory_deltas })
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
            state: std::sync::atomic::AtomicU8::new(ENTRY_VALID),
        });
        Ok(StagedData { entry, temp })
    }

    fn publish_and_commit(&mut self, plan: &Plan, staged: Vec<StagedData>) -> Result<(), StorageError> {
        let changes = self.prepare_changes(plan)?;
        let replacing = self.begin_replacements(&changes)?;
        let mut renamed = HashSet::new();
        renamed.try_reserve(staged.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        for data in staged {
            let destination = self.read_state.namespaces.path(&self.root, data.entry.key).ok_or(StorageError::UnknownNamespace)?;
            if let Err(error) = persist_with_retry(data.temp, &destination).map_err(StorageError::Io) {
                self.finish_failed_replacements(&replacing, &renamed, &changes, &HashSet::new(), is_simulated_crash(&error));
                return Err(error);
            }
            renamed.insert(data.entry.key);
            if let Err(error) = self.hooks.check(WritePoint::DataRenamed).map_err(StorageError::Io) {
                self.finish_failed_replacements(&replacing, &renamed, &changes, &HashSet::new(), is_simulated_crash(&error));
                return Err(error);
            }
        }

        let mut committed = HashSet::new();
        committed.try_reserve(changes.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        for chunk in changes.chunks(MAX_BATCH_ROWS) {
            let mutations = mutations(chunk)?;
            if let Err(error) = self.hooks.check(WritePoint::IndexBatchStarted).map_err(StorageError::Io) {
                let crashed = is_simulated_crash(&error);
                self.finish_failed_replacements(&replacing, &renamed, &changes, &committed, crashed);
                return Err(error);
            }
            if let Err(error) = self.index.apply(mutations) {
                let storage_error = self.index_failure(error, &changes);
                self.finish_failed_replacements(&replacing, &renamed, &changes, &committed, false);
                return Err(storage_error);
            }
            self.publish_changes(chunk);
            for change in chunk {
                committed.insert(change.key);
            }
            if let Err(error) = self.hooks.check(WritePoint::IndexBatchCommitted).map_err(StorageError::Io) {
                self.finish_failed_replacements(&replacing, &renamed, &changes, &committed, is_simulated_crash(&error));
                return Err(error);
            }
        }
        Ok(())
    }

    fn prepare_changes(&mut self, plan: &Plan) -> Result<Vec<PlannedChange>, StorageError> {
        let mut changes = Vec::new();
        changes.try_reserve_exact(plan.changes.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let entries = read_map(&self.read_state.entries);
        for (key, next) in &plan.changes {
            changes.push(PlannedChange { key: *key, previous: entries.get(key).cloned(), next: next.clone() });
        }
        drop(entries);
        changes.sort_unstable_by_key(|change| change.key);
        let additional = changes.iter().filter(|change| change.next.is_some()).count();
        let mut entries = write_map(&self.read_state.entries);
        entries.try_reserve(additional).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        drop(entries);
        self.referenced_keys.try_reserve(additional).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        self.directory_counts
            .try_reserve(plan.directory_deltas.len())
            .map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        self.lru.try_reserve(additional).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
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
        if !crashed {
            for change in changes {
                if !committed.contains(&change.key) && change.previous.is_none() && change.next.is_some() && renamed.contains(&change.key) {
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
        for change in changes {
            if let Some(previous) = &change.previous {
                self.lru.remove(&change.key);
                self.referenced_keys.remove(&change.key);
                self.data_bytes = self.data_bytes.saturating_sub(previous.size);
                self.entry_count = self.entry_count.saturating_sub(1);
                self.adjust_live_directories(&change.key, -1);
            }
            if let Some(entry) = &change.next {
                self.lru.insert(change.key, entry.last_use.load(Ordering::Relaxed));
                self.referenced_keys.insert(change.key);
                self.data_bytes = self.data_bytes.saturating_add(entry.size);
                self.entry_count = self.entry_count.saturating_add(1);
                self.adjust_live_directories(&change.key, 1);
            } else {
                delete_data(&self.root, &self.read_state.namespaces, &change.key);
            }
        }
        self.update_accounted_size();
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
        let change = PlannedChange { key, previous: Some(entry), next: None };
        self.commit_changes(std::slice::from_ref(&change))?;
        Ok(true)
    }

    fn remove_invalid(&mut self, key: Key, target: Arc<Entry>) -> Result<(), StorageError> {
        let current = read_map(&self.read_state.entries).get(&key).cloned();
        if !current.as_ref().is_some_and(|entry| Arc::ptr_eq(entry, &target)) {
            return Ok(());
        }
        let change = PlannedChange { key, previous: Some(target), next: None };
        self.commit_changes(std::slice::from_ref(&change))
    }

    fn commit_changes(&mut self, changes: &[PlannedChange]) -> Result<(), StorageError> {
        for chunk in changes.chunks(MAX_BATCH_ROWS) {
            let mutations = mutations(chunk)?;
            if let Err(error) = self.index.apply(mutations) {
                return Err(self.index_failure(error, changes));
            }
            self.publish_changes(chunk);
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
        self.enforce_limit(limit)?;
        if self.accounted_size() > limit {
            return Err(StorageError::TooLarge);
        }
        self.size_limit = limit;
        self.read_state.size_limit.store(limit, Ordering::Relaxed);
        Ok(())
    }

    fn enforce_limit(&mut self, limit: u64) -> Result<(), StorageError> {
        self.drain_touches_best_effort();
        let entries = read_map(&self.read_state.entries);
        let mut base_order = self.lru.oldest().peekable();
        let mut changes = HashMap::<Key, Option<Arc<Entry>>>::new();
        let mut directory_deltas = HashMap::<DirectoryId, i64>::new();
        let mut data_bytes = self.data_bytes;
        let mut directory_count = self.directory_count;
        let mut entry_count = self.entry_count;
        let mut no_new_order = BTreeSet::new();
        let mut no_new_stamps = HashMap::new();
        while !self.fits(data_bytes, entry_count, directory_count, limit)? {
            let key = next_victim(&mut base_order, &mut no_new_order, &mut no_new_stamps, None, &changes, &entries)
                .ok_or(StorageError::TooLarge)?;
            let entry = effective_entry(&key, &changes, &entries).ok_or(StorageError::TooLarge)?;
            self.adjust_directories(&key, -1, &mut directory_deltas, &mut directory_count)?;
            data_bytes = data_bytes.checked_sub(entry.size).ok_or(StorageError::TooLarge)?;
            entry_count = entry_count.saturating_sub(1);
            changes.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
            changes.insert(key, None);
        }
        drop(base_order);
        drop(entries);
        if changes.is_empty() {
            return Ok(());
        }
        let plan = Plan { changes, directory_deltas };
        let changes = self.prepare_changes(&plan)?;
        self.commit_changes(&changes)
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
        let mut records = Vec::new();
        records.try_reserve_exact(touches.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        for (key, touched) in touches {
            if entries.get(&key).is_some_and(|entry| Arc::ptr_eq(entry, &touched)) {
                self.lru.insert(key, touched.last_use.load(Ordering::Relaxed));
                records.push(touched.record());
            }
        }
        drop(entries);
        for chunk in records.chunks(MAX_BATCH_ROWS) {
            let mut batch = Vec::new();
            batch.try_reserve_exact(chunk.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
            batch.extend_from_slice(chunk);
            if let Err(error) = self.index.touch(batch) {
                return Err(self.index_failure(error, &[]));
            }
        }
        self.update_accounted_size();
        Ok(())
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

fn mutations(changes: &[PlannedChange]) -> Result<Vec<Mutation>, StorageError> {
    let mut mutations = Vec::new();
    mutations.try_reserve_exact(changes.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
    for change in changes {
        mutations.push(match &change.next {
            Some(entry) => Mutation::Put(entry.record()),
            None => Mutation::Remove(change.key),
        });
    }
    Ok(mutations)
}

fn is_index_file(root: &Path, path: &Path) -> bool {
    if path.parent() != Some(root) {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == INDEX_FILE
        || name.strip_prefix(INDEX_FILE).is_some_and(|suffix| matches!(suffix, "-wal" | "-shm" | ".maintenance.lock"))
        || name == "lock"
}

fn effective_entry(key: &Key, changes: &HashMap<Key, Option<Arc<Entry>>>, entries: &HashMap<Key, Arc<Entry>>) -> Option<Arc<Entry>> {
    match changes.get(key) {
        Some(entry) => entry.clone(),
        None => entries.get(key).cloned(),
    }
}

fn remove_new_candidate(key: &Key, order: &mut BTreeSet<(u64, Key)>, stamps: &mut HashMap<Key, u64>) {
    if let Some(stamp) = stamps.remove(key) {
        order.remove(&(stamp, *key));
    }
}

fn next_victim(
    base_order: &mut std::iter::Peekable<impl Iterator<Item = (u64, Key)>>,
    new_order: &mut BTreeSet<(u64, Key)>,
    new_stamps: &mut HashMap<Key, u64>,
    protected: Option<Key>,
    changes: &HashMap<Key, Option<Arc<Entry>>>,
    entries: &HashMap<Key, Arc<Entry>>,
) -> Option<Key> {
    loop {
        while let Some((_, key)) = base_order.peek().cloned() {
            if Some(&key) == protected.as_ref() || changes.contains_key(&key) || effective_entry(&key, changes, entries).is_none() {
                base_order.next();
            } else {
                break;
            }
        }
        let stale = new_order
            .iter()
            .find(|&(_, key)| changes.get(key).is_some_and(Option::is_none) || effective_entry(key, changes, entries).is_none())
            .cloned();
        if let Some(stale) = stale {
            new_order.remove(&stale);
            new_stamps.remove(&stale.1);
            continue;
        }
        let base = base_order.peek().cloned();
        let new = new_order.iter().find(|&(_, key)| Some(key) != protected.as_ref()).cloned();
        let selected = match (base, new) {
            (Some(base), Some(new)) if base <= new => {
                base_order.next();
                Some(base)
            }
            (_, Some(new)) => {
                new_order.remove(&new);
                new_stamps.remove(&new.1);
                Some(new)
            }
            (Some(base), None) => {
                base_order.next();
                Some(base)
            }
            (None, None) => None,
        };
        if let Some((_, key)) = selected {
            if effective_entry(&key, changes, entries).is_some() {
                return Some(key);
            }
        } else {
            return None;
        }
    }
}

fn account_size(index_bytes: u64, data_bytes: u64, entry_count: usize, directory_count: u64) -> Option<u64> {
    let files = u64::try_from(entry_count).ok()?.checked_add(INDEX_FILE_OVERHEAD)?;
    let directories = directory_count.checked_add(1)?;
    DIRECTORY_OVERHEAD
        .checked_mul(directories)?
        .checked_add(index_bytes)?
        .checked_add(data_bytes)?
        .checked_add(FILE_OVERHEAD.checked_mul(files)?)
}

fn read_map(entries: &RwLock<HashMap<Key, Arc<Entry>>>) -> std::sync::RwLockReadGuard<'_, HashMap<Key, Arc<Entry>>> {
    match entries.read() {
        Ok(entries) => entries,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn write_map(entries: &RwLock<HashMap<Key, Arc<Entry>>>) -> std::sync::RwLockWriteGuard<'_, HashMap<Key, Arc<Entry>>> {
    match entries.write() {
        Ok(entries) => entries,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn reply_result<T>(sender: SyncSender<Result<T, StorageError>>, value: Result<T, StorageError>) {
    match sender.send(value) {
        Ok(()) | Err(_) => {}
    }
}

fn clone_error(error: &StorageError) -> StorageError {
    match error {
        StorageError::InUse => StorageError::InUse,
        StorageError::InvalidNamespaces => StorageError::InvalidNamespaces,
        StorageError::UnknownNamespace => StorageError::UnknownNamespace,
        StorageError::Unavailable(reason) => StorageError::Unavailable(*reason),
        StorageError::LimitTooSmall => StorageError::LimitTooSmall,
        StorageError::TooLarge => StorageError::TooLarge,
        StorageError::Io(error) => StorageError::Io(io::Error::new(error.kind(), error.to_string())),
        StorageError::WorkerPanicked => StorageError::WorkerPanicked,
    }
}

fn simulated_crash_error() -> StorageError {
    StorageError::Io(io::Error::new(io::ErrorKind::Interrupted, "simulated cache crash"))
}

fn is_simulated_crash(error: &StorageError) -> bool {
    matches!(error, StorageError::Io(error) if error.kind() == io::ErrorKind::Interrupted)
}
