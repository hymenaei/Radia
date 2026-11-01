use std::{
    error::Error,
    fmt,
    mem::size_of,
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
    Blob, Budget, Kind, Request,
    budget::Reservation,
    cache_policy::{AssetEvictionPolicy, DecodeCosts},
    mesh::{MAX_MESH_ASSET_BYTES, MAX_MESH_BLOCK_BYTES, MAX_MESH_HEADER_BYTES, MeshLod},
};

pub use storage::EvictionReason as CacheEvictionReason;

const MAX_IN_FLIGHT_WRITES: usize = 2;
const TEXTURES: NamespaceId = NamespaceId::new(0);
const PBR_MATERIALS: NamespaceId = NamespaceId::new(1);
const LEGACY_MATERIALS: NamespaceId = NamespaceId::new(2);
const ENVIRONMENTS: NamespaceId = NamespaceId::new(3);
const MESH_HEADER: NamespaceId = NamespaceId::new(4);
const MESH_LOD_LOWEST: NamespaceId = NamespaceId::new(5);
const MESH_LOD_LOW: NamespaceId = NamespaceId::new(6);
const MESH_LOD_MEDIUM: NamespaceId = NamespaceId::new(7);
const MESH_LOD_HIGH: NamespaceId = NamespaceId::new(8);
pub(crate) const MESH_NAMESPACE_IDS: [NamespaceId; 5] = [MESH_HEADER, MESH_LOD_LOWEST, MESH_LOD_LOW, MESH_LOD_MEDIUM, MESH_LOD_HIGH];
pub(super) const MESH_HEADER_FRAME_BYTES: usize = size_of::<u64>();
pub(super) const MESH_LOD_FRAME_BYTES: usize = 3 * size_of::<u64>();
const NAMESPACES: [Namespace; 9] = [
    Namespace::new("textures", "j2c"),
    Namespace::new("materials/pbr", "llsd"),
    Namespace::new("materials/legacy", "llsd"),
    Namespace::new("environments", "llsd"),
    Namespace::new("meshes/header", "llmesh"),
    Namespace::new("meshes/lod/lowest", "llmesh"),
    Namespace::new("meshes/lod/low", "llmesh"),
    Namespace::new("meshes/lod/medium", "llmesh"),
    Namespace::new("meshes/lod/high", "llmesh"),
];

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum CacheUnit {
    Whole,
    Header,
    Lod(MeshLod),
}

pub(crate) fn cache_unit(request: Request) -> CacheUnit {
    match request {
        Request::MeshHeader(_) => CacheUnit::Header,
        Request::MeshLod(block) => CacheUnit::Lod(block.lod()),
        _ => CacheUnit::Whole,
    }
}

pub(crate) fn mesh_header_frame(total_length: u64, header: &[u8]) -> Option<Vec<u8>> {
    if total_length == 0 || total_length > MAX_MESH_ASSET_BYTES as u64 || header.is_empty() || header.len() > MAX_MESH_HEADER_BYTES {
        return None;
    }
    let capacity = MESH_HEADER_FRAME_BYTES.checked_add(header.len())?;
    let mut frame = Vec::new();
    frame.try_reserve_exact(capacity).ok()?;
    frame.extend_from_slice(&total_length.to_le_bytes());
    frame.extend_from_slice(header);
    Some(frame)
}

pub(crate) fn parse_mesh_header_frame(bytes: &[u8]) -> Option<(u64, &[u8])> {
    let max_length = MESH_HEADER_FRAME_BYTES.checked_add(MAX_MESH_HEADER_BYTES)?;
    if bytes.len() <= MESH_HEADER_FRAME_BYTES || bytes.len() > max_length {
        return None;
    }
    let total_length = read_u64(bytes, 0)?;
    if total_length == 0 || total_length > MAX_MESH_ASSET_BYTES as u64 {
        return None;
    }
    Some((total_length, bytes.get(MESH_HEADER_FRAME_BYTES..)?))
}

pub(crate) fn mesh_lod_frame(request: crate::mesh::MeshLodRequest, block: &[u8]) -> Option<Vec<u8>> {
    let range = request.range();
    let block_length = range.end.checked_sub(range.start)?;
    if u64::try_from(block.len()).ok()? != block_length || block_length > MAX_MESH_BLOCK_BYTES as u64 {
        return None;
    }
    let capacity = MESH_LOD_FRAME_BYTES.checked_add(block.len())?;
    let mut frame = Vec::new();
    frame.try_reserve_exact(capacity).ok()?;
    frame.extend_from_slice(&range.start.to_le_bytes());
    frame.extend_from_slice(&block_length.to_le_bytes());
    frame.extend_from_slice(&request.asset_length().to_le_bytes());
    frame.extend_from_slice(block);
    Some(frame)
}

pub(crate) fn parse_mesh_lod_frame(request: crate::mesh::MeshLodRequest, bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() < MESH_LOD_FRAME_BYTES.checked_add(1)? || bytes.len() > MESH_LOD_FRAME_BYTES.checked_add(MAX_MESH_BLOCK_BYTES)? {
        return None;
    }
    let range = request.range();
    let block_length = range.end.checked_sub(range.start)?;
    if read_u64(bytes, 0)? != range.start
        || read_u64(bytes, size_of::<u64>())? != block_length
        || read_u64(bytes, 2 * size_of::<u64>())? != request.asset_length()
    {
        return None;
    }
    let block = bytes.get(MESH_LOD_FRAME_BYTES..)?;
    (u64::try_from(block.len()).ok()? == block_length).then_some(block)
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(size_of::<u64>())?;
    let value = bytes.get(offset..end)?.try_into().ok()?;
    Some(u64::from_le_bytes(value))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssetLookup {
    Hit(CacheValue),
    Miss,
    Corrupt,
    Unavailable(UnavailableReason),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AssetCacheEntriesByKind {
    pub textures: usize,
    pub pbr_materials: usize,
    pub legacy_materials: usize,
    pub environments: usize,
    pub meshes: usize,
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
    pub entries_by_kind: AssetCacheEntriesByKind,
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

pub(crate) fn read_work_bytes_for_request(request: Request) -> Option<usize> {
    let resource = request.resource();
    cache_limit(request).checked_add(resource.kind.max_cache_bytes())
}

pub struct AssetCache {
    storage: StorageCache,
    counters: Counters,
    decode_costs: Arc<DecodeCosts>,
}

pub struct CacheWrite {
    cache: Arc<AssetCache>,
    request: Request,
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

    pub fn lookup(&self, request: Request) -> AssetLookup {
        let key = storage_key(request);
        let mut bytes_read = 0u64;
        let result = match self.storage.lookup_bounded(key, cache_limit(request)) {
            StorageLookup::Hit(value) => {
                bytes_read = u64::try_from(value.bytes.len()).unwrap_or(u64::MAX);
                let valid = match request {
                    Request::MeshHeader(_) => parse_mesh_header_frame(&value.bytes).is_some(),
                    Request::MeshLod(block) => parse_mesh_lod_frame(block, &value.bytes).is_some(),
                    _ => true,
                };
                if valid {
                    AssetLookup::Hit(value)
                } else {
                    self.storage.discard(key);
                    AssetLookup::Corrupt
                }
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

    pub fn put_complete(&self, request: Request, bytes: Vec<u8>) -> Result<(), AssetCacheError> {
        if bytes.len() > cache_limit(request) {
            return Err(AssetCacheError::TooLarge);
        }
        let key = storage_key(request);
        let written = u64::try_from(bytes.len()).map_err(|_| AssetCacheError::TooLarge)?;
        self.storage.put(key, bytes).map_err(map_storage_error)?;
        self.counters.bytes_written.fetch_add(written, Ordering::Relaxed);
        Ok(())
    }

    pub(crate) fn protect(&self, request: Request) -> Result<Protection, AssetCacheError> {
        self.storage.protect(storage_key(request)).map_err(AssetCacheError::Storage)
    }

    pub(crate) fn observe_decode(&self, kind: Kind, duration: Duration) {
        let micros = u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
        self.decode_costs.observe(kind, micros);
    }

    pub(crate) fn discard(&self, request: Request) {
        self.storage.discard(storage_key(request));
    }

    pub(crate) fn discard_corrupt(&self, request: Request) {
        self.counters.corrupt_entries.fetch_add(1, Ordering::Relaxed);
        self.discard(request);
    }

    pub(crate) fn queue_write(self: &Arc<Self>, request: Request, payload: Blob, budget: &Budget) -> Option<CacheWrite> {
        let payload_bytes = payload.len();
        if payload_bytes > cache_limit(request) {
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
        Some(CacheWrite { cache: Arc::clone(self), request, payload, _reservation: reservation, finished: false })
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
            entries_by_kind: AssetCacheEntriesByKind {
                textures: self.storage.namespace_entry_count(TEXTURES),
                pbr_materials: self.storage.namespace_entry_count(PBR_MATERIALS),
                legacy_materials: self.storage.namespace_entry_count(LEGACY_MATERIALS),
                environments: self.storage.namespace_entry_count(ENVIRONMENTS),
                meshes: MESH_NAMESPACE_IDS.iter().map(|namespace| self.storage.namespace_entry_count(*namespace)).sum(),
            },
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
        if self.cache.put_complete(self.request, self.payload.to_vec()).is_err() {
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

fn cache_limit(request: Request) -> usize {
    match request {
        Request::Texture { .. } => Kind::Texture.max_cache_bytes().min(storage::MAX_VALUE_BYTES),
        Request::PbrMaterial(_) => Kind::PbrMaterial.max_cache_bytes().min(storage::MAX_VALUE_BYTES),
        Request::LegacyMaterial(_) => Kind::LegacyMaterial.max_cache_bytes().min(storage::MAX_VALUE_BYTES),
        Request::Environment(_) => Kind::Environment.max_cache_bytes().min(storage::MAX_VALUE_BYTES),
        Request::MeshHeader(_) => MESH_HEADER_FRAME_BYTES.saturating_add(MAX_MESH_HEADER_BYTES).min(storage::MAX_VALUE_BYTES),
        Request::MeshLod(_) => MESH_LOD_FRAME_BYTES.saturating_add(MAX_MESH_BLOCK_BYTES).min(storage::MAX_VALUE_BYTES),
    }
}

fn storage_key(request: Request) -> StorageKey {
    let (namespace, id) = match request {
        Request::Texture { id, .. } => (TEXTURES, id),
        Request::PbrMaterial(id) => (PBR_MATERIALS, id),
        Request::LegacyMaterial(id) => (LEGACY_MATERIALS, id),
        Request::Environment(id) => (ENVIRONMENTS, id),
        Request::MeshHeader(id) => (MESH_HEADER, id),
        Request::MeshLod(block) => {
            let namespace = match block.lod() {
                MeshLod::Lowest => MESH_LOD_LOWEST,
                MeshLod::Low => MESH_LOD_LOW,
                MeshLod::Medium => MESH_LOD_MEDIUM,
                MeshLod::High => MESH_LOD_HIGH,
            };
            (namespace, block.asset_id())
        }
    };
    StorageKey::new(namespace, *id.uuid().as_bytes())
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
