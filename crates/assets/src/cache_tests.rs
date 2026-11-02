use std::sync::Arc;

use tempfile::tempdir;
use uuid::Uuid;

use super::*;
use crate::Resource;
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
    cache.put_complete(Request::new(resource), bytes.clone()).expect("store raw payload");

    let path = directory.path().join("assets/textures/ab/abcdef01-2345-6789-abcd-ef0123456789.j2c");
    assert_eq!(std::fs::read(&path).expect("read texture file"), bytes);
    std::assert_matches!(cache.lookup(Request::new(resource)), AssetLookup::Hit(CacheValue { bytes: cached, .. }) if cached == bytes);
    assert_eq!(cache.stats().bytes_read, u64::try_from(bytes.len()).expect("payload length"));
    assert_eq!(cache.stats().bytes_written, u64::try_from(bytes.len()).expect("payload length"));
    cache.shutdown().expect("close cache");
    assert!(directory.path().join("assets/index.sqlite").exists());
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
    assert!(directory.path().join("assets/index.sqlite").exists());
}

#[test]
fn llsd_kinds_use_distinct_paths() {
    let directory = tempdir().expect("cache directory");
    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache");
    let pbr = resource(0xabcdef0123456789abcdef0123456789, Kind::PbrMaterial);
    let legacy = resource(0xabcdef0123456789abcdef0123456789, Kind::LegacyMaterial);
    let environment = resource(0xabcdef0123456789abcdef0123456789, Kind::Environment);
    let mesh = resource(0xabcdef0123456789abcdef0123456789, Kind::Mesh);
    let texture = resource(0xabcdef0123456789abcdef0123456788, Kind::Texture);
    let mesh_request = Request::MeshHeader(mesh.id);
    let mesh_bytes = mesh_header_frame(100, b"mesh header").expect("frame mesh header");
    cache.put_complete(Request::new(pbr), b"pbr llsd".to_vec()).expect("store PBR material");
    cache.put_complete(Request::new(legacy), b"legacy llsd".to_vec()).expect("store legacy material");
    cache.put_complete(Request::new(environment), b"environment llsd".to_vec()).expect("store environment");
    cache.put_complete(mesh_request, mesh_bytes.clone()).expect("store mesh header");
    cache.put_complete(Request::new(texture), b"texture payload".to_vec()).expect("store texture");

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
    assert_eq!(
        std::fs::read(directory.path().join(format!("assets/meshes/header/ab/{name}.llmesh"))).expect("read mesh header file"),
        mesh_bytes
    );
    assert!(!directory.path().join("assets/animations").exists());
    assert_eq!(
        cache.stats().entries_by_kind,
        AssetCacheEntriesByKind { textures: 1, pbr_materials: 1, legacy_materials: 1, environments: 1, meshes: 1 }
    );
    cache.shutdown().expect("close cache");
}

#[test]
fn retired_whole_mesh_namespace_is_removed() {
    let directory = tempdir().expect("cache directory");
    let id = AssetId::new(Uuid::from_u128(55));
    let uuid = id.uuid().to_string();
    let shard = uuid.get(..2).expect("UUID shard");
    let old_path = directory.path().join(format!("assets/meshes/{shard}/{uuid}.llmesh"));
    let old_cache =
        storage::Cache::open(directory.path().join("assets"), storage::DEFAULT_SIZE_LIMIT, &[storage::Namespace::new("meshes", "llmesh")])
            .expect("open previous mesh namespace");
    old_cache
        .put(storage::Key::new(storage::NamespaceId::new(0), *id.uuid().as_bytes()), b"whole mesh".to_vec())
        .expect("write previous mesh entry");
    old_cache.shutdown().expect("close previous cache");
    assert!(old_path.exists());

    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open block cache");
    assert_eq!(cache.stats().entries_by_kind.meshes, 0);
    cache.shutdown().expect("finish namespace migration");
    assert!(!old_path.exists());
}

#[test]
fn open_uses_asset_policy_for_real_eviction() {
    const ENTRY_BYTES: usize = 16 * 1024;
    let directory = tempdir().expect("cache directory");
    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache");
    let expensive_mesh = resource(60, Kind::Mesh);
    let cheap_texture = resource(61, Kind::Texture);
    cache.observe_decode(Kind::Mesh, std::time::Duration::from_secs(30));
    let mesh_request = Request::MeshHeader(expensive_mesh.id);
    let mesh_header = vec![1; ENTRY_BYTES - crate::cache::MESH_HEADER_FRAME_BYTES];
    let mesh_entry = crate::cache::mesh_header_frame(ENTRY_BYTES as u64, &mesh_header).expect("frame mesh header");
    cache.put_complete(mesh_request, mesh_entry).expect("store expensive mesh first");
    let one_entry_limit = cache.accounted_size();
    cache.put_complete(Request::new(cheap_texture), vec![2; ENTRY_BYTES]).expect("store cheap texture second");

    cache.set_size_limit(one_entry_limit).expect("evict under asset cache limit");

    std::assert_matches!(cache.lookup(mesh_request), AssetLookup::Hit(_));
    assert_eq!(cache.lookup(Request::new(cheap_texture)), AssetLookup::Miss);
    cache.shutdown().expect("close cache");
}

#[test]
fn lookup_rejects_payload_over_kind_bound() {
    let directory = tempdir().expect("cache directory");
    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache");
    let resource = resource(4, Kind::Texture);
    let request = Request::new(resource);
    let key = storage_key(request);
    cache.storage.put(key, vec![0; cache_limit(request) + 1]).expect("write over-bound storage entry");
    assert_eq!(cache.lookup(request), AssetLookup::Corrupt);
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
                .queue_write(
                    Request::new(resource(10 + u128::try_from(index).expect("index"), Kind::Texture)),
                    charged_blob(&budget, payload),
                    &budget,
                )
                .expect("admit write")
        })
        .collect::<Vec<_>>();
    assert_eq!(budget.stats().charged_bytes, payload.len() * MAX_IN_FLIGHT_WRITES * 2);
    assert!(cache.queue_write(Request::new(resource(20, Kind::Texture)), charged_blob(&budget, payload), &budget).is_none());
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
    let write = cache.queue_write(Request::new(resource), charged_blob(&budget, b"settings"), &budget).expect("admit write");
    write.run();
    assert_eq!(cache.stats().bytes_written, 8);
    assert_eq!(budget.stats().charged_bytes, 0);
    std::assert_matches!(cache.lookup(Request::new(resource)), AssetLookup::Hit(CacheValue { bytes, .. }) if bytes == b"settings");
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn unused_asset_cache_write_releases_admission() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let budget = Budget::default();
    let resource = resource(40, Kind::Texture);
    let write = cache.queue_write(Request::new(resource), charged_blob(&budget, b"not written"), &budget).expect("admit write");
    assert_eq!(budget.stats().charged_bytes, 22);
    drop(write);
    assert_eq!(budget.stats().charged_bytes, 0);
    assert_eq!(cache.stats().dropped_writes, 1);
    assert_eq!(cache.lookup(Request::new(resource)), AssetLookup::Miss);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn cache_open_uses_its_assets_root_index() {
    let directory = tempdir().expect("cache directory");
    let cache = AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache");
    assert_eq!(cache.lookup(Request::new(resource(50, Kind::Texture))), AssetLookup::Miss);
    cache.storage.flush().expect("flush index");
    assert!(directory.path().join("assets/index.sqlite").exists());
    assert!(!directory.path().join("snapshot").exists());
    cache.shutdown().expect("close cache");
}
