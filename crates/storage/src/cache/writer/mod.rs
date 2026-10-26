mod disk;
mod queue;

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs::{self, File, OpenOptions, ReadDir},
    io::{self, Seek, SeekFrom, Write},
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
    DIRECTORY_OVERHEAD, DirectoryId, ENTRY_INVALID, ENTRY_REPLACING, ENTRY_VALID, Entry, FILE_OVERHEAD, INDEX_OVERHEAD, JOURNAL_FILE, Key,
    NamespaceTable, ReadState, StorageError, UnavailableReason, format_error,
    journal::{self, Operation},
};
use crate::cache::eviction::Lru;
use disk::{
    DATA_TEMP_PREFIX, delete_data, ensure_journal, fail_temporary, persist_with_retry, remove_path, reset_journal_atomic,
    write_snapshot_atomic,
};

pub(in crate::cache) use queue::{Command, WriteQueue, writer_loop};

const JOURNAL_COMPACT_MIN_BYTES: u64 = 1024 * 1024;
const QUEUE_SWEEP_CHUNK: usize = 64;
const MAX_SWEEP_DEPTH: usize = 32;

pub(super) fn rewrite_namespace_table(
    root: &Path,
    entries: &HashMap<Key, Arc<Entry>>,
    sequence: u64,
    namespaces: &NamespaceTable,
    hooks: &FaultHooks,
) -> Result<(u64, u64), StorageError> {
    let mut records = Vec::new();
    records.try_reserve_exact(entries.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
    records.extend(entries.values().map(|entry| entry.record()));
    records.sort_unstable_by_key(|record| record.key);
    let snapshot = journal::encode_snapshot(sequence, &records, namespaces).map_err(format_error)?;
    let snapshot_bytes = u64::try_from(snapshot.len()).map_err(|_| StorageError::TooLarge)?;
    write_snapshot_atomic(root, &snapshot, hooks, || {})?;
    reset_journal_atomic(root, namespaces, hooks, || {})?;
    let journal_bytes = journal::journal_header_size(namespaces).ok_or(StorageError::TooLarge)?;
    Ok((snapshot_bytes, journal_bytes))
}

#[repr(usize)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WritePoint {
    DataTempCreated = 1,
    DataWriteFailure,
    #[cfg(test)]
    DataWritePartial,
    DataWritten,
    DataRenamed,
    JournalAppendStarted,
    JournalAppendPartial,
    JournalAppended,
    SnapshotTempCreated,
    SnapshotWritten,
    SnapshotSynced,
    SnapshotRenamed,
    JournalResetTempCreated,
    JournalResetWritten,
    JournalResetSynced,
    JournalResetRenamed,
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
            let mut pause_guard = match self.pause.lock() {
                Ok(pause) => pause,
                Err(poisoned) => poisoned.into_inner(),
            };
            let selected = pause_guard.iter().position(|pause| pause.point == point);
            let pause = selected.map(|index| pause_guard.remove(index));
            drop(pause_guard);
            if let Some(pause) = pause {
                pause.started.send(()).map_err(|_| io::Error::other("test pause receiver stopped"))?;
                pause.resume.recv().map_err(|_| io::Error::other("test pause was released"))?;
            }
            if self.fail_at.compare_exchange(point as usize, 0, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                let kind = if point == WritePoint::DataWriteFailure { io::ErrorKind::StorageFull } else { io::ErrorKind::Interrupted };
                let message = if point == WritePoint::DataWriteFailure { "simulated full disk" } else { "simulated cache crash" };
                return Err(io::Error::new(kind, message));
            }
        }
        #[cfg(not(test))]
        let _ = point;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn arm(&self, point: WritePoint) {
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
    directory_count: u64,
    encoded: Vec<u8>,
    sequence: u64,
    data_bytes: u64,
    live_index_bytes: u64,
    entry_count: usize,
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
    read_state: Arc<ReadState>,
    lru: Lru,
    referenced_keys: HashSet<Key>,
    directory_counts: HashMap<DirectoryId, usize>,
    directory_count: u64,
    sequence: u64,
    snapshot_bytes: u64,
    journal_bytes: u64,
    live_index_bytes: u64,
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
        sequence: u64,
        snapshot_bytes: u64,
        journal_bytes: u64,
        hooks: Arc<FaultHooks>,
    ) -> Result<Self, StorageError> {
        let entries = match read_state.entries.read() {
            Ok(entries) => entries,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut lru = Lru::default();
        lru.try_reserve(entries.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let mut referenced_keys = HashSet::new();
        let mut directory_counts = HashMap::<DirectoryId, usize>::new();
        referenced_keys.try_reserve(entries.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        directory_counts
            .try_reserve(entries.len().min(read_state.namespaces.len().saturating_mul(256)).saturating_add(read_state.namespaces.len() * 8))
            .map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let mut data_bytes = 0u64;
        let mut live_index_bytes = journal::snapshot_header_size(&read_state.namespaces).ok_or(StorageError::TooLarge)? + 32;
        for entry in entries.values() {
            let last_use = entry.last_use.load(Ordering::Relaxed);
            lru.insert(entry.key, last_use);
            referenced_keys.insert(entry.key);
            if let Some((prefixes, shard)) = read_state.namespaces.directories(entry.key) {
                for directory in prefixes.iter().copied().chain(std::iter::once(shard)) {
                    let count = directory_counts.entry(directory).or_default();
                    *count = count.checked_add(1).ok_or(StorageError::TooLarge)?;
                }
            }
            data_bytes = data_bytes.checked_add(entry.size).ok_or(StorageError::TooLarge)?;
            live_index_bytes = live_index_bytes
                .checked_add(journal::entry_size(&entry.record()).ok_or(StorageError::TooLarge)?)
                .ok_or(StorageError::TooLarge)?;
        }
        let entry_count = entries.len();
        let directory_count = u64::try_from(directory_counts.len()).map_err(|_| StorageError::TooLarge)?;
        drop(entries);
        let journal_bytes = ensure_journal(&root, journal_bytes, &read_state.namespaces)?;
        let mut state = Self {
            root,
            _lock_file: lock_file,
            size_limit: read_state.size_limit.load(Ordering::Relaxed),
            read_state,
            lru,
            referenced_keys,
            directory_counts,
            directory_count,
            sequence,
            snapshot_bytes,
            journal_bytes,
            live_index_bytes,
            data_bytes,
            entry_count,
            hooks,
            sweep: Vec::new(),
            sweep_complete: false,
        };
        state.read_state.accounted_size.store(state.accounted_size(), Ordering::Relaxed);
        if state.accounted_size() > state.size_limit {
            state.enforce_limit(state.size_limit)?;
            state.compact()?;
            if state.accounted_size() > state.size_limit {
                return Err(StorageError::TooLarge);
            }
        }
        Ok(state)
    }

    pub fn accounted_size(&self) -> u64 {
        account_size(self.snapshot_bytes, self.journal_bytes, self.data_bytes, self.entry_count, self.directory_count).unwrap_or(u64::MAX)
    }

    fn journal_header_bytes(&self) -> u64 {
        journal::journal_header_size(&self.read_state.namespaces).unwrap_or(u64::MAX)
    }

    fn put_batch(&mut self, puts: Vec<(Key, Vec<u8>)>) -> Vec<Result<(), StorageError>> {
        self.drain_touches();
        let total = puts.len();
        let mut staged = Vec::<StagedData>::new();
        let mut staged_indices = Vec::new();
        if staged.try_reserve(total).is_err() || staged_indices.try_reserve(total).is_err() {
            return (0..total).map(|_| Err(StorageError::Unavailable(UnavailableReason::Allocation))).collect();
        }
        let mut results = std::iter::repeat_with(|| None).take(total).collect::<Vec<Option<Result<(), StorageError>>>>();
        let mut crashed = false;
        for (index, (key, bytes)) in puts.into_iter().enumerate() {
            match self.write_data(key, &bytes) {
                Ok(data) => {
                    staged.push(data);
                    staged_indices.push(index);
                }
                Err(error) => {
                    let crash = matches!(&error, StorageError::Io(io_error) if io_error.kind() == io::ErrorKind::Interrupted);
                    if crash {
                        let failure = StorageError::Io(io::Error::new(io::ErrorKind::Interrupted, "simulated cache crash"));
                        for result in results.iter_mut() {
                            *result = Some(Err(clone_error(&failure)));
                        }
                        crashed = true;
                        break;
                    }
                    results[index] = Some(Err(error));
                }
            }
        }
        if crashed {
            return results.into_iter().map(|result| result.unwrap_or(Ok(()))).collect();
        }
        if staged.is_empty() {
            return results.into_iter().map(|result| result.unwrap_or(Ok(()))).collect();
        }
        self.drain_touches();
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
        let mut plan = match self.plan_puts_with_compaction(&staged_entries) {
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
        match self.publish_and_commit(&mut plan, staged) {
            Ok(()) => {
                if let Err(error) = self.maybe_compact() {
                    tracing::warn!(error = %error, "cache journal compaction was deferred");
                }
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

    fn plan_puts_with_compaction(&mut self, staged: &[Arc<Entry>]) -> Result<Plan, StorageError> {
        match self.plan_puts(staged) {
            Err(StorageError::TooLarge)
                if self.journal_bytes > self.journal_header_bytes() || self.snapshot_bytes != self.live_index_bytes =>
            {
                self.compact()?;
                self.plan_puts(staged)
            }
            result => result,
        }
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
        let mut encoded = Vec::new();
        let mut sequence = self.sequence;
        let mut data_bytes = self.data_bytes;
        let mut directory_count = self.directory_count;
        let mut live_index_bytes = self.live_index_bytes;
        let mut entry_count = self.entry_count;

        for entry in staged {
            let key = entry.key;
            let current = effective_entry(&key, &changes, &entries);
            if let Some(previous) = current.as_ref() {
                self.adjust_directories(&key, -1, &mut directory_deltas, &mut directory_count)?;
                data_bytes = data_bytes.checked_sub(previous.size).ok_or(StorageError::TooLarge)?;
                live_index_bytes = live_index_bytes
                    .checked_sub(journal::entry_size(&previous.record()).ok_or(StorageError::TooLarge)?)
                    .ok_or(StorageError::TooLarge)?;
                entry_count = entry_count.saturating_sub(1);
            }
            remove_new_candidate(&key, &mut new_order, &mut new_stamps);
            self.adjust_directories(&key, 1, &mut directory_deltas, &mut directory_count)?;
            data_bytes = data_bytes.checked_add(entry.size).ok_or(StorageError::TooLarge)?;
            live_index_bytes = live_index_bytes
                .checked_add(journal::entry_size(&entry.record()).ok_or(StorageError::TooLarge)?)
                .ok_or(StorageError::TooLarge)?;
            entry_count = entry_count.checked_add(1).ok_or(StorageError::TooLarge)?;
            changes.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
            changes.insert(key, Some(Arc::clone(entry)));
            let stamp = entry.last_use.load(Ordering::Relaxed);
            new_order.insert((stamp, key));
            new_stamps.insert(key, stamp);
            append_operation(&mut sequence, Operation::Put(entry.record()), &mut encoded)?;
            while !fits(self, data_bytes, live_index_bytes, entry_count, directory_count, encoded.len(), self.size_limit)? {
                let victim = next_victim(&mut base_order, &mut new_order, &mut new_stamps, Some(key), &changes, &entries)
                    .ok_or(StorageError::TooLarge)?;
                let previous = effective_entry(&victim, &changes, &entries).ok_or(StorageError::TooLarge)?;
                self.adjust_directories(&victim, -1, &mut directory_deltas, &mut directory_count)?;
                data_bytes = data_bytes.checked_sub(previous.size).ok_or(StorageError::TooLarge)?;
                live_index_bytes = live_index_bytes
                    .checked_sub(journal::entry_size(&previous.record()).ok_or(StorageError::TooLarge)?)
                    .ok_or(StorageError::TooLarge)?;
                entry_count = entry_count.saturating_sub(1);
                changes.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
                changes.insert(victim, None);
                append_operation(&mut sequence, Operation::Remove(victim), &mut encoded)?;
            }
        }
        Ok(Plan { changes, directory_deltas, directory_count, encoded, sequence, data_bytes, live_index_bytes, entry_count })
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

    fn publish_and_commit(&mut self, plan: &mut Plan, staged: Vec<StagedData>) -> Result<(), StorageError> {
        let next_journal_len = self
            .journal_bytes
            .checked_add(u64::try_from(plan.encoded.len()).map_err(|_| StorageError::TooLarge)?)
            .ok_or(StorageError::TooLarge)?;
        if next_journal_len > u64::try_from(journal::MAX_JOURNAL_BYTES).map_err(|_| StorageError::TooLarge)? {
            self.compact()?;
            let next_journal_len = self
                .journal_bytes
                .checked_add(u64::try_from(plan.encoded.len()).map_err(|_| StorageError::TooLarge)?)
                .ok_or(StorageError::TooLarge)?;
            if next_journal_len > u64::try_from(journal::MAX_JOURNAL_BYTES).map_err(|_| StorageError::TooLarge)? {
                return Err(StorageError::TooLarge);
            }
        }
        let changes = self.prepare_changes(plan)?;
        let mut renamed = HashSet::new();
        renamed.try_reserve(staged.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let replacing = self.begin_replacements(&changes)?;
        let mut journal_appended = false;
        let publish_result = (|| {
            for data in staged {
                let destination = self.read_state.namespaces.path(&self.root, data.entry.key).ok_or(StorageError::UnknownNamespace)?;
                persist_with_retry(data.temp, &destination)?;
                renamed.insert(data.entry.key);
                self.hooks.check(WritePoint::DataRenamed).map_err(StorageError::Io)?;
            }
            self.append(&plan.encoded)?;
            journal_appended = true;
            self.check_journal_appended()
        })();
        if let Err(error) = publish_result {
            self.finish_failed_replacements(replacing, &renamed, &changes, journal_appended);
            return Err(error);
        }
        self.commit_plan(plan, changes);
        self.read_state.accounted_size.store(self.accounted_size(), Ordering::Relaxed);
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
        replacing: Vec<Arc<Entry>>,
        renamed: &HashSet<Key>,
        changes: &[PlannedChange],
        journal_appended: bool,
    ) {
        let mut invalid = lock(&self.read_state.invalid);
        for entry in replacing {
            if renamed.contains(&entry.key) {
                entry.state.store(ENTRY_INVALID, Ordering::Release);
                invalid.insert(entry.key, entry);
            } else {
                entry.state.store(ENTRY_VALID, Ordering::Release);
            }
        }
        drop(invalid);
        if !journal_appended {
            for change in changes {
                if change.previous.is_none() && change.next.is_some() && renamed.contains(&change.key) {
                    delete_data(&self.root, &self.read_state.namespaces, &change.key);
                }
            }
        }
    }

    fn append(&mut self, encoded: &[u8]) -> Result<(), StorageError> {
        if encoded.is_empty() {
            return Ok(());
        }
        let next_len = self
            .journal_bytes
            .checked_add(u64::try_from(encoded.len()).map_err(|_| StorageError::TooLarge)?)
            .ok_or(StorageError::TooLarge)?;
        if next_len > u64::try_from(journal::MAX_JOURNAL_BYTES).map_err(|_| StorageError::TooLarge)? {
            self.compact()?;
        }
        self.hooks.check(WritePoint::JournalAppendStarted).map_err(StorageError::Io)?;
        let path = self.root.join(JOURNAL_FILE);
        let mut file = OpenOptions::new().append(true).read(true).open(path)?;
        let start = file.metadata()?.len();
        if let Err(error) = self.hooks.check(WritePoint::JournalAppendPartial) {
            let prefix_len = encoded.len() / 2;
            if let Some(prefix) = encoded.get(..prefix_len) {
                file.write_all(prefix)?;
            }
            return Err(StorageError::Io(error));
        }
        if let Err(error) = file.write_all(encoded) {
            file.set_len(start)?;
            file.seek(SeekFrom::End(0))?;
            return Err(StorageError::Io(error));
        }
        self.journal_bytes =
            start.checked_add(u64::try_from(encoded.len()).map_err(|_| StorageError::TooLarge)?).ok_or(StorageError::TooLarge)?;
        Ok(())
    }

    fn check_journal_appended(&self) -> Result<(), StorageError> {
        self.hooks.check(WritePoint::JournalAppended).map_err(StorageError::Io)
    }

    fn append_and_check(&mut self, encoded: &[u8]) -> Result<(), StorageError> {
        if encoded.is_empty() {
            return Ok(());
        }
        self.append(encoded)?;
        self.check_journal_appended()
    }

    fn commit_plan(&mut self, plan: &mut Plan, changes: Vec<PlannedChange>) {
        self.sequence = plan.sequence;
        self.data_bytes = plan.data_bytes;
        self.live_index_bytes = plan.live_index_bytes;
        self.entry_count = plan.entry_count;
        {
            let mut entries = write_map(&self.read_state.entries);
            for change in &changes {
                if let Some(entry) = &change.next {
                    entries.insert(change.key, Arc::clone(entry));
                } else {
                    entries.remove(&change.key);
                }
            }
        }
        for change in changes {
            self.lru.remove(&change.key);
            if let Some(previous) = change.previous {
                self.referenced_keys.remove(&previous.key);
            }
            if let Some(entry) = change.next {
                self.lru.insert(change.key, entry.last_use.load(Ordering::Relaxed));
                self.referenced_keys.insert(entry.key);
            } else {
                delete_data(&self.root, &self.read_state.namespaces, &change.key);
            }
        }
        self.directory_count = plan.directory_count;
        for (directory, delta) in plan.directory_deltas.drain() {
            let existing = self.directory_counts.get(&directory).copied().unwrap_or(0);
            let Some(count) = i64::try_from(existing).ok().and_then(|count| count.checked_add(delta)) else {
                tracing::error!("cache directory accounting invariant failed");
                continue;
            };
            if count <= 0 {
                self.directory_counts.remove(&directory);
            } else if let Ok(count) = usize::try_from(count) {
                self.directory_counts.insert(directory, count);
            } else {
                tracing::error!("cache directory accounting invariant failed");
            }
        }
    }

    fn remove(&mut self, key: Key) -> Result<bool, StorageError> {
        self.drain_touches();
        let entry = read_map(&self.read_state.entries).get(&key).cloned();
        let Some(entry) = entry else {
            return Ok(false);
        };
        let mut encoded = Vec::new();
        let mut sequence = self.sequence;
        append_operation(&mut sequence, Operation::Remove(key), &mut encoded)?;
        self.append_and_check(&encoded)?;
        self.sequence = sequence;
        self.remove_published(&key, &entry);
        self.maybe_compact()?;
        Ok(true)
    }

    fn remove_invalid(&mut self, key: Key, target: Arc<Entry>) -> Result<(), StorageError> {
        let current = read_map(&self.read_state.entries).get(&key).cloned();
        if !current.as_ref().is_some_and(|entry| Arc::ptr_eq(entry, &target)) {
            return Ok(());
        }
        let mut encoded = Vec::new();
        let mut sequence = self.sequence;
        append_operation(&mut sequence, Operation::Remove(key), &mut encoded)?;
        self.append_and_check(&encoded)?;
        self.sequence = sequence;
        self.remove_published(&key, &target);
        self.maybe_compact()?;
        Ok(())
    }

    fn remove_published(&mut self, key: &Key, entry: &Arc<Entry>) {
        let read_state = Arc::clone(&self.read_state);
        let mut entries = write_map(&read_state.entries);
        let matches = entries.get(key).is_some_and(|current| Arc::ptr_eq(current, entry));
        if matches {
            self.lru.remove(key);
            entries.remove(key);
            self.referenced_keys.remove(&entry.key);
            self.remove_directories(key);
            self.data_bytes = self.data_bytes.saturating_sub(entry.size);
            if let Some(size) = journal::entry_size(&entry.record()) {
                self.live_index_bytes = self.live_index_bytes.saturating_sub(size);
            }
            self.entry_count = self.entry_count.saturating_sub(1);
        }
        drop(entries);
        self.read_state.accounted_size.store(self.accounted_size(), Ordering::Relaxed);
        if matches {
            delete_data(&self.root, &self.read_state.namespaces, key);
        }
    }

    fn remove_directories(&mut self, key: &Key) {
        let Some((prefixes, shard)) = self.read_state.namespaces.directories(*key) else {
            return;
        };
        for directory in prefixes.iter().copied().chain(std::iter::once(shard)) {
            let remove = match self.directory_counts.get_mut(&directory) {
                Some(count) if *count > 1 => {
                    *count -= 1;
                    false
                }
                Some(_) => true,
                None => false,
            };
            if remove {
                self.directory_counts.remove(&directory);
                self.directory_count = self.directory_count.saturating_sub(1);
            }
        }
    }

    fn clear(&mut self) -> Result<(), StorageError> {
        self.drain_touches();
        let mut encoded = Vec::new();
        let mut sequence = self.sequence;
        append_operation(&mut sequence, Operation::Clear, &mut encoded)?;
        self.append_and_check(&encoded)?;
        self.sequence = sequence;
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
        self.live_index_bytes = journal::snapshot_header_size(&self.read_state.namespaces).unwrap_or(u64::MAX) + 32;
        lock(&self.read_state.invalid).clear();
        lock(&self.read_state.touches).clear();
        self.read_state.accounted_size.store(self.accounted_size(), Ordering::Relaxed);
        for key in old.keys() {
            delete_data(&self.root, &self.read_state.namespaces, key);
        }
        self.maybe_compact()?;
        Ok(())
    }

    fn set_limit(&mut self, limit: u64) -> Result<(), StorageError> {
        if limit < self.read_state.minimum_size_limit {
            return Err(StorageError::LimitTooSmall);
        }
        self.enforce_limit(limit)?;
        self.compact()?;
        if self.accounted_size() > limit {
            return Err(StorageError::TooLarge);
        }
        self.size_limit = limit;
        self.read_state.size_limit.store(limit, Ordering::Relaxed);
        Ok(())
    }

    fn enforce_limit(&mut self, limit: u64) -> Result<(), StorageError> {
        self.drain_touches();
        let entries = read_map(&self.read_state.entries);
        let mut base_order = self.lru.oldest().peekable();
        let mut changes = HashMap::<Key, Option<Arc<Entry>>>::new();
        let mut prior = HashMap::<Key, Option<Arc<Entry>>>::new();
        let mut encoded = Vec::new();
        let mut sequence = self.sequence;
        let mut data_bytes = self.data_bytes;
        let mut directory_count = self.directory_count;
        let mut directory_deltas = HashMap::<DirectoryId, i64>::new();
        let mut live_index_bytes = self.live_index_bytes;
        let mut entry_count = self.entry_count;
        let mut no_new_order = BTreeSet::new();
        let mut no_new_stamps = HashMap::new();
        while !fits_compacted(data_bytes, live_index_bytes, entry_count, directory_count, self.journal_header_bytes(), limit)? {
            let candidate = next_victim(&mut base_order, &mut no_new_order, &mut no_new_stamps, None, &changes, &entries)
                .ok_or(StorageError::TooLarge)?;
            let Some(entry) = effective_entry(&candidate, &changes, &entries) else {
                return Err(StorageError::TooLarge);
            };
            remember_prior(&candidate, &mut prior, &entries)?;
            self.adjust_directories(&candidate, -1, &mut directory_deltas, &mut directory_count)?;
            changes.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
            changes.insert(candidate, None);
            data_bytes = data_bytes.checked_sub(entry.size).ok_or(StorageError::TooLarge)?;
            live_index_bytes = live_index_bytes
                .checked_sub(journal::entry_size(&entry.record()).ok_or(StorageError::TooLarge)?)
                .ok_or(StorageError::TooLarge)?;
            entry_count = entry_count.saturating_sub(1);
            append_operation(&mut sequence, Operation::Remove(candidate), &mut encoded)?;
        }
        drop(base_order);
        drop(entries);
        if !encoded.is_empty() {
            self.append_and_check(&encoded)?;
            self.sequence = sequence;
            for (key, _) in changes {
                let Some(old) = prior.get(&key).and_then(Option::as_ref) else {
                    continue;
                };
                self.remove_published(&key, old);
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        self.drain_touches();
        self.compact()
    }

    fn drain_touches(&mut self) {
        let touches = {
            let mut pending = lock(&self.read_state.touches);
            std::mem::take(&mut *pending)
        };
        if touches.is_empty() {
            return;
        }
        let entries = read_map(&self.read_state.entries);
        for (key, touched) in touches {
            if entries.get(&key).is_some_and(|entry| Arc::ptr_eq(entry, &touched)) {
                self.lru.insert(key, touched.last_use.load(Ordering::Relaxed));
            }
        }
    }

    fn drain_invalid(&mut self) {
        let invalid = {
            let mut pending = lock(&self.read_state.invalid);
            std::mem::take(&mut *pending)
        };
        for (key, entry) in invalid {
            if let Err(error) = self.remove_invalid(key, entry) {
                tracing::warn!(error = %error, "failed to remove a corrupt cache entry");
                let current = read_map(&self.read_state.entries).get(&key).cloned();
                if let Some(current) = current
                    && current.state.load(Ordering::Acquire) == ENTRY_INVALID
                {
                    lock(&self.read_state.invalid).insert(key, current);
                }
            }
        }
    }

    fn maybe_compact(&mut self) -> Result<(), StorageError> {
        let compacted_size =
            account_size(self.live_index_bytes, self.journal_header_bytes(), self.data_bytes, self.entry_count, self.directory_count)
                .ok_or(StorageError::TooLarge)?;
        let headroom = self.size_limit.saturating_sub(compacted_size);
        let threshold = JOURNAL_COMPACT_MIN_BYTES.max(self.live_index_bytes / 2).min(headroom);
        if self.journal_bytes.saturating_sub(self.journal_header_bytes()) > threshold {
            self.compact()?;
        }
        Ok(())
    }

    fn compact(&mut self) -> Result<(), StorageError> {
        self.drain_touches();
        let entries = read_map(&self.read_state.entries);
        let mut records = Vec::new();
        records.try_reserve_exact(entries.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        records.extend(entries.values().map(|entry| entry.record()));
        records.sort_unstable_by_key(|record| record.key);
        drop(entries);
        let bytes = journal::encode_snapshot(self.sequence, &records, &self.read_state.namespaces).map_err(format_error)?;
        let snapshot_bytes = u64::try_from(bytes.len()).map_err(|_| StorageError::TooLarge)?;
        let root = self.root.clone();
        let hooks = Arc::clone(&self.hooks);
        write_snapshot_atomic(&root, &bytes, &hooks, || {
            self.snapshot_bytes = snapshot_bytes;
            self.live_index_bytes = snapshot_bytes;
            self.read_state.accounted_size.store(self.accounted_size(), Ordering::Relaxed);
        })?;
        let journal_header_bytes = self.journal_header_bytes();
        let namespaces = Arc::clone(&self.read_state.namespaces);
        reset_journal_atomic(&root, &namespaces, &hooks, || {
            self.journal_bytes = journal_header_bytes;
            self.read_state.accounted_size.store(self.accounted_size(), Ordering::Relaxed);
        })?;
        Ok(())
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
                    let name = item.file_name().to_string_lossy().into_owned();
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
                        let index_file =
                            path.parent() == Some(self.root.as_path()) && matches!(name.as_str(), "snapshot" | "journal" | "lock");
                        let referenced = path
                            .strip_prefix(&self.root)
                            .ok()
                            .and_then(|relative| self.read_state.namespaces.key_from_relative_path(relative))
                            .is_some_and(|key| self.referenced_keys.contains(&key));
                        if !index_file
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

fn append_operation(sequence: &mut u64, operation: Operation, encoded: &mut Vec<u8>) -> Result<(), StorageError> {
    *sequence = sequence.checked_add(1).ok_or(StorageError::TooLarge)?;
    let record = journal::encode_operation(*sequence, &operation).map_err(format_error)?;
    encoded.try_reserve(record.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
    encoded.extend_from_slice(&record);
    Ok(())
}

fn effective_entry(key: &Key, changes: &HashMap<Key, Option<Arc<Entry>>>, entries: &HashMap<Key, Arc<Entry>>) -> Option<Arc<Entry>> {
    match changes.get(key) {
        Some(entry) => entry.clone(),
        None => entries.get(key).cloned(),
    }
}

fn remember_prior(key: &Key, prior: &mut HashMap<Key, Option<Arc<Entry>>>, entries: &HashMap<Key, Arc<Entry>>) -> Result<(), StorageError> {
    if !prior.contains_key(key) {
        prior.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        prior.insert(*key, entries.get(key).cloned());
    }
    Ok(())
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

fn fits(
    state: &WorkerState,
    data_bytes: u64,
    live_index_bytes: u64,
    entry_count: usize,
    directory_count: u64,
    pending_journal: usize,
    limit: u64,
) -> Result<bool, StorageError> {
    if entry_count > journal::MAX_ENTRIES
        || live_index_bytes > u64::try_from(journal::MAX_SNAPSHOT_BYTES).map_err(|_| StorageError::TooLarge)?
    {
        return Ok(false);
    }
    let journal_bytes = state
        .journal_bytes
        .checked_add(u64::try_from(pending_journal).map_err(|_| StorageError::TooLarge)?)
        .ok_or(StorageError::TooLarge)?;
    Ok(account_size(state.snapshot_bytes, journal_bytes, data_bytes, entry_count, directory_count).ok_or(StorageError::TooLarge)? <= limit)
}

fn fits_compacted(
    data_bytes: u64,
    index_bytes: u64,
    entry_count: usize,
    directory_count: u64,
    journal_header_bytes: u64,
    limit: u64,
) -> Result<bool, StorageError> {
    if entry_count > journal::MAX_ENTRIES || index_bytes > u64::try_from(journal::MAX_SNAPSHOT_BYTES).map_err(|_| StorageError::TooLarge)? {
        return Ok(false);
    }
    Ok(account_size(index_bytes, journal_header_bytes, data_bytes, entry_count, directory_count).ok_or(StorageError::TooLarge)? <= limit)
}

fn account_size(snapshot_bytes: u64, journal_bytes: u64, data_bytes: u64, entry_count: usize, directory_count: u64) -> Option<u64> {
    let files = u64::try_from(entry_count).ok()?.checked_add(INDEX_OVERHEAD)?;
    let directories = directory_count.checked_add(1)?;
    DIRECTORY_OVERHEAD
        .checked_mul(directories)?
        .checked_add(snapshot_bytes)?
        .checked_add(journal_bytes)?
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

fn is_simulated_crash(error: &StorageError) -> bool {
    matches!(error, StorageError::Io(error) if error.kind() == io::ErrorKind::Interrupted)
}
