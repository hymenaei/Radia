use llsd::binary;
use test_support::texture::{GRAY_4096_J2C, RGBA_J2C, RGBA_RAW};
use uuid::Uuid;

use super::*;
use crate::{
    Image, Priority,
    budget::{ASSET_WORK_BYTES, TEXTURE_WORK_BYTES},
    material::{MaterialDescription, PbrMaterial},
};

fn resource(id: u128) -> Resource {
    Resource { id: AssetId::new(Uuid::from_u128(id)), kind: Kind::Texture }
}
fn ready(demand: &Demand) -> Image {
    let Status::Ready(Content::Texture(blob)) = demand.status() else { panic!("expected ready asset") };
    blob.image().unwrap()
}

fn prepared(job: &LoadJob) -> Result<Prepared, Failure> {
    job.prepare(RGBA_J2C.to_vec())
}

#[test]
fn visible_work_precedes_baselines() {
    let now = Instant::now();
    let mut store = Store::new(Budget::default());
    let route = store.register_route().unwrap();
    let baseline = store.request(route, Request::new(resource(1)), Priority::Background, now).unwrap();
    let later = now + Duration::from_secs(3600);
    let visible = store.request(route, Request::new(resource(2)), Priority::Visible, later).unwrap();
    let mut job = store.next_job(later).unwrap();
    assert!(store.try_start_decode(&mut job));
    assert_eq!(job.resource(), resource(2), "visible work takes priority over older regional prefetches");
    let result = prepared(&job);
    store.complete(job, result, later);
    assert_eq!(ready(&visible).size(), [8; 2]);
    let mut job = store.next_job(later).unwrap();
    assert!(store.try_start_decode(&mut job));
    assert_eq!(job.resource(), resource(1), "background queue time does not consume the fetch deadline");
    assert!(matches!(baseline.status(), Status::Decoding));
    store.maintain(later + Duration::from_secs(61));
    assert!(job.is_cancelled(), "running background work still has a bounded deadline");
    assert!(matches!(baseline.status(), Status::Failed { failure: Failure::Deadline, .. }));
}

#[test]
fn shared_detail_refines_and_releases() {
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().unwrap();
    let id = resource(1).id;
    let low = store.request(route, Request::Texture { id, size: TextureSize::new(64) }, Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    assert!(!job.has_source());
    let bytes = GRAY_4096_J2C;
    let result = job.prepare(bytes.to_vec());
    store.complete(job, result, now);
    let retained = ready(&low);
    assert_eq!((retained.source_size(), retained.size(), retained.discard_level()), ([4096; 2], [64; 2], 6));
    assert_eq!(budget.stats().resident_bytes, bytes.len() + 64 * 64);
    let later = now + Duration::from_secs(3600);
    let high = store.request(route, Request::Texture { id, size: TextureSize::new(512) }, Priority::Visible, later).unwrap();
    let clone = high.clone();
    let mut job = store.next_job(later).unwrap();
    assert!(store.try_start_decode(&mut job));
    assert!(job.has_source(), "refinement reuses the encoded asset after the original request deadline");
    let Status::Refining(Content::Texture(old)) = low.status() else { panic!("expected usable fallback") };
    assert_eq!(old.info().unwrap().size, [64; 2]);
    drop(high);
    store.maintain(later);
    assert!(!job.is_cancelled(), "cloned demand keeps the higher target alive");
    let result = job.prepare(Vec::new());
    store.complete(job, result, later);
    assert_eq!((ready(&low).size(), ready(&low).discard_level()), ([512; 2], 3));
    assert!(store.next_job(later).is_none());
    drop(clone);
    let mut job = store.next_job(later).unwrap();
    assert!(store.try_start_decode(&mut job));
    assert!(job.has_source());
    let Status::Refining(Content::Texture(current)) = low.status() else { panic!("expected reducing detail with fallback") };
    assert_eq!(current.info().unwrap().size, [512; 2], "old quality stays usable while reducing residency");
    drop(current);
    let result = job.prepare(Vec::new());
    store.complete(job, result, later);
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
    let now = Instant::now();
    let mut store = Store::new(Budget::default());
    let route = store.register_route().unwrap();
    let id = resource(1).id;
    let low = store.request(route, Request::Texture { id, size: TextureSize::new(32) }, Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let result = job.prepare(GRAY_4096_J2C.to_vec());
    store.complete(job, result, now);
    let high = store.request(route, Request::Texture { id, size: TextureSize::new(256) }, Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    store.complete(job, Err(Failure::Unsupported), now);
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
    let now = Instant::now();
    let mut store = Store::new(Budget::default());
    let route = store.register_route().unwrap();
    let id = resource(1).id;
    let high_request = Request::Texture { id, size: TextureSize::new(512) };
    let low = store.request(route, Request::Texture { id, size: TextureSize::new(32) }, Priority::Visible, now).unwrap();
    let high = store.request(route, high_request, Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let result = job.prepare(GRAY_4096_J2C.to_vec());
    store.complete(job, result, now);
    assert_eq!(ready(&low).size(), [512; 2]);
    drop(high);
    let mut reducing = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut reducing));
    let result = reducing.prepare(Vec::new());
    let _high = store.request(route, high_request, Priority::Visible, now).unwrap();
    store.complete(reducing, result, now);
    assert_eq!(ready(&low).size(), [512; 2], "obsolete reduction cannot replace the still-needed detail");
    assert!(store.next_job(now).is_none());
}

#[test]
fn shared_demand_controls_cancellation() {
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().unwrap();
    let first = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let second = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let clone = first.clone();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    assert!(store.next_job(now).is_none());
    drop(first);
    drop(clone);
    store.maintain(now);
    assert!(!job.is_cancelled(), "one remaining face still needs the shared fetch");
    drop(second);
    store.maintain(now);
    assert!(job.is_cancelled());
    assert_eq!(budget.stats().charged_bytes, TEXTURE_WORK_BYTES, "cancellation cannot release a running job's reservation");
    store.complete(job, Err(Failure::Cancelled), now);
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[test]
fn route_failures_share_success() {
    let now = Instant::now();
    let mut store = Store::new(Budget::default());
    let first = store.register_route().unwrap();
    let a = store.request(first, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    store.complete(job, Err(Failure::MissingCapability), now);
    assert!(matches!(a.status(), Status::Failed { failure: Failure::MissingCapability, retry_at: None }));
    assert!(store.next_job(now + Duration::from_secs(61)).is_none());
    let second = store.register_route().unwrap();
    let b = store.request(second, Request::new(resource(1)), Priority::Visible, now + Duration::from_secs(61)).unwrap();
    let mut job = store.next_job(now + Duration::from_secs(61)).unwrap();
    assert!(store.try_start_decode(&mut job));
    assert_eq!(job.route(), second);
    let result = prepared(&job);
    store.complete(job, result, now + Duration::from_secs(61));
    assert_eq!(ready(&a).pixels(), RGBA_RAW);
    assert!(std::ptr::eq(ready(&a).pixels().as_ptr(), ready(&b).pixels().as_ptr()));
    let cached = store.request(first, Request::new(resource(1)), Priority::Visible, now + Duration::from_secs(62)).unwrap();
    assert!(std::ptr::eq(ready(&a).pixels().as_ptr(), ready(&cached).pixels().as_ptr()));
    assert!(store.next_job(now + Duration::from_secs(62)).is_none());
}

#[test]
fn retry_limits_and_server_delays() {
    let mut now = Instant::now();
    let mut store = Store::new(Budget::default());
    let route = store.register_route().unwrap();
    let demand = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    for attempt in 1..=3 {
        let mut job = store.next_job(now).unwrap();
        assert!(store.try_start_decode(&mut job));
        store.complete(job, Err(Failure::Transient { retry_after: Duration::ZERO }), now);
        let Status::Failed { retry_at, .. } = demand.status() else { panic!("expected classified retry failure") };
        if attempt == 3 {
            assert!(retry_at.is_none());
            break;
        }
        assert!(store.next_job(now).is_none());
        now = retry_at.unwrap();
    }
    assert!(store.next_job(now + Duration::from_secs(3600)).is_none());
    assert_eq!(store.next_deadline(now + Duration::from_secs(3600)), None);
    let demand = store.request(route, Request::new(resource(2)), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    store.complete(job, Err(Failure::Transient { retry_after: Duration::from_secs(120) }), now);
    assert!(matches!(demand.status(), Status::Failed { retry_at: None, .. }));
}

#[test]
fn retained_pixels_allow_work() {
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().unwrap();
    let demand = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let result = prepared(&job);
    store.complete(job, result, now);
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

#[test]
fn retired_routes_reject_late_results() {
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().unwrap();
    let demand = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let result = prepared(&job);
    store.retire_route(route);
    assert!(job.is_cancelled());
    assert!(matches!(demand.status(), Status::Failed { failure: Failure::RouteClosed, .. }));
    store.complete(job, result, now);
    assert_eq!(budget.stats().charged_bytes, 0);
    let replacement = store.register_route().unwrap();
    let waiting = store.request(replacement, Request::new(resource(1)), Priority::Visible, now).unwrap();
    store.maintain(now + DEMAND_DEADLINE);
    assert!(matches!(waiting.status(), Status::Failed { failure: Failure::Deadline, retry_at: None }));
    assert!(store.next_job(now + DEMAND_DEADLINE).is_none());
    assert_eq!(store.next_deadline(now + DEMAND_DEADLINE), None);
}

#[test]
fn route_failures_preserve_status() {
    let now = Instant::now();
    let mut store = Store::new(Budget::default());
    let first = store.register_route().unwrap();
    let second = store.register_route().unwrap();
    let a = store.request(first, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    store.complete(job, Err(Failure::MissingCapability), now);
    let b = store.request(second, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    store.complete(job, Err(Failure::NotFound), now);
    assert!(matches!(a.status(), Status::Failed { failure: Failure::MissingCapability, retry_at: None }));
    assert!(matches!(b.status(), Status::Failed { failure: Failure::NotFound, retry_at: None }));
    assert!(store.next_job(now).is_none());
}

#[test]
fn shutdown_retains_running_work() {
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().unwrap();
    let demand = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    assert!(matches!(demand.status(), Status::Decoding));
    let result = prepared(&job);
    store.close();
    assert_eq!(budget.stats().charged_bytes, TEXTURE_WORK_BYTES);
    store.complete(job, result, now);
    assert_eq!(budget.stats().charged_bytes, 0);
    assert!(matches!(demand.status(), Status::StaleSession));
}

#[test]
fn demand_has_no_cache_cap() {
    let now = Instant::now();
    let mut store = Store::new(Budget::default());
    let route = store.register_route().unwrap();
    let demands: Vec<_> = (1..=4097).map(|id| store.request(route, Request::new(resource(id)), Priority::Visible, now).unwrap()).collect();
    assert_eq!(demands.len(), 4097, "live handles are not rejected by the former 4096-entry cache limit");
    drop(demands);
    store.maintain(now);
    assert!(store.next_job(now).is_none());
    let demand = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let result = prepared(&job);
    store.complete(job, result, now);
    assert_eq!(ready(&demand).size(), [8; 2]);
}

#[test]
fn fetches_overlap_decodes() {
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().unwrap();
    let demands: Vec<_> = (1..=12)
        .map(|id| {
            let mut resource = resource(id);
            if id > 2 {
                resource.kind = Kind::PbrMaterial;
            }
            store.request(route, Request::new(resource), Priority::Visible, now).unwrap()
        })
        .collect();
    let bytes = RGBA_J2C;
    let mut decoding = Vec::new();
    for _ in 0..MAX_ASSET_DECODES {
        let mut job = store.next_job(now).unwrap();
        job.retain_response(bytes.len()).unwrap();
        assert!(store.try_start_decode(&mut job));
        decoding.push(job);
    }
    let mut fetched = Vec::new();
    for _ in 0..MAX_ASSET_FETCHES {
        let mut job = store.next_job(now).expect("fetching continues while both decoders are occupied");
        job.retain_response(bytes.len()).unwrap();
        assert!(!store.try_start_decode(&mut job), "decoding remains bounded independently of fetching");
        fetched.push(job);
    }
    assert!(store.next_job(now).is_none(), "fetched responses retain fetch admission until decoding starts");
    assert_eq!(budget.stats().charged_bytes, 2 * TEXTURE_WORK_BYTES + 8 * bytes.len());
    let job = decoding.pop().unwrap();
    let result = job.prepare(bytes.to_vec());
    store.complete(job, result, now);
    assert!(store.try_start_decode(&mut fetched[0]), "a completed decoder admits waiting data");
    assert!(store.next_job(now).is_some(), "starting a decoder releases one fetch slot");
    assert!(matches!(demands[0].status(), Status::Decoding));
    assert!(matches!(demands[1].status(), Status::Ready(_)));
    store.close();
    drop(decoding);
    drop(fetched);
    drop(demands);
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[test]
fn waiting_data_cancels_cleanly() {
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().unwrap();
    let demand = store.request(route, Request::new(resource(1)), Priority::Background, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    job.retain_response(1024).unwrap();
    assert_eq!(store.priority(&job), Priority::Background);
    let visible = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    assert_eq!(store.priority(&job), Priority::Visible, "waiting work uses current demand, not fetch-time priority");
    drop(visible);
    assert_eq!(store.priority(&job), Priority::Background);
    assert_eq!(budget.stats().charged_bytes, 1024);
    assert!(matches!(demand.status(), Status::Fetching));
    assert_eq!(job.prepare(RGBA_J2C.to_vec()).err(), Some(Failure::Malformed), "unadmitted decoding cannot allocate");
    drop(demand);
    store.maintain(now);
    assert!(job.is_cancelled());
    assert!(!store.try_start_decode(&mut job));
    assert_eq!(budget.stats().charged_bytes, 1024, "cancellation preserves queued ownership until delivery");
    store.complete(job, Err(Failure::Cancelled), now);
    assert_eq!(budget.stats().charged_bytes, 0);
    let next = store.request(route, Request::new(resource(2)), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert_eq!(job.retain_response(Kind::Texture.max_response_bytes() + 1), Err(Failure::TooLarge));
    assert_eq!(budget.stats().charged_bytes, Kind::Texture.max_response_bytes());
    store.complete(job, Err(Failure::TooLarge), now);
    assert!(matches!(next.status(), Status::Failed { failure: Failure::TooLarge, .. }));
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[test]
fn dropped_jobs_wake_sessions() {
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let next = Store::new(budget.clone());
    let mut first_wake = store.wake();
    let mut second_wake = next.wake();
    let route = store.register_route().unwrap();
    let demand = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    first_wake.borrow_and_update();
    second_wake.borrow_and_update();
    drop(job);
    assert!(first_wake.has_changed().unwrap());
    assert!(second_wake.has_changed().unwrap(), "reconnect and retiring owners must not steal each other's wakeups");
    store.maintain(now);
    assert!(matches!(demand.status(), Status::Failed { failure: Failure::Cancelled, retry_at: None }));
    assert!(store.next_job(now).is_none());
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[test]
fn reacquisition_waits_for_cancelled_job() {
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().unwrap();
    let first = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    let job = store.next_job(now).unwrap();
    drop(first);
    store.maintain(now);
    assert!(job.is_cancelled());
    let replacement = store.request(route, Request::new(resource(1)), Priority::Visible, now).unwrap();
    assert!(store.next_job(now).is_none(), "reacquisition must not duplicate a resource's in-flight work");
    store.complete(job, Err(Failure::Cancelled), now);
    let mut job = store.next_job(now).expect("the replacement starts after cancelled work completes");
    assert!(store.try_start_decode(&mut job));
    let result = prepared(&job);
    store.complete(job, result, now);
    assert_eq!(ready(&replacement).size(), [8; 2]);
    drop(replacement);
    store.maintain(now);
    assert_eq!(budget.stats().charged_bytes, 0);
}

proptest::proptest! {
    #[test]
    fn demand_churn_preserves_lifetimes(
        operations in proptest::collection::vec((0u8..4, 0usize..16), 0..120)
    ) {
        let now = Instant::now();
        let budget = Budget::default();
        let mut store = Store::new(budget.clone());
        let route = store.register_route().unwrap();
        let mut leases: Vec<Option<(Resource, Demand)>> = (0..16).map(|_| None).collect();
        let mut jobs = Vec::<LoadJob>::new();
        for (operation, slot) in operations {
            match operation {
                0 if leases[slot].is_none() => {
                    let resource = resource((slot % 8 + 1) as u128);
                    leases[slot] = Some((resource, store.request(route, Request::new(resource), Priority::Visible, now).unwrap()));
                }
                1 => { leases[slot] = None; }
                2 => {
                    if let Some(job) = store.next_job(now) {
                        proptest::prop_assert!(!jobs.iter().any(|active| active.resource() == job.resource()));
                        proptest::prop_assert!(leases.iter().flatten().any(|(resource, _)| *resource == job.resource()));
                        jobs.push(job);
                        proptest::prop_assert!(jobs.len() <= MAX_ASSET_FETCHES);
                    }
                }
                3 => {
                    if let Some(mut job) = jobs.pop() {
                        if job.is_cancelled() {
                            store.complete(job, Err(Failure::Cancelled), now);
                        } else if store.try_start_decode(&mut job) {
                            let result = prepared(&job);
                            store.complete(job, result, now);
                        } else {
                            jobs.push(job);
                        }
                    }
                }
                _ => {}
            }
            store.maintain(now);
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
        drop(jobs);
        proptest::prop_assert_eq!(budget.stats().charged_bytes, 0);
    }
}

const PBR_JSON: &str = r#"{"asset":{"version":"2.0"},"materials":[{}]}"#;

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
fn shared_material_retains_charge() {
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().unwrap();
    let resource = Resource { id: AssetId::new(Uuid::from_u128(2)), kind: Kind::PbrMaterial };
    let first = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let second = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let prepared = job.prepare(material_envelope(PBR_JSON)).unwrap();
    store.complete(job, Ok(prepared), now);
    let Status::Ready(Content::Material(material)) = first.status() else { panic!("expected material") };
    let Status::Ready(Content::Material(shared)) = second.status() else { panic!("expected shared material") };
    assert!(std::ptr::eq(&*material, &*shared));
    assert_eq!(budget.stats().resident_bytes, size_of::<MaterialDescription>());
    store.close();
    assert!(matches!(second.status(), Status::StaleSession));
    assert_eq!(*material, MaterialDescription::Pbr(PbrMaterial::default()));
    drop(material);
    assert_eq!(budget.stats().charged_bytes, size_of::<MaterialDescription>());
    drop(shared);
    assert_eq!(budget.stats().charged_bytes, 0);
}

#[test]
fn retired_material_cannot_publish() {
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().unwrap();
    let resource = Resource { id: AssetId::new(Uuid::from_u128(3)), kind: Kind::LegacyMaterial };
    let old = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let prepared = job.prepare(legacy_envelope(resource.id)).unwrap();
    store.retire_route(route);
    store.complete(job, Ok(prepared), now);
    assert!(matches!(old.status(), Status::Failed { failure: Failure::RouteClosed, .. }));
    assert_eq!(budget.stats().charged_bytes, 0);
    let route = store.register_route().unwrap();
    let current = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let result = job.prepare(legacy_envelope(resource.id));
    store.complete(job, result, now);
    let Status::Ready(Content::Material(material)) = current.status() else { panic!("expected legacy material") };
    assert_eq!(*material, crate::material::legacy::decode(&binary::to_vec(&llsd::Value::Map(legacy_fields(2, 128))).unwrap()).unwrap());
}

#[test]
fn material_identity_must_match() {
    let now = Instant::now();
    let budget = Budget::default();
    let mut store = Store::new(budget.clone());
    let route = store.register_route().unwrap();
    let resource = Resource { id: AssetId::new(Uuid::from_u128(1)), kind: Kind::PbrMaterial };
    let demand = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
    let mut job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut job));
    let other = Resource { id: AssetId::new(Uuid::from_u128(2)), ..resource };
    let _other_demand = store.request(route, Request::new(other), Priority::Visible, now).unwrap();
    let mut other_job = store.next_job(now).unwrap();
    assert!(store.try_start_decode(&mut other_job));
    let result = other_job.prepare(material_envelope(PBR_JSON));
    drop(other_job);
    store.complete(job, result, now);
    assert!(matches!(demand.status(), Status::Failed { failure: Failure::Malformed, retry_at: None }));
    assert_eq!(budget.stats().charged_bytes, 0);
}
