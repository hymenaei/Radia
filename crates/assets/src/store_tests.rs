use std::{
    assert_matches,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

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
    cache
        .put_extent(
            resource,
            crate::cache::ValidatedExtent::from_validated(u64::try_from(RGBA_J2C.len()).expect("fixture size"), 0, RGBA_J2C.to_vec()),
        )
        .expect("seed validated texture bytes");
    let budget = Budget::default();
    let mut store = Store::with_cache(budget.clone(), Arc::clone(&cache));
    let now = Instant::now();
    let route = store.register_route().expect("register route");
    let demand = store.request(route, Request::new(resource), Priority::Visible, now).expect("request texture");
    let LoadJob::ReadCache(read) = store.next_job(now).expect("cache read job") else { panic!("first job reads cache") };
    assert_eq!(
        budget.stats().charged_bytes,
        read_work_bytes(Kind::Texture).expect("bounded cache read reservation"),
        "disk reads reserve for the stored record and decoded extents",
    );
    let input = store.complete_cache_read(read.run(), now).expect("cache hit supplies input");
    let LoadJob::Decode(input) = input else { panic!("cache hit follows decode admission") };
    assert_eq!(store.entries.get(&resource).and_then(|entry| entry.views.get(&route)).map(|view| view.attempts), Some(0));
    let decode = store.start_decode(input).expect("decode admission");
    assert!(store.complete(decode.run(), now).is_empty());
    assert_matches!(demand.status(), Status::Ready(Content::Texture(_)));
    assert_eq!(cache.stats().hits, 1);
    assert_eq!(cache.stats().bytes_read, u64::try_from(RGBA_J2C.len() + 20 + 16).expect("encoded fixture size"));
    assert_eq!(budget.stats().charged_bytes, RGBA_J2C.len() + RGBA_RAW.len());
    drop(demand);
    store.close();
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn retired_route_discards_a_completed_disk_read() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let resource = resource(12);
    cache
        .put_extent(
            resource,
            crate::cache::ValidatedExtent::from_validated(u64::try_from(RGBA_J2C.len()).expect("fixture size"), 0, RGBA_J2C.to_vec()),
        )
        .expect("seed validated texture bytes");
    let budget = Budget::default();
    let mut store = Store::with_cache(budget.clone(), Arc::clone(&cache));
    let now = Instant::now();
    let route = store.register_route().expect("register route");
    let demand = store.request(route, Request::new(resource), Priority::Visible, now).expect("request texture");
    let LoadJob::ReadCache(read) = store.next_job(now).expect("cache read job") else { panic!("first job reads cache") };
    let (started, started_receive) = mpsc::sync_channel(0);
    let (release, release_receive) = mpsc::sync_channel(0);
    let worker = thread::spawn(move || {
        read.run_lookup(|cache, resource| {
            started.send(()).expect("signal active disk read");
            release_receive.recv().expect("release disk read");
            cache.lookup(resource)
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
    cache
        .put_extent(
            resource,
            crate::cache::ValidatedExtent::from_validated(u64::try_from(RGBA_J2C.len()).expect("fixture size"), 0, RGBA_J2C.to_vec()),
        )
        .expect("seed validated texture bytes");
    let budget = Budget::default();
    let mut store = Store::with_cache(budget.clone(), Arc::clone(&cache));
    let now = Instant::now();
    let route = store.register_route().expect("register route");
    let demand = store.request(route, Request::new(resource), Priority::Visible, now).expect("request texture");
    let LoadJob::ReadCache(read) = store.next_job(now).expect("cache read job") else { panic!("first job reads cache") };
    let (started, started_receive) = mpsc::sync_channel(0);
    let (release, release_receive) = mpsc::sync_channel(0);
    let worker = thread::spawn(move || {
        read.run_lookup(|cache, resource| {
            started.send(()).expect("signal active disk read");
            release_receive.recv().expect("release disk read");
            cache.lookup(resource)
        })
    });
    started_receive.recv_timeout(Duration::from_secs(3)).expect("disk read job started");
    assert_eq!(budget.stats().charged_bytes, read_work_bytes(Kind::Texture).expect("read reservation"));
    drop(demand);
    store.maintain(now);
    release.send(()).expect("resume disk read");
    let completion = worker.join().expect("disk read worker completes");
    assert!(store.complete_cache_read(completion, now).is_none());
    assert_eq!(store.running_fetches, 0);
    assert_eq!(store.running_decodes, 0);
    assert_eq!(budget.stats().charged_bytes, 0);
    assert!(store.next_job(now).is_none(), "cancelled demand must not enter decode admission");

    let retry = store.request(route, Request::new(resource), Priority::Visible, now).expect("retry texture demand");
    let LoadJob::ReadCache(read) = store.next_job(now).expect("later demand reads cache") else { panic!("retry begins with disk read") };
    let Some(LoadJob::Decode(input)) = store.complete_cache_read(read.run(), now) else { panic!("cache hit supplies decode input") };
    let decode = store.start_decode(input).expect("admit retry decode");
    assert!(store.complete(decode.run(), now).is_empty());
    assert_matches!(retry.status(), Status::Ready(Content::Texture(_)));
    assert_eq!(store.running_fetches, 0);
    assert_eq!(store.running_decodes, 0);
    store.close();
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn malformed_disk_payload_is_discarded_without_failing_demand_attempts() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let resource = resource(13);
    cache
        .put_extent(resource, crate::cache::ValidatedExtent::from_validated(3, 0, b"bad".to_vec()))
        .expect("seed semantically malformed payload");
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
    let crate::cache::AssetLookup::Hit(asset) = cache.lookup(resource) else { panic!("valid network bytes replace corrupt cache data") };
    assert_eq!(asset.extents.first().map(|extent| extent.bytes.as_slice()), Some(RGBA_J2C));
    drop(store);
    Arc::try_unwrap(cache).expect("cache handle released").shutdown().expect("close cache");
}

#[test]
fn full_write_admission_does_not_hold_network_fetch_admission() {
    let directory = tempdir().expect("cache directory");
    let cache = Arc::new(crate::cache::AssetCache::open(directory.path(), storage::DEFAULT_SIZE_LIMIT).expect("open cache"));
    let write_budget = Budget::default();
    let writes = (0..2u128)
        .map(|id| cache.queue_write(resource(20 + id), charged_blob(&write_budget, b"payload"), &write_budget).expect("write slot"))
        .collect::<Vec<_>>();
    assert!(cache.queue_write(resource(22), charged_blob(&write_budget, b"dropped"), &write_budget).is_none());
    assert_eq!(cache.stats().dropped_writes, 1);
    let mut store = Store::with_cache(Budget::default(), Arc::clone(&cache));
    let now = Instant::now();
    let route = store.register_route().expect("register route");
    let demands = (30..30 + MAX_ASSET_FETCHES)
        .map(|id| store.request(route, Request::new(resource(id as u128)), Priority::Visible, now).expect("request texture"))
        .collect::<Vec<_>>();
    let mut fetches = Vec::new();
    for _ in 0..MAX_ASSET_FETCHES {
        let LoadJob::ReadCache(read) = store.next_job(now).expect("cache read job") else { panic!("cold asset reads cache") };
        let Some(LoadJob::Fetch(fetch)) = store.complete_cache_read(read.run(), now) else { panic!("cache miss schedules fetch") };
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
        let LoadJob::ReadCache(read) = store.next_job(now).expect("cold cache read") else { panic!("first work reads disk") };
        let Some(LoadJob::Fetch(fetch)) = store.complete_cache_read(read.run(), now) else { panic!("cache miss schedules network") };
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
    assert_eq!(cache.stats().misses, 3);
    assert!(cache.stats().bytes_written > 0);
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
    let demands: Vec<_> = (1..=12)
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
    for _ in 0..MAX_ASSET_DECODES {
        let input = fetch(&mut store, now, RGBA_J2C.to_vec());
        let job = store.start_decode(input).expect("decode admission");
        decoding.push(job);
    }
    let mut fetched = Vec::<DecodeInput>::new();
    let mut fetched_bytes = 0;
    for _ in 0..MAX_ASSET_FETCHES {
        let bytes = material_envelope(material_json);
        fetched_bytes += bytes.capacity();
        let input = next_fetch(&mut store, now).complete(Ok(bytes)).expect("valid material response is attached to its fetch");
        let input = store.start_decode(input).expect_err("decoding remains bounded independently of fetching");
        fetched.push(input);
    }
    assert!(store.next_job(now).is_none(), "fetched responses retain fetch admission until decoding starts");
    assert_eq!(budget.stats().charged_bytes, 2 * TEXTURE_WORK_BYTES + fetched_bytes);
    let job = decoding.pop().unwrap();
    let result = job.run();
    store.maintain(now);
    let waiting = store.start_decode(fetched.remove(0)).expect_err("an undelivered completion retains its decode slot");
    store.complete(result, now);
    let job = store.start_decode(waiting).expect("a delivered completion admits waiting data");
    assert_eq!(job.resource(), Resource { id: resource(3).id, kind: Kind::PbrMaterial });
    store.complete(job.run(), now);
    let Status::Ready(Content::Material(content)) = demands[2].status() else { panic!("waiting material publishes decoded content") };
    let MaterialDescription::Pbr(material) = &*content else { panic!("waiting response decodes to PBR fields") };
    assert_eq!(material.base_color_factor, [0.2, 0.3, 0.4, 0.5]);
    drop(content);
    assert!(store.next_job(now).is_some(), "starting a decoder releases one fetch slot");
    assert_matches!(demands[0].status(), Status::Decoding);
    assert_matches!(demands[1].status(), Status::Ready(_));
    store.close();
    drop(decoding);
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
fn dropped_jobs_wake_sessions() {
    let budget = Budget::default();
    let StoreHarness { now, mut store, route, .. } = StoreHarness::with_budget(budget.clone());
    let next = Store::new(budget.clone());
    let mut first_wake = store.wake();
    let mut second_wake = next.wake();
    let demand = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let input = fetch(&mut store, now, RGBA_J2C.to_vec());
    let job = store.start_decode(input).expect("decode admission");
    assert_eq!(store.running_decodes, 1);
    first_wake.borrow_and_update();
    second_wake.borrow_and_update();
    drop(job);
    assert!(first_wake.has_changed().unwrap());
    assert!(second_wake.has_changed().unwrap(), "reconnect and retiring owners must not steal each other's wakeups");
    store.maintain(now);
    assert_eq!(store.running_decodes, 0);
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
    assert_eq!(store.running_fetches, 1);
    drop(first);
    store.maintain(now);
    assert_eq!(store.running_fetches, 1);
    assert!(job.is_cancelled());
    let replacement = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    assert!(store.next_job(now).is_none(), "reacquisition must not duplicate a resource's in-flight work");
    let completion = job.complete(Err(Failure::NotFound)).expect_err("cancellation wins over a late fetch failure");
    assert_eq!(completion.failure(), Failure::Cancelled);
    store.complete(completion.into(), now);
    assert_eq!(store.running_fetches, 0);
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
                        proptest::prop_assert!(jobs.len() <= MAX_ASSET_FETCHES);
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
            proptest::prop_assert_eq!(store.running_fetches, fetching);
            proptest::prop_assert_eq!(store.running_decodes, decoding);
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
        proptest::prop_assert_eq!(store.running_fetches, 0);
        proptest::prop_assert_eq!(store.running_decodes, 0);
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
    assert_eq!(store.running_decodes, 1);
    let prepared = job.run();
    store.retire_route(route);
    store.complete(prepared, now);
    assert_eq!(store.running_decodes, 0);
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
