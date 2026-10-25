use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[cfg(test)]
use std::sync::Mutex;

use sha2::{Digest, Sha256};
use tempfile::{Builder, NamedTempFile};

use crate::format::{self, FormatError, Record};

pub const DEFAULT_SIZE_LIMIT: u64 = 1024 * 1024 * 1024;
pub const MAX_VALUE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_IN_FLIGHT_READS: usize = 8;
pub const MAX_QUEUED_WRITES: usize = 1;
const DIRECTORY_OVERHEAD: u64 = 4096;
const FILE_OVERHEAD: u64 = 512;
const MIN_INDEX_SIZE: u64 = format::INDEX_HEADER_BYTES;
const MIN_SIZE_LIMIT_INTERNAL: u64 = DIRECTORY_OVERHEAD + FILE_OVERHEAD * 2 + MIN_INDEX_SIZE;
const DATA_DIRECTORY: &str = "data";
const INDEX_FILE: &str = "index";
const LOCK_FILE: &str = "lock";
const INDEX_TEMP_PREFIX: &str = ".index-tmp-";
const DATA_TEMP_PREFIX: &str = ".data-tmp-";
const PERSIST_RETRIES: u32 = 4;

pub const MIN_SIZE_LIMIT: u64 = MIN_SIZE_LIMIT_INTERNAL;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Key([u8; 32]);

impl Key {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnavailableReason {
    ReadLimit,
    WriteQueueFull,
    WriterStopped,
    Io,
    Allocation,
}

#[derive(Debug)]
pub enum StorageError {
    InUse,
    Unavailable(UnavailableReason),
    LimitTooSmall,
    TooLarge,
    Io(io::Error),
    WorkerPanicked,
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InUse => f.write_str("cache directory is already in use"),
            Self::Unavailable(reason) => write!(f, "cache is unavailable: {reason:?}"),
            Self::LimitTooSmall => f.write_str("cache size limit is below the minimum accounting size"),
            Self::TooLarge => f.write_str("cache value or index exceeds its size limit"),
            Self::Io(error) => write!(f, "cache I/O failed: {error}"),
            Self::WorkerPanicked => f.write_str("cache writer stopped unexpectedly"),
        }
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for StorageError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheValue {
    pub bytes: Vec<u8>,
    pub last_use: u64,
    pub use_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Lookup {
    Hit(CacheValue),
    Miss,
    Corrupt,
    Unavailable(UnavailableReason),
}

struct Entry {
    key: Key,
    file_name: String,
    size: u64,
    hash: [u8; 32],
    last_use: AtomicU64,
    use_count: AtomicU64,
    invalid: AtomicBool,
}

impl Entry {
    fn from_record(record: Record) -> Self {
        Self {
            key: record.key,
            file_name: record.file_name,
            size: record.size,
            hash: record.hash,
            last_use: AtomicU64::new(record.last_use),
            use_count: AtomicU64::new(record.use_count),
            invalid: AtomicBool::new(false),
        }
    }

    fn record(&self) -> Record {
        Record {
            key: self.key,
            file_name: self.file_name.clone(),
            size: self.size,
            hash: self.hash,
            last_use: self.last_use.load(Ordering::Relaxed),
            use_count: self.use_count.load(Ordering::Relaxed),
        }
    }

    fn path(&self, data_dir: &Path) -> PathBuf {
        data_dir.join(&self.file_name)
    }
}

struct ReadState {
    data_dir: PathBuf,
    entries: RwLock<HashMap<Key, Arc<Entry>>>,
    active_reads: AtomicUsize,
    usage_clock: AtomicU64,
    size_limit: AtomicU64,
    accounted_size: AtomicU64,
    #[cfg(test)]
    hooks: Arc<FaultHooks>,
}

#[cfg(test)]
struct TestPause {
    point: WritePoint,
    started: SyncSender<()>,
    resume: Receiver<()>,
}

#[derive(Default)]
struct FaultHooks {
    #[cfg(test)]
    fail_at: AtomicUsize,
    #[cfg(test)]
    pause: Mutex<Option<TestPause>>,
}

impl FaultHooks {
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
                match pause.started.send(()) {
                    Ok(()) => {}
                    Err(_) => return Err(io::Error::other("test pause receiver stopped")),
                }
                match pause.resume.recv() {
                    Ok(()) => {}
                    Err(_) => return Err(io::Error::other("test pause was released")),
                }
            }
            if self.fail_at.compare_exchange(point as usize, 0, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                return Err(io::Error::other("simulated cache crash"));
            }
        }
        #[cfg(not(test))]
        let _ = point;
        Ok(())
    }

    #[cfg(test)]
    fn arm(&self, point: WritePoint) {
        self.fail_at.store(point as usize, Ordering::Release);
    }

    #[cfg(test)]
    fn pause_at(&self, point: WritePoint) -> (Receiver<()>, SyncSender<()>) {
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

#[repr(usize)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WritePoint {
    DataTempCreated = 1,
    DataWritten,
    DataSynced,
    DataRenamed,
    IndexTempCreated,
    IndexWritten,
    IndexSynced,
    IndexRenamed,
}

struct ReadPermit<'a> {
    active: &'a AtomicUsize,
}

impl<'a> ReadPermit<'a> {
    fn acquire(active: &'a AtomicUsize) -> Option<Self> {
        let mut count = active.load(Ordering::Relaxed);
        loop {
            if count >= MAX_IN_FLIGHT_READS {
                return None;
            }
            match active.compare_exchange_weak(count, count + 1, Ordering::Acquire, Ordering::Relaxed) {
                Ok(_) => return Some(Self { active }),
                Err(actual) => count = actual,
            }
        }
    }
}

impl Drop for ReadPermit<'_> {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Release);
    }
}

struct WriteFailure {
    error: io::Error,
    committed: bool,
}

struct WorkerState {
    root: PathBuf,
    data_dir: PathBuf,
    _lock_file: File,
    entries: HashMap<Key, Arc<Entry>>,
    size_limit: u64,
    read_state: Arc<ReadState>,
    hooks: Arc<FaultHooks>,
}

impl WorkerState {
    fn put(&mut self, key: Key, bytes: &[u8]) -> Result<(), StorageError> {
        let temp = Builder::new().prefix(DATA_TEMP_PREFIX).tempfile_in(&self.data_dir)?;
        let file_name = final_data_name(&temp)?;
        let tick = saturating_increment(&self.read_state.usage_clock);
        let entry = Arc::new(Entry {
            key,
            file_name: file_name.clone(),
            size: u64::try_from(bytes.len()).map_err(|_| StorageError::TooLarge)?,
            hash: Sha256::digest(bytes).into(),
            last_use: AtomicU64::new(tick),
            use_count: AtomicU64::new(0),
            invalid: AtomicBool::new(false),
        });
        let mut entries = self
            .entries
            .iter()
            .filter(|(_, entry)| !entry.invalid.load(Ordering::Acquire))
            .map(|(key, entry)| (*key, Arc::clone(entry)))
            .collect::<HashMap<_, _>>();
        entries.insert(key, Arc::clone(&entry));
        self.evict_to_limit(&mut entries, Some(key), self.size_limit)?;
        if !entries.contains_key(&key) {
            return Err(StorageError::TooLarge);
        }
        let final_path = self.data_dir.join(&file_name);
        if let Err(failure) = write_data(temp, bytes, &final_path, &self.hooks) {
            return Err(StorageError::Io(failure.error));
        }
        let encoded = match encode_entries(&entries) {
            Ok(encoded) => encoded,
            Err(error) => {
                match fs::remove_file(&final_path) {
                    Ok(()) => {}
                    Err(remove_error) if remove_error.kind() == io::ErrorKind::NotFound => {}
                    Err(remove_error) => return Err(StorageError::Io(remove_error)),
                }
                return Err(error);
            }
        };
        match write_index_atomic(&self.root, &encoded, &self.hooks) {
            Ok(()) => self.commit(entries),
            Err(failure) => {
                if !failure.committed {
                    match fs::remove_file(final_path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(StorageError::Io(error)),
                    }
                }
                Err(StorageError::Io(failure.error))
            }
        }
    }

    fn remove(&mut self, key: Key) -> Result<bool, StorageError> {
        let Some(_) = self.entries.get(&key) else {
            return Ok(false);
        };
        let mut entries = self
            .entries
            .iter()
            .filter(|(_, entry)| !entry.invalid.load(Ordering::Acquire))
            .map(|(key, entry)| (*key, Arc::clone(entry)))
            .collect::<HashMap<_, _>>();
        let removed = entries.remove(&key).is_some();
        self.replace(entries)?;
        Ok(removed)
    }

    fn clear(&mut self) -> Result<(), StorageError> {
        self.replace(HashMap::new())
    }

    fn set_limit(&mut self, limit: u64) -> Result<(), StorageError> {
        if limit < MIN_SIZE_LIMIT {
            return Err(StorageError::LimitTooSmall);
        }
        let mut entries = self
            .entries
            .iter()
            .filter(|(_, entry)| !entry.invalid.load(Ordering::Acquire))
            .map(|(key, entry)| (*key, Arc::clone(entry)))
            .collect::<HashMap<_, _>>();
        self.evict_to_limit(&mut entries, None, limit)?;
        self.replace_at_limit(entries, limit)
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        let entries = self
            .entries
            .iter()
            .filter(|(_, entry)| !entry.invalid.load(Ordering::Acquire))
            .map(|(key, entry)| (*key, Arc::clone(entry)))
            .collect::<HashMap<_, _>>();
        self.replace(entries)
    }

    fn remove_invalid(&mut self, key: Key, target: &Arc<Entry>) -> Result<(), StorageError> {
        let matches = self.entries.get(&key).is_some_and(|entry| Arc::ptr_eq(entry, target));
        if !matches {
            return Ok(());
        }
        self.prune_invalid()
    }

    fn prune_invalid(&mut self) -> Result<(), StorageError> {
        if !self.entries.values().any(|entry| entry.invalid.load(Ordering::Acquire)) {
            return Ok(());
        }
        let entries = self
            .entries
            .iter()
            .filter(|(_, entry)| !entry.invalid.load(Ordering::Acquire))
            .map(|(key, entry)| (*key, Arc::clone(entry)))
            .collect::<HashMap<_, _>>();
        match self.replace(entries) {
            Ok(()) => Ok(()),
            Err(error) => {
                if let Err(cleanup_error) = self.remove_invalid_data() {
                    tracing::warn!(error = %cleanup_error, "failed to unlink a corrupt cache payload");
                }
                Err(error)
            }
        }
    }

    fn remove_invalid_data(&self) -> Result<(), StorageError> {
        for entry in self.entries.values().filter(|entry| entry.invalid.load(Ordering::Acquire)) {
            match fs::remove_file(entry.path(&self.data_dir)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(StorageError::Io(error)),
            }
        }
        Ok(())
    }

    fn evict_to_limit(&self, entries: &mut HashMap<Key, Arc<Entry>>, protected: Option<Key>, limit: u64) -> Result<(), StorageError> {
        let records = records(entries);
        let mut encoded_bytes = index_size(&records).ok_or(StorageError::TooLarge)?;
        let mut data_bytes = record_data_size(&records).ok_or(StorageError::TooLarge)?;
        let mut file_count = u64::try_from(entries.len()).map_err(|_| StorageError::TooLarge)?;
        let mut accounted = account_parts(encoded_bytes, data_bytes, file_count).ok_or(StorageError::TooLarge)?;
        if encoded_bytes > format::MAX_INDEX_BYTES || records.len() > format::MAX_ENTRIES {
            accounted = u64::MAX;
        }
        for victim in eviction_order(entries) {
            if accounted <= limit {
                break;
            }
            if Some(victim) == protected {
                continue;
            }
            if let Some(entry) = entries.remove(&victim) {
                encoded_bytes = encoded_bytes.saturating_sub(format::entry_size(&entry.file_name).ok_or(StorageError::TooLarge)?);
                data_bytes = data_bytes.saturating_sub(entry.size);
                file_count = file_count.saturating_sub(1);
                accounted = account_parts(encoded_bytes, data_bytes, file_count).ok_or(StorageError::TooLarge)?;
                if encoded_bytes > format::MAX_INDEX_BYTES || entries.len() > format::MAX_ENTRIES {
                    accounted = u64::MAX;
                }
            }
        }
        if accounted > limit {
            return Err(StorageError::TooLarge);
        }
        Ok(())
    }

    fn replace(&mut self, entries: HashMap<Key, Arc<Entry>>) -> Result<(), StorageError> {
        self.replace_at_limit(entries, self.size_limit)
    }

    fn replace_at_limit(&mut self, entries: HashMap<Key, Arc<Entry>>, limit: u64) -> Result<(), StorageError> {
        let records = records(&entries);
        let accounted = account_size(&records).ok_or(StorageError::TooLarge)?;
        if accounted > limit {
            return Err(StorageError::TooLarge);
        }
        let encoded = encode_entries(&entries)?;
        match write_index_atomic(&self.root, &encoded, &self.hooks) {
            Ok(()) => {
                self.size_limit = limit;
                self.commit(entries)
            }
            Err(failure) => Err(StorageError::Io(failure.error)),
        }
    }

    fn commit(&mut self, entries: HashMap<Key, Arc<Entry>>) -> Result<(), StorageError> {
        let old_entries = std::mem::replace(&mut self.entries, entries);
        let snapshot = self.entries.iter().map(|(key, entry)| (*key, Arc::clone(entry))).collect::<HashMap<_, _>>();
        {
            let mut read_entries = match self.read_state.entries.write() {
                Ok(entries) => entries,
                Err(poisoned) => poisoned.into_inner(),
            };
            *read_entries = snapshot;
        }
        self.read_state.size_limit.store(self.size_limit, Ordering::Relaxed);
        let current_records = records(&self.entries);
        let accounted = account_size(&current_records).ok_or(StorageError::TooLarge)?;
        self.read_state.accounted_size.store(accounted, Ordering::Relaxed);
        let mut removed = old_entries
            .values()
            .filter(|old| !self.entries.get(&old.key).is_some_and(|current| Arc::ptr_eq(current, old)))
            .map(|entry| entry.file_name.clone())
            .collect::<HashSet<_>>();
        for file_name in removed.drain() {
            match fs::remove_file(self.data_dir.join(file_name)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(StorageError::Io(error)),
            }
        }
        Ok(())
    }
}

enum Command {
    Put {
        key: Key,
        bytes: Vec<u8>,
        reply: SyncSender<Result<(), StorageError>>,
    },
    Remove {
        key: Key,
        reply: SyncSender<Result<bool, StorageError>>,
    },
    Clear {
        reply: SyncSender<Result<(), StorageError>>,
    },
    SetLimit {
        limit: u64,
        reply: SyncSender<Result<(), StorageError>>,
    },
    Invalidate {
        key: Key,
        entry: Arc<Entry>,
    },
    Flush {
        reply: SyncSender<Result<(), StorageError>>,
    },
    Shutdown {
        reply: SyncSender<Result<(), StorageError>>,
    },
    #[cfg(test)]
    Abandon {
        reply: SyncSender<()>,
    },
}

pub struct Cache {
    commands: SyncSender<Command>,
    worker: Option<JoinHandle<()>>,
    read_state: Arc<ReadState>,
}

impl fmt::Debug for Cache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Cache")
    }
}

impl Cache {
    pub fn open(directory: impl AsRef<Path>, size_limit: u64) -> Result<Self, StorageError> {
        Self::open_internal(directory.as_ref(), size_limit, Arc::new(FaultHooks::default()))
    }

    pub fn lookup(&self, key: Key) -> Lookup {
        let Some(_permit) = ReadPermit::acquire(&self.read_state.active_reads) else {
            return Lookup::Unavailable(UnavailableReason::ReadLimit);
        };
        let (entry, file) = {
            let entries = match self.read_state.entries.read() {
                Ok(entries) => entries,
                Err(poisoned) => poisoned.into_inner(),
            };
            let Some(entry) = entries.get(&key).cloned() else {
                return Lookup::Miss;
            };
            if entry.invalid.load(Ordering::Acquire) {
                self.invalidate(key, entry);
                return Lookup::Corrupt;
            }
            match File::open(entry.path(&self.read_state.data_dir)) {
                Ok(file) => (entry, file),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    drop(entries);
                    self.invalidate(key, entry);
                    return Lookup::Corrupt;
                }
                Err(_) => return Lookup::Unavailable(UnavailableReason::Io),
            }
        };
        let metadata = match file.metadata() {
            Ok(metadata) => metadata,
            Err(_) => return Lookup::Unavailable(UnavailableReason::Io),
        };
        if !metadata.is_file() || metadata.len() != entry.size {
            self.invalidate(key, entry);
            return Lookup::Corrupt;
        }
        let capacity = match usize::try_from(entry.size) {
            Ok(capacity) => capacity,
            Err(_) => {
                self.invalidate(key, entry);
                return Lookup::Corrupt;
            }
        };
        let mut bytes = Vec::new();
        if bytes.try_reserve_exact(capacity).is_err() {
            return Lookup::Unavailable(UnavailableReason::Allocation);
        }
        let limit = entry.size.saturating_add(1);
        let read_result = file.take(limit).read_to_end(&mut bytes);
        if let Err(error) = read_result {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                self.invalidate(key, entry);
                return Lookup::Corrupt;
            }
            return Lookup::Unavailable(UnavailableReason::Io);
        }
        if u64::try_from(bytes.len()).ok() != Some(entry.size) || Sha256::digest(&bytes).as_slice() != entry.hash {
            self.invalidate(key, entry);
            return Lookup::Corrupt;
        }
        let stamp = saturating_increment(&self.read_state.usage_clock);
        entry.last_use.store(stamp, Ordering::Relaxed);
        let use_count = saturating_increment(&entry.use_count);
        Lookup::Hit(CacheValue { bytes, last_use: stamp, use_count })
    }

    pub fn put(&self, key: Key, bytes: Vec<u8>) -> Result<(), StorageError> {
        if bytes.len() > MAX_VALUE_BYTES {
            return Err(StorageError::TooLarge);
        }
        let (reply, receiver) = mpsc::sync_channel(0);
        self.send(Command::Put { key, bytes, reply })?;
        receive(receiver)
    }

    pub fn remove(&self, key: Key) -> Result<bool, StorageError> {
        let (reply, receiver) = mpsc::sync_channel(0);
        self.send(Command::Remove { key, reply })?;
        receive(receiver)
    }

    pub fn discard(&self, key: Key) {
        let entries = match self.read_state.entries.read() {
            Ok(entries) => entries,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(entry) = entries.get(&key).cloned() {
            drop(entries);
            self.invalidate(key, entry);
        }
    }

    pub fn clear(&self) -> Result<(), StorageError> {
        let (reply, receiver) = mpsc::sync_channel(0);
        self.send(Command::Clear { reply })?;
        receive(receiver)
    }

    pub fn set_size_limit(&self, limit: u64) -> Result<(), StorageError> {
        if limit < MIN_SIZE_LIMIT {
            return Err(StorageError::LimitTooSmall);
        }
        let (reply, receiver) = mpsc::sync_channel(0);
        self.send(Command::SetLimit { limit, reply })?;
        receive(receiver)
    }

    pub fn flush(&self) -> Result<(), StorageError> {
        let (reply, receiver) = mpsc::sync_channel(0);
        self.send(Command::Flush { reply })?;
        receive(receiver)
    }

    pub fn size_limit(&self) -> u64 {
        self.read_state.size_limit.load(Ordering::Relaxed)
    }

    pub fn accounted_size(&self) -> u64 {
        self.read_state.accounted_size.load(Ordering::Relaxed)
    }

    pub fn entry_count(&self) -> usize {
        let entries = match self.read_state.entries.read() {
            Ok(entries) => entries,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.values().filter(|entry| !entry.invalid.load(Ordering::Acquire)).count()
    }

    pub fn shutdown(mut self) -> Result<(), StorageError> {
        let (reply, receiver) = mpsc::sync_channel(0);
        self.commands.send(Command::Shutdown { reply }).map_err(|_| StorageError::Unavailable(UnavailableReason::WriterStopped))?;
        let result = receive(receiver);
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| StorageError::WorkerPanicked)?;
        }
        result
    }

    #[cfg(test)]
    fn simulate_crash(mut self) -> Result<(), StorageError> {
        let (reply, receiver) = mpsc::sync_channel(0);
        self.commands.send(Command::Abandon { reply }).map_err(|_| StorageError::Unavailable(UnavailableReason::WriterStopped))?;
        receiver.recv().map_err(|_| StorageError::Unavailable(UnavailableReason::WriterStopped))?;
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| StorageError::WorkerPanicked)?;
        }
        Ok(())
    }

    fn open_internal(directory: &Path, size_limit: u64, hooks: Arc<FaultHooks>) -> Result<Self, StorageError> {
        if size_limit < MIN_SIZE_LIMIT {
            return Err(StorageError::LimitTooSmall);
        }
        fs::create_dir_all(directory)?;
        let root = directory.to_path_buf();
        let data_dir = root.join(DATA_DIRECTORY);
        fs::create_dir_all(&data_dir)?;
        let lock_file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(root.join(LOCK_FILE))?;
        match lock_file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(StorageError::InUse),
            Err(std::fs::TryLockError::Error(error)) => return Err(StorageError::Io(error)),
        }
        remove_prefixed_files(&root, INDEX_TEMP_PREFIX)?;
        let (entries, index_changed) = recover(&root, &data_dir)?;
        let mut entries = entries;
        let mut adjusted = index_changed;
        let initial_records = records(&entries);
        let mut accounted = account_size(&initial_records).ok_or(StorageError::TooLarge)?;
        if accounted > size_limit {
            let mut index_bytes = index_size(&initial_records).ok_or(StorageError::TooLarge)?;
            let mut data_bytes = record_data_size(&initial_records).ok_or(StorageError::TooLarge)?;
            let mut file_count = u64::try_from(entries.len()).map_err(|_| StorageError::TooLarge)?;
            for key in eviction_order(&entries) {
                if accounted <= size_limit {
                    break;
                }
                if let Some(entry) = entries.remove(&key) {
                    let entry_index_size = format::entry_size(&entry.file_name).ok_or(StorageError::TooLarge)?;
                    index_bytes = index_bytes.saturating_sub(entry_index_size);
                    data_bytes = data_bytes.saturating_sub(entry.size);
                    file_count = file_count.saturating_sub(1);
                    accounted = account_parts(index_bytes, data_bytes, file_count).ok_or(StorageError::TooLarge)?;
                    adjusted = true;
                }
            }
        }
        if adjusted || !root.join(INDEX_FILE).exists() {
            let encoded = encode_entries(&entries)?;
            write_index_atomic(&root, &encoded, &hooks).map_err(|failure| StorageError::Io(failure.error))?;
        }
        let referenced = entries.values().map(|entry| entry.file_name.clone()).collect::<HashSet<_>>();
        remove_unreferenced_files(&data_dir, &referenced)?;
        let records = records(&entries);
        accounted = account_size(&records).ok_or(StorageError::TooLarge)?;
        if accounted > size_limit {
            return Err(StorageError::TooLarge);
        }
        let max_tick = entries.values().map(|entry| entry.last_use.load(Ordering::Relaxed)).max().unwrap_or(0);
        let read_state = Arc::new(ReadState {
            data_dir: data_dir.clone(),
            entries: RwLock::new(entries.clone()),
            active_reads: AtomicUsize::new(0),
            usage_clock: AtomicU64::new(max_tick),
            size_limit: AtomicU64::new(size_limit),
            accounted_size: AtomicU64::new(accounted),
            #[cfg(test)]
            hooks: Arc::clone(&hooks),
        });
        let worker_state =
            WorkerState { root, data_dir, _lock_file: lock_file, entries, size_limit, read_state: Arc::clone(&read_state), hooks };
        let (commands, receiver) = mpsc::sync_channel(MAX_QUEUED_WRITES);
        let worker = thread::Builder::new().name("storage-cache-writer".to_owned()).spawn(move || writer_loop(worker_state, receiver))?;
        Ok(Self { commands, worker: Some(worker), read_state })
    }

    fn send(&self, command: Command) -> Result<(), StorageError> {
        match self.commands.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(StorageError::Unavailable(UnavailableReason::WriteQueueFull)),
            Err(TrySendError::Disconnected(_)) => Err(StorageError::Unavailable(UnavailableReason::WriterStopped)),
        }
    }

    fn invalidate(&self, key: Key, entry: Arc<Entry>) {
        entry.invalid.store(true, Ordering::Release);
        match self.commands.try_send(Command::Invalidate { key, entry }) {
            Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
        }
    }

    #[cfg(test)]
    fn arm_fault(&self, point: WritePoint) {
        self.read_state.hooks.arm(point);
    }

    #[cfg(test)]
    fn pause_at(&self, point: WritePoint) -> (Receiver<()>, SyncSender<()>) {
        self.read_state.hooks.pause_at(point)
    }
}

fn resolve_default_cache_dir() -> Result<PathBuf, StorageError> {
    dirs::cache_dir()
        .map(|directory| directory.join("radia").join("storage"))
        .ok_or(StorageError::Io(io::Error::new(io::ErrorKind::NotFound, "local cache directory is unavailable")))
}

pub fn default_cache_dir() -> Result<PathBuf, StorageError> {
    resolve_default_cache_dir()
}

fn saturating_increment(value: &AtomicU64) -> u64 {
    match value.try_update(Ordering::Relaxed, Ordering::Relaxed, |current| Some(current.saturating_add(1))) {
        Ok(previous) => previous.saturating_add(1),
        Err(current) => current,
    }
}

fn receive<T>(receiver: Receiver<Result<T, StorageError>>) -> Result<T, StorageError> {
    receiver.recv().map_err(|_| StorageError::Unavailable(UnavailableReason::WriterStopped))?
}

fn reply<T>(sender: SyncSender<T>, value: T) {
    match sender.send(value) {
        Ok(()) | Err(_) => {}
    }
}

fn writer_loop(mut state: WorkerState, receiver: Receiver<Command>) {
    loop {
        let command = match receiver.recv() {
            Ok(command) => command,
            Err(_) => {
                if let Err(error) = state.prune_invalid().and_then(|()| state.flush()) {
                    tracing::warn!(error = %error, "failed to flush cache usage during shutdown");
                }
                return;
            }
        };
        match command {
            Command::Put { key, bytes, reply: response } => {
                let result = state.prune_invalid().and_then(|()| state.put(key, &bytes));
                reply(response, result);
            }
            Command::Remove { key, reply: response } => {
                let result = state.prune_invalid().and_then(|()| state.remove(key));
                reply(response, result);
            }
            Command::Clear { reply: response } => {
                let result = state.prune_invalid().and_then(|()| state.clear());
                reply(response, result);
            }
            Command::SetLimit { limit, reply: response } => {
                let result = state.prune_invalid().and_then(|()| state.set_limit(limit));
                reply(response, result);
            }
            Command::Invalidate { key, entry } => {
                let result = state.remove_invalid(key, &entry).and_then(|()| state.prune_invalid());
                if let Err(error) = result {
                    tracing::warn!(error = %error, "failed to remove a corrupt cache entry");
                }
            }
            Command::Flush { reply: response } => {
                let result = state.prune_invalid().and_then(|()| state.flush());
                reply(response, result);
            }
            Command::Shutdown { reply: response } => {
                let result = state.prune_invalid().and_then(|()| state.flush());
                reply(response, result);
                return;
            }
            #[cfg(test)]
            Command::Abandon { reply: response } => {
                reply(response, ());
                return;
            }
        }
    }
}

fn records(entries: &HashMap<Key, Arc<Entry>>) -> Vec<Record> {
    let mut records = entries.values().map(|entry| entry.record()).collect::<Vec<_>>();
    records.sort_unstable_by_key(|record| record.key);
    records
}

fn encode_entries(entries: &HashMap<Key, Arc<Entry>>) -> Result<Vec<u8>, StorageError> {
    format::encode(&records(entries)).map_err(|error| match error {
        FormatError::TooLarge => StorageError::TooLarge,
        FormatError::Invalid | FormatError::UnsupportedVersion => {
            StorageError::Io(io::Error::new(io::ErrorKind::InvalidData, "invalid cache index"))
        }
    })
}

fn index_size(records: &[Record]) -> Option<u64> {
    format::encoded_size(records)
}

fn record_data_size(records: &[Record]) -> Option<u64> {
    records.iter().try_fold(0u64, |sum, record| sum.checked_add(record.size))
}

fn account_parts(index_bytes: u64, data_bytes: u64, entry_count: u64) -> Option<u64> {
    let file_count = entry_count.checked_add(2)?;
    DIRECTORY_OVERHEAD.checked_add(index_bytes)?.checked_add(data_bytes)?.checked_add(FILE_OVERHEAD.checked_mul(file_count)?)
}

fn account_size(records: &[Record]) -> Option<u64> {
    let index_bytes = format::encoded_size(records)?;
    let data_bytes = record_data_size(records)?;
    account_parts(index_bytes, data_bytes, u64::try_from(records.len()).ok()?)
}

fn eviction_order(entries: &HashMap<Key, Arc<Entry>>) -> Vec<Key> {
    let mut order = entries.values().map(|entry| (entry.last_use.load(Ordering::Relaxed), entry.key)).collect::<Vec<_>>();
    order.sort_unstable();
    order.into_iter().map(|(_, key)| key).collect()
}

fn final_data_name(temp: &NamedTempFile) -> Result<String, StorageError> {
    let base_name = temp
        .path()
        .file_name()
        .ok_or_else(|| StorageError::Io(io::Error::new(io::ErrorKind::InvalidData, "temporary data file has no name")))?;
    let base_name = base_name.to_string_lossy();
    let nonce = base_name
        .strip_prefix(DATA_TEMP_PREFIX)
        .ok_or_else(|| StorageError::Io(io::Error::new(io::ErrorKind::InvalidData, "temporary data file name is invalid")))?;
    let file_name = format!("data-{nonce}.bin");
    if !format::valid_file_name(&file_name) {
        return Err(StorageError::Io(io::Error::new(io::ErrorKind::InvalidData, "temporary data file name is too long")));
    }
    Ok(file_name)
}

fn write_data(mut temp: NamedTempFile, bytes: &[u8], destination: &Path, hooks: &FaultHooks) -> Result<(), WriteFailure> {
    if let Err(error) = hooks.check(WritePoint::DataTempCreated) {
        preserve_temp(temp).map_err(|preserve| WriteFailure { error: preserve, committed: false })?;
        return Err(WriteFailure { error, committed: false });
    }
    if let Err(error) = temp.write_all(bytes) {
        return Err(WriteFailure { error, committed: false });
    }
    if let Err(error) = hooks.check(WritePoint::DataWritten) {
        preserve_temp(temp).map_err(|preserve| WriteFailure { error: preserve, committed: false })?;
        return Err(WriteFailure { error, committed: false });
    }
    if let Err(error) = temp.as_file().sync_all() {
        return Err(WriteFailure { error, committed: false });
    }
    if let Err(error) = hooks.check(WritePoint::DataSynced) {
        preserve_temp(temp).map_err(|preserve| WriteFailure { error: preserve, committed: false })?;
        return Err(WriteFailure { error, committed: false });
    }
    persist_with_retry(temp, destination).map_err(|error| WriteFailure { error, committed: false })?;
    if let Err(error) = hooks.check(WritePoint::DataRenamed) {
        return Err(WriteFailure { error, committed: true });
    }
    Ok(())
}

fn preserve_temp(temp: NamedTempFile) -> io::Result<()> {
    temp.keep().map(|_| ()).map_err(|error| error.error)
}

fn write_index_atomic(root: &Path, bytes: &[u8], hooks: &FaultHooks) -> Result<(), WriteFailure> {
    let destination = root.join(INDEX_FILE);
    let temp = Builder::new().prefix(INDEX_TEMP_PREFIX).tempfile_in(root).map_err(|error| WriteFailure { error, committed: false })?;
    let mut temp = temp;
    if let Err(error) = hooks.check(WritePoint::IndexTempCreated) {
        preserve_temp(temp).map_err(|preserve| WriteFailure { error: preserve, committed: false })?;
        return Err(WriteFailure { error, committed: false });
    }
    if let Err(error) = temp.write_all(bytes) {
        return Err(WriteFailure { error, committed: false });
    }
    if let Err(error) = hooks.check(WritePoint::IndexWritten) {
        preserve_temp(temp).map_err(|preserve| WriteFailure { error: preserve, committed: false })?;
        return Err(WriteFailure { error, committed: false });
    }
    if let Err(error) = temp.as_file().sync_all() {
        return Err(WriteFailure { error, committed: false });
    }
    if let Err(error) = hooks.check(WritePoint::IndexSynced) {
        preserve_temp(temp).map_err(|preserve| WriteFailure { error: preserve, committed: false })?;
        return Err(WriteFailure { error, committed: false });
    }
    persist_with_retry(temp, &destination).map_err(|error| WriteFailure { error, committed: false })?;
    if let Err(error) = hooks.check(WritePoint::IndexRenamed) {
        return Err(WriteFailure { error, committed: true });
    }
    Ok(())
}

fn persist_with_retry(mut temp: NamedTempFile, destination: &Path) -> io::Result<()> {
    for attempt in 0..PERSIST_RETRIES {
        match temp.persist(destination) {
            Ok(file) => {
                drop(file);
                return Ok(());
            }
            Err(failure) => {
                let error = failure.error;
                if error.kind() != io::ErrorKind::PermissionDenied || attempt + 1 == PERSIST_RETRIES {
                    return Err(error);
                }
                temp = failure.file;
                thread::sleep(Duration::from_millis(u64::from(attempt + 1) * 2));
            }
        }
    }
    Err(io::Error::other("temporary file persistence retries exhausted"))
}

fn recover(root: &Path, data_dir: &Path) -> Result<(HashMap<Key, Arc<Entry>>, bool), StorageError> {
    let index_path = root.join(INDEX_FILE);
    let bytes = match read_bounded(&index_path, format::MAX_INDEX_BYTES as usize) {
        Ok(Some(bytes)) => Some(bytes),
        Ok(None) => None,
        Err(_) => None,
    };
    let decoded = bytes.as_deref().map(format::decode);
    let mut changed = false;
    let records = match decoded {
        Some(Ok(records)) => records,
        Some(Err(_)) => {
            clear_data_directory(data_dir)?;
            changed = true;
            Vec::new()
        }
        None => {
            changed = true;
            Vec::new()
        }
    };
    let mut entries = HashMap::with_capacity(records.len());
    for record in records {
        let path = data_dir.join(&record.file_name);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                changed = true;
                continue;
            }
        };
        if !metadata.file_type().is_file() || metadata.len() != record.size {
            changed = true;
            continue;
        }
        entries.insert(record.key, Arc::new(Entry::from_record(record)));
    }
    let referenced = entries.values().map(|entry| entry.file_name.clone()).collect::<HashSet<_>>();
    if remove_unreferenced_files(data_dir, &referenced)? {
        changed = true;
    }
    Ok((entries, changed))
}

fn read_bounded(path: &Path, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if metadata.len() > u64::try_from(limit).unwrap_or(u64::MAX) {
        return Ok(Some(Vec::new()));
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "file length is too large"))?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(capacity).map_err(|_| io::Error::new(io::ErrorKind::OutOfMemory, "index allocation failed"))?;
    let max_read = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    file.take(max_read).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Ok(Some(Vec::new()));
    }
    Ok(Some(bytes))
}

fn remove_prefixed_files(directory: &Path, prefix: &str) -> io::Result<()> {
    for item in fs::read_dir(directory)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name.starts_with(prefix) {
            remove_path(&item.path())?;
        }
    }
    Ok(())
}

fn remove_unreferenced_files(data_dir: &Path, referenced: &HashSet<String>) -> io::Result<bool> {
    let mut changed = false;
    for item in fs::read_dir(data_dir)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        if !referenced.contains(&name) {
            remove_path(&item.path())?;
            changed = true;
        }
    }
    Ok(changed)
}

fn clear_data_directory(data_dir: &Path) -> io::Result<()> {
    for item in fs::read_dir(data_dir)? {
        let item = item?;
        remove_path(&item.path())?;
    }
    Ok(())
}

fn remove_path(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
