use std::{
    assert_matches,
    io::Write,
    sync::{Arc, Barrier, mpsc},
    thread,
    time::Duration,
};

use flate2::{Compression, write::ZlibEncoder};
use llsd::binary;
use tempfile::tempdir;
use test_support::texture::{GRAY_4096_J2C, RGBA_J2C, RGBA_RAW};
use uuid::Uuid;

use super::*;
use crate::{
    Image, Priority,
    budget::{ASSET_WORK_BYTES, TEXTURE_WORK_BYTES},
    material::{MaterialDescription, PbrMaterial},
};

const PBR_JSON: &str = r#"{"asset":{"version":"2.0"},"materials":[{}]}"#;

struct StoreHarness {
    now: Instant,
    store: Store,
    route: Route,
}

impl StoreHarness {
    fn new() -> Self {
        Self::with_budget(Budget::default())
    }

    fn with_budget(budget: Budget) -> Self {
        let now = Instant::now();
        let mut store = Store::new(budget);
        let route = store.register_route().unwrap();
        Self { now, store, route }
    }
}

fn resource(id: u128) -> Resource {
    Resource { id: AssetId::new(Uuid::from_u128(id)), kind: Kind::Texture }
}

fn mesh_resource(id: u128) -> Resource {
    Resource { id: AssetId::new(Uuid::from_u128(id)), kind: Kind::Mesh }
}

fn mesh_llsd_integer(value: i32) -> Vec<u8> {
    let mut bytes = vec![b'i'];
    bytes.extend_from_slice(&value.to_be_bytes());
    bytes
}

fn mesh_llsd_real(value: f64) -> Vec<u8> {
    let mut bytes = vec![b'r'];
    bytes.extend_from_slice(&value.to_be_bytes());
    bytes
}

fn mesh_llsd_map(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut bytes = vec![b'{'];
    bytes.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_be_bytes());
    for (key, value) in entries {
        bytes.push(b'k');
        bytes.extend_from_slice(&u32::try_from(key.len()).unwrap().to_be_bytes());
        bytes.extend_from_slice(key.as_bytes());
        bytes.extend_from_slice(value);
    }
    bytes.push(b'}');
    bytes
}

fn mesh_llsd_array(items: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = vec![b'['];
    bytes.extend_from_slice(&u32::try_from(items.len()).unwrap().to_be_bytes());
    for item in items {
        bytes.extend_from_slice(item);
    }
    bytes.push(b']');
    bytes
}

fn mesh_llsd_binary(bytes: &[u8]) -> Vec<u8> {
    let mut encoded = vec![b'b'];
    encoded.extend_from_slice(&u32::try_from(bytes.len()).unwrap().to_be_bytes());
    encoded.extend_from_slice(bytes);
    encoded
}

fn mesh_llsd_domain() -> Vec<u8> {
    let vector = |values: &[f64]| mesh_llsd_array(&values.iter().copied().map(mesh_llsd_real).collect::<Vec<_>>());
    mesh_llsd_map(&[("Min", vector(&[0.0, 0.0, 0.0])), ("Max", vector(&[1.0, 1.0, 1.0]))])
}

fn mesh_block_fixture() -> Vec<u8> {
    let position = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0x00, 0x00];
    let uv = [0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff];
    let triangles = [0x00, 0x00, 0x01, 0x00, 0x02, 0x00];
    let submesh = mesh_llsd_map(&[
        ("Position", mesh_llsd_binary(&position)),
        ("PositionDomain", mesh_llsd_domain()),
        ("TexCoord0", mesh_llsd_binary(&uv)),
        (
            "TexCoord0Domain",
            mesh_llsd_map(&[
                ("Min", mesh_llsd_array(&[mesh_llsd_real(0.0), mesh_llsd_real(0.0)])),
                ("Max", mesh_llsd_array(&[mesh_llsd_real(1.0), mesh_llsd_real(1.0)])),
            ]),
        ),
        ("TriangleList", mesh_llsd_binary(&triangles)),
    ]);
    let mut payload = b"<?llsd/binary?>\n".to_vec();
    payload.extend(mesh_llsd_array(&[submesh]));
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&payload).unwrap();
    encoder.finish().unwrap()
}

fn mesh_lod_requests(id: AssetId) -> (MeshLodRequest, MeshLodRequest, Vec<u8>) {
    let block = mesh_block_fixture();
    let encoded_entry = |offset: i32, size: i32| mesh_llsd_map(&[("offset", mesh_llsd_integer(offset)), ("size", mesh_llsd_integer(size))]);
    let block_size = i32::try_from(block.len()).unwrap();
    let block_header = |low_offset: i32, high_offset: i32| {
        let mut bytes = b"<?llsd/binary?>\n".to_vec();
        bytes.extend(mesh_llsd_map(&[
            ("version", mesh_llsd_integer(1)),
            ("lowest_lod", encoded_entry(-1, 0)),
            ("low_lod", encoded_entry(low_offset, block_size)),
            ("medium_lod", encoded_entry(-1, 0)),
            ("high_lod", encoded_entry(high_offset, block_size)),
            ("physics_mesh", encoded_entry(-1, 0)),
            ("skin", encoded_entry(-1, 0)),
        ]));
        bytes
    };
    let header = block_header(0, block_size);
    let total = u64::try_from(header.len() + block.len() * 2).unwrap();
    let header = crate::mesh::decode_header(&header, total).unwrap();
    (header.lod(id, MeshLod::Low).unwrap(), header.lod(id, MeshLod::High).unwrap(), block)
}

fn mesh_lod_request(id: AssetId, block: &[u8]) -> MeshLodRequest {
    let entry = mesh_llsd_map(&[("offset", mesh_llsd_integer(0)), ("size", mesh_llsd_integer(i32::try_from(block.len()).unwrap()))]);
    let mut header_bytes = b"<?llsd/binary?>\n".to_vec();
    header_bytes.extend(mesh_llsd_map(&[("version", mesh_llsd_integer(1)), ("low_lod", entry)]));
    let asset_length = u64::try_from(header_bytes.len() + block.len()).unwrap();
    crate::mesh::decode_header(&header_bytes, asset_length).unwrap().lod(id, MeshLod::Low).unwrap()
}

fn large_mesh_lod_request(id: AssetId, size: usize) -> MeshLodRequest {
    let entry = mesh_llsd_map(&[("offset", mesh_llsd_integer(0)), ("size", mesh_llsd_integer(i32::try_from(size).unwrap()))]);
    let mut header = b"<?llsd/binary?>\n".to_vec();
    header.extend(mesh_llsd_map(&[("version", mesh_llsd_integer(1)), ("low_lod", entry)]));
    let asset_length = u64::try_from(header.len().checked_add(size).unwrap()).unwrap();
    crate::mesh::decode_header(&header, asset_length).unwrap().lod(id, MeshLod::Low).unwrap()
}

fn mesh_full_asset_fixture(id: AssetId, block: &[u8]) -> (MeshLodRequest, Vec<u8>) {
    let entry = mesh_llsd_map(&[("offset", mesh_llsd_integer(0)), ("size", mesh_llsd_integer(i32::try_from(block.len()).unwrap()))]);
    let mut header = b"<?llsd/binary?>\n".to_vec();
    header.extend(mesh_llsd_map(&[("version", mesh_llsd_integer(1)), ("low_lod", entry)]));
    let asset_length = u64::try_from(header.len() + block.len()).unwrap();
    let request = crate::mesh::decode_header(&header, asset_length).unwrap().lod(id, MeshLod::Low).unwrap();
    header.extend_from_slice(block);
    (request, header)
}

fn mesh_lod_record(offset: u64, length: u64, asset_length: u64, block: &[u8]) -> Vec<u8> {
    let mut record = Vec::new();
    record.extend_from_slice(&offset.to_le_bytes());
    record.extend_from_slice(&length.to_le_bytes());
    record.extend_from_slice(&asset_length.to_le_bytes());
    record.extend_from_slice(block);
    record
}

fn mesh_lod_record_for(request: MeshLodRequest, block: &[u8]) -> Vec<u8> {
    let range = request.range();
    mesh_lod_record(range.start, range.end - range.start, request.asset_length(), block)
}

fn mesh_header_record(total_length: u64, header: &[u8]) -> Vec<u8> {
    let mut record = Vec::new();
    record.extend_from_slice(&total_length.to_le_bytes());
    record.extend_from_slice(header);
    record
}

fn large_mesh_block_fixture(vertex_count: usize, face_count: usize) -> Vec<u8> {
    let position = mesh_llsd_binary(&vec![0; vertex_count * 6]);
    let mut triangle_list = Vec::with_capacity(vertex_count * 6);
    for index in 0..vertex_count * 3 {
        triangle_list.extend_from_slice(&u16::try_from(index % vertex_count).unwrap().to_le_bytes());
    }
    let submesh = mesh_llsd_map(&[("Position", position), ("TriangleList", mesh_llsd_binary(&triangle_list))]);
    let faces = vec![submesh; face_count];
    let mut payload = b"<?llsd/binary?>\n".to_vec();
    payload.extend(mesh_llsd_array(&faces));
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&payload).unwrap();
    encoder.finish().unwrap()
}

fn mesh_decode_job(budget: Budget, now: Instant, id: AssetId, bytes: Vec<u8>) -> (Store, Demand, DecodeJob) {
    let mut store = Store::new(budget);
    let route = store.register_route().unwrap();
    let request = mesh_lod_request(id, &bytes);
    let demand = store.request(route, Request::MeshLod(request), Priority::Visible, now).unwrap();
    let input = next_fetch(&mut store, now)
        .complete_response(Ok(FetchResponse::Partial { bytes, range: request.range(), total_length: Some(request.asset_length()) }))
        .unwrap();
    let job = store.start_decode(input).unwrap();
    (store, demand, job)
}

fn charged_blob(budget: &Budget, bytes: &[u8]) -> crate::Blob {
    let mut bytes_owned = Vec::new();
    bytes_owned.try_reserve_exact(bytes.len()).expect("reserve blob bytes");
    bytes_owned.extend_from_slice(bytes);
    budget.reserve(bytes_owned.capacity()).expect("reserve blob budget").finish(bytes_owned).expect("finish blob")
}

fn ready(demand: &Demand) -> Image {
    let Status::Ready(Content::Texture(blob)) = demand.status() else { panic!("expected ready asset") };
    blob.image().unwrap()
}

fn fetch(store: &mut Store, now: Instant, bytes: Vec<u8>) -> DecodeInput {
    next_fetch(store, now).complete(Ok(bytes)).expect("response is within the asset bound")
}

fn next_fetch(store: &mut Store, now: Instant) -> FetchJob {
    let LoadJob::Fetch(job) = store.next_job(now).expect("network fetch is scheduled") else { panic!("expected fetch job") };
    job
}

fn cached(store: &mut Store, now: Instant) -> DecodeInput {
    let LoadJob::Decode(input) = store.next_job(now).expect("cached decode is scheduled") else {
        panic!("retained texture source should bypass fetching")
    };
    input
}

#[test]
fn disk_hit_uses_decode_admission_and_does_not_count_as_a_fetch_attempt() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let resource = resource(11);
    cache.put_complete(Request::new(resource), RGBA_J2C.to_vec()).expect("seed validated texture bytes");
    let budget = Budget::default();
    let mut store = Store::with_cache(budget.clone(), Arc::clone(&cache));
    let now = Instant::now();
    let route = store.register_route().expect("register route");
    let demand = store.request(route, Request::new(resource), Priority::Visible, now).expect("request texture");
    let LoadJob::ReadCache(read) = store.next_job(now).expect("cache read job") else { panic!("first job reads cache") };
    assert_eq!(
        budget.stats().charged_bytes,
        read_work_bytes_for_request(Request::new(resource)).expect("bounded cache read reservation"),
        "cache reads reserve the bounded payload before decode admission",
    );
    let input = store.complete_cache_read(read.run(), now).expect("cache hit supplies input");
    let LoadJob::Decode(input) = input else { panic!("cache hit follows decode admission") };
    assert_eq!(
        store
            .entries
            .get(&LoadKey { resource, unit: CacheUnit::Whole })
            .and_then(|entry| entry.views.get(&route))
            .map(|view| view.attempts),
        Some(0)
    );
    let decode = store.start_decode(input).expect("decode admission");
    assert!(store.complete(decode.run(), now).is_empty());
    assert_matches!(demand.status(), Status::Ready(Content::Texture(_)));
    assert_eq!(cache.stats().hits, 1);
    assert_eq!(cache.stats().bytes_read, u64::try_from(RGBA_J2C.len()).expect("raw fixture size"));
    assert_eq!(budget.stats().charged_bytes, RGBA_J2C.len() + RGBA_RAW.len());
    drop(demand);
    store.close();
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn cached_mesh_reads_stay_within_storage_limit() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let now = Instant::now();
    let block = mesh_block_fixture();
    let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let route = store.register_route().expect("register route");
    let mut demands = Vec::new();
    for value in 200..200 + storage::MAX_IN_FLIGHT_READS + 4 {
        let id = AssetId::new(Uuid::from_u128(u128::try_from(value).expect("fixture asset ID")));
        let request = mesh_lod_request(id, &block);
        cache.put_complete(Request::MeshLod(request), mesh_lod_record_for(request, &block)).expect("seed cached mesh block");
        demands.push(store.request(route, Request::MeshLod(request), Priority::Visible, now).expect("request cached mesh"));
    }

    let mut completed = 0;
    while completed < demands.len() {
        let mut reads = Vec::new();
        while let Some(job) = store.next_job(now) {
            match job {
                LoadJob::ReadCache(read) => reads.push(read),
                LoadJob::Fetch(_) => panic!("cached mesh requests stay on disk"),
                LoadJob::Decode(_) => panic!("mesh data is not resident before the first read"),
            }
        }
        assert!(!reads.is_empty(), "a released read permit admits the next cached mesh");
        if completed == 0 {
            assert_eq!(reads.len(), storage::MAX_IN_FLIGHT_READS);
        }
        let mut inputs = Vec::new();
        for read in reads {
            let Some(LoadJob::Decode(input)) = store.complete_cache_read(read.run(), now) else {
                panic!("disk hit supplies mesh decode input")
            };
            inputs.push(input);
        }
        if completed == 0 {
            let mut remaining_reads = Vec::new();
            while let Some(job) = store.next_job(now) {
                let LoadJob::ReadCache(read) = job else { panic!("cached mesh hits do not fall through to HTTP") };
                remaining_reads.push(read);
            }
            assert_eq!(remaining_reads.len(), demands.len() - inputs.len(), "mesh decode inputs do not hold storage read permits");
            for read in remaining_reads {
                let Some(LoadJob::Decode(input)) = store.complete_cache_read(read.run(), now) else {
                    panic!("disk hit supplies mesh decode input")
                };
                inputs.push(input);
            }
        }
        for input in inputs {
            let decode = store.start_decode(input).expect("admit disk mesh decode");
            assert!(store.complete(decode.run(), now).is_empty());
            completed += 1;
        }
    }

    assert_eq!(cache.stats().hits, u64::try_from(demands.len()).expect("fixture count"));
    assert!(demands.iter().all(|demand| matches!(demand.status(), Status::Ready(Content::Mesh(_)))));
    drop(demands);
    store.close();
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn cache_read_limit_requeues_lookup() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let now = Instant::now();
    let block = mesh_block_fixture();
    let request = mesh_lod_request(AssetId::new(Uuid::from_u128(212)), &block);
    cache.put_complete(Request::MeshLod(request), mesh_lod_record_for(request, &block)).expect("seed cached mesh block");
    let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let route = store.register_route().expect("register route");
    let demand = store.request(route, Request::MeshLod(request), Priority::Visible, now).expect("request cached mesh");
    let LoadJob::ReadCache(read) = store.next_job(now).expect("cache lookup") else { panic!("warm LOD reads cache") };
    let completion = read.run_lookup(|_, _| AssetLookup::Unavailable(storage::UnavailableReason::ReadLimit));
    assert!(store.complete_cache_read(completion, now).is_none());
    assert_matches!(demand.status(), Status::Queued);

    let LoadJob::ReadCache(read) = store.next_job(now).expect("deferred lookup is retried") else {
        panic!("ReadLimit is not a cache miss")
    };
    let Some(LoadJob::Decode(input)) = store.complete_cache_read(read.run(), now) else { panic!("disk hit supplies mesh decode input") };
    let decode = store.start_decode(input).expect("admit cached mesh decode");
    assert!(store.complete(decode.run(), now).is_empty());
    assert_matches!(demand.status(), Status::Ready(Content::Mesh(_)));
    assert_eq!(cache.stats().hits, 1);
    drop(demand);
    store.close();
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn demand_lease_protects_cache_key() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let resource = resource(10);
    let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let route = store.register_route().expect("register route");
    let demand = store.request(route, Request::new(resource), Priority::Visible, Instant::now()).expect("request texture");
    assert_eq!(cache.stats().protected_bytes, 0);

    cache.put_complete(Request::new(resource), vec![7; 1024]).expect("write demanded key");
    assert_eq!(cache.stats().protected_bytes, 1024);
    let second_demand = demand.clone();
    drop(demand);
    assert_eq!(cache.stats().protected_bytes, 1024);
    drop(second_demand);
    assert_eq!(cache.stats().protected_bytes, 0);

    store.close();
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn cache_miss_skips_disk_job() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let now = Instant::now();
    let route = store.register_route().expect("register route");
    let demand = store.request(route, Request::new(resource(15)), Priority::Visible, now).expect("request texture");

    assert_matches!(store.next_job(now), Some(LoadJob::Fetch(_)));
    assert_eq!(cache.stats().misses, 0);
    assert_matches!(demand.status(), Status::Fetching);

    drop(demand);
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn retired_route_discards_a_completed_disk_read() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let resource = resource(12);
    cache.put_complete(Request::new(resource), RGBA_J2C.to_vec()).expect("seed validated texture bytes");
    let budget = Budget::default();
    let mut store = Store::with_cache(budget.clone(), Arc::clone(&cache));
    let now = Instant::now();
    let route = store.register_route().expect("register route");
    let demand = store.request(route, Request::new(resource), Priority::Visible, now).expect("request texture");
    let LoadJob::ReadCache(read) = store.next_job(now).expect("cache read job") else { panic!("first job reads cache") };
    let (started, started_receive) = mpsc::sync_channel(0);
    let (release, release_receive) = mpsc::sync_channel(0);
    let worker = thread::spawn(move || {
        read.run_lookup(|cache, request| {
            started.send(()).expect("signal active disk read");
            release_receive.recv().expect("release disk read");
            cache.lookup(request)
        })
    });
    started_receive.recv_timeout(Duration::from_secs(3)).expect("disk read job started");
    store.retire_route(route);
    release.send(()).expect("resume disk read");
    let completion = worker.join().expect("disk read worker completes");
    assert!(store.complete_cache_read(completion, now).is_none());
    assert_eq!(budget.stats().charged_bytes, 0);
    assert_eq!(cache.stats().hits, 1);
    assert_matches!(demand.status(), Status::Failed { failure: Failure::RouteClosed, .. });
    let retry_route = store.register_route().expect("register replacement route");
    let retry_demand = store.request(retry_route, Request::new(resource), Priority::Visible, now).expect("retry texture demand");
    assert_matches!(store.next_job(now), Some(LoadJob::ReadCache(_)));
    drop(retry_demand);
    drop(demand);
    store.close();
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn cancelled_read_allows_retry() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let resource = resource(14);
    cache.put_complete(Request::new(resource), RGBA_J2C.to_vec()).expect("seed validated texture bytes");
    let budget = Budget::default();
    let mut store = Store::with_cache(budget.clone(), Arc::clone(&cache));
    let now = Instant::now();
    let route = store.register_route().expect("register route");
    let demand = store.request(route, Request::new(resource), Priority::Visible, now).expect("request texture");
    let LoadJob::ReadCache(read) = store.next_job(now).expect("cache read job") else { panic!("first job reads cache") };
    let (started, started_receive) = mpsc::sync_channel(0);
    let (release, release_receive) = mpsc::sync_channel(0);
    let worker = thread::spawn(move || {
        read.run_lookup(|cache, request| {
            started.send(()).expect("signal active disk read");
            release_receive.recv().expect("release disk read");
            cache.lookup(request)
        })
    });
    started_receive.recv_timeout(Duration::from_secs(3)).expect("disk read job started");
    assert_eq!(budget.stats().charged_bytes, read_work_bytes_for_request(Request::new(resource)).expect("read reservation"));
    drop(demand);
    store.maintain(now);
    release.send(()).expect("resume disk read");
    let completion = worker.join().expect("disk read worker completes");
    assert!(store.complete_cache_read(completion, now).is_none());
    assert_eq!(store.running_mesh_fetches + store.running_other_fetches, 0);
    assert_eq!(store.running_texture_decodes + store.running_other_decodes, 0);
    assert_eq!(budget.stats().charged_bytes, 0);
    assert!(store.next_job(now).is_none(), "cancelled demand must not enter decode admission");

    let retry = store.request(route, Request::new(resource), Priority::Visible, now).expect("retry texture demand");
    let LoadJob::ReadCache(read) = store.next_job(now).expect("later demand reads cache") else { panic!("retry begins with disk read") };
    let Some(LoadJob::Decode(input)) = store.complete_cache_read(read.run(), now) else { panic!("cache hit supplies decode input") };
    let decode = store.start_decode(input).expect("admit retry decode");
    assert!(store.complete(decode.run(), now).is_empty());
    assert_matches!(retry.status(), Status::Ready(Content::Texture(_)));
    assert_eq!(store.running_mesh_fetches + store.running_other_fetches, 0);
    assert_eq!(store.running_texture_decodes + store.running_other_decodes, 0);
    store.close();
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn malformed_disk_payload_is_discarded_without_failing_demand_attempts() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let resource = resource(13);
    cache.put_complete(Request::new(resource), b"bad".to_vec()).expect("seed semantically malformed payload");
    let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let now = Instant::now();
    let route = store.register_route().expect("register route");
    let _demand = store.request(route, Request::new(resource), Priority::Visible, now).expect("request texture");
    let LoadJob::ReadCache(read) = store.next_job(now).expect("cache read job") else { panic!("first job reads cache") };
    let Some(LoadJob::Decode(input)) = store.complete_cache_read(read.run(), now) else { panic!("cache hit enters decode") };
    let decode = store.start_decode(input).expect("decode admission");
    let _ = store.complete(decode.run(), now);
    assert_eq!(cache.stats().corrupt_entries, 1);
    let LoadJob::Fetch(fetch) = store.next_job(now).expect("malformed cache entry falls back to network") else {
        panic!("malformed cache entry schedules a fetch")
    };
    let input = fetch.complete(Ok(RGBA_J2C.to_vec())).expect("valid network response");
    let decode = store.start_decode(input).expect("decode admission");
    let writes = store.complete(decode.run(), now);
    assert_eq!(writes.len(), 1);
    for write in writes {
        write.run();
    }
    let crate::cache::AssetLookup::Hit(asset) = cache.lookup(Request::new(resource)) else {
        panic!("valid network bytes replace corrupt cache data")
    };
    assert_eq!(asset.bytes, RGBA_J2C);
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn full_write_admission_does_not_hold_network_fetch_admission() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let write_budget = Budget::default();
    let writes = (0..2u128)
        .map(|id| {
            cache.queue_write(Request::new(resource(20 + id)), charged_blob(&write_budget, b"payload"), &write_budget).expect("write slot")
        })
        .collect::<Vec<_>>();
    assert!(cache.queue_write(Request::new(resource(22)), charged_blob(&write_budget, b"dropped"), &write_budget).is_none());
    assert_eq!(cache.stats().dropped_writes, 1);
    let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let now = Instant::now();
    let route = store.register_route().expect("register route");
    let demands = (30..30 + MAX_NON_MESH_FETCHES)
        .map(|id| store.request(route, Request::new(resource(id as u128)), Priority::Visible, now).expect("request texture"))
        .collect::<Vec<_>>();
    let mut fetches = Vec::new();
    for _ in 0..MAX_NON_MESH_FETCHES {
        let LoadJob::Fetch(fetch) = store.next_job(now).expect("cold asset fetch") else { panic!("cache miss bypasses disk reads") };
        assert!(!fetch.is_cancelled());
        fetches.push(fetch);
    }
    assert!(store.next_job(now).is_none(), "running fetch limit remains enforced under cache write pressure");
    drop(fetches);
    drop(writes);
    assert_eq!(write_budget.stats().charged_bytes, 0);
    drop(demands);
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn decoded_material_and_environment_payloads_survive_disk_round_trips() {
    fn round_trip(cache: &Arc<crate::cache::AssetCache>, resource: Resource, response: Vec<u8>) {
        let now = Instant::now();
        let mut store = Store::with_cache(Budget::default(), Arc::clone(cache));
        let route = store.register_route().expect("register route");
        let demand = store.request(route, Request::new(resource), Priority::Visible, now).expect("request asset");
        let LoadJob::Fetch(fetch) = store.next_job(now).expect("cold asset fetch") else { panic!("cache miss bypasses disk reads") };
        let input = fetch.complete(Ok(response)).expect("attach encoded asset response");
        let decode = store.start_decode(input).expect("admit network decode");
        let writes = store.complete(decode.run(), now);
        assert_eq!(writes.len(), 1);
        for write in writes {
            write.run();
        }
        assert_matches!(demand.status(), Status::Ready(_));
        drop(demand);
        drop(store);

        let mut store = Store::with_cache(Budget::default(), Arc::clone(cache));
        let route = store.register_route().expect("register warm route");
        let demand = store.request(route, Request::new(resource), Priority::Visible, now).expect("request warm asset");
        let LoadJob::ReadCache(read) = store.next_job(now).expect("warm cache read") else { panic!("warm work reads disk") };
        let Some(LoadJob::Decode(input)) = store.complete_cache_read(read.run(), now) else { panic!("disk hit supplies decode input") };
        let decode = store.start_decode(input).expect("admit disk decode");
        assert!(store.complete(decode.run(), now).is_empty());
        assert_matches!(demand.status(), Status::Ready(_));
        drop(demand);
        store.close();
    }

    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let pbr = Resource { id: AssetId::new(Uuid::from_u128(101)), kind: Kind::PbrMaterial };
    let legacy = Resource { id: AssetId::new(Uuid::from_u128(102)), kind: Kind::LegacyMaterial };
    let environment = Resource { id: AssetId::new(Uuid::from_u128(103)), kind: Kind::Environment };
    round_trip(&cache, pbr, material_envelope(PBR_JSON));
    round_trip(&cache, legacy, legacy_envelope(legacy.id));
    round_trip(&cache, environment, b"{'type':'sky'}".to_vec());
    assert_eq!(cache.stats().hits, 3);
    assert_eq!(cache.stats().misses, 0);
    assert!(cache.stats().bytes_written > 0);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn cached_full_mesh_response_finishes() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let budget = Budget::default();
    let now = Instant::now();
    let id = AssetId::new(Uuid::from_u128(104));
    let block = mesh_block_fixture();
    let (lod_request, asset) = mesh_full_asset_fixture(id, &block);
    let request = Request::MeshLod(lod_request);
    let mut store = Store::with_cache(budget, Arc::clone(&cache));
    let route = store.register_route().expect("register route");
    let demand = store.request(route, request, Priority::Visible, now).expect("request mesh LOD");

    let LoadJob::Fetch(fetch) = store.next_job(now).expect("cold mesh fetch") else { panic!("cache miss bypasses disk reads") };
    assert!(fetch.response_limit() < asset.len());
    store.complete(LoadJob::Fetch(fetch).fail(Failure::FullAssetRequired), now);
    let LoadJob::Fetch(fetch) = store.next_job(now).expect("full mesh retry follows an oversized range response") else {
        panic!("full mesh retry stays on the network")
    };
    assert!(fetch.force_full_mesh());
    assert_eq!(fetch.response_limit(), Kind::Mesh.max_response_bytes());
    let input = fetch.complete(Ok(asset.clone())).expect("full asset response enters decoding after retry");
    let decode = store.start_decode(input).expect("admit mesh decode");
    let writes = store.complete(decode.run(), now);

    assert_matches!(demand.status(), Status::Ready(Content::Mesh(mesh)) if mesh.lod() == MeshLod::Low);
    assert_eq!(writes.len(), 1);
    for write in writes {
        write.run();
    }
    let crate::cache::AssetLookup::Hit(cached) = cache.lookup(request) else { panic!("full mesh LOD was cached") };
    assert_eq!(crate::cache::parse_mesh_lod_frame(lod_request, &cached.bytes), Some(block.as_slice()));
    assert!(cached.bytes.len() < asset.len());
    drop(demand);
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");

    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("reopen cache"));
    let crate::cache::AssetLookup::Hit(cached) = cache.lookup(request) else { panic!("SQLite index retained the mesh LOD") };
    assert_eq!(crate::cache::parse_mesh_lod_frame(lod_request, &cached.bytes), Some(block.as_slice()));
    assert_eq!(cache.stats().entries_by_kind.meshes, 1, "a full response stores only its requested LOD");
    let mut next_store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let next_route = next_store.register_route().expect("register next route");
    let next_demand = next_store.request(next_route, request, Priority::Visible, now).expect("request persisted LOD");
    let LoadJob::ReadCache(read) = next_store.next_job(now).expect("read persisted LOD") else { panic!("second session reads cache") };
    let Some(LoadJob::Decode(input)) = next_store.complete_cache_read(read.run(), now) else { panic!("second session hits cache") };
    let decode = next_store.start_decode(input).expect("admit cached LOD decode");
    assert!(next_store.complete(decode.run(), now).is_empty());
    assert_matches!(next_demand.status(), Status::Ready(Content::Mesh(mesh)) if mesh.lod() == MeshLod::Low);
    drop(next_demand);
    next_store.close();
    drop(next_store);
    Arc::try_unwrap(cache).expect("reopened cache handle released").shutdown().expect("close reopened cache");
}

#[test]
fn partial_mesh_lod_is_cached() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let now = Instant::now();
    let id = AssetId::new(Uuid::from_u128(105));
    let (request, _, bytes) = mesh_lod_requests(id);
    let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let route = store.register_route().expect("register route");
    let demand = store.request(route, Request::MeshLod(request), Priority::Visible, now).expect("request mesh LOD");

    let LoadJob::Fetch(fetch) = store.next_job(now).expect("cold mesh fetch") else { panic!("cache miss bypasses disk reads") };
    let input = fetch
        .complete_response(Ok(FetchResponse::Partial { bytes, range: request.range(), total_length: Some(request.asset_length()) }))
        .expect("partial mesh block enters decoding");
    let decode = store.start_decode(input).expect("admit mesh decode");
    let writes = store.complete(decode.run(), now);

    assert_matches!(demand.status(), Status::Ready(Content::Mesh(mesh)) if mesh.lod() == MeshLod::Low);
    assert_eq!(writes.len(), 1, "a successfully decoded partial LOD is persisted as its own block");
    for write in writes {
        write.run();
    }
    drop(demand);
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");

    let cache = crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("reopen cache");
    assert_matches!(cache.lookup(Request::MeshLod(request)), crate::cache::AssetLookup::Hit(_));
    cache.shutdown().expect("close reopened cache");
}

#[test]
fn partial_mesh_metadata_is_validated() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let now = Instant::now();

    for (index, malformed) in ["range", "missing total", "wrong total"].into_iter().enumerate() {
        let id = AssetId::new(Uuid::from_u128(110 + u128::try_from(index).expect("case index")));
        let (request, _, bytes) = mesh_lod_requests(id);
        let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
        let route = store.register_route().expect("register route");
        let demand = store.request(route, Request::MeshLod(request), Priority::Visible, now).expect("request mesh LOD");
        let LoadJob::Fetch(fetch) = store.next_job(now).expect("cold LOD fetch") else { panic!("cache miss bypasses disk reads") };
        let range = if malformed == "range" { 0..request.range().end } else { request.range() };
        let total_length = match malformed {
            "missing total" => None,
            "wrong total" => Some(request.asset_length().saturating_sub(1)),
            _ => Some(request.asset_length()),
        };
        let input = fetch.complete_response(Ok(FetchResponse::Partial { bytes, range, total_length })).expect("attach range response");
        let decode = store.start_decode(input).expect("admit LOD decode");
        let completion = decode.run();
        assert_eq!(completion.failure(), Some(Failure::MalformedBlock));
        assert!(store.complete(completion, now).is_empty());
        assert_matches!(demand.status(), Status::Failed { failure: Failure::MalformedBlock, .. });
        assert_eq!(cache.lookup(Request::MeshLod(request)), crate::cache::AssetLookup::Miss);
        drop(demand);
        store.close();
    }
}

#[test]
fn malformed_mesh_blocks_are_not_cached() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let now = Instant::now();
    let id = AssetId::new(Uuid::from_u128(114));
    let request = mesh_lod_request(id, b"bad");
    let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let route = store.register_route().expect("register route");
    let demand = store.request(route, Request::MeshLod(request), Priority::Visible, now).expect("request mesh LOD");
    let LoadJob::Fetch(fetch) = store.next_job(now).expect("cold LOD fetch") else { panic!("cache miss bypasses disk reads") };
    let input = fetch
        .complete_response(Ok(FetchResponse::Partial {
            bytes: b"bad".to_vec(),
            range: request.range(),
            total_length: Some(request.asset_length()),
        }))
        .expect("attach malformed block");
    let decode = store.start_decode(input).expect("admit LOD decode");
    let completion = decode.run();
    assert_eq!(completion.failure(), Some(Failure::MalformedBlock));
    assert!(store.complete(completion, now).is_empty());
    assert_matches!(demand.status(), Status::Failed { failure: Failure::MalformedBlock, .. });
    assert_eq!(cache.lookup(Request::MeshLod(request)), crate::cache::AssetLookup::Miss);
    drop(demand);
    store.close();
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn corrupt_mesh_lods_are_refetched() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let now = Instant::now();
    let block = mesh_block_fixture();

    for (index, corruption) in ["hash", "framing", "offset", "length", "body length", "asset length"].into_iter().enumerate() {
        let id = AssetId::new(Uuid::from_u128(120 + u128::try_from(index).expect("corruption index")));
        let request = mesh_lod_request(id, &block);
        let range = request.range();
        let valid_record = mesh_lod_record_for(request, &block);
        let bytes = match corruption {
            "framing" => b"bad framing".to_vec(),
            "offset" => mesh_lod_record(range.start.saturating_add(1), range.end - range.start, request.asset_length(), &block),
            "length" => mesh_lod_record(range.start, range.end - range.start + 1, request.asset_length(), &block),
            "body length" => mesh_lod_record(
                range.start,
                range.end - range.start,
                request.asset_length(),
                block.get(..block.len().saturating_sub(1)).unwrap_or_default(),
            ),
            "asset length" => mesh_lod_record(range.start, range.end - range.start, request.asset_length().saturating_add(1), &block),
            _ => valid_record,
        };
        cache.put_complete(Request::MeshLod(request), bytes).expect("seed corrupt LOD");
        if corruption == "hash" {
            let uuid = id.uuid().to_string();
            let shard = uuid.get(..2).expect("UUID shard");
            let path = directory.path().join(format!("assets/meshes/lod/low/{shard}/{uuid}.llmesh"));
            let mut stored = std::fs::read(&path).expect("read cached LOD");
            if let Some(first) = stored.first_mut() {
                *first ^= 0xff;
            }
            std::fs::write(path, stored).expect("damage cached LOD hash");
        }

        let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
        let route = store.register_route().expect("register route");
        let demand = store.request(route, Request::MeshLod(request), Priority::Visible, now).expect("request mesh LOD");
        let LoadJob::ReadCache(read) = store.next_job(now).expect("cache lookup") else { panic!("warm LOD reads cache") };
        let Some(LoadJob::Fetch(fetch)) = store.complete_cache_read(read.run(), now) else { panic!("corrupt LOD refetches") };
        let input = fetch
            .complete_response(Ok(FetchResponse::Partial {
                bytes: block.clone(),
                range: request.range(),
                total_length: Some(request.asset_length()),
            }))
            .expect("attach valid block");
        let decode = store.start_decode(input).expect("admit LOD decode");
        let writes = store.complete(decode.run(), now);
        assert_eq!(writes.len(), 1, "{corruption} corruption is replaced by a validated network block");
        for write in writes {
            write.run();
        }
        assert_matches!(demand.status(), Status::Ready(Content::Mesh(mesh)) if mesh.lod() == MeshLod::Low);
        let crate::cache::AssetLookup::Hit(cached) = cache.lookup(Request::MeshLod(request)) else {
            panic!("{corruption} entry was replaced")
        };
        assert_eq!(crate::cache::parse_mesh_lod_frame(request, &cached.bytes), Some(block.as_slice()));
        assert_eq!(cache.stats().corrupt_entries, u64::try_from(index + 1).expect("corruption count"));
        drop(demand);
        store.close();
    }

    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn malformed_cached_mesh_lod_is_refetched() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let now = Instant::now();
    let id = AssetId::new(Uuid::from_u128(133));
    let block = mesh_block_fixture();
    let request = mesh_lod_request(id, &block);
    let mut malformed = block.clone();
    if let Some(first) = malformed.first_mut() {
        *first ^= 0xff;
    }
    cache.put_complete(Request::MeshLod(request), mesh_lod_record_for(request, &malformed)).expect("seed semantically malformed block");

    let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let route = store.register_route().expect("register route");
    let demand = store.request(route, Request::MeshLod(request), Priority::Visible, now).expect("request mesh LOD");
    let LoadJob::ReadCache(read) = store.next_job(now).expect("cache read") else { panic!("warm LOD reads cache") };
    let Some(LoadJob::Decode(input)) = store.complete_cache_read(read.run(), now) else { panic!("framed entry is a cache hit") };
    let decode = store.start_decode(input).expect("admit disk decode");
    let completion = decode.run();
    assert_eq!(completion.failure(), Some(Failure::MalformedBlock));
    assert!(store.complete(completion, now).is_empty());
    let LoadJob::Fetch(fetch) = store.next_job(now).expect("malformed cache block falls back") else {
        panic!("malformed cache data schedules network fetch")
    };
    let input = fetch
        .complete_response(Ok(FetchResponse::Partial {
            bytes: block.clone(),
            range: request.range(),
            total_length: Some(request.asset_length()),
        }))
        .expect("attach valid network block");
    let decode = store.start_decode(input).expect("admit network decode");
    let writes = store.complete(decode.run(), now);
    assert_eq!(writes.len(), 1);
    for write in writes {
        write.run();
    }
    assert_matches!(demand.status(), Status::Ready(Content::Mesh(mesh)) if mesh.lod() == MeshLod::Low);
    assert_eq!(cache.stats().corrupt_entries, 1);
    let crate::cache::AssetLookup::Hit(cached) = cache.lookup(Request::MeshLod(request)) else {
        panic!("valid block replaced malformed block")
    };
    assert_eq!(crate::cache::parse_mesh_lod_frame(request, &cached.bytes), Some(block.as_slice()));
    drop(demand);
    store.close();
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn corrupt_mesh_header_is_refetched() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let now = Instant::now();
    let id = AssetId::new(Uuid::from_u128(126));
    let block = mesh_block_fixture();
    let (lod_request, asset) = mesh_full_asset_fixture(id, &block);
    let request = Request::MeshHeader(id);
    cache.put_complete(request, b"invalid header frame".to_vec()).expect("seed malformed header");
    let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let route = store.register_route().expect("register route");
    let demand = store.request(route, request, Priority::Visible, now).expect("request mesh header");
    let LoadJob::ReadCache(read) = store.next_job(now).expect("cache lookup") else { panic!("warm header reads cache") };
    let Some(LoadJob::Fetch(fetch)) = store.complete_cache_read(read.run(), now) else { panic!("corrupt header refetches") };
    let input = fetch.complete(Ok(asset.clone())).expect("attach full mesh");
    let decode = store.start_decode(input).expect("admit header decode");
    let writes = store.complete(decode.run(), now);
    assert_eq!(writes.len(), 1);
    for write in writes {
        write.run();
    }
    assert_matches!(demand.status(), Status::Ready(Content::MeshHeader(header)) if header.asset_length() == asset.len() as u64);
    let crate::cache::AssetLookup::Hit(cached) = cache.lookup(request) else { panic!("valid header replaced malformed framing") };
    let (length, header_bytes) = crate::cache::parse_mesh_header_frame(&cached.bytes).expect("valid framed header");
    assert_eq!(length, asset.len() as u64);
    assert_eq!(header_bytes.len(), usize::try_from(lod_request.range().start).expect("header size"));
    assert_eq!(cache.stats().entries_by_kind.meshes, 1);
    drop(demand);
    store.close();
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn cached_header_total_rejects_lod_with_mismatched_asset_length() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let now = Instant::now();
    let id = AssetId::new(Uuid::from_u128(132));
    let block = mesh_block_fixture();
    let (request, asset) = mesh_full_asset_fixture(id, &block);
    let header_length = usize::try_from(request.range().start).expect("header length");
    let header_bytes = asset.get(..header_length).expect("header prefix");
    let header_request = Request::MeshHeader(id);
    cache.put_complete(header_request, mesh_header_record(request.asset_length(), header_bytes)).expect("seed header frame");
    let wrong_total = request.asset_length().saturating_add(1);
    cache
        .put_complete(
            Request::MeshLod(request),
            mesh_lod_record(request.range().start, request.range().end - request.range().start, wrong_total, &block),
        )
        .expect("seed LOD with mismatched total");

    let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let route = store.register_route().expect("register route");
    let header_demand = store.request(route, header_request, Priority::Visible, now).expect("request cached header");
    let LoadJob::ReadCache(header_read) = store.next_job(now).expect("read header") else { panic!("header uses cache") };
    let Some(LoadJob::Decode(header_input)) = store.complete_cache_read(header_read.run(), now) else { panic!("header entry hits") };
    let header_decode = store.start_decode(header_input).expect("admit cached header");
    assert!(store.complete(header_decode.run(), now).is_empty());
    let Status::Ready(Content::MeshHeader(header)) = header_demand.status() else { panic!("header decoded") };
    let block_request = header.lod(id, MeshLod::Low).expect("header contains low LOD");
    assert_eq!(block_request.asset_length(), request.asset_length());
    let lod_demand =
        store.request(route, Request::MeshLod(block_request), Priority::Visible, now).expect("request block described by header");
    let LoadJob::ReadCache(lod_read) = store.next_job(now).expect("read LOD") else { panic!("LOD uses cache") };
    let Some(LoadJob::Fetch(fetch)) = store.complete_cache_read(lod_read.run(), now) else { panic!("mismatched LOD refetches") };
    let input = fetch
        .complete_response(Ok(FetchResponse::Partial {
            bytes: block,
            range: block_request.range(),
            total_length: Some(block_request.asset_length()),
        }))
        .expect("attach correct network block");
    let decode = store.start_decode(input).expect("admit network LOD");
    let writes = store.complete(decode.run(), now);
    assert_eq!(writes.len(), 1);
    for write in writes {
        write.run();
    }
    assert_matches!(lod_demand.status(), Status::Ready(Content::Mesh(mesh)) if mesh.lod() == MeshLod::Low);
    assert_eq!(cache.stats().corrupt_entries, 1);
    assert_eq!(cache.stats().entries_by_kind.meshes, 2, "the header remains while only the mismatched LOD is replaced");
    drop(lod_demand);
    drop(header_demand);
    store.close();
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn present_mesh_lod_is_cached() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let now = Instant::now();
    let id = AssetId::new(Uuid::from_u128(127));
    let block = mesh_block_fixture();
    let (_lod_request, asset) = mesh_full_asset_fixture(id, &block);
    let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let route = store.register_route().expect("register route");
    let header_demand = store.request(route, Request::MeshHeader(id), Priority::Visible, now).expect("request header");
    let LoadJob::Fetch(header_fetch) = store.next_job(now).expect("cold header fetch") else { panic!("cache miss bypasses disk reads") };
    let header_input = header_fetch.complete(Ok(asset.clone())).expect("attach full mesh header response");
    let header_decode = store.start_decode(header_input).expect("admit header decode");
    let header_writes = store.complete(header_decode.run(), now);
    for write in header_writes {
        write.run();
    }
    let Status::Ready(Content::MeshHeader(header)) = header_demand.status() else { panic!("mesh header decoded") };
    assert_eq!(header.lod(id, MeshLod::High), Err(Failure::MissingLod(MeshLod::High)));
    let present = header.lod(id, MeshLod::Low).expect("low LOD is present");
    let lod_demand = store.request(route, Request::MeshLod(present), Priority::Visible, now).expect("request present LOD");
    let LoadJob::Decode(lod_input) = store.next_job(now).expect("header prefix supplies the LOD") else {
        panic!("complete LOD in the header is decoded directly")
    };
    let lod_decode = store.start_decode(lod_input).expect("admit LOD decode");
    let lod_writes = store.complete(lod_decode.run(), now);
    for write in lod_writes {
        write.run();
    }
    drop(lod_demand);
    drop(header_demand);
    store.close();
    drop(store);

    let mut next_store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let next_route = next_store.register_route().expect("register next route");
    let next_demand = next_store.request(next_route, Request::MeshLod(present), Priority::Visible, now).expect("request cached LOD");
    let LoadJob::ReadCache(read) = next_store.next_job(now).expect("read present LOD") else { panic!("present LOD reads cache") };
    let Some(LoadJob::Decode(input)) = next_store.complete_cache_read(read.run(), now) else { panic!("present LOD cache hit") };
    let decode = next_store.start_decode(input).expect("admit cached LOD decode");
    assert!(next_store.complete(decode.run(), now).is_empty());
    assert_matches!(next_demand.status(), Status::Ready(Content::Mesh(mesh)) if mesh.lod() == MeshLod::Low);
    assert_eq!(cache.stats().entries_by_kind.meshes, 2, "only the header and present LOD have entries");
    assert!(asset.len() > block.len());
    drop(next_demand);
    next_store.close();
    drop(next_store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn same_mesh_block_shares_fetch() {
    let now = Instant::now();
    let mut store = Store::new(Budget::default());
    let route = store.register_route().expect("register route");
    let id = AssetId::new(Uuid::from_u128(128));
    let block = mesh_lod_request(id, &mesh_block_fixture());
    let first = store.request(route, Request::MeshLod(block), Priority::Visible, now).expect("request mesh LOD");
    let second = store.request(route, Request::MeshLod(block), Priority::Visible, now).expect("request same LOD");
    let fetch = next_fetch(&mut store, now);
    assert_eq!(fetch.request(), Request::MeshLod(block));
    assert!(store.next_job(now).is_none(), "the same block has only one running fetch");
    drop(first);
    drop(second);
    store.close();
}

#[test]
fn mesh_blocks_are_protected_independently() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let id = AssetId::new(Uuid::from_u128(129));
    let (low, high, block) = mesh_lod_requests(id);
    let low_record = mesh_lod_record_for(low, &block);
    let high_record = mesh_lod_record_for(high, &block);
    cache.put_complete(Request::MeshLod(low), low_record).expect("seed low LOD");
    let one_entry_limit = cache.accounted_size();
    cache.put_complete(Request::MeshLod(high), high_record).expect("seed high LOD");
    let uuid = id.uuid().to_string();
    let shard = uuid.get(..2).expect("UUID shard");
    assert!(directory.path().join(format!("assets/meshes/lod/low/{shard}/{uuid}.llmesh")).exists());
    assert!(directory.path().join(format!("assets/meshes/lod/high/{shard}/{uuid}.llmesh")).exists());
    let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let route = store.register_route().expect("register route");
    let demand = store.request(route, Request::MeshLod(high), Priority::Visible, Instant::now()).expect("protect high LOD");
    cache.set_size_limit(one_entry_limit).expect("evict one unprotected block");
    assert_eq!(cache.lookup(Request::MeshLod(low)), crate::cache::AssetLookup::Miss);
    assert_matches!(cache.lookup(Request::MeshLod(high)), crate::cache::AssetLookup::Hit(_));
    assert_eq!(cache.stats().entries_by_kind.meshes, 1);
    drop(demand);
    store.close();
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn mesh_cache_pressure_keeps_entry() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let id = AssetId::new(Uuid::from_u128(130));
    let block_bytes = mesh_block_fixture();
    let request = mesh_lod_request(id, &block_bytes);
    cache.put_complete(Request::MeshLod(request), mesh_lod_record_for(request, &block_bytes)).expect("seed validated LOD");
    let budget = Budget::default();
    let now = Instant::now();
    let mut store = Store::with_cache(budget.clone(), Arc::clone(&cache));
    let route = store.register_route().expect("register route");
    let demand = store.request(route, Request::MeshLod(request), Priority::Background, now).expect("request cached LOD");
    let LoadJob::ReadCache(read) = store.next_job(now).expect("cache lookup") else { panic!("warm LOD reads cache") };
    let Some(LoadJob::Decode(input)) = store.complete_cache_read(read.run(), now) else { panic!("LOD is a cache hit") };
    let pressure = budget.reserve(ASSET_WORK_BYTES.saturating_sub(budget.stats().charged_bytes)).expect("fill decode budget");
    let decode = store.start_decode(input).expect("admit cached LOD");
    let completion = decode.run();
    assert_eq!(completion.failure(), Some(Failure::MemoryPressure));
    assert!(store.complete(completion, now).is_empty());
    assert_matches!(demand.status(), Status::Queued);
    assert_eq!(store.next_deadline(now), Some(now + Duration::from_secs(1)));
    assert_eq!(cache.stats().corrupt_entries, 0);
    assert_matches!(cache.lookup(Request::MeshLod(request)), crate::cache::AssetLookup::Hit(_));
    let bytes_read = cache.stats().bytes_read;
    assert!(store.next_job(now).is_none());
    assert!(store.next_job(now + Duration::from_millis(500)).is_none());
    assert_eq!(cache.stats().bytes_read, bytes_read, "cache pressure delays another disk read");
    drop(pressure);
    let retry_at = now + Duration::from_secs(1);
    let LoadJob::ReadCache(read) = store.next_job(retry_at).expect("retry cache read after pressure delay") else {
        panic!("retry reads retained entry")
    };
    let Some(LoadJob::Decode(input)) = store.complete_cache_read(read.run(), retry_at) else { panic!("retained LOD is still a hit") };
    let decode = store.start_decode(input).expect("admit retry decode");
    assert!(store.complete(decode.run(), retry_at).is_empty());
    assert_matches!(demand.status(), Status::Ready(Content::Mesh(mesh)) if mesh.lod() == MeshLod::Low);
    drop(demand);
    store.close();
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn unavailable_mesh_cache_fetches() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let now = Instant::now();
    let id = AssetId::new(Uuid::from_u128(131));
    let block = mesh_block_fixture();
    let request = mesh_lod_request(id, &block);
    cache.put_complete(Request::MeshLod(request), mesh_lod_record_for(request, &block)).expect("seed indexed LOD");
    let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let route = store.register_route().expect("register route");
    let demand = store.request(route, Request::MeshLod(request), Priority::Visible, now).expect("request mesh LOD");
    let LoadJob::ReadCache(read) = store.next_job(now).expect("indexed cache read") else { panic!("indexed asset checks disk") };
    let completion = read.run_lookup(|_, _| crate::cache::AssetLookup::Unavailable(storage::UnavailableReason::Io));
    let Some(LoadJob::Fetch(fetch)) = store.complete_cache_read(completion, now) else { panic!("unavailable cache falls back") };
    assert_matches!(demand.status(), Status::Fetching);
    let failed = LoadJob::Fetch(fetch).fail(Failure::Transient { retry_after: Duration::ZERO });
    store.complete(failed, now);
    let retry_at = match demand.status() {
        Status::Failed { failure: Failure::Transient { .. }, retry_at: Some(retry_at) } => retry_at,
        status => panic!("network failure should retain the first-attempt retry delay: {status:?}"),
    };
    assert_eq!(retry_at, now + Duration::from_secs(1));
    let LoadJob::Fetch(fetch) = store.next_job(retry_at).expect("network retry follows cache failure") else {
        panic!("cache error is not retried in place of the network")
    };
    let input = fetch
        .complete_response(Ok(FetchResponse::Partial { bytes: block, range: request.range(), total_length: Some(request.asset_length()) }))
        .expect("attach block after cache error");
    let decode = store.start_decode(input).expect("admit LOD decode");
    let writes = store.complete(decode.run(), now);
    for write in writes {
        write.run();
    }
    assert_matches!(demand.status(), Status::Ready(Content::Mesh(mesh)) if mesh.lod() == MeshLod::Low);
    drop(demand);
    store.close();
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

fn legacy_fields(mode: i32, cutoff: i32) -> llsd::Map {
    let mut fields: llsd::Map = [
        ("NormMap".into(), llsd::Value::Uuid(Uuid::from_u128(7))),
        ("SpecMap".into(), llsd::Value::Uuid(Uuid::nil())),
        ("SpecColor".into(), llsd::Value::Array([51, 102, 153, 204].map(llsd::Value::Integer).into())),
    ]
    .into();
    for (name, value) in [
        ("NormOffsetX", 2500),
        ("NormOffsetY", -5000),
        ("NormRepeatX", 20000),
        ("NormRepeatY", 30000),
        ("NormRotation", 15000),
        ("SpecOffsetX", -1250),
        ("SpecOffsetY", 7500),
        ("SpecRepeatX", -10000),
        ("SpecRepeatY", 0),
        ("SpecRotation", -20000),
        ("SpecExp", 137),
        ("EnvIntensity", 73),
        ("DiffuseAlphaMode", mode),
        ("AlphaMaskCutoff", cutoff),
    ] {
        fields.insert(name.into(), llsd::Value::Integer(value));
    }
    fields
}

fn material_envelope(json: &str) -> Vec<u8> {
    let mut bytes = b"{\0\0\0\x03k\0\0\0\x07versions\0\0\0\x031.1k\0\0\0\x04types\0\0\0\x08GLTF 2.0k\0\0\0\x04datas".to_vec();
    bytes.extend((json.len() as u32).to_be_bytes());
    bytes.extend_from_slice(json.as_bytes());
    bytes.push(b'}');
    bytes
}

fn legacy_envelope(id: AssetId) -> Vec<u8> {
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;
    let record = binary::to_vec(&llsd::Value::Array(vec![llsd::Value::Map(
        [
            ("ID".into(), llsd::Value::Binary(llsd::Binary::new(id.uuid().as_bytes().to_vec()))),
            ("Material".into(), llsd::Value::Map(legacy_fields(2, 128))),
        ]
        .into(),
    )]))
    .unwrap();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&record).unwrap();
    let zipped = encoder.finish().unwrap();
    llsd::xml::to_vec(&llsd::Value::Map([("Zipped".into(), llsd::Value::Binary(llsd::Binary::new(zipped)))].into())).unwrap()
}

#[test]
fn visible_work_precedes_baselines() {
    let StoreHarness { now, mut store, route, .. } = StoreHarness::new();
    let baseline = store.request(route, Request::new(resource(1)), Priority::Background, now).unwrap();
    let later = now + Duration::from_secs(3600);
    let visible = store.request(route, Request::new(resource(2)), Priority::Visible, later).unwrap();
    let job = fetch(&mut store, later, RGBA_J2C.to_vec());
    let job = store.start_decode(job).expect("decode admission");
    assert_eq!(job.resource(), resource(2), "visible work takes priority over older regional prefetches");
    store.complete(job.run(), later);
    assert_eq!(ready(&visible).size(), [8; 2]);
    let job = fetch(&mut store, later, RGBA_J2C.to_vec());
    let job = store.start_decode(job).expect("decode admission");
    assert_eq!(job.resource(), resource(1), "background queue time does not consume the fetch deadline");
    assert_matches!(baseline.status(), Status::Decoding);
    store.maintain(later + Duration::from_secs(61));
    assert!(job.is_cancelled(), "running background work still has a bounded deadline");
    assert_matches!(baseline.status(), Status::Failed { failure: Failure::Deadline, .. });
}

#[test]
fn mesh_importance_precedes_age() {
    let now = Instant::now();
    let mut store = Store::new(Budget::default());
    let route = store.register_route().unwrap();
    let far = mesh_resource(801).id;
    let near = mesh_resource(802).id;
    let _far = store.request_with_importance(route, Request::MeshHeader(far), Priority::Visible, 64, now).expect("request far mesh header");
    let _near = store
        .request_with_importance(route, Request::MeshHeader(near), Priority::Visible, 96, now + Duration::from_secs(1))
        .expect("request near mesh header");

    assert_eq!(next_fetch(&mut store, now + Duration::from_secs(1)).request(), Request::MeshHeader(near));
}

#[test]
fn importance_update_keeps_fetch_and_reorders_queue() {
    let now = Instant::now();
    let mut store = Store::new(Budget::default());
    let route = store.register_route().unwrap();
    let running = store
        .request_with_importance(route, Request::MeshHeader(mesh_resource(803).id), Priority::Visible, 20, now)
        .expect("request running mesh");
    let lower = store
        .request_with_importance(route, Request::MeshHeader(mesh_resource(804).id), Priority::Visible, 10, now)
        .expect("request lower mesh");
    let middle = store
        .request_with_importance(route, Request::MeshHeader(mesh_resource(805).id), Priority::Visible, 15, now)
        .expect("request middle mesh");
    let running_fetch = next_fetch(&mut store, now);
    assert_eq!(running_fetch.request(), Request::MeshHeader(mesh_resource(803).id));

    lower.update_importance(25);
    store.maintain(now);
    assert!(!running_fetch.is_cancelled(), "importance changes do not cancel the running fetch");
    assert_eq!(next_fetch(&mut store, now).request(), Request::MeshHeader(mesh_resource(804).id));
    assert_eq!(next_fetch(&mut store, now).request(), Request::MeshHeader(mesh_resource(805).id));
    drop((running, lower, middle, running_fetch));
}

#[test]
fn mesh_fetch_pool_independent_of_textures() {
    let now = Instant::now();
    let mut store = Store::new(Budget::default());
    let route = store.register_route().unwrap();
    let mut demands = Vec::new();
    for index in 0..MAX_MESH_FETCHES + 8 {
        let id = mesh_resource(900 + u128::try_from(index).unwrap()).id;
        demands.push(store.request(route, Request::MeshHeader(id), Priority::Visible, now).unwrap());
    }
    for index in 0..MAX_NON_MESH_FETCHES + 4 {
        let id = resource(1_000 + u128::try_from(index).unwrap()).id;
        demands.push(store.request(route, Request::Texture { id, size: TextureSize::new(128) }, Priority::Visible, now).unwrap());
    }

    let mut mesh_fetches = 0;
    let mut other_fetches = 0;
    let mut jobs = Vec::new();
    while let Some(job) = store.next_job(now) {
        let LoadJob::Fetch(fetch) = &job else { panic!("network demands schedule fetches") };
        if fetch.resource().kind == Kind::Mesh {
            mesh_fetches += 1;
        } else {
            other_fetches += 1;
        }
        jobs.push(job);
    }

    assert_eq!(mesh_fetches, MAX_MESH_FETCHES);
    assert_eq!(other_fetches, MAX_NON_MESH_FETCHES);
    assert!(store.next_job(now).is_none());
}

#[test]
fn reservation_block_skips_to_small_work() {
    let budget = Budget::default();
    let now = Instant::now();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().expect("register route");
    let decoder_demand = store
        .request(route, Request::PbrMaterial(AssetId::new(Uuid::from_u128(1200))), Priority::Visible, now)
        .expect("request active decoder");
    let input = next_fetch(&mut store, now).complete(Ok(Vec::new())).expect("attach decoder response");
    let decoder_job = store.start_decode(input).expect("occupy non-texture decode slot");
    let blocker_bytes = ASSET_WORK_BYTES.saturating_sub(budget.stats().charged_bytes).saturating_sub(4 * 1024 * 1024);
    let blocker = budget.reserve(blocker_bytes).expect("leave only a small reservation window");

    let block = large_mesh_lod_request(AssetId::new(Uuid::from_u128(1201)), 8 * 1024 * 1024);
    let large = store.request_with_importance(route, Request::MeshLod(block), Priority::Visible, 100, now).expect("request large mesh LOD");
    let material_id = AssetId::new(Uuid::from_u128(1202));
    let small =
        store.request_with_importance(route, Request::PbrMaterial(material_id), Priority::Visible, 1, now).expect("request small material");

    let LoadJob::Fetch(fetch) = store.next_job(now).expect("small admissible work follows blocked mesh") else {
        panic!("material starts as a network fetch")
    };
    assert_eq!(fetch.request(), Request::PbrMaterial(material_id));
    assert_matches!(large.status(), Status::Queued);

    store.complete(LoadJob::Fetch(fetch).fail(Failure::Cancelled), now);
    drop(blocker);
    store.complete(decoder_job.run(), now);
    drop((decoder_demand, small, large));
    store.maintain(now);
    store.close();
    drop(store);
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[test]
fn shared_detail_refines_and_releases() {
    let budget = Budget::default();
    let StoreHarness { now, mut store, route, .. } = StoreHarness::with_budget(budget.clone());
    let id = resource(1).id;
    let low = store.request(route, Request::Texture { id, size: TextureSize::new(64) }, Priority::Visible, now).unwrap();
    let bytes = GRAY_4096_J2C;
    let job = fetch(&mut store, now, bytes.to_vec());
    let job = store.start_decode(job).expect("decode admission");
    let result = job.run();
    store.complete(result, now);
    let retained = ready(&low);
    assert_eq!((retained.source_size(), retained.size(), retained.discard_level()), ([4096; 2], [64; 2], 6));
    assert_eq!(budget.stats().resident_bytes, bytes.len() + 64 * 64);
    let later = now + Duration::from_secs(3600);
    let high = store.request(route, Request::Texture { id, size: TextureSize::new(512) }, Priority::Visible, later).unwrap();
    let clone = high.clone();
    let job = cached(&mut store, later);
    assert_eq!(
        job.resource(),
        Resource { id, kind: Kind::Texture },
        "refinement reuses the encoded asset after the original request deadline"
    );
    let job = store.start_decode(job).expect("decode admission");
    let Status::Refining(Content::Texture(old)) = low.status() else { panic!("expected usable fallback") };
    assert_eq!(old.info().unwrap().size, [64; 2]);
    drop(high);
    store.maintain(later);
    assert!(!job.is_cancelled(), "cloned demand keeps the higher target alive");
    store.complete(job.run(), later);
    assert_eq!((ready(&low).size(), ready(&low).discard_level()), ([512; 2], 3));
    assert!(store.next_job(later).is_none());
    drop(clone);
    let job = cached(&mut store, later);
    assert_eq!(job.resource(), Resource { id, kind: Kind::Texture });
    let job = store.start_decode(job).expect("decode admission");
    let Status::Refining(Content::Texture(current)) = low.status() else { panic!("expected reducing detail with fallback") };
    assert_eq!(current.info().unwrap().size, [512; 2], "old quality stays usable while reducing residency");
    drop(current);
    store.complete(job.run(), later);
    assert_eq!(ready(&low).size(), [64; 2]);
    assert!(store.next_job(later).is_none());
    store.close();
    drop(low);
    drop(old);
    assert_eq!(budget.stats().resident_bytes, retained.pixels().len());
    drop(retained);
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[test]
fn failed_refinement_keeps_image() {
    let StoreHarness { now, mut store, route, .. } = StoreHarness::new();
    let id = resource(1).id;
    let low = store.request(route, Request::Texture { id, size: TextureSize::new(32) }, Priority::Visible, now).unwrap();
    let job = fetch(&mut store, now, GRAY_4096_J2C.to_vec());
    let job = store.start_decode(job).expect("decode admission");
    store.complete(job.run(), now);
    let high = store.request(route, Request::Texture { id, size: TextureSize::new(256) }, Priority::Visible, now).unwrap();
    let input = cached(&mut store, now);
    store.complete(LoadJob::Decode(input).fail(Failure::Unsupported), now);
    let Status::RefinementFailed { content: Content::Texture(image), failure } = high.status() else {
        panic!("expected failure with fallback")
    };
    assert_eq!(failure, Failure::Unsupported);
    assert_eq!(image.info().unwrap().size, [32; 2]);
    assert!(store.next_job(now).is_none());
    drop(high);
    assert!(store.next_job(now).is_none(), "the remaining low-detail demand is already satisfied");
    assert_eq!(ready(&low).size(), [32; 2]);
}

#[test]
fn obsolete_detail_keeps_current_image() {
    let StoreHarness { now, mut store, route, .. } = StoreHarness::new();
    let id = resource(1).id;
    let high_request = Request::Texture { id, size: TextureSize::new(512) };
    let low = store.request(route, Request::Texture { id, size: TextureSize::new(32) }, Priority::Visible, now).unwrap();
    let high = store.request(route, high_request, Priority::Visible, now).unwrap();
    let job = fetch(&mut store, now, GRAY_4096_J2C.to_vec());
    let job = store.start_decode(job).expect("decode admission");
    store.complete(job.run(), now);
    assert_eq!(ready(&low).size(), [512; 2]);
    drop(high);
    let reducing = cached(&mut store, now);
    let reducing = store.start_decode(reducing).expect("decode admission");
    let _high = store.request(route, high_request, Priority::Visible, now).unwrap();
    store.complete(reducing.run(), now);
    assert_eq!(ready(&low).size(), [512; 2], "obsolete reduction cannot replace the still-needed detail");
    assert!(store.next_job(now).is_none());
}

#[test]
fn shared_demand_controls_cancellation() {
    let budget = Budget::default();
    let StoreHarness { now, mut store, route, .. } = StoreHarness::with_budget(budget.clone());
    let first = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let second = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let clone = first.clone();
    let job = fetch(&mut store, now, RGBA_J2C.to_vec());
    let job = store.start_decode(job).expect("decode admission");
    assert!(store.next_job(now).is_none());
    drop(first);
    drop(clone);
    store.maintain(now);
    assert!(!job.is_cancelled(), "one remaining face still needs the shared fetch");
    drop(second);
    store.maintain(now);
    assert!(job.is_cancelled());
    assert_eq!(budget.stats().charged_bytes, TEXTURE_WORK_BYTES, "cancellation cannot release a running job's reservation");
    store.complete(job.run(), now);
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[test]
fn route_failures_share_success() {
    let StoreHarness { now, mut store, route: first, .. } = StoreHarness::new();
    let a = store.request(first, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let completion =
        next_fetch(&mut store, now).complete(Err(Failure::MissingCapability)).expect_err("fetch failure completes without decode input");
    store.complete(completion.into(), now);
    assert_matches!(a.status(), Status::Failed { failure: Failure::MissingCapability, retry_at: None });
    assert!(store.next_job(now + Duration::from_secs(61)).is_none());
    let second = store.register_route().unwrap();
    let b = store.request(second, Request::new(resource(1)), Priority::Visible, now + Duration::from_secs(61)).unwrap();
    let job = next_fetch(&mut store, now + Duration::from_secs(61));
    assert_eq!(job.route(), second);
    let input = job.complete(Ok(RGBA_J2C.to_vec())).expect("valid response enters decode");
    let job = store.start_decode(input).expect("decode admission");
    store.complete(job.run(), now + Duration::from_secs(61));
    assert_eq!(ready(&a).pixels(), RGBA_RAW);
    assert!(std::ptr::eq(ready(&a).pixels().as_ptr(), ready(&b).pixels().as_ptr()));
    let cached = store.request(first, Request::new(resource(1)), Priority::Visible, now + Duration::from_secs(62)).unwrap();
    assert!(std::ptr::eq(ready(&a).pixels().as_ptr(), ready(&cached).pixels().as_ptr()));
    assert!(store.next_job(now + Duration::from_secs(62)).is_none());
}

#[test]
fn retry_limits_and_server_delays() {
    let StoreHarness { mut now, mut store, route, .. } = StoreHarness::new();
    let demand = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    for attempt in 1..=3 {
        let completion = next_fetch(&mut store, now)
            .complete(Err(Failure::Transient { retry_after: Duration::ZERO }))
            .expect_err("transient fetch failure completes without decode input");
        store.complete(completion.into(), now);
        let Status::Failed { retry_at, .. } = demand.status() else { panic!("expected classified retry failure") };
        if attempt == 3 {
            assert!(retry_at.is_none());
            break;
        }
        assert!(store.next_job(now).is_none(), "retry backoff is not shortened");
        now = retry_at.unwrap();
    }
    assert!(store.next_job(now + Duration::from_secs(3600)).is_none());
    assert_eq!(store.next_deadline(now + Duration::from_secs(3600)), None);
    let demand = store.request(route, Request::new(resource(2)), Priority::Visible, now).unwrap();
    let completion = next_fetch(&mut store, now)
        .complete(Err(Failure::Transient { retry_after: Duration::from_secs(5) }))
        .expect_err("transient fetch failure completes without decode input");
    store.complete(completion.into(), now);
    let Status::Failed { retry_at: Some(retry_at), .. } = demand.status() else { panic!("server retry delay is retained") };
    assert_eq!(retry_at, now + Duration::from_secs(5));
    assert!(store.next_job(now + Duration::from_secs(4)).is_none(), "server retry delay is not shortened");
    let retry_job = next_fetch(&mut store, retry_at);
    let completion = retry_job.complete(Err(Failure::NotFound)).expect_err("fetch failure completes without decode input");
    store.complete(completion.into(), retry_at);
    let demand = store.request(route, Request::new(resource(3)), Priority::Visible, retry_at).unwrap();
    let completion = next_fetch(&mut store, retry_at)
        .complete(Err(Failure::Transient { retry_after: Duration::from_secs(120) }))
        .expect_err("transient fetch failure completes without decode input");
    store.complete(completion.into(), retry_at);
    assert_matches!(demand.status(), Status::Failed { retry_at: None, .. });
    assert!(store.next_job(retry_at + Duration::from_secs(30)).is_none(), "an excessive server delay is not retried");
}

#[test]
fn negative_cache_expires() {
    let StoreHarness { now, mut store, route } = StoreHarness::new();
    let first = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let completion =
        next_fetch(&mut store, now).complete(Err(Failure::NotFound)).expect_err("fetch failure completes without decode input");
    store.complete(completion.into(), now);
    assert_matches!(first.status(), Status::Failed { failure: Failure::NotFound, retry_at: None });
    drop(first);

    let before_expiry = now + Duration::from_secs(29);
    let waiting = store.request(route, Request::new(resource(1)), Priority::Visible, before_expiry).unwrap();
    assert_matches!(
        waiting.status(),
        Status::Failed { failure: Failure::NotFound, retry_at: None },
        "negative result is cached before its expiry"
    );
    assert!(store.next_job(before_expiry).is_none(), "negative result remains cached before expiry");
    drop(waiting);

    let after_expiry = now + Duration::from_secs(31);
    let retry = store.request(route, Request::new(resource(1)), Priority::Visible, after_expiry).unwrap();
    let input = next_fetch(&mut store, after_expiry).complete(Ok(RGBA_J2C.to_vec())).unwrap();
    let job = store.start_decode(input).expect("expired negative result admits a new fetch");
    store.complete(job.run(), after_expiry);
    assert_eq!(ready(&retry).pixels(), RGBA_RAW);
}

#[test]
fn retained_pixels_allow_work() {
    let budget = Budget::default();
    let StoreHarness { now, mut store, route, .. } = StoreHarness::with_budget(budget.clone());
    let demand = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let job = fetch(&mut store, now, RGBA_J2C.to_vec());
    let job = store.start_decode(job).expect("decode admission");
    store.complete(job.run(), now);
    let retained = ready(&demand);
    drop(demand);
    store.close();
    assert_eq!(budget.stats().resident_bytes, retained.pixels().len());
    let work = budget.reserve(ASSET_WORK_BYTES).expect("retained asset data does not consume scratch admission");
    assert!(budget.reserve(1).is_none(), "in-flight work still has a hard bound");
    assert_eq!(budget.stats().charged_bytes, ASSET_WORK_BYTES + retained.pixels().len());
    drop(work);
    assert_eq!(budget.stats().charged_bytes, retained.pixels().len());
    drop(retained);
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[tokio::test]
async fn retired_routes_reject_late_results() {
    let budget = Budget::default();
    let StoreHarness { now, mut store, route, .. } = StoreHarness::with_budget(budget.clone());
    let mut demand = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let job = fetch(&mut store, now, RGBA_J2C.to_vec());
    let job = store.start_decode(job).expect("decode admission");
    let result = job.run();
    store.retire_route(route);
    assert_matches!(demand.status(), Status::Failed { failure: Failure::RouteClosed, .. });
    for _ in 0..2 {
        assert_matches!(
            demand.changed().await,
            Status::Failed { failure: Failure::RouteClosed, retry_at: None },
            "closing one route must not report that the whole asset session ended"
        );
    }
    store.complete(result, now);
    assert_eq!(budget.stats().charged_bytes, 0);
    let replacement = store.register_route().unwrap();
    let waiting = store.request(replacement, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let deadline = store.next_deadline(now).expect("live demand has a deadline");
    store.maintain(deadline);
    assert_matches!(waiting.status(), Status::Failed { failure: Failure::Deadline, retry_at: None });
    assert!(store.next_job(deadline).is_none());
    assert_eq!(store.next_deadline(deadline), None);
}

#[test]
fn route_failures_preserve_status() {
    let StoreHarness { now, mut store, route: first, .. } = StoreHarness::new();
    let second = store.register_route().unwrap();
    let a = store.request(first, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let fetch = next_fetch(&mut store, now);
    store.complete(LoadJob::Fetch(fetch).fail(Failure::MissingCapability), now);
    let b = store.request(second, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let fetch = next_fetch(&mut store, now);
    store.complete(LoadJob::Fetch(fetch).fail(Failure::NotFound), now);
    assert_matches!(a.status(), Status::Failed { failure: Failure::MissingCapability, retry_at: None });
    assert_matches!(b.status(), Status::Failed { failure: Failure::NotFound, retry_at: None });
    assert!(store.next_job(now).is_none());
}

#[test]
fn shutdown_retains_running_work() {
    let budget = Budget::default();
    let StoreHarness { now, mut store, route, .. } = StoreHarness::with_budget(budget.clone());
    let demand = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let job = fetch(&mut store, now, RGBA_J2C.to_vec());
    let job = store.start_decode(job).expect("decode admission");
    assert_matches!(demand.status(), Status::Decoding);
    let result = job.run();
    store.close();
    assert_eq!(budget.stats().charged_bytes, TEXTURE_WORK_BYTES);
    store.complete(result, now);
    assert_eq!(budget.stats().charged_bytes, 0);
    assert_matches!(demand.status(), Status::StaleSession);
}

#[test]
fn demand_has_no_cache_cap() {
    let StoreHarness { now, mut store, route, .. } = StoreHarness::new();
    let demands: Vec<_> = (1..=4097).map(|id| store.request(route, Request::new(resource(id)), Priority::Visible, now).unwrap()).collect();
    assert_eq!(demands.len(), 4097, "live handles are not rejected by the former 4096-entry cache limit");
    drop(demands);
    store.maintain(now);
    assert!(store.next_job(now).is_none());
    let demand = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let job = fetch(&mut store, now, RGBA_J2C.to_vec());
    let job = store.start_decode(job).expect("decode admission");
    store.complete(job.run(), now);
    assert_eq!(ready(&demand).size(), [8; 2]);
}

#[test]
fn foreign_decode_preserves_owner() {
    let budget = Budget::default();
    let StoreHarness { now, store: mut owner, route, .. } = StoreHarness::with_budget(budget.clone());
    let demand = owner.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let input = fetch(&mut owner, now, RGBA_J2C.to_vec());
    let mut other = Store::new(Budget::default());
    let other_route = other.register_route().unwrap();
    let other_demand = other.request(other_route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let other_job = other.next_job(now).unwrap();
    let before = budget.stats().charged_bytes;
    let input = other.start_decode(input).expect_err("another store cannot admit the owner's input");
    assert_eq!(budget.stats().charged_bytes, before, "rejected admission preserves the original reservation");
    assert_eq!(input.resource(), resource(1));
    assert_matches!(demand.status(), Status::Fetching);
    assert_matches!(other_demand.status(), Status::Fetching);
    let job = owner.start_decode(input).expect("the returned input remains eligible for its owner");
    owner.complete(job.run(), now);
    assert_eq!(ready(&demand).pixels(), RGBA_RAW);
    assert_matches!(other_demand.status(), Status::Fetching);
    drop(demand);
    drop(owner);
    drop(other_job);
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[test]
fn fetches_overlap_decodes() {
    let budget = Budget::default();
    let StoreHarness { now, mut store, route, .. } = StoreHarness::with_budget(budget.clone());
    let demands: Vec<_> = (1..=13)
        .map(|id| {
            let mut resource = resource(id);
            if id > 2 {
                resource.kind = Kind::PbrMaterial;
            }
            store.request(route, Request::new(resource), Priority::Visible, now).unwrap()
        })
        .collect();
    let material_json = r#"{"asset":{"version":"2.0"},"materials":[{"pbrMetallicRoughness":{"baseColorFactor":[0.2,0.3,0.4,0.5]}}]}"#;
    let mut decoding = Vec::new();
    for _ in 0..MAX_TEXTURE_DECODES {
        let input = fetch(&mut store, now, RGBA_J2C.to_vec());
        let job = store.start_decode(input).expect("decode admission");
        decoding.push(job);
    }
    let mut material_decodes = Vec::new();
    let mut fetched = Vec::<DecodeInput>::new();
    for _ in 0..MAX_NON_MESH_FETCHES {
        let bytes = material_envelope(material_json);
        let input = next_fetch(&mut store, now).complete(Ok(bytes)).expect("valid material response is attached to its fetch");
        match store.start_decode(input) {
            Ok(job) => material_decodes.push(job),
            Err(input) => fetched.push(input),
        }
    }
    assert_eq!(material_decodes.len(), MAX_NON_TEXTURE_DECODES, "non-texture work has its own bounded decode slots");
    for _ in 0..MAX_TEXTURE_DECODES {
        let input = next_fetch(&mut store, now)
            .complete(Ok(material_envelope(material_json)))
            .expect("another material fetch can overlap active decodes");
        fetched.push(store.start_decode(input).expect_err("full decode pool leaves fetched data queued"));
    }
    assert!(store.next_job(now).is_none(), "fetched responses retain fetch admission until decoding starts");
    let waiting = fetched.remove(0);
    let job = material_decodes.remove(0);
    let result = job.run();
    store.maintain(now);
    let waiting = store.start_decode(waiting).expect_err("other decoders retain their pool until completion delivery");
    store.complete(result, now);
    let job = store.start_decode(waiting).expect("a delivered completion admits waiting data");
    assert_eq!(job.resource(), Resource { id: resource(5).id, kind: Kind::PbrMaterial });
    store.complete(job.run(), now);
    let Status::Ready(Content::Material(content)) = demands[2].status() else { panic!("first material publishes decoded content") };
    let MaterialDescription::Pbr(material) = &*content else { panic!("waiting response decodes to PBR fields") };
    assert_eq!(material.base_color_factor, [0.2, 0.3, 0.4, 0.5]);
    drop(content);
    assert!(store.next_job(now).is_some(), "starting a decoder releases one fetch slot");
    assert_matches!(demands[0].status(), Status::Decoding);
    assert_matches!(demands[1].status(), Status::Decoding);
    assert_matches!(demands[3].status(), Status::Decoding);
    store.close();
    drop(decoding);
    drop(material_decodes);
    drop(fetched);
    drop(demands);
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[test]
fn waiting_data_cancels_cleanly() {
    let budget = Budget::default();
    let StoreHarness { now, mut store, route, .. } = StoreHarness::with_budget(budget.clone());
    let material = Resource { id: resource(1).id, kind: Kind::PbrMaterial };
    let demand = store.request(route, Request::new(material), Priority::Background, now).unwrap();
    let bytes = Vec::with_capacity(1024);
    let capacity = bytes.capacity();
    let input = next_fetch(&mut store, now).complete(Ok(bytes)).expect("small response remains queued");
    assert_eq!(store.priority(&input), Priority::Background);
    let visible = store.request(route, Request::new(material), Priority::Visible, now).unwrap();
    assert_eq!(store.priority(&input), Priority::Visible, "waiting work uses current demand, not fetch-time priority");
    drop(visible);
    assert_eq!(store.priority(&input), Priority::Background);
    assert_eq!(budget.stats().charged_bytes, capacity);
    assert_matches!(demand.status(), Status::Fetching);
    drop(demand);
    store.maintain(now);
    assert!(input.is_cancelled(), "dropping the last demand cancels queued data");
    let input = store.start_decode(input).expect_err("cancelled work cannot enter decoding");
    assert_eq!(budget.stats().charged_bytes, capacity, "cancellation preserves queued ownership until delivery");
    store.complete(input.fail(Failure::Cancelled), now);
    assert_eq!(budget.stats().charged_bytes, 0);
    let next =
        store.request(route, Request::new(Resource { id: resource(2).id, kind: Kind::PbrMaterial }), Priority::Visible, now).unwrap();
    let completion = next_fetch(&mut store, now)
        .complete(Ok(Vec::with_capacity(Kind::PbrMaterial.max_response_bytes() + 1)))
        .expect_err("actual response capacity is checked before decoding");
    assert_eq!(completion.failure(), Failure::TooLarge);
    store.complete(completion.into(), now);
    assert_matches!(next.status(), Status::Failed { failure: Failure::TooLarge, .. });
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[test]
fn demand_priority_updates_in_place() {
    let StoreHarness { now, mut store, route, .. } = StoreHarness::with_budget(Budget::default());
    let request = Request::new(Resource { id: resource(901).id, kind: Kind::PbrMaterial });
    let demand = store.request(route, request, Priority::Background, now).unwrap();
    let input = next_fetch(&mut store, now).complete(Ok(Vec::with_capacity(1024))).expect("small response remains queued");
    assert_eq!(store.priority(&input), Priority::Background);
    demand.update_priority(Priority::Visible);
    assert_eq!(store.priority(&input), Priority::Visible);
    demand.update_priority(Priority::Background);
    assert_eq!(store.priority(&input), Priority::Background);
    assert!(!input.is_cancelled());
}

#[test]
fn dropped_jobs_wake_sessions() {
    let budget = Budget::default();
    let StoreHarness { now, mut store, route, .. } = StoreHarness::with_budget(budget.clone());
    let next = Store::new(budget.clone());
    let mut first_wake = store.wake();
    let mut second_wake = next.wake();
    let demand = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let input = fetch(&mut store, now, RGBA_J2C.to_vec());
    let job = store.start_decode(input).expect("decode admission");
    assert_eq!(store.running_texture_decodes + store.running_other_decodes, 1);
    first_wake.borrow_and_update();
    second_wake.borrow_and_update();
    drop(job);
    assert!(first_wake.has_changed().unwrap());
    assert!(second_wake.has_changed().unwrap(), "reconnect and retiring owners must not steal each other's wakeups");
    store.maintain(now);
    assert_eq!(store.running_texture_decodes + store.running_other_decodes, 0);
    assert_matches!(demand.status(), Status::Failed { failure: Failure::Cancelled, retry_at: None });
    assert!(store.next_job(now).is_none());
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[test]
fn reacquisition_waits_for_cancelled_job() {
    let budget = Budget::default();
    let StoreHarness { now, mut store, route, .. } = StoreHarness::with_budget(budget.clone());
    let first = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let job = next_fetch(&mut store, now);
    assert_eq!(store.running_mesh_fetches + store.running_other_fetches, 1);
    drop(first);
    store.maintain(now);
    assert_eq!(store.running_mesh_fetches + store.running_other_fetches, 1);
    assert!(job.is_cancelled());
    let replacement = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    assert!(store.next_job(now).is_none(), "reacquisition must not duplicate a resource's in-flight work");
    let completion = job.complete(Err(Failure::NotFound)).expect_err("cancellation wins over a late fetch failure");
    assert_eq!(completion.failure(), Failure::Cancelled);
    store.complete(completion.into(), now);
    assert_eq!(store.running_mesh_fetches + store.running_other_fetches, 0);
    let job = store.next_job(now).expect("the replacement starts after cancelled work completes");
    let job = match job {
        LoadJob::Fetch(job) => job.complete(Ok(RGBA_J2C.to_vec())).expect("response attachment"),
        LoadJob::Decode(input) => input,
        LoadJob::ReadCache(_) => panic!("uncached Store cannot schedule a disk read"),
    };
    let job = store.start_decode(job).expect("decode admission");
    store.complete(job.run(), now);
    assert_eq!(ready(&replacement).size(), [8; 2]);
    drop(replacement);
    store.maintain(now);
    assert_eq!(budget.stats().charged_bytes, 0);
}

proptest::proptest! {
    #[test]
    fn demand_churn_preserves_lifetimes(
        operations in proptest::collection::vec((0u8..5, 0usize..16), 0..120)
    ) {
        let budget = Budget::default();
        let StoreHarness { now, mut store, route, .. } = StoreHarness::with_budget(budget.clone());
        let mut leases: Vec<Option<(Resource, Demand)>> = (0..16).map(|_| None).collect();
        let mut jobs = Vec::<DecodeInput>::new();
        for (operation, slot) in operations {
            match operation {
                0 if leases[slot].is_none() => {
                    let resource = resource((slot % 8 + 1) as u128);
                    leases[slot] = Some((resource, store.request(route, Request::new(resource), Priority::Visible, now).unwrap()));
                }
                1 => { leases[slot] = None; }
                2 => {
                    if let Some(job) = store.next_job(now) {
                        let input = match job {
                            LoadJob::Fetch(job) => job.complete(Ok(RGBA_J2C.to_vec())).unwrap(),
                            LoadJob::Decode(input) => input,
                            LoadJob::ReadCache(_) => panic!("uncached Store cannot schedule a disk read"),
                        };
                        proptest::prop_assert!(!jobs.iter().any(|active| active.resource() == input.resource()));
                        proptest::prop_assert!(leases.iter().flatten().any(|(resource, _)| *resource == input.resource()));
                        jobs.push(input);
                        proptest::prop_assert!(jobs.len() <= MAX_NON_MESH_FETCHES);
                    }
                }
                3 => {
                    if let Some(input) = jobs.pop() {
                        if input.is_cancelled() {
                            store.complete(input.fail(Failure::Cancelled), now);
                        } else {
                            match store.start_decode(input) {
                                Ok(job) => {
                                    store.complete(job.run(), now);
                                }
                                Err(input) => jobs.push(input),
                            }
                        }
                    }
                }
                4 => { drop(jobs.pop()); }
                _ => {}
            }
            store.maintain(now);
            let fetching = store.entries.values().filter(|entry| matches!(entry.phase, LoadPhase::Running { stage: Stage::Fetching, .. })).count();
            let decoding = store.entries.values().filter(|entry| matches!(entry.phase, LoadPhase::Running { stage: Stage::Decoding, .. })).count();
            proptest::prop_assert_eq!(store.running_mesh_fetches + store.running_other_fetches, fetching);
            let decoding_textures = store.entries.values().filter(|entry| {
                entry.request.resource().kind == Kind::Texture && matches!(entry.phase, LoadPhase::Running { stage: Stage::Decoding, .. })
            }).count();
            let decoding_other = store.entries.values().filter(|entry| {
                entry.request.resource().kind != Kind::Texture && matches!(entry.phase, LoadPhase::Running { stage: Stage::Decoding, .. })
            }).count();
            proptest::prop_assert_eq!(store.running_texture_decodes, decoding_textures);
            proptest::prop_assert_eq!(store.running_other_decodes, decoding_other);
            proptest::prop_assert_eq!(store.running_texture_decodes + store.running_other_decodes, decoding);
            for job in &jobs {
                if !leases.iter().flatten().any(|(resource, _)| *resource == job.resource()) {
                    proptest::prop_assert!(job.is_cancelled());
                }
            }
            let stats = budget.stats();
            proptest::prop_assert!(stats.charged_bytes - stats.resident_bytes <= ASSET_WORK_BYTES);
            proptest::prop_assert!(stats.resident_bytes <= stats.charged_bytes);
        }
        drop(leases);
        store.close();
        proptest::prop_assert_eq!(store.running_mesh_fetches + store.running_other_fetches, 0);
        proptest::prop_assert_eq!(store.running_texture_decodes + store.running_other_decodes, 0);
        drop(jobs);
        proptest::prop_assert_eq!(budget.stats().charged_bytes, 0);
    }
}

#[test]
fn shared_material_retains_charge() {
    let budget = Budget::default();
    let StoreHarness { now, mut store, route, .. } = StoreHarness::with_budget(budget.clone());
    let resource = Resource { id: AssetId::new(Uuid::from_u128(2)), kind: Kind::PbrMaterial };
    let first = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let second = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let input = fetch(&mut store, now, material_envelope(PBR_JSON));
    let job = store.start_decode(input).expect("decode admission");
    store.complete(job.run(), now);
    let Status::Ready(Content::Material(material)) = first.status() else { panic!("expected material") };
    let Status::Ready(Content::Material(shared)) = second.status() else { panic!("expected shared material") };
    assert!(std::ptr::eq(&*material, &*shared));
    assert_eq!(budget.stats().resident_bytes, size_of::<MaterialDescription>());
    store.close();
    assert_matches!(second.status(), Status::StaleSession);
    assert_eq!(*material, MaterialDescription::Pbr(PbrMaterial::default()));
    drop(material);
    assert_eq!(budget.stats().charged_bytes, size_of::<MaterialDescription>());
    drop(shared);
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[test]
fn retired_material_cannot_publish() {
    let budget = Budget::default();
    let StoreHarness { now, mut store, route, .. } = StoreHarness::with_budget(budget.clone());
    let resource = Resource { id: AssetId::new(Uuid::from_u128(3)), kind: Kind::LegacyMaterial };
    let old = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let input = fetch(&mut store, now, legacy_envelope(resource.id));
    let job = store.start_decode(input).expect("decode admission");
    assert_eq!(store.running_texture_decodes + store.running_other_decodes, 1);
    let prepared = job.run();
    store.retire_route(route);
    store.complete(prepared, now);
    assert_eq!(store.running_texture_decodes + store.running_other_decodes, 0);
    assert_matches!(old.status(), Status::Failed { failure: Failure::RouteClosed, .. });
    assert_eq!(budget.stats().charged_bytes, 0);
    let route = store.register_route().unwrap();
    let current = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let input = fetch(&mut store, now, legacy_envelope(resource.id));
    let job = store.start_decode(input).expect("decode admission");
    let result = job.run();
    store.complete(result, now);
    let Status::Ready(Content::Material(material)) = current.status() else { panic!("expected legacy material") };
    assert_eq!(*material, crate::material::legacy::decode(&binary::to_vec(&llsd::Value::Map(legacy_fields(2, 128))).unwrap()).unwrap());
}

#[test]
fn mesh_decodes_share_budget() {
    let budget = Budget::default();
    let now = Instant::now();

    let mut texture_store = Store::new(budget.clone());
    let texture_route = texture_store.register_route().unwrap();
    let texture_demand = texture_store.request(texture_route, Request::new(resource(301)), Priority::Visible, now).unwrap();
    let texture_input = fetch(&mut texture_store, now, RGBA_J2C.to_vec());
    let texture_decode = texture_store.start_decode(texture_input).unwrap();

    let mut material_store = Store::new(budget.clone());
    let material_route = material_store.register_route().unwrap();
    let material_id = AssetId::new(Uuid::from_u128(302));
    let material_demand = material_store.request(material_route, Request::PbrMaterial(material_id), Priority::Visible, now).unwrap();
    let material_input = fetch(&mut material_store, now, material_envelope(PBR_JSON));
    let material_decode = material_store.start_decode(material_input).unwrap();
    assert_matches!(texture_demand.status(), Status::Decoding);
    assert_matches!(material_demand.status(), Status::Decoding);

    let mut stores = vec![texture_store, material_store];
    let mut demands = vec![texture_demand, material_demand];
    let mut jobs = vec![texture_decode, material_decode];
    for id in 303..306 {
        let (store, demand, job) = mesh_decode_job(budget.clone(), now, AssetId::new(Uuid::from_u128(id)), mesh_block_fixture());
        stores.push(store);
        demands.push(demand);
        jobs.push(job);
    }
    let (store, demand, job) = mesh_decode_job(
        budget.clone(),
        now,
        AssetId::new(Uuid::from_u128(306)),
        large_mesh_block_fixture(crate::mesh::MAX_MESH_VERTICES_PER_FACE, 8),
    );
    stores.push(store);
    demands.push(demand);
    jobs.push(job);

    assert!(budget.stats().charged_bytes < ASSET_WORK_BYTES);
    let barrier = Arc::new(Barrier::new(jobs.len() + 1));
    let completions = thread::scope(|scope| {
        let handles = jobs
            .into_iter()
            .map(|job| {
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    job.run()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        handles.into_iter().map(|handle| handle.join().unwrap()).collect::<Vec<_>>()
    });
    for ((mut store, demand), completion) in stores.into_iter().zip(demands).zip(completions) {
        store.complete(completion, now);
        assert_matches!(demand.status(), Status::Ready(_));
    }
}

#[test]
fn mesh_decode_bypasses_texture_pool() {
    let budget = Budget::default();
    let now = Instant::now();
    let mut store = Store::new(budget);
    let route = store.register_route().expect("register route");
    let mut texture_demands = Vec::new();
    let mut texture_decodes = Vec::new();
    for id in 401..401 + MAX_TEXTURE_DECODES as u128 {
        texture_demands.push(store.request(route, Request::new(resource(id)), Priority::Visible, now).expect("request texture"));
        let input = fetch(&mut store, now, RGBA_J2C.to_vec());
        texture_decodes.push(store.start_decode(input).expect("admit texture decode"));
    }

    let mesh_id = AssetId::new(Uuid::from_u128(403));
    let mesh_demand = store.request(route, Request::MeshHeader(mesh_id), Priority::Visible, now).expect("request mesh header");
    let block = mesh_block_fixture();
    let (_, asset) = mesh_full_asset_fixture(mesh_id, &block);
    let mesh_input = fetch(&mut store, now, asset);
    let mesh_decode = store.start_decode(mesh_input).expect("texture decoders do not occupy mesh decode slots");
    assert_matches!(mesh_demand.status(), Status::Decoding);
    assert!(texture_demands.iter().all(|demand| matches!(demand.status(), Status::Decoding)));

    store.close();
    drop(mesh_decode);
    drop(texture_decodes);
    drop(mesh_demand);
    drop(texture_demands);
    assert_eq!(store.running_texture_decodes + store.running_other_decodes, 0);
}

#[test]
fn failed_mesh_lod_keeps_another_lod_ready() {
    let id = mesh_resource(101).id;
    let (low, high, block) = mesh_lod_requests(id);
    let now = Instant::now();
    let mut store = Store::new(Budget::default());
    let route = store.register_route().unwrap();
    let low_demand = store.request(route, Request::MeshLod(low), Priority::Visible, now).unwrap();
    let high_demand = store.request(route, Request::MeshLod(high), Priority::Visible, now).unwrap();

    let fetch = next_fetch(&mut store, now);
    assert_eq!(fetch.request(), Request::MeshLod(low));
    let input = fetch
        .complete_response(Ok(FetchResponse::Partial { bytes: block.clone(), range: low.range(), total_length: Some(low.asset_length()) }))
        .unwrap();
    let decode = store.start_decode(input).unwrap();
    store.complete(decode.run(), now);
    assert_matches!(low_demand.status(), Status::Ready(Content::Mesh(mesh)) if mesh.lod() == MeshLod::Low);

    let fetch = next_fetch(&mut store, now);
    assert_eq!(fetch.request(), Request::MeshLod(high));
    let input = fetch
        .complete_response(Ok(FetchResponse::Partial {
            bytes: vec![0; block.len()],
            range: high.range(),
            total_length: Some(high.asset_length()),
        }))
        .unwrap();
    let decode = store.start_decode(input).unwrap();
    store.complete(decode.run(), now);
    assert_matches!(high_demand.status(), Status::Failed { failure: Failure::MalformedBlock, retry_at: None });
    assert_matches!(low_demand.status(), Status::Ready(Content::Mesh(mesh)) if mesh.lod() == MeshLod::Low);
}

#[test]
fn mesh_lod_fetches_cancel_independently() {
    let id = mesh_resource(102).id;
    let (low, high, _) = mesh_lod_requests(id);
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().unwrap();
    let low_demand = store.request(route, Request::MeshLod(low), Priority::Visible, now).unwrap();
    let high_demand = store.request(route, Request::MeshLod(high), Priority::Visible, now).unwrap();
    let low_fetch = next_fetch(&mut store, now);
    let high_fetch = next_fetch(&mut store, now);
    assert_eq!(low_fetch.request(), Request::MeshLod(low));
    assert_eq!(high_fetch.request(), Request::MeshLod(high));

    drop(low_demand);
    store.maintain(now);
    assert!(low_fetch.is_cancelled());
    assert!(!high_fetch.is_cancelled());

    drop(high_demand);
    store.maintain(now);
    assert!(high_fetch.is_cancelled());
    store.complete(low_fetch.complete_response(Err(Failure::Cancelled)).unwrap_err().into(), now);
    store.complete(high_fetch.complete_response(Err(Failure::Cancelled)).unwrap_err().into(), now);
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[test]
fn completion_preserves_store_identity() {
    let budget = Budget::default();
    let StoreHarness { now, mut store, route, .. } = StoreHarness::with_budget(budget.clone());
    let resource = resource(1);
    let demand = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let input = fetch(&mut store, now, RGBA_J2C.to_vec());
    let job = store.start_decode(input).expect("decode admission");
    let completion = job.run();
    let mut other = Store::new(Budget::default());
    let other_route = other.register_route().unwrap();
    let other_demand = other.request(other_route, Request::new(resource), Priority::Visible, now).unwrap();
    let other_job = other.next_job(now).unwrap();
    other.complete(completion, now);
    assert_matches!(other_demand.status(), Status::Fetching, "a completion from another store cannot publish");
    store.maintain(now);
    assert_matches!(demand.status(), Status::Failed { failure: Failure::Cancelled, retry_at: None });
    drop(other_job);
    assert_eq!(budget.stats().charged_bytes, 0);
}
