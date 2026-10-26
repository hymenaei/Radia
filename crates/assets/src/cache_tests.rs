use std::sync::Arc;

use tempfile::tempdir;
use uuid::Uuid;

use super::*;
use crate::budget::Blob;
use utils::AssetId;

fn resource(id: u128, kind: Kind) -> Resource {
    Resource { id: AssetId::new(Uuid::from_u128(id)), kind }
}

fn charged_blob(budget: &Budget, bytes: &[u8]) -> Blob {
    let mut owned = Vec::new();
    owned.try_reserve_exact(bytes.len()).expect("reserve payload");
    owned.extend_from_slice(bytes);
    budget.reserve(owned.capacity()).expect("reserve budget").finish(owned).expect("create blob")
}

#[test]
fn texture_payload_uses_sharded_raw_path() {
    let directory = tempdir().expect("cache directory");
    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache");
    let resource = Resource {
        id: AssetId::new(Uuid::parse_str("abcdef01-2345-6789-abcd-ef0123456789").expect("valid fixture UUID")),
        kind: Kind::Texture,
    };
    let bytes = b"normalized texture payload".to_vec();
    cache.put_complete(resource, bytes.clone()).expect("store raw payload");

    let path = directory.path().join("assets/textures/ab/abcdef01-2345-6789-abcd-ef0123456789.j2c");
    assert_eq!(std::fs::read(&path).expect("read texture file"), bytes);
    std::assert_matches!(cache.lookup(resource), AssetLookup::Hit(CacheValue { bytes: cached, .. }) if cached == bytes);
    assert_eq!(cache.stats().bytes_read, u64::try_from(bytes.len()).expect("payload length"));
    assert_eq!(cache.stats().bytes_written, u64::try_from(bytes.len()).expect("payload length"));
    cache.shutdown().expect("close cache");
    assert!(directory.path().join("assets/snapshot").exists());
    assert!(directory.path().join("assets/journal").exists());
    assert!(directory.path().join("assets/lock").exists());
}

#[test]
fn open_discards_the_previous_cache_root_and_sweeps_old_assets() {
    let directory = tempdir().expect("cache directory");
    std::fs::write(directory.path().join("snapshot"), b"old snapshot").expect("write old snapshot");
    std::fs::write(directory.path().join("journal"), b"old journal").expect("write old journal");
    let old_data = directory.path().join("data/old.bin");
    std::fs::create_dir_all(old_data.parent().expect("old data directory")).expect("create old data directory");
    std::fs::write(&old_data, b"old flat payload").expect("write old flat payload");
    let old_asset = directory.path().join("assets/textures/ab/abcdef01-2345-6789-abcd-ef0123456789.j2c");
    std::fs::create_dir_all(old_asset.parent().expect("old asset shard")).expect("create old asset shard");
    std::fs::write(&old_asset, b"old sharded payload").expect("write old sharded payload");

    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache");
    assert!(!directory.path().join("snapshot").exists());
    assert!(!directory.path().join("journal").exists());
    assert!(!directory.path().join("data").exists());
    cache.shutdown().expect("sweep old asset files");
    assert!(!old_asset.exists());
    assert!(directory.path().join("assets/snapshot").exists());
}

#[test]
fn llsd_kinds_use_distinct_paths() {
    let directory = tempdir().expect("cache directory");
    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache");
    let pbr = resource(0xabcdef0123456789abcdef0123456789, Kind::PbrMaterial);
    let legacy = resource(0xabcdef0123456789abcdef0123456789, Kind::LegacyMaterial);
    let environment = resource(0xabcdef0123456789abcdef0123456789, Kind::Environment);
    cache.put_complete(pbr, b"pbr llsd".to_vec()).expect("store PBR material");
    cache.put_complete(legacy, b"legacy llsd".to_vec()).expect("store legacy material");
    cache.put_complete(environment, b"environment llsd".to_vec()).expect("store environment");

    let name = "abcdef01-2345-6789-abcd-ef0123456789";
    assert_eq!(std::fs::read(directory.path().join(format!("assets/materials/pbr/ab/{name}.llsd"))).expect("read PBR file"), b"pbr llsd");
    assert_eq!(
        std::fs::read(directory.path().join(format!("assets/materials/legacy/ab/{name}.llsd"))).expect("read legacy file"),
        b"legacy llsd"
    );
    assert_eq!(
        std::fs::read(directory.path().join(format!("assets/environments/ab/{name}.llsd"))).expect("read environment file"),
        b"environment llsd"
    );
    assert!(!directory.path().join("assets/animations").exists());
    cache.shutdown().expect("close cache");
}

#[test]
fn lookup_rejects_payload_over_kind_bound() {
    let directory = tempdir().expect("cache directory");
    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache");
    let resource = resource(4, Kind::Texture);
    let key = storage_key(resource);
    cache.storage.put(key, vec![0; cache_limit(resource.kind) + 1]).expect("write over-bound storage entry");
    assert_eq!(cache.lookup(resource), AssetLookup::Corrupt);
    assert_eq!(cache.stats().corrupt_entries, 1);
    cache.shutdown().expect("close cache");
}

#[test]
fn write_admission_charges_raw_copy_and_bounds_in_flight() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let budget = Budget::default();
    let payload = b"raw payload";
    let writes = (0..MAX_IN_FLIGHT_WRITES)
        .map(|index| {
            cache
                .queue_write(resource(10 + u128::try_from(index).expect("index"), Kind::Texture), charged_blob(&budget, payload), &budget)
                .expect("admit write")
        })
        .collect::<Vec<_>>();
    assert_eq!(budget.stats().charged_bytes, payload.len() * MAX_IN_FLIGHT_WRITES * 2);
    assert!(cache.queue_write(resource(20, Kind::Texture), charged_blob(&budget, payload), &budget).is_none());
    assert_eq!(cache.stats().dropped_writes, 1);
    drop(writes);
    assert_eq!(budget.stats().charged_bytes, 0);
    assert_eq!(cache.stats().dropped_writes, 3);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn writes_record_raw_byte_counts() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let budget = Budget::default();
    let resource = resource(30, Kind::Environment);
    let write = cache.queue_write(resource, charged_blob(&budget, b"settings"), &budget).expect("admit write");
    write.run();
    assert_eq!(cache.stats().bytes_written, 8);
    assert_eq!(budget.stats().charged_bytes, 0);
    std::assert_matches!(cache.lookup(resource), AssetLookup::Hit(CacheValue { bytes, .. }) if bytes == b"settings");
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn unused_asset_cache_write_releases_admission() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let budget = Budget::default();
    let resource = resource(40, Kind::Texture);
    let write = cache.queue_write(resource, charged_blob(&budget, b"not written"), &budget).expect("admit write");
    assert_eq!(budget.stats().charged_bytes, 22);
    drop(write);
    assert_eq!(budget.stats().charged_bytes, 0);
    assert_eq!(cache.stats().dropped_writes, 1);
    assert_eq!(cache.lookup(resource), AssetLookup::Miss);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn cache_open_uses_its_assets_root_index() {
    let directory = tempdir().expect("cache directory");
    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache");
    assert_eq!(cache.lookup(resource(50, Kind::Texture)), AssetLookup::Miss);
    cache.storage.flush().expect("flush index");
    assert!(directory.path().join("assets/snapshot").exists());
    assert!(directory.path().join("assets/journal").exists());
    assert!(!directory.path().join("snapshot").exists());
    cache.shutdown().expect("close cache");
}
