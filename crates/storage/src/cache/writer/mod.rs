mod disk;
mod queue;

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs::{self, File, OpenOptions, ReadDir},
    io::{self, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::{Arc, Mutex, RwLock, atomic::Ordering, mpsc::SyncSender},
};

use sha2::{Digest, Sha256};
use tempfile::Builder;

#[cfg(test)]
use std::sync::{
    atomic::AtomicUsize,
    mpsc::{self, Receiver},
};

use super::{
    DIRECTORY_OVERHEAD, Entry, FILE_OVERHEAD, INDEX_OVERHEAD, JOURNAL_FILE, Key, MIN_SIZE_LIMIT, ReadState, StorageError,
    UnavailableReason, format_error,
    journal::{self, Operation},
};
use crate::cache::eviction::Lru;
use disk::{
    DATA_TEMP_PREFIX, JOURNAL_TEMP_PREFIX, SNAPSHOT_TEMP_PREFIX, delete_data, ensure_journal, fail_temporary, final_data_name,
    persist_with_retry, remove_path, reset_journal_atomic, write_snapshot_atomic,
};

pub(in crate::cache) use queue::{Command, WriteQueue, writer_loop};

const JOURNAL_COMPACT_MIN_BYTES: u64 = 1024 * 1024;
const QUEUE_SWEEP_CHUNK: usize = 64;

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
    pause: Mutex<Option<TestPause>>,
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

    fn check(&self, point: WritePoint) -> io::Result<()> {
        #[cfg(test)]
        {
            let mut pause_guard = match self.pause.lock() {
                Ok(pause) => pause,
                Err(poisoned) => poisoned.into_inner(),
            };
            let selected = pause_guard.as_ref().is_some_and(|pause| pause.point == point);
            let pause = if selected { pause_guard.take() } else { None };
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
            Ok(mut current) => *current = Some(pause),
            Err(poisoned) => *poisoned.into_inner() = Some(pause),
        }
        (started_receiver, resume_sender)
    }
}

struct Plan {
    changes: HashMap<Key, Option<Arc<Entry>>>,
    prior: HashMap<Key, Option<Arc<Entry>>>,
    staged: Vec<Arc<Entry>>,
    encoded: Vec<u8>,
    sequence: u64,
    data_bytes: u64,
    live_index_bytes: u64,
    entry_count: usize,
}

pub(super) struct WorkerState {
    root: PathBuf,
    data_dir: PathBuf,
    _lock_file: File,
    read_state: Arc<ReadState>,
    lru: Lru,
    referenced_names: HashSet<String>,
    sequence: u64,
    snapshot_bytes: u64,
    journal_bytes: u64,
    live_index_bytes: u64,
    data_bytes: u64,
    entry_count: usize,
    size_limit: u64,
    hooks: Arc<FaultHooks>,
    sweep: Option<ReadDir>,
    sweeping_data: bool,
}

impl WorkerState {
    pub fn new(
        root: PathBuf,
        data_dir: PathBuf,
        lock_file: File,
        read_state: Arc<ReadState>,
        sequence: u64,
        snapshot_bytes: u64,
        journal_bytes: u64,
        hooks: Arc<FaultHooks>,
    ) -> Result<Self, StorageError> {
        let entries = read_map(&read_state.entries);
        let mut lru = Lru::default();
        let mut referenced_names = HashSet::new();
        referenced_names.try_reserve(entries.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let mut data_bytes = 0u64;
        let mut live_index_bytes = journal::SNAPSHOT_HEADER_BYTES + 32;
        for entry in entries.values() {
            let last_use = entry.last_use.load(Ordering::Relaxed);
            lru.insert(entry.key, last_use);
            referenced_names.insert(entry.file_name.clone());
            data_bytes = data_bytes.checked_add(entry.size).ok_or(StorageError::TooLarge)?;
            live_index_bytes = live_index_bytes
                .checked_add(journal::entry_size(&entry.record()).ok_or(StorageError::TooLarge)?)
                .ok_or(StorageError::TooLarge)?;
        }
        if snapshot_bytes == 0 {
            live_index_bytes = journal::SNAPSHOT_HEADER_BYTES + 32;
        }
        let entry_count = entries.len();
        drop(entries);
        let journal_bytes = ensure_journal(&root, journal_bytes)?;
        let mut state = Self {
            root,
            data_dir,
            _lock_file: lock_file,
            size_limit: read_state.size_limit.load(Ordering::Relaxed),
            read_state,
            lru,
            referenced_names,
            sequence,
            snapshot_bytes,
            journal_bytes,
            live_index_bytes,
            data_bytes,
            entry_count,
            hooks,
            sweep: None,
            sweeping_data: false,
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
        account_size(self.snapshot_bytes, self.journal_bytes, self.data_bytes, self.entry_count, 0).unwrap_or(u64::MAX)
    }

    fn put_batch(&mut self, puts: Vec<(Key, Vec<u8>)>) -> Vec<Result<(), StorageError>> {
        self.drain_touches();
        let total = puts.len();
        let mut staged = Vec::new();
        let mut staged_indices = Vec::new();
        if staged.try_reserve(total).is_err() || staged_indices.try_reserve(total).is_err() {
            return (0..total).map(|_| Err(StorageError::Unavailable(UnavailableReason::Allocation))).collect();
        }
        let mut results = std::iter::repeat_with(|| None).take(total).collect::<Vec<Option<Result<(), StorageError>>>>();
        let mut crashed = false;
        for (index, (key, bytes)) in puts.into_iter().enumerate() {
            match self.write_data(key, &bytes) {
                Ok(entry) => {
                    staged.push(entry);
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
        let mut plan = match self.plan_puts(&staged) {
            Ok(plan) => plan,
            Err(error) => {
                for entry in &staged {
                    delete_data(&self.data_dir, &entry.file_name);
                }
                for index in staged_indices {
                    if let Some(result) = results.get_mut(index) {
                        *result = Some(Err(clone_error(&error)));
                    }
                }
                return results.into_iter().map(|result| result.unwrap_or(Ok(()))).collect();
            }
        };
        match self.append(&plan.encoded) {
            Ok(()) => {
                self.commit_plan(&mut plan);
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
                if !is_simulated_crash(&error) {
                    for entry in &staged {
                        delete_data(&self.data_dir, &entry.file_name);
                    }
                }
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
        let mut prior = HashMap::new();
        changes.try_reserve(staged.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        prior.try_reserve(staged.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        let mut new_order = BTreeSet::<(u64, Key)>::new();
        let mut new_stamps = HashMap::<Key, u64>::new();
        let mut base_order = self.lru.oldest().peekable();
        let mut encoded = Vec::new();
        let mut sequence = self.sequence;
        let mut data_bytes = self.data_bytes;
        let mut live_index_bytes = self.live_index_bytes;
        let mut entry_count = self.entry_count;

        for entry in staged {
            let key = entry.key;
            let current = effective_entry(key, &changes, &entries);
            remember_prior(key, &mut prior, &entries)?;
            if let Some(previous) = current.as_ref() {
                data_bytes = data_bytes.checked_sub(previous.size).ok_or(StorageError::TooLarge)?;
                live_index_bytes = live_index_bytes
                    .checked_sub(journal::entry_size(&previous.record()).ok_or(StorageError::TooLarge)?)
                    .ok_or(StorageError::TooLarge)?;
                entry_count = entry_count.saturating_sub(1);
            }
            remove_new_candidate(key, &mut new_order, &mut new_stamps);
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
            while !fits(self, data_bytes, live_index_bytes, entry_count, encoded.len(), self.size_limit)? {
                let victim = next_victim(&mut base_order, &mut new_order, &mut new_stamps, Some(key), &changes, &entries)
                    .ok_or(StorageError::TooLarge)?;
                let previous = effective_entry(victim, &changes, &entries).ok_or(StorageError::TooLarge)?;
                remember_prior(victim, &mut prior, &entries)?;
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
        let mut planned_staged = Vec::new();
        planned_staged.try_reserve_exact(staged.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        planned_staged.extend(staged.iter().cloned());
        Ok(Plan { changes, prior, staged: planned_staged, encoded, sequence, data_bytes, live_index_bytes, entry_count })
    }

    fn write_data(&self, key: Key, bytes: &[u8]) -> Result<Arc<Entry>, StorageError> {
        let mut temp = Builder::new().prefix(DATA_TEMP_PREFIX).tempfile_in(&self.data_dir)?;
        let file_name = final_data_name(&temp)?;
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
        persist_with_retry(temp, &self.data_dir.join(&file_name)).map_err(StorageError::Io)?;
        self.hooks.check(WritePoint::DataRenamed).map_err(StorageError::Io)?;
        let stamp = super::saturating_increment(&self.read_state.usage_clock);
        let size = u64::try_from(bytes.len()).map_err(|_| StorageError::TooLarge)?;
        Ok(Arc::new(Entry {
            key,
            file_name,
            size,
            hash: Sha256::digest(bytes).into(),
            last_use: std::sync::atomic::AtomicU64::new(stamp),
            use_count: std::sync::atomic::AtomicU64::new(0),
            invalid: std::sync::atomic::AtomicBool::new(false),
        }))
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
        self.hooks.check(WritePoint::JournalAppended).map_err(StorageError::Io)?;
        self.journal_bytes =
            start.checked_add(u64::try_from(encoded.len()).map_err(|_| StorageError::TooLarge)?).ok_or(StorageError::TooLarge)?;
        Ok(())
    }

    fn commit_plan(&mut self, plan: &mut Plan) {
        self.sequence = plan.sequence;
        self.data_bytes = plan.data_bytes;
        self.live_index_bytes = plan.live_index_bytes;
        self.entry_count = plan.entry_count;
        for (key, next) in plan.changes.drain() {
            self.lru.remove(key);
            let mut entries = write_map(&self.read_state.entries);
            let previous = entries.remove(&key);
            if let Some(previous) = previous {
                self.referenced_names.remove(&previous.file_name);
            }
            if let Some(entry) = next {
                self.lru.insert(key, entry.last_use.load(Ordering::Relaxed));
                self.referenced_names.insert(entry.file_name.clone());
                entries.insert(key, entry);
            }
        }
        self.read_state.accounted_size.store(self.accounted_size(), Ordering::Relaxed);
        for old in plan.prior.values().flatten() {
            let current = read_map(&self.read_state.entries).get(&old.key).cloned();
            if !current.as_ref().is_some_and(|entry| Arc::ptr_eq(entry, old)) {
                delete_data(&self.data_dir, &old.file_name);
            }
        }
        for staged in &plan.staged {
            let current = read_map(&self.read_state.entries).get(&staged.key).cloned();
            if !current.as_ref().is_some_and(|entry| Arc::ptr_eq(entry, staged)) {
                delete_data(&self.data_dir, &staged.file_name);
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
        self.append(&encoded)?;
        self.sequence = sequence;
        self.remove_published(key, &entry);
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
        self.append(&encoded)?;
        self.sequence = sequence;
        self.remove_published(key, &target);
        self.maybe_compact()?;
        Ok(())
    }

    fn remove_published(&mut self, key: Key, entry: &Arc<Entry>) {
        let mut entries = write_map(&self.read_state.entries);
        let matches = entries.get(&key).is_some_and(|current| Arc::ptr_eq(current, entry));
        if matches {
            self.lru.remove(key);
            entries.remove(&key);
            self.referenced_names.remove(&entry.file_name);
            self.data_bytes = self.data_bytes.saturating_sub(entry.size);
            if let Some(size) = journal::entry_size(&entry.record()) {
                self.live_index_bytes = self.live_index_bytes.saturating_sub(size);
            }
            self.entry_count = self.entry_count.saturating_sub(1);
        }
        drop(entries);
        self.read_state.accounted_size.store(self.accounted_size(), Ordering::Relaxed);
        delete_data(&self.data_dir, &entry.file_name);
    }

    fn clear(&mut self) -> Result<(), StorageError> {
        self.drain_touches();
        let mut encoded = Vec::new();
        let mut sequence = self.sequence;
        append_operation(&mut sequence, Operation::Clear, &mut encoded)?;
        self.append(&encoded)?;
        self.sequence = sequence;
        let old = {
            let mut entries = write_map(&self.read_state.entries);
            std::mem::take(&mut *entries)
        };
        self.lru.clear();
        self.referenced_names.clear();
        self.data_bytes = 0;
        self.entry_count = 0;
        self.live_index_bytes = journal::SNAPSHOT_HEADER_BYTES + 32;
        lock(&self.read_state.invalid).clear();
        lock(&self.read_state.touches).clear();
        self.read_state.accounted_size.store(self.accounted_size(), Ordering::Relaxed);
        for entry in old.values() {
            delete_data(&self.data_dir, &entry.file_name);
        }
        self.maybe_compact()?;
        Ok(())
    }

    fn set_limit(&mut self, limit: u64) -> Result<(), StorageError> {
        if limit < MIN_SIZE_LIMIT {
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
        let mut live_index_bytes = self.live_index_bytes;
        let mut entry_count = self.entry_count;
        let mut no_new_order = BTreeSet::new();
        let mut no_new_stamps = HashMap::new();
        while !fits_compacted(data_bytes, live_index_bytes, entry_count, limit)? {
            let candidate = next_victim(&mut base_order, &mut no_new_order, &mut no_new_stamps, None, &changes, &entries)
                .ok_or(StorageError::TooLarge)?;
            let Some(entry) = effective_entry(candidate, &changes, &entries) else {
                return Err(StorageError::TooLarge);
            };
            remember_prior(candidate, &mut prior, &entries)?;
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
            self.append(&encoded)?;
            self.sequence = sequence;
            for (key, _) in changes {
                let Some(old) = prior.get(&key).and_then(Option::as_ref) else {
                    continue;
                };
                self.remove_published(key, old);
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
                    && current.invalid.load(Ordering::Acquire)
                {
                    lock(&self.read_state.invalid).insert(key, current);
                }
            }
        }
    }

    fn maybe_compact(&mut self) -> Result<(), StorageError> {
        let threshold = JOURNAL_COMPACT_MIN_BYTES.max(self.live_index_bytes / 2);
        if self.journal_bytes > threshold {
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
        let bytes = journal::encode_snapshot(self.sequence, &records).map_err(format_error)?;
        let snapshot_bytes = u64::try_from(bytes.len()).map_err(|_| StorageError::TooLarge)?;
        let root = self.root.clone();
        let hooks = Arc::clone(&self.hooks);
        write_snapshot_atomic(&root, &bytes, &hooks, || {
            self.snapshot_bytes = snapshot_bytes;
            self.live_index_bytes = snapshot_bytes;
            self.read_state.accounted_size.store(self.accounted_size(), Ordering::Relaxed);
        })?;
        reset_journal_atomic(&root, &hooks, || {
            self.journal_bytes = journal::JOURNAL_HEADER_BYTES;
            self.read_state.accounted_size.store(self.accounted_size(), Ordering::Relaxed);
        })?;
        Ok(())
    }

    fn sweep_step(&mut self, limit: usize) -> bool {
        let mut processed = 0usize;
        while processed < limit {
            if self.sweep.is_none() {
                let directory = if self.sweeping_data { &self.data_dir } else { &self.root };
                match fs::read_dir(directory) {
                    Ok(read_dir) => self.sweep = Some(read_dir),
                    Err(error) => {
                        tracing::warn!(error = %error, "cache orphan sweep could not open a directory");
                        if self.sweeping_data {
                            return true;
                        }
                        self.sweeping_data = true;
                        continue;
                    }
                }
            }
            let next = self.sweep.as_mut().and_then(|entries| entries.next());
            match next {
                Some(Ok(item)) => {
                    processed += 1;
                    let name = item.file_name().to_string_lossy().into_owned();
                    let path = item.path();
                    let remove = if self.sweeping_data {
                        !self.referenced_names.contains(&name)
                    } else {
                        name.starts_with(SNAPSHOT_TEMP_PREFIX) || name.starts_with(JOURNAL_TEMP_PREFIX)
                    };
                    if remove && let Err(error) = remove_path(&path) {
                        tracing::warn!(error = %error, "cache orphan sweep could not remove a file");
                    }
                }
                Some(Err(error)) => {
                    processed += 1;
                    tracing::warn!(error = %error, "cache orphan sweep could not read a directory entry");
                }
                None if !self.sweeping_data => {
                    self.sweep = None;
                    self.sweeping_data = true;
                }
                None => {
                    self.sweep = None;
                    return true;
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

fn effective_entry(key: Key, changes: &HashMap<Key, Option<Arc<Entry>>>, entries: &HashMap<Key, Arc<Entry>>) -> Option<Arc<Entry>> {
    match changes.get(&key) {
        Some(entry) => entry.clone(),
        None => entries.get(&key).cloned(),
    }
}

fn remember_prior(key: Key, prior: &mut HashMap<Key, Option<Arc<Entry>>>, entries: &HashMap<Key, Arc<Entry>>) -> Result<(), StorageError> {
    if !prior.contains_key(&key) {
        prior.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        prior.insert(key, entries.get(&key).cloned());
    }
    Ok(())
}

fn remove_new_candidate(key: Key, order: &mut BTreeSet<(u64, Key)>, stamps: &mut HashMap<Key, u64>) {
    if let Some(stamp) = stamps.remove(&key) {
        order.remove(&(stamp, key));
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
        while let Some((_, key)) = base_order.peek().copied() {
            if Some(key) == protected || changes.contains_key(&key) || effective_entry(key, changes, entries).is_none() {
                base_order.next();
            } else {
                break;
            }
        }
        let stale = new_order
            .iter()
            .copied()
            .find(|(_, key)| changes.get(key).is_some_and(Option::is_none) || effective_entry(*key, changes, entries).is_none());
        if let Some(stale) = stale {
            new_order.remove(&stale);
            new_stamps.remove(&stale.1);
            continue;
        }
        let base = base_order.peek().copied();
        let new = new_order.iter().copied().find(|(_, key)| Some(*key) != protected);
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
            if effective_entry(key, changes, entries).is_some() {
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
    Ok(account_size(state.snapshot_bytes, journal_bytes, data_bytes, entry_count, 0).ok_or(StorageError::TooLarge)? <= limit)
}

fn fits_compacted(data_bytes: u64, index_bytes: u64, entry_count: usize, limit: u64) -> Result<bool, StorageError> {
    if entry_count > journal::MAX_ENTRIES || index_bytes > u64::try_from(journal::MAX_SNAPSHOT_BYTES).map_err(|_| StorageError::TooLarge)? {
        return Ok(false);
    }
    Ok(account_size(index_bytes, journal::JOURNAL_HEADER_BYTES, data_bytes, entry_count, 0).ok_or(StorageError::TooLarge)? <= limit)
}

fn account_size(snapshot_bytes: u64, journal_bytes: u64, data_bytes: u64, entry_count: usize, extra_files: u64) -> Option<u64> {
    let count = u64::try_from(entry_count).ok()?.checked_add(INDEX_OVERHEAD)?.checked_add(extra_files)?;
    DIRECTORY_OVERHEAD
        .checked_add(snapshot_bytes)?
        .checked_add(journal_bytes)?
        .checked_add(data_bytes)?
        .checked_add(FILE_OVERHEAD.checked_mul(count)?)
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
