mod eviction;
mod index;
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
        atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
};

use sha2::{Digest, Sha256};

#[cfg(test)]
use std::sync::mpsc::SyncSender;

use self::{
    index::{Index, Record},
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
const INDEX_FILE_OVERHEAD: u64 = 2;
const MAX_NAMESPACE_BYTES: usize = 127;
const MAX_NAMESPACE_SEGMENTS: usize = 8;
const MAX_NAMESPACE_SEGMENT_BYTES: usize = 32;
const MAX_EXTENSION_BYTES: usize = 16;
const MAX_NAMESPACE_TABLE_BYTES: u64 = 256 * (MAX_NAMESPACE_BYTES as u64 + MAX_EXTENSION_BYTES as u64 + 32);
const MIN_SIZE_LIMIT_INTERNAL: u64 = DIRECTORY_OVERHEAD * 3 + FILE_OVERHEAD * 4 + index::INDEX_BASE_BYTES + MAX_NAMESPACE_TABLE_BYTES;
const LOOKUP_MAX_ATTEMPTS: usize = 3;
const SNAPSHOT_FILE: &str = "snapshot";
const JOURNAL_FILE: &str = "journal";
const LOCK_FILE: &str = "lock";
const INDEX_FILE: &str = "index.sqlite";
const ENTRY_VALID: u8 = 0;
const ENTRY_REPLACING: u8 = 1;
const ENTRY_INVALID: u8 = 2;

pub const MIN_SIZE_LIMIT: u64 = MIN_SIZE_LIMIT_INTERNAL;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Namespace {
    directory: &'static str,
    extension: &'static str,
}

impl Namespace {
    pub const fn new(directory: &'static str, extension: &'static str) -> Self {
        Self { directory, extension }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NamespaceId(u8);

impl NamespaceId {
    pub const fn new(index: u8) -> Self {
        Self(index)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Key {
    namespace: NamespaceId,
    name: [u8; 16],
}

impl Key {
    pub const fn new(namespace: NamespaceId, name: [u8; 16]) -> Self {
        Self { namespace, name }
    }

    pub(super) const fn namespace(self) -> NamespaceId {
        self.namespace
    }

    pub(super) const fn name(self) -> [u8; 16] {
        self.name
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct DirectoryId(u32);

pub(super) struct NamespaceInfo {
    definition: Namespace,
    prefixes: Vec<DirectoryId>,
    shards: Vec<DirectoryId>,
}

pub(super) struct NamespaceTable {
    entries: Vec<NamespaceInfo>,
}

impl NamespaceTable {
    fn new(definitions: &[Namespace]) -> Result<Self, StorageError> {
        if definitions.len() > usize::from(u8::MAX) + 1 {
            return Err(StorageError::InvalidNamespaces);
        }
        for (index, definition) in definitions.iter().enumerate() {
            if definitions[..index]
                .iter()
                .any(|previous| previous.directory == definition.directory && previous.extension == definition.extension)
            {
                return Err(StorageError::InvalidNamespaces);
            }
        }
        let mut directory_ids = HashMap::<String, DirectoryId>::new();
        let mut entries = Vec::new();
        entries.try_reserve_exact(definitions.len()).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
        for definition in definitions {
            if !valid_namespace(definition.directory) || !valid_extension(definition.extension) {
                return Err(StorageError::InvalidNamespaces);
            }
            let mut prefixes = Vec::new();
            prefixes
                .try_reserve_exact(definition.directory.split('/').count())
                .map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
            let mut prefix = String::new();
            for segment in definition.directory.split('/') {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(segment);
                prefixes.push(directory_id(&mut directory_ids, &prefix)?);
            }
            let mut shards = Vec::new();
            shards.try_reserve_exact(256).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
            for shard in 0..=u8::MAX {
                let mut path = String::new();
                path.try_reserve_exact(definition.directory.len().saturating_add(3))
                    .map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
                path.push_str(definition.directory);
                path.push('/');
                path.push(hex_digit(shard >> 4));
                path.push(hex_digit(shard & 0x0f));
                shards.push(directory_id(&mut directory_ids, &path)?);
            }
            entries.push(NamespaceInfo { definition: *definition, prefixes, shards });
        }
        Ok(Self { entries })
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn definitions(&self) -> impl Iterator<Item = Namespace> + '_ {
        self.entries.iter().map(|entry| entry.definition)
    }

    fn contains(&self, id: NamespaceId) -> bool {
        self.entries.get(usize::from(id.0)).is_some()
    }

    pub(super) fn config(&self, id: NamespaceId) -> Option<Namespace> {
        self.entries.get(usize::from(id.0)).map(|entry| entry.definition)
    }

    pub(super) fn directories(&self, key: Key) -> Option<(&[DirectoryId], DirectoryId)> {
        let entry = self.entries.get(usize::from(key.namespace.0))?;
        let shard = entry.shards.get(usize::from(key.name[0]))?;
        Some((&entry.prefixes, *shard))
    }

    pub(super) fn path(&self, root: &Path, key: Key) -> Option<PathBuf> {
        let definition = self.config(key.namespace)?;
        let mut path = root.to_path_buf();
        path.push(definition.directory);
        let shard = [hex_digit(key.name[0] >> 4) as u8, hex_digit(key.name[0] & 0x0f) as u8];
        path.push(String::from_utf8_lossy(&shard).as_ref());
        let mut file_name = [0u8; 53];
        let mut cursor = 0usize;
        for (index, byte) in key.name.iter().copied().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                file_name[cursor] = b'-';
                cursor += 1;
            }
            file_name[cursor] = hex_digit(byte >> 4) as u8;
            file_name[cursor + 1] = hex_digit(byte & 0x0f) as u8;
            cursor += 2;
        }
        file_name[cursor] = b'.';
        cursor += 1;
        file_name.get_mut(cursor..cursor + definition.extension.len())?.copy_from_slice(definition.extension.as_bytes());
        cursor += definition.extension.len();
        path.push(String::from_utf8_lossy(file_name.get(..cursor)?).as_ref());
        Some(path)
    }

    pub(super) fn key_from_relative_path(&self, relative: &Path) -> Option<Key> {
        for (index, entry) in self.entries.iter().enumerate() {
            let Ok(data_path) = relative.strip_prefix(entry.definition.directory) else {
                continue;
            };
            let mut components = data_path.components();
            let Some(shard) = components.next().and_then(|part| part.as_os_str().to_str()) else {
                continue;
            };
            let Some(file_name) = components.next().and_then(|part| part.as_os_str().to_str()) else {
                continue;
            };
            if components.next().is_some() {
                continue;
            }
            let Some(name) = parse_file_name(file_name, entry.definition.extension) else {
                continue;
            };
            if shard.as_bytes() != shard_name(name[0]) {
                continue;
            }
            let Ok(namespace) = u8::try_from(index) else {
                continue;
            };
            return Some(Key::new(NamespaceId::new(namespace), name));
        }
        None
    }
}

fn directory_id(directories: &mut HashMap<String, DirectoryId>, path: &str) -> Result<DirectoryId, StorageError> {
    if let Some(id) = directories.get(path) {
        return Ok(*id);
    }
    let id = DirectoryId(u32::try_from(directories.len()).map_err(|_| StorageError::TooLarge)?);
    directories.try_reserve(1).map_err(|_| StorageError::Unavailable(UnavailableReason::Allocation))?;
    directories.insert(path.to_owned(), id);
    Ok(id)
}

pub(super) fn valid_namespace(namespace: &str) -> bool {
    !namespace.is_empty()
        && namespace.len() <= MAX_NAMESPACE_BYTES
        && namespace.is_ascii()
        && namespace.split('/').count() <= MAX_NAMESPACE_SEGMENTS
        && namespace.split('/').all(|segment| {
            !segment.is_empty()
                && segment.len() <= MAX_NAMESPACE_SEGMENT_BYTES
                && segment.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_'))
        })
}

pub(super) fn valid_extension(extension: &str) -> bool {
    !extension.is_empty()
        && extension.len() <= MAX_EXTENSION_BYTES
        && extension.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn shard_name(byte: u8) -> [u8; 2] {
    [hex_digit(byte >> 4) as u8, hex_digit(byte & 0x0f) as u8]
}

fn parse_file_name(file_name: &str, extension: &str) -> Option<[u8; 16]> {
    let bytes = file_name.as_bytes();
    if bytes.len() != 37usize.checked_add(extension.len())? || bytes.get(36) != Some(&b'.') || bytes.get(37..)? != extension.as_bytes() {
        return None;
    }
    let mut name = [0; 16];
    for (index, byte) in name.iter_mut().enumerate() {
        let separator_count = usize::from(index >= 4) + usize::from(index >= 6) + usize::from(index >= 8) + usize::from(index >= 10);
        let position = index.checked_mul(2)?.checked_add(separator_count)?;
        if matches!(index, 4 | 6 | 8 | 10) && bytes.get(position.wrapping_sub(1)) != Some(&b'-') {
            return None;
        }
        let high = hex_value(*bytes.get(position)?)?;
        let low = hex_value(*bytes.get(position.checked_add(1)?)?)?;
        *byte = (high << 4) | low;
    }
    Some(name)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn hex_digit(value: u8) -> char {
    if value < 10 { char::from(b'0' + value) } else { char::from(b'a' + (value - 10)) }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnavailableReason {
    ReadLimit,
    WriteQueueFull,
    WriterStopped,
    Busy,
    Io,
    Allocation,
}

#[derive(Debug)]
pub enum StorageError {
    InUse,
    InvalidNamespaces,
    UnknownNamespace,
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
            Self::InvalidNamespaces => f.write_str("cache namespace table is invalid"),
            Self::UnknownNamespace => f.write_str("cache key refers to an undeclared namespace"),
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
    pub size: u64,
    pub hash: [u8; 32],
    pub last_use: AtomicU64,
    pub use_count: AtomicU64,
    state: AtomicU8,
}

impl Entry {
    fn from_record(record: Record) -> Self {
        Self {
            key: record.key,
            size: record.size,
            hash: record.hash,
            last_use: AtomicU64::new(record.last_use),
            use_count: AtomicU64::new(record.use_count),
            state: AtomicU8::new(ENTRY_VALID),
        }
    }

    fn record(&self) -> Record {
        Record {
            key: self.key,
            size: self.size,
            hash: self.hash,
            last_use: self.last_use.load(Ordering::Relaxed),
            use_count: self.use_count.load(Ordering::Relaxed),
        }
    }
}

enum EntryReadFailure {
    Corrupt,
    Unavailable(UnavailableReason),
}

struct ReadState {
    root: PathBuf,
    namespaces: Arc<NamespaceTable>,
    entries: Arc<RwLock<HashMap<Key, Arc<Entry>>>>,
    touches: Arc<Mutex<HashMap<Key, Arc<Entry>>>>,
    invalid: Arc<Mutex<HashMap<Key, Arc<Entry>>>>,
    active_reads: AtomicUsize,
    usage_clock: AtomicU64,
    size_limit: AtomicU64,
    minimum_size_limit: u64,
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
    pub fn open(directory: impl AsRef<Path>, size_limit: u64, namespaces: &[Namespace]) -> Result<Self, StorageError> {
        Self::open_internal(directory.as_ref(), size_limit, namespaces, Arc::new(writer::FaultHooks::default()))
    }

    pub fn lookup(&self, key: Key) -> Lookup {
        self.lookup_bounded(key, MAX_VALUE_BYTES)
    }

    pub fn lookup_bounded(&self, key: Key, max_bytes: usize) -> Lookup {
        if !self.read_state.namespaces.contains(key.namespace) {
            return Lookup::Miss;
        }
        let Some(_permit) = ReadPermit::acquire(&self.read_state.active_reads) else {
            return Lookup::Unavailable(UnavailableReason::ReadLimit);
        };
        for _ in 0..LOOKUP_MAX_ATTEMPTS {
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
            #[cfg(test)]
            if self.read_state.hooks.check(WritePoint::LookupEntrySelected).is_err() {
                return Lookup::Unavailable(UnavailableReason::Io);
            }
            match entry.state.load(Ordering::Acquire) {
                ENTRY_INVALID => {
                    if self.entry_is_current(&key, &entry) {
                        return Lookup::Corrupt;
                    }
                    continue;
                }
                ENTRY_VALID | ENTRY_REPLACING => {}
                _ => return Lookup::Unavailable(UnavailableReason::Io),
            }
            if usize::try_from(entry.size).map_or(true, |size| size > max_bytes.min(MAX_VALUE_BYTES)) {
                if let Some(result) = self.invalid_lookup_result(&key, &entry) {
                    return result;
                }
                continue;
            }
            let bytes = match read_entry(&self.read_state.root, &self.read_state.namespaces, &entry) {
                Ok(bytes) => bytes,
                Err(EntryReadFailure::Corrupt) => {
                    #[cfg(test)]
                    if self.read_state.hooks.check(WritePoint::LookupHashMismatch).is_err() {
                        return Lookup::Unavailable(UnavailableReason::Io);
                    }
                    if let Some(result) = self.invalid_lookup_result(&key, &entry) {
                        return result;
                    }
                    continue;
                }
                Err(EntryReadFailure::Unavailable(reason)) => {
                    if self.entry_is_current(&key, &entry) {
                        return Lookup::Unavailable(reason);
                    }
                    continue;
                }
            };
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
            return Lookup::Hit(CacheValue { bytes, last_use: stamp, use_count });
        }
        Lookup::Miss
    }

    pub fn put(&self, key: Key, bytes: Vec<u8>) -> Result<(), StorageError> {
        if !self.read_state.namespaces.contains(key.namespace) {
            return Err(StorageError::UnknownNamespace);
        }
        if bytes.len() > MAX_VALUE_BYTES {
            return Err(StorageError::TooLarge);
        }
        let weight = bytes.len();
        let (reply, receiver) = mpsc::sync_channel(0);
        self.queue.try_send(Command::Put { key, bytes, reply }, weight)?;
        receive(receiver)
    }

    pub fn remove(&self, key: Key) -> Result<bool, StorageError> {
        if !self.read_state.namespaces.contains(key.namespace) {
            return Err(StorageError::UnknownNamespace);
        }
        let (reply, receiver) = mpsc::sync_channel(0);
        self.queue.try_send(Command::Remove { key, reply }, 0)?;
        receive(receiver)
    }

    pub fn discard(&self, key: Key) {
        if !self.read_state.namespaces.contains(key.namespace) {
            return;
        }
        let entries = match self.read_state.entries.read() {
            Ok(entries) => entries,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(entry) = entries.get(&key).cloned() else {
            return;
        };
        drop(entries);
        self.mark_invalid(&key, entry);
    }

    pub fn clear(&self) -> Result<(), StorageError> {
        let (reply, receiver) = mpsc::sync_channel(0);
        self.queue.try_send(Command::Clear { reply }, 0)?;
        receive(receiver)
    }

    pub fn set_size_limit(&self, limit: u64) -> Result<(), StorageError> {
        if limit < self.read_state.minimum_size_limit {
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
        entries.values().filter(|entry| entry.state.load(Ordering::Acquire) != ENTRY_INVALID).count()
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

    fn open_internal(
        directory: &Path,
        size_limit: u64,
        namespace_config: &[Namespace],
        hooks: Arc<writer::FaultHooks>,
    ) -> Result<Self, StorageError> {
        let root = directory.to_path_buf();
        let namespaces = Arc::new(NamespaceTable::new(namespace_config)?);
        let minimum_size_limit =
            DIRECTORY_OVERHEAD * 3 + FILE_OVERHEAD * 4 + index::INDEX_BASE_BYTES + index::namespace_bytes(&namespaces)?;
        if size_limit < minimum_size_limit {
            return Err(StorageError::LimitTooSmall);
        }
        fs::create_dir_all(directory)?;
        let lock_file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(root.join(LOCK_FILE))?;
        match lock_file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(StorageError::InUse),
            Err(std::fs::TryLockError::Error(error)) => return Err(StorageError::Io(error)),
        }
        discard_old_index_files(&root)?;
        let (index, records) = Index::open(root.join(INDEX_FILE), &namespaces)?;
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
            root: root.clone(),
            namespaces,
            entries,
            touches,
            invalid,
            active_reads: AtomicUsize::new(0),
            usage_clock: AtomicU64::new(max_tick),
            size_limit: AtomicU64::new(size_limit),
            minimum_size_limit,
            accounted_size: AtomicU64::new(0),
            #[cfg(test)]
            hooks: Arc::clone(&hooks),
        });
        let worker_state = WorkerState::new(root, lock_file, Arc::clone(&read_state), index, hooks)?;
        let startup_queue = Arc::clone(&queue);
        let worker =
            thread::Builder::new().name("storage-cache-writer".to_owned()).spawn(move || writer_loop(worker_state, startup_queue))?;
        let cache = Self { queue, worker: Some(worker), read_state };
        if cache.accounted_size() > size_limit {
            cache.set_size_limit(size_limit)?;
        }
        Ok(cache)
    }

    fn entry_is_current(&self, key: &Key, entry: &Arc<Entry>) -> bool {
        let entries = match self.read_state.entries.read() {
            Ok(entries) => entries,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.get(key).is_some_and(|current| Arc::ptr_eq(current, entry))
    }

    fn mark_invalid(&self, key: &Key, entry: Arc<Entry>) -> bool {
        let entries = match self.read_state.entries.read() {
            Ok(entries) => entries,
            Err(poisoned) => poisoned.into_inner(),
        };
        let matches = entries.get(key).is_some_and(|current| Arc::ptr_eq(current, &entry));
        if !matches {
            return false;
        }
        match entry.state.compare_exchange(ENTRY_VALID, ENTRY_INVALID, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) | Err(ENTRY_INVALID) => {}
            Err(ENTRY_REPLACING) => return false,
            Err(_) => return false,
        }
        drop(entries);
        let mut invalid = match self.read_state.invalid.lock() {
            Ok(invalid) => invalid,
            Err(poisoned) => poisoned.into_inner(),
        };
        if invalid.try_reserve(1).is_err() {
            return true;
        }
        invalid.insert(*key, entry);
        true
    }

    fn invalid_lookup_result(&self, key: &Key, entry: &Arc<Entry>) -> Option<Lookup> {
        if self.mark_invalid(key, Arc::clone(entry)) {
            return Some(Lookup::Corrupt);
        }
        if entry.state.load(Ordering::Acquire) == ENTRY_REPLACING && self.entry_is_current(key, entry) {
            return Some(Lookup::Unavailable(UnavailableReason::Io));
        }
        None
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

fn read_entry(root: &Path, namespaces: &NamespaceTable, entry: &Entry) -> Result<Vec<u8>, EntryReadFailure> {
    let path = namespaces.path(root, entry.key).ok_or(EntryReadFailure::Corrupt)?;
    let file = File::open(&path).map_err(|error| file_read_failure(&path, error))?;
    let capacity = usize::try_from(entry.size).map_err(|_| EntryReadFailure::Corrupt)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(capacity).map_err(|_| EntryReadFailure::Unavailable(UnavailableReason::Allocation))?;
    file.take(entry.size.saturating_add(1)).read_to_end(&mut bytes).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof { EntryReadFailure::Corrupt } else { file_read_failure(&path, error) }
    })?;
    if u64::try_from(bytes.len()).ok() != Some(entry.size) || Sha256::digest(&bytes).as_slice() != entry.hash {
        return Err(EntryReadFailure::Corrupt);
    }
    Ok(bytes)
}

fn file_read_failure(path: &Path, error: io::Error) -> EntryReadFailure {
    if error.kind() == io::ErrorKind::NotFound {
        return EntryReadFailure::Corrupt;
    }
    if matches!(error.kind(), io::ErrorKind::PermissionDenied | io::ErrorKind::IsADirectory)
        && fs::metadata(path).is_ok_and(|metadata| !metadata.is_file())
    {
        return EntryReadFailure::Corrupt;
    }
    EntryReadFailure::Unavailable(UnavailableReason::Io)
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

fn discard_old_index_files(root: &Path) -> Result<(), StorageError> {
    for name in [SNAPSHOT_FILE, JOURNAL_FILE] {
        match fs::remove_file(root.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(StorageError::Io(error)),
        }
    }
    for item in fs::read_dir(root)?.take(128) {
        let item = item?;
        if !item.file_type()?.is_file() {
            continue;
        }
        let name = item.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".snapshot-tmp-") || name.starts_with(".journal-tmp-") || name.starts_with(".lock-tmp-") {
            fs::remove_file(item.path())?;
        }
    }
    Ok(())
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

pub fn discard_legacy_layout(directory: impl AsRef<Path>) -> Result<(), StorageError> {
    let root = directory.as_ref();
    let snapshot = root.join(SNAPSHOT_FILE);
    let journal_path = root.join(JOURNAL_FILE);
    let data = root.join("data");
    if !snapshot.try_exists()? && !journal_path.try_exists()? && !data.try_exists()? {
        return Ok(());
    }
    let lock_file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(root.join(LOCK_FILE))?;
    match lock_file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Err(StorageError::InUse),
        Err(std::fs::TryLockError::Error(error)) => return Err(StorageError::Io(error)),
    }
    for path in [snapshot, journal_path] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(StorageError::Io(error)),
        }
    }
    match fs::remove_dir_all(data) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(StorageError::Io(error)),
    }
    Ok(())
}

#[cfg(test)]
mod tests;
