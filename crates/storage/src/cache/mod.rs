mod eviction;
mod journal;
mod recovery;
mod writer;

use std::{
    collections::HashMap,
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
};

use sha2::{Digest, Sha256};

#[cfg(test)]
use std::sync::mpsc::SyncSender;

use self::{
    journal::{FormatError, Record},
    recovery::{discard_index_files, recover},
    writer::{Command, WorkerState, WriteQueue, writer_loop},
};

#[cfg(test)]
use self::writer::WritePoint;

pub const DEFAULT_SIZE_LIMIT: u64 = 1024 * 1024 * 1024;
pub const MAX_VALUE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_IN_FLIGHT_READS: usize = 8;
pub const MAX_QUEUED_WRITE_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_QUEUED_WRITES: usize = 1024;
const DIRECTORY_OVERHEAD: u64 = 4096;
const FILE_OVERHEAD: u64 = 512;
const INDEX_OVERHEAD: u64 = 3;
const MIN_SIZE_LIMIT_INTERNAL: u64 = DIRECTORY_OVERHEAD + FILE_OVERHEAD * 4 + journal::SNAPSHOT_HEADER_BYTES;
const DATA_DIRECTORY: &str = "data";
const SNAPSHOT_FILE: &str = "snapshot";
const JOURNAL_FILE: &str = "journal";
const LOCK_FILE: &str = "lock";

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

    pub(super) const fn as_bytes(&self) -> &[u8; 32] {
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
    pub key: Key,
    pub file_name: String,
    pub size: u64,
    pub hash: [u8; 32],
    pub last_use: AtomicU64,
    pub use_count: AtomicU64,
    pub invalid: AtomicBool,
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
    entries: Arc<RwLock<HashMap<Key, Arc<Entry>>>>,
    touches: Arc<Mutex<HashMap<Key, Arc<Entry>>>>,
    invalid: Arc<Mutex<HashMap<Key, Arc<Entry>>>>,
    active_reads: AtomicUsize,
    usage_clock: AtomicU64,
    size_limit: AtomicU64,
    accounted_size: AtomicU64,
    #[cfg(test)]
    hooks: Arc<writer::FaultHooks>,
}

struct ReadPermit<'a> {
    active: &'a AtomicUsize,
}

impl ReadPermit<'_> {
    fn acquire(active: &AtomicUsize) -> Option<ReadPermit<'_>> {
        let mut count = active.load(Ordering::Relaxed);
        loop {
            if count >= MAX_IN_FLIGHT_READS {
                return None;
            }
            match active.compare_exchange_weak(count, count + 1, Ordering::Acquire, Ordering::Relaxed) {
                Ok(_) => return Some(ReadPermit { active }),
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

pub struct Cache {
    queue: Arc<WriteQueue>,
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
        Self::open_internal(directory.as_ref(), size_limit, Arc::new(writer::FaultHooks::default()))
    }

    pub fn lookup(&self, key: Key) -> Lookup {
        let Some(_permit) = ReadPermit::acquire(&self.read_state.active_reads) else {
            return Lookup::Unavailable(UnavailableReason::ReadLimit);
        };
        let entry = {
            let entries = match self.read_state.entries.read() {
                Ok(entries) => entries,
                Err(poisoned) => poisoned.into_inner(),
            };
            let Some(entry) = entries.get(&key).cloned() else {
                return Lookup::Miss;
            };
            entry
        };
        if entry.invalid.load(Ordering::Acquire) {
            return Lookup::Corrupt;
        }
        let file = match File::open(entry.path(&self.read_state.data_dir)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.mark_invalid(key, entry);
                return Lookup::Corrupt;
            }
            Err(_) => return Lookup::Unavailable(UnavailableReason::Io),
        };
        let metadata = match file.metadata() {
            Ok(metadata) => metadata,
            Err(_) => return Lookup::Unavailable(UnavailableReason::Io),
        };
        if !metadata.is_file() || metadata.len() != entry.size {
            self.mark_invalid(key, entry);
            return Lookup::Corrupt;
        }
        let capacity = match usize::try_from(entry.size) {
            Ok(capacity) => capacity,
            Err(_) => {
                self.mark_invalid(key, entry);
                return Lookup::Corrupt;
            }
        };
        let mut bytes = Vec::new();
        if bytes.try_reserve_exact(capacity).is_err() {
            return Lookup::Unavailable(UnavailableReason::Allocation);
        }
        match file.take(entry.size.saturating_add(1)).read_to_end(&mut bytes) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                self.mark_invalid(key, entry);
                return Lookup::Corrupt;
            }
            Err(_) => return Lookup::Unavailable(UnavailableReason::Io),
        }
        if u64::try_from(bytes.len()).ok() != Some(entry.size) || Sha256::digest(&bytes).as_slice() != entry.hash {
            self.mark_invalid(key, entry);
            return Lookup::Corrupt;
        }
        let stamp = saturating_increment(&self.read_state.usage_clock);
        entry.last_use.fetch_max(stamp, Ordering::Relaxed);
        let use_count = saturating_increment(&entry.use_count);
        let mut touches = match self.read_state.touches.lock() {
            Ok(touches) => touches,
            Err(poisoned) => poisoned.into_inner(),
        };
        if touches.try_reserve(1).is_ok() {
            touches.insert(key, Arc::clone(&entry));
        }
        Lookup::Hit(CacheValue { bytes, last_use: stamp, use_count })
    }

    pub fn put(&self, key: Key, bytes: Vec<u8>) -> Result<(), StorageError> {
        if bytes.len() > MAX_VALUE_BYTES {
            return Err(StorageError::TooLarge);
        }
        let weight = bytes.len();
        let (reply, receiver) = mpsc::sync_channel(0);
        self.queue.try_send(Command::Put { key, bytes, reply }, weight)?;
        receive(receiver)
    }

    pub fn remove(&self, key: Key) -> Result<bool, StorageError> {
        let (reply, receiver) = mpsc::sync_channel(0);
        self.queue.try_send(Command::Remove { key, reply }, 0)?;
        receive(receiver)
    }

    pub fn discard(&self, key: Key) {
        let entries = match self.read_state.entries.read() {
            Ok(entries) => entries,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(entry) = entries.get(&key).cloned() else {
            return;
        };
        drop(entries);
        self.mark_invalid(key, entry);
    }

    pub fn clear(&self) -> Result<(), StorageError> {
        let (reply, receiver) = mpsc::sync_channel(0);
        self.queue.try_send(Command::Clear { reply }, 0)?;
        receive(receiver)
    }

    pub fn set_size_limit(&self, limit: u64) -> Result<(), StorageError> {
        if limit < MIN_SIZE_LIMIT {
            return Err(StorageError::LimitTooSmall);
        }
        let (reply, receiver) = mpsc::sync_channel(0);
        self.queue.try_send(Command::SetLimit { limit, reply }, 0)?;
        receive(receiver)
    }

    pub fn flush(&self) -> Result<(), StorageError> {
        let (reply, receiver) = mpsc::sync_channel(0);
        self.queue.try_send(Command::Flush { reply }, 0)?;
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
        self.queue.send_terminal(Command::Shutdown { reply })?;
        let result = receive(receiver);
        self.queue.close();
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| StorageError::WorkerPanicked)?;
        }
        result
    }

    #[cfg(test)]
    fn simulate_crash(mut self) -> Result<(), StorageError> {
        self.queue.close();
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| StorageError::WorkerPanicked)?;
        }
        Ok(())
    }

    fn open_internal(directory: &Path, size_limit: u64, hooks: Arc<writer::FaultHooks>) -> Result<Self, StorageError> {
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
        let (records, sequence, snapshot_bytes, journal_bytes, reset) = recover(&root)?;
        if reset {
            discard_index_files(&root)?;
        }
        let mut entries = HashMap::new();
        entries.try_reserve(records.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        for record in records {
            entries.insert(record.key, Arc::new(Entry::from_record(record)));
        }
        let max_tick = entries.values().map(|entry| entry.last_use.load(Ordering::Relaxed)).max().unwrap_or(0);
        let entries = Arc::new(RwLock::new(entries));
        let touches = Arc::new(Mutex::new(HashMap::new()));
        let invalid = Arc::new(Mutex::new(HashMap::new()));
        let queue = Arc::new(WriteQueue::default());
        let read_state = Arc::new(ReadState {
            data_dir,
            entries,
            touches,
            invalid,
            active_reads: AtomicUsize::new(0),
            usage_clock: AtomicU64::new(max_tick),
            size_limit: AtomicU64::new(size_limit),
            accounted_size: AtomicU64::new(0),
            #[cfg(test)]
            hooks: Arc::clone(&hooks),
        });
        let worker_state = WorkerState::new(
            root,
            read_state.data_dir.clone(),
            lock_file,
            Arc::clone(&read_state),
            sequence,
            snapshot_bytes,
            journal_bytes,
            hooks,
        )?;
        let startup_queue = Arc::clone(&queue);
        let worker =
            thread::Builder::new().name("storage-cache-writer".to_owned()).spawn(move || writer_loop(worker_state, startup_queue))?;
        let cache = Self { queue, worker: Some(worker), read_state };
        if cache.accounted_size() > size_limit {
            cache.set_size_limit(size_limit)?;
        }
        Ok(cache)
    }

    fn mark_invalid(&self, key: Key, entry: Arc<Entry>) {
        let entries = match self.read_state.entries.read() {
            Ok(entries) => entries,
            Err(poisoned) => poisoned.into_inner(),
        };
        let matches = entries.get(&key).is_some_and(|current| Arc::ptr_eq(current, &entry));
        if !matches {
            return;
        }
        entry.invalid.store(true, Ordering::Release);
        drop(entries);
        let mut invalid = match self.read_state.invalid.lock() {
            Ok(invalid) => invalid,
            Err(poisoned) => poisoned.into_inner(),
        };
        if invalid.try_reserve(1).is_err() {
            return;
        }
        invalid.insert(key, entry);
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

impl Drop for Cache {
    fn drop(&mut self) {
        self.queue.close();
    }
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

fn format_error(error: FormatError) -> StorageError {
    match error {
        FormatError::TooLarge => StorageError::TooLarge,
        FormatError::Allocation => StorageError::Unavailable(UnavailableReason::Allocation),
        FormatError::Invalid | FormatError::UnsupportedVersion => {
            StorageError::Io(io::Error::new(io::ErrorKind::InvalidData, "cache index encoding failed"))
        }
    }
}

pub fn default_cache_dir() -> Result<PathBuf, StorageError> {
    let root =
        dirs::cache_dir().ok_or(StorageError::Io(io::Error::new(io::ErrorKind::NotFound, "local cache directory is unavailable")))?;
    Ok(if cfg!(windows) {
        root.join("Radia").join("cache")
    } else if cfg!(target_os = "macos") {
        root.join("Radia")
    } else {
        root.join("radia")
    })
}

#[cfg(test)]
mod tests;
