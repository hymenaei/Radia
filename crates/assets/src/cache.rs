use std::{
    error::Error,
    fmt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use storage::{
    Cache as StorageCache, CacheValue, EvictionReason, Key as StorageKey, Lookup as StorageLookup, Namespace, NamespaceId, Protection,
    StorageError, UnavailableReason,
};

use crate::{
    Blob, Budget, Kind, Resource,
    budget::Reservation,
    cache_policy::{AssetEvictionPolicy, DecodeCosts},
};

pub use storage::EvictionReason as CacheEvictionReason;

const MAX_IN_FLIGHT_WRITES: usize = 2;
const TEXTURES: NamespaceId = NamespaceId::new(0);
const PBR_MATERIALS: NamespaceId = NamespaceId::new(1);
const LEGACY_MATERIALS: NamespaceId = NamespaceId::new(2);
const ENVIRONMENTS: NamespaceId = NamespaceId::new(3);
const MESHES: NamespaceId = NamespaceId::new(4);
const NAMESPACES: [Namespace; 5] = [
    Namespace::new("textures", "j2c"),
    Namespace::new("materials/pbr", "llsd"),
    Namespace::new("materials/legacy", "llsd"),
    Namespace::new("environments", "llsd"),
    Namespace::new("meshes", "llmesh"),
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssetLookup {
    Hit(CacheValue),
    Miss,
    Corrupt,
    Unavailable(UnavailableReason),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AssetCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub corrupt_entries: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub dropped_writes: u64,
    pub size_bytes: u64,
    pub size_limit_bytes: u64,
    pub entries_by_kind: [usize; 4],
    pub protected_bytes: u64,
    pub recent_evictions: [Option<EvictionReason>; 8],
}

#[derive(Default)]
struct Counters {
    hits: AtomicU64,
    misses: AtomicU64,
    corrupt_entries: AtomicU64,
    bytes_read: AtomicU64,
    bytes_written: AtomicU64,
    dropped_writes: AtomicU64,
    in_flight_writes: AtomicUsize,
}

#[derive(Debug)]
pub enum AssetCacheError {
    Unavailable(UnavailableReason),
    Storage(StorageError),
    TooLarge,
}

impl fmt::Display for AssetCacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "asset cache is unavailable: {reason:?}"),
            Self::Storage(error) => write!(f, "asset cache storage failed: {error}"),
            Self::TooLarge => f.write_str("asset exceeds the cache payload limit"),
        }
    }
}

impl Error for AssetCacheError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}

pub(crate) fn read_work_bytes(kind: Kind) -> Option<usize> {
    cache_limit(kind).checked_add(kind.max_cache_bytes())
}

pub struct AssetCache {
    storage: StorageCache,
    counters: Counters,
    decode_costs: Arc<DecodeCosts>,
}

pub struct CacheWrite {
    cache: Arc<AssetCache>,
    resource: Resource,
    payload: Blob,
    _reservation: Reservation,
    finished: bool,
}

impl fmt::Debug for AssetCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AssetCache")
    }
}

impl AssetCache {
    pub fn open(directory: impl AsRef<Path>, size_limit: u64) -> Result<Self, AssetCacheError> {
        let directory = directory.as_ref();
        storage::discard_legacy_layout(directory).map_err(AssetCacheError::Storage)?;
        let decode_costs = Arc::new(DecodeCosts::default());
        let policy = AssetEvictionPolicy::new(Arc::clone(&decode_costs));
        let storage =
            StorageCache::open_with_policy(directory.join("assets"), size_limit, &NAMESPACES, policy).map_err(AssetCacheError::Storage)?;
        Ok(Self { storage, counters: Counters::default(), decode_costs })
    }

    pub fn open_default() -> Result<Self, AssetCacheError> {
        let directory = storage::default_cache_dir().map_err(AssetCacheError::Storage)?;
        Self::open(directory, storage::DEFAULT_SIZE_LIMIT)
    }

    pub fn lookup(&self, resource: Resource) -> AssetLookup {
        let key = storage_key(resource);
        let mut bytes_read = 0u64;
        let result = match self.storage.lookup_bounded(key, cache_limit(resource.kind)) {
            StorageLookup::Hit(value) => {
                bytes_read = u64::try_from(value.bytes.len()).unwrap_or(u64::MAX);
                AssetLookup::Hit(value)
            }
            StorageLookup::Miss => AssetLookup::Miss,
            StorageLookup::Corrupt => AssetLookup::Corrupt,
            StorageLookup::Unavailable(reason) => AssetLookup::Unavailable(reason),
        };
        self.counters.bytes_read.fetch_add(bytes_read, Ordering::Relaxed);
        match &result {
            AssetLookup::Hit(_) => {
                self.counters.hits.fetch_add(1, Ordering::Relaxed);
            }
            AssetLookup::Miss => {
                self.counters.misses.fetch_add(1, Ordering::Relaxed);
            }
            AssetLookup::Corrupt => {
                self.counters.corrupt_entries.fetch_add(1, Ordering::Relaxed);
            }
            AssetLookup::Unavailable(_) => {}
        }
        result
    }

    pub fn put_complete(&self, resource: Resource, bytes: Vec<u8>) -> Result<(), AssetCacheError> {
        if bytes.len() > cache_limit(resource.kind) {
            return Err(AssetCacheError::TooLarge);
        }
        let key = storage_key(resource);
        let written = u64::try_from(bytes.len()).map_err(|_| AssetCacheError::TooLarge)?;
        self.storage.put(key, bytes).map_err(map_storage_error)?;
        self.counters.bytes_written.fetch_add(written, Ordering::Relaxed);
        Ok(())
    }

    pub(crate) fn protect(&self, resource: Resource) -> Result<Protection, AssetCacheError> {
        self.storage.protect(storage_key(resource)).map_err(AssetCacheError::Storage)
    }

    pub(crate) fn observe_decode(&self, kind: Kind, duration: Duration) {
        let micros = u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
        self.decode_costs.observe(kind, micros);
    }

    pub(crate) fn discard(&self, resource: Resource) {
        self.storage.discard(storage_key(resource));
    }

    pub(crate) fn discard_corrupt(&self, resource: Resource) {
        self.counters.corrupt_entries.fetch_add(1, Ordering::Relaxed);
        self.discard(resource);
    }

    pub(crate) fn queue_write(self: &Arc<Self>, resource: Resource, payload: Blob, budget: &Budget) -> Option<CacheWrite> {
        let payload_bytes = payload.len();
        if payload_bytes > cache_limit(resource.kind) {
            self.counters.dropped_writes.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        if self
            .counters
            .in_flight_writes
            .try_update(Ordering::AcqRel, Ordering::Acquire, |current| (current < MAX_IN_FLIGHT_WRITES).then_some(current + 1))
            .is_err()
        {
            self.counters.dropped_writes.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let Some(reservation) = budget.reserve(payload_bytes) else {
            self.counters.in_flight_writes.fetch_sub(1, Ordering::AcqRel);
            self.counters.dropped_writes.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        Some(CacheWrite { cache: Arc::clone(self), resource, payload, _reservation: reservation, finished: false })
    }

    pub fn stats(&self) -> AssetCacheStats {
        AssetCacheStats {
            hits: self.counters.hits.load(Ordering::Relaxed),
            misses: self.counters.misses.load(Ordering::Relaxed),
            corrupt_entries: self.counters.corrupt_entries.load(Ordering::Relaxed),
            bytes_read: self.counters.bytes_read.load(Ordering::Relaxed),
            bytes_written: self.counters.bytes_written.load(Ordering::Relaxed),
            dropped_writes: self.counters.dropped_writes.load(Ordering::Relaxed),
            size_bytes: self.storage.accounted_size(),
            size_limit_bytes: self.storage.size_limit(),
            entries_by_kind: [
                self.storage.namespace_entry_count(TEXTURES),
                self.storage.namespace_entry_count(PBR_MATERIALS),
                self.storage.namespace_entry_count(LEGACY_MATERIALS),
                self.storage.namespace_entry_count(ENVIRONMENTS),
            ],
            protected_bytes: self.storage.protected_bytes(),
            recent_evictions: self.storage.recent_evictions(),
        }
    }

    pub fn clear(&self) -> Result<(), AssetCacheError> {
        self.storage.clear().map_err(AssetCacheError::Storage)
    }

    pub fn set_size_limit(&self, limit: u64) -> Result<(), AssetCacheError> {
        self.storage.set_size_limit(limit).map_err(AssetCacheError::Storage)
    }

    pub fn accounted_size(&self) -> u64 {
        self.storage.accounted_size()
    }

    pub fn size_limit(&self) -> u64 {
        self.storage.size_limit()
    }

    pub fn shutdown(self) -> Result<(), AssetCacheError> {
        self.storage.shutdown().map_err(AssetCacheError::Storage)
    }
}

impl CacheWrite {
    pub fn run(mut self) {
        if self.cache.put_complete(self.resource, self.payload.to_vec()).is_err() {
            self.cache.counters.dropped_writes.fetch_add(1, Ordering::Relaxed);
        }
        self.finished = true;
    }
}

impl Drop for CacheWrite {
    fn drop(&mut self) {
        if !self.finished {
            self.cache.counters.dropped_writes.fetch_add(1, Ordering::Relaxed);
        }
        self.cache.counters.in_flight_writes.fetch_sub(1, Ordering::AcqRel);
    }
}

fn cache_limit(kind: Kind) -> usize {
    kind.max_cache_bytes().min(storage::MAX_VALUE_BYTES)
}

fn storage_key(resource: Resource) -> StorageKey {
    let namespace = match resource.kind {
        Kind::Texture => TEXTURES,
        Kind::PbrMaterial => PBR_MATERIALS,
        Kind::LegacyMaterial => LEGACY_MATERIALS,
        Kind::Environment => ENVIRONMENTS,
        Kind::Mesh => MESHES,
    };
    StorageKey::new(namespace, *resource.id.uuid().as_bytes())
}

fn map_storage_error(error: StorageError) -> AssetCacheError {
    match error {
        StorageError::Unavailable(reason) => AssetCacheError::Unavailable(reason),
        error => AssetCacheError::Storage(error),
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
