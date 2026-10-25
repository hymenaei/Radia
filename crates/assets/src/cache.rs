use std::{
    error::Error,
    fmt,
    io::{self, Cursor, Read, Write},
    path::Path,
    sync::{Mutex, MutexGuard},
};

use storage::{Cache as StorageCache, CacheValue, Key as StorageKey, Lookup as StorageLookup, StorageError, UnavailableReason};

use crate::{Kind, Resource};

const MAGIC: &[u8; 8] = b"RADEXT01";
const HEADER_BYTES: usize = 20;
const MAX_EXTENTS: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Extent {
    pub offset: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedExtent {
    total_size: u64,
    offset: u64,
    bytes: Vec<u8>,
}

impl ValidatedExtent {
    pub fn from_validated(total_size: u64, offset: u64, bytes: Vec<u8>) -> Self {
        Self { total_size, offset, bytes }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedAsset {
    pub total_size: u64,
    pub extents: Vec<Extent>,
    pub last_use: u64,
    pub use_count: u64,
}

impl CachedAsset {
    pub fn is_complete(&self) -> bool {
        match self.extents.as_slice() {
            [extent] => extent.offset == 0 && u64::try_from(extent.bytes.len()).ok() == Some(self.total_size),
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssetLookup {
    Hit(CachedAsset),
    Partial(CachedAsset),
    Miss,
    Corrupt,
    Unavailable(UnavailableReason),
}

#[derive(Debug)]
pub enum AssetCacheError {
    Unavailable(UnavailableReason),
    Storage(StorageError),
    InvalidExtent,
    SizeMismatch,
    ConflictingOverlap,
    CorruptEntry,
    TooLarge,
}

impl fmt::Display for AssetCacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "asset cache is unavailable: {reason:?}"),
            Self::Storage(error) => write!(f, "asset cache storage failed: {error}"),
            Self::InvalidExtent => f.write_str("asset extent is outside its payload"),
            Self::SizeMismatch => f.write_str("asset extents disagree about the payload size"),
            Self::ConflictingOverlap => f.write_str("overlapping asset extents contain different bytes"),
            Self::CorruptEntry => f.write_str("cached asset failed validation and was removed"),
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

pub struct AssetCache {
    storage: StorageCache,
    write_lock: Mutex<()>,
}

impl fmt::Debug for AssetCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AssetCache")
    }
}

impl AssetCache {
    pub fn open(directory: impl AsRef<Path>, size_limit: u64) -> Result<Self, AssetCacheError> {
        let storage = StorageCache::open(directory, size_limit).map_err(AssetCacheError::Storage)?;
        Ok(Self { storage, write_lock: Mutex::new(()) })
    }

    pub fn open_default(size_limit: u64) -> Result<Self, AssetCacheError> {
        let directory = storage::default_cache_dir().map_err(AssetCacheError::Storage)?.join("assets");
        Self::open(directory, size_limit)
    }

    pub fn lookup(&self, resource: Resource) -> AssetLookup {
        let key = storage_key(resource);
        match self.storage.lookup(key) {
            StorageLookup::Hit(value) => match decode(resource.kind, value) {
                Ok(asset) if asset.is_complete() => AssetLookup::Hit(asset),
                Ok(asset) => AssetLookup::Partial(asset),
                Err(()) => {
                    self.storage.discard(key);
                    AssetLookup::Corrupt
                }
            },
            StorageLookup::Miss => AssetLookup::Miss,
            StorageLookup::Corrupt => AssetLookup::Corrupt,
            StorageLookup::Unavailable(reason) => AssetLookup::Unavailable(reason),
        }
    }

    pub fn put_extent(&self, resource: Resource, extent: ValidatedExtent) -> Result<(), AssetCacheError> {
        let ValidatedExtent { total_size, offset, bytes } = extent;
        validate_extent(resource.kind, total_size, offset, &bytes)?;
        let _guard = self.write_guard()?;
        let key = storage_key(resource);
        let existing = match self.storage.lookup(key) {
            StorageLookup::Hit(value) => match decode(resource.kind, value) {
                Ok(asset) => Some(asset),
                Err(()) => {
                    self.storage.discard(key);
                    return Err(AssetCacheError::CorruptEntry);
                }
            },
            StorageLookup::Miss => None,
            StorageLookup::Corrupt => return Err(AssetCacheError::CorruptEntry),
            StorageLookup::Unavailable(reason) => return Err(AssetCacheError::Unavailable(reason)),
        };
        let old_extents = match existing {
            Some(asset) if asset.total_size != total_size => return Err(AssetCacheError::SizeMismatch),
            Some(asset) => asset.extents,
            None => Vec::new(),
        };
        let mut next = old_extents;
        merge_extent(&mut next, Extent { offset, bytes })?;
        let payload = encode(total_size, &next, resource.kind)?;
        self.storage.put(key, payload).map_err(map_storage_error)
    }

    pub fn clear(&self) -> Result<(), AssetCacheError> {
        let _guard = self.write_guard()?;
        self.storage.clear().map_err(AssetCacheError::Storage)
    }

    pub fn set_size_limit(&self, limit: u64) -> Result<(), AssetCacheError> {
        let _guard = self.write_guard()?;
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

    fn write_guard(&self) -> Result<MutexGuard<'_, ()>, AssetCacheError> {
        match self.write_lock.try_lock() {
            Ok(guard) => Ok(guard),
            Err(std::sync::TryLockError::WouldBlock) => Err(AssetCacheError::Unavailable(UnavailableReason::WriteQueueFull)),
            Err(std::sync::TryLockError::Poisoned(error)) => Ok(error.into_inner()),
        }
    }
}

fn map_storage_error(error: StorageError) -> AssetCacheError {
    match error {
        StorageError::Unavailable(reason) => AssetCacheError::Unavailable(reason),
        error => AssetCacheError::Storage(error),
    }
}

fn storage_key(resource: Resource) -> StorageKey {
    let mut bytes = [0; 32];
    if let Some(namespace) = bytes.first_mut() {
        *namespace = 0x41;
    }
    if let Some(kind) = bytes.get_mut(1) {
        *kind = match resource.kind {
            Kind::Texture => 1,
            Kind::PbrMaterial => 2,
            Kind::LegacyMaterial => 3,
            Kind::Environment => 4,
        };
    }
    if let Some(asset_id) = bytes.get_mut(2..18) {
        asset_id.copy_from_slice(resource.id.uuid().as_bytes());
    }
    StorageKey::new(bytes)
}

fn validate_extent(kind: Kind, total_size: u64, offset: u64, bytes: &[u8]) -> Result<(), AssetCacheError> {
    let end =
        offset.checked_add(u64::try_from(bytes.len()).map_err(|_| AssetCacheError::TooLarge)?).ok_or(AssetCacheError::InvalidExtent)?;
    if total_size == 0 || bytes.is_empty() || end > total_size {
        return Err(AssetCacheError::InvalidExtent);
    }
    if total_size > u64::try_from(kind.max_response_bytes()).map_err(|_| AssetCacheError::TooLarge)? {
        return Err(AssetCacheError::TooLarge);
    }
    Ok(())
}

fn merge_extent(extents: &mut Vec<Extent>, incoming: Extent) -> Result<(), AssetCacheError> {
    let mut all = std::mem::take(extents);
    all.try_reserve(1).map_err(|_| AssetCacheError::TooLarge)?;
    all.push(incoming);
    all.sort_unstable_by_key(|extent| extent.offset);
    let mut merged = Vec::<Extent>::new();
    merged.try_reserve_exact(all.len()).map_err(|_| AssetCacheError::TooLarge)?;
    for extent in all {
        let extent_end = extent
            .offset
            .checked_add(u64::try_from(extent.bytes.len()).map_err(|_| AssetCacheError::TooLarge)?)
            .ok_or(AssetCacheError::InvalidExtent)?;
        let Some(previous) = merged.last_mut() else {
            merged.push(extent);
            continue;
        };
        let previous_end = previous
            .offset
            .checked_add(u64::try_from(previous.bytes.len()).map_err(|_| AssetCacheError::TooLarge)?)
            .ok_or(AssetCacheError::InvalidExtent)?;
        if previous_end < extent.offset {
            merged.push(extent);
            continue;
        }
        let overlap_start = previous.offset.max(extent.offset);
        let overlap_end = previous_end.min(extent_end);
        if overlap_start < overlap_end {
            let previous_start = usize::try_from(overlap_start - previous.offset).map_err(|_| AssetCacheError::TooLarge)?;
            let previous_end = usize::try_from(overlap_end - previous.offset).map_err(|_| AssetCacheError::TooLarge)?;
            let extent_start = usize::try_from(overlap_start - extent.offset).map_err(|_| AssetCacheError::TooLarge)?;
            let extent_end = usize::try_from(overlap_end - extent.offset).map_err(|_| AssetCacheError::TooLarge)?;
            let previous_bytes = previous.bytes.get(previous_start..previous_end).ok_or(AssetCacheError::InvalidExtent)?;
            let overlap_bytes = extent.bytes.get(extent_start..extent_end).ok_or(AssetCacheError::InvalidExtent)?;
            if previous_bytes != overlap_bytes {
                return Err(AssetCacheError::ConflictingOverlap);
            }
        }
        let start = previous.offset.min(extent.offset);
        let end = previous_end.max(extent_end);
        let merged_len = usize::try_from(end - start).map_err(|_| AssetCacheError::TooLarge)?;
        let mut merged_bytes = Vec::new();
        merged_bytes.try_reserve_exact(merged_len).map_err(|_| AssetCacheError::TooLarge)?;
        merged_bytes.resize(merged_len, 0);
        let previous_start = usize::try_from(previous.offset - start).map_err(|_| AssetCacheError::TooLarge)?;
        let extent_start = usize::try_from(extent.offset - start).map_err(|_| AssetCacheError::TooLarge)?;
        let previous_target_end = previous_start.checked_add(previous.bytes.len()).ok_or(AssetCacheError::TooLarge)?;
        let extent_target_end = extent_start.checked_add(extent.bytes.len()).ok_or(AssetCacheError::TooLarge)?;
        let previous_target = merged_bytes.get_mut(previous_start..previous_target_end).ok_or(AssetCacheError::InvalidExtent)?;
        previous_target.copy_from_slice(&previous.bytes);
        let extent_target = merged_bytes.get_mut(extent_start..extent_target_end).ok_or(AssetCacheError::InvalidExtent)?;
        extent_target.copy_from_slice(&extent.bytes);
        previous.offset = start;
        previous.bytes = merged_bytes;
    }
    if merged.len() > MAX_EXTENTS {
        return Err(AssetCacheError::TooLarge);
    }
    *extents = merged;
    Ok(())
}

fn encode(total_size: u64, extents: &[Extent], kind: Kind) -> Result<Vec<u8>, AssetCacheError> {
    if extents.is_empty() || extents.len() > MAX_EXTENTS {
        return Err(AssetCacheError::InvalidExtent);
    }
    let mut size = HEADER_BYTES;
    for extent in extents {
        validate_extent(kind, total_size, extent.offset, &extent.bytes)?;
        size = size.checked_add(16).and_then(|size| size.checked_add(extent.bytes.len())).ok_or(AssetCacheError::TooLarge)?;
    }
    if size > storage::MAX_VALUE_BYTES {
        return Err(AssetCacheError::TooLarge);
    }
    let mut output = Vec::new();
    output.try_reserve_exact(size).map_err(|_| AssetCacheError::TooLarge)?;
    output.write_all(MAGIC).map_err(|_| AssetCacheError::TooLarge)?;
    write_u64(&mut output, total_size)?;
    write_u32(&mut output, u32::try_from(extents.len()).map_err(|_| AssetCacheError::TooLarge)?)?;
    for extent in extents {
        write_u64(&mut output, extent.offset)?;
        write_u64(&mut output, u64::try_from(extent.bytes.len()).map_err(|_| AssetCacheError::TooLarge)?)?;
        output.write_all(&extent.bytes).map_err(|_| AssetCacheError::TooLarge)?;
    }
    Ok(output)
}

fn decode(kind: Kind, value: CacheValue) -> Result<CachedAsset, ()> {
    if value.bytes.len() < HEADER_BYTES || value.bytes.len() > storage::MAX_VALUE_BYTES {
        return Err(());
    }
    let mut input = Cursor::new(value.bytes.as_slice());
    let mut magic = [0; 8];
    input.read_exact(&mut magic).map_err(|_| ())?;
    if &magic != MAGIC {
        return Err(());
    }
    let total_size = read_u64(&mut input).map_err(|_| ())?;
    if total_size == 0 || total_size > u64::try_from(kind.max_response_bytes()).map_err(|_| ())? {
        return Err(());
    }
    let count = usize::try_from(read_u32(&mut input).map_err(|_| ())?).map_err(|_| ())?;
    if count == 0 || count > MAX_EXTENTS {
        return Err(());
    }
    let mut extents = Vec::new();
    extents.try_reserve_exact(count).map_err(|_| ())?;
    let mut previous_end = 0u64;
    for index in 0..count {
        let offset = read_u64(&mut input).map_err(|_| ())?;
        let len = read_u64(&mut input).map_err(|_| ())?;
        let end = offset.checked_add(len).ok_or(())?;
        if len == 0 || end > total_size || (index > 0 && offset <= previous_end) {
            return Err(());
        }
        let len_usize = usize::try_from(len).map_err(|_| ())?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len_usize).map_err(|_| ())?;
        bytes.resize(len_usize, 0);
        input.read_exact(&mut bytes).map_err(|_| ())?;
        extents.push(Extent { offset, bytes });
        previous_end = end;
    }
    if input.position() != u64::try_from(value.bytes.len()).map_err(|_| ())? {
        return Err(());
    }
    Ok(CachedAsset { total_size, extents, last_use: value.last_use, use_count: value.use_count })
}

fn write_u32(output: &mut Vec<u8>, value: u32) -> Result<(), AssetCacheError> {
    output.write_all(&value.to_le_bytes()).map_err(|_| AssetCacheError::TooLarge)
}

fn write_u64(output: &mut Vec<u8>, value: u64) -> Result<(), AssetCacheError> {
    output.write_all(&value.to_le_bytes()).map_err(|_| AssetCacheError::TooLarge)
}

fn read_u32(input: &mut Cursor<&[u8]>) -> io::Result<u32> {
    let mut bytes = [0; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(input: &mut Cursor<&[u8]>) -> io::Result<u64> {
    let mut bytes = [0; 8];
    input.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
