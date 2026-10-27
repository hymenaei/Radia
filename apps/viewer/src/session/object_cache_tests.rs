use std::{path::Path, time::Instant};

use network::{Grid, SimulatorId};
use protocol::{
    lludp::{Message, Packet},
    messages::{
        CachedObjectReference, ObjectUpdate as FullUpdate, ObjectUpdateCached, RegionHandshake, region::region_handshake::RegionInfo2,
    },
};
use rusqlite::{Transaction, params};
use storage::sqlite::{SqliteBatch, SqliteDb};
use test_support::{lludp::vectors, parse_hex_fixture};
use tokio::runtime::{Builder, Handle};
use uuid::Uuid;
use world::{OrderEvidence, Region};

use super::{ObjectCacheEvent, ObjectCacheSession, RegionLoadResult, open_at};
use crate::session::inbound::decode_object_message;

const SIMULATOR: SimulatorId = SimulatorId::new(77);
const REGION: Region = Region::new(77);
const REGION_ID: Uuid = Uuid::from_u128(0x1234);
const CACHE_ID: Uuid = Uuid::from_u128(0x5678);
const NEXT_CACHE_ID: Uuid = Uuid::from_u128(0x9abc);

fn fixture_message(fixture: &str) -> Message {
    let bytes = parse_hex_fixture(fixture).expect("fixture is valid hex");
    Packet::decode(&bytes).expect("fixture is a valid LLUDP packet").into_message()
}

fn region_handshake(cache_id: Uuid) -> RegionHandshake {
    let Message::RegionHandshake(mut handshake) = fixture_message(vectors::REGION_HANDSHAKE) else {
        panic!("region handshake fixture should decode as a handshake");
    };
    handshake.region_info1.cache_id = cache_id;
    if let Some(info) = &mut handshake.region_info2 {
        info.region_id = REGION_ID;
    } else {
        handshake.region_info2 = Some(RegionInfo2 { region_id: REGION_ID });
        handshake.region_info3 = Some(Default::default());
    }
    handshake
}

fn open_cache(path: impl AsRef<Path>) -> ObjectCacheSession {
    let (db, status) = open_at(path).expect("open test database");
    let mut cache = ObjectCacheSession::new(Grid::Agni);
    assert!(cache.handle_event(ObjectCacheEvent::Ready { db, status }).is_none());
    cache.set_region_identity(SIMULATOR, &region_handshake(CACHE_ID));
    cache
}

async fn load_region(cache: &mut ObjectCacheSession) -> u32 {
    if let Some(flags) = cache.begin_region_load(SIMULATOR, &Handle::current()) {
        return flags;
    }
    let event = cache.next_event().await.expect("region cache load completes");
    cache.handle_event(event).expect("region cache load completes handshake reply").1
}

fn cached_references(message: &Message) -> (protocol::messages::object::RegionData, Vec<CachedObjectReference>) {
    match message {
        Message::ObjectUpdate(update) => {
            let references = update.objects.iter().map(|object| CachedObjectReference {
                local_id: object.local_id,
                crc: object.crc,
                update_flags: object.update_flags,
            });
            (update.region_data, references.collect())
        }
        Message::ObjectUpdateCompressed(update) => {
            let references = update.objects.iter().map(|object| CachedObjectReference {
                local_id: object.local_id,
                crc: object.crc,
                update_flags: object.update_flags,
            });
            (update.region_data, references.collect())
        }
        _ => panic!("object update fixture should be full or compressed"),
    }
}

fn cached_message(region_data: protocol::messages::object::RegionData, objects: Vec<CachedObjectReference>) -> ObjectUpdateCached {
    ObjectUpdateCached { region_data, objects }
}

fn decoded_cached_batch(update: &ObjectUpdateCached, sequence: u32) -> super::ObjectBatch {
    let message = Message::ObjectUpdateCached(update.clone());
    decode_object_message(REGION, sequence, OrderEvidence::Ordered(u64::from(sequence)), &message)
        .expect("cached update decodes into an object batch")
}

fn resolve(cache: &mut ObjectCacheSession, update: &ObjectUpdateCached, sequence: u32) -> super::ObjectBatch {
    let batch = decoded_cached_batch(update, sequence);
    cache.resolve_cached(SIMULATOR, REGION, sequence, OrderEvidence::Ordered(u64::from(sequence)), update, batch)
}

async fn flush(cache: &ObjectCacheSession) {
    let barrier = SqliteBatch::new(0, 0, |_: &Transaction<'_>| Ok(())).expect("barrier is within SQLite limits");
    let receipt = cache.db.as_ref().expect("cache database is open").submit_batches(vec![barrier]).expect("queue database barrier");
    receipt.wait().await.expect("database barrier completes");
}

async fn cache_count(db: &SqliteDb) -> usize {
    let receipt = db
        .read(32, |transaction| transaction.query_row("SELECT COUNT(*) FROM object_entries", [], |row| row.get::<_, i64>(0)))
        .expect("queue cache count");
    receipt.wait().await.expect("read cache count") as usize
}

#[tokio::test]
async fn full_and_compressed_payloads_survive_restart_and_resolve_as_hits() {
    let directory = tempfile::tempdir().expect("create isolated cache directory");
    let path = directory.path().join("objects.sqlite");
    let mut cache = open_cache(&path);
    assert_eq!(load_region(&mut cache).await, 7);
    assert_eq!(cache.region_handshake_flags(SIMULATOR), 7);

    for fixture in [vectors::OBJECT_UPDATE, vectors::OBJECT_UPDATE_COMPRESSED_TYPED] {
        let mut update = fixture_message(fixture);
        if let Message::ObjectUpdateCompressed(update) = &mut update {
            for object in &mut update.objects {
                object.local_id = object.local_id.saturating_add(10_000);
            }
        }
        let (_, references) = cached_references(&update);
        cache.record_update(SIMULATOR, &update);
        flush(&cache).await;
        assert_eq!(cache.region_handshake_flags(SIMULATOR), 5);
        let cached = cached_message(cached_references(&update).0, references);
        let resolved = resolve(&mut cache, &cached, 40);
        assert_eq!(resolved.blocks.len(), cached.objects.len());
        assert!(
            resolved
                .blocks
                .iter()
                .all(|operation| matches!(operation, Some(super::ObjectOperation::Observe(_) | super::ObjectOperation::Patch(_))))
        );
    }
    flush(&cache).await;
    cache.close().await;

    let mut reopened = open_cache(&path);
    assert_eq!(reopened.region_handshake_flags(SIMULATOR), 7);
    assert_eq!(load_region(&mut reopened).await, 5);
    assert_eq!(reopened.region_handshake_flags(SIMULATOR), 5);
    for fixture in [vectors::OBJECT_UPDATE, vectors::OBJECT_UPDATE_COMPRESSED_TYPED] {
        let mut update = fixture_message(fixture);
        if let Message::ObjectUpdateCompressed(update) = &mut update {
            for object in &mut update.objects {
                object.local_id = object.local_id.saturating_add(10_000);
            }
        }
        let (region_data, references) = cached_references(&update);
        let resolved = resolve(&mut reopened, &cached_message(region_data, references), 41);
        assert!(
            resolved
                .blocks
                .iter()
                .all(|operation| { matches!(operation, Some(super::ObjectOperation::Observe(_) | super::ObjectOperation::Patch(_))) })
        );
    }
    reopened.close().await;
}

#[tokio::test]
async fn crc_mismatch_requests_refresh_and_missing_objects_request_full_updates() {
    let directory = tempfile::tempdir().expect("create isolated cache directory");
    let mut cache = open_cache(directory.path().join("objects.sqlite"));
    assert_eq!(load_region(&mut cache).await, 7);
    let full = fixture_message(vectors::OBJECT_UPDATE);
    cache.record_update(SIMULATOR, &full);
    flush(&cache).await;
    let (region_data, mut references) = cached_references(&full);
    let expected_crc = references[0].crc;
    references[0].crc = expected_crc.wrapping_add(1);
    references.push(CachedObjectReference { local_id: 0x7fff_fffe, crc: 1, update_flags: 0 });

    let resolved = resolve(&mut cache, &cached_message(region_data, references), 51);
    assert!(matches!(
        resolved.blocks[0],
        Some(super::ObjectOperation::Cached { cache_miss_type: protocol::messages::CacheMissType::CrcMismatch, .. })
    ));
    assert!(matches!(
        resolved.blocks.last(),
        Some(Some(super::ObjectOperation::Cached { cache_miss_type: protocol::messages::CacheMissType::Full, .. }))
    ));

    flush(&cache).await;
    let (region_data, mut references) = cached_references(&full);
    references.truncate(1);
    let resolved = resolve(&mut cache, &cached_message(region_data, references), 52);
    assert!(matches!(
        resolved.blocks[0],
        Some(super::ObjectOperation::Cached { cache_miss_type: protocol::messages::CacheMissType::Full, .. })
    ));
    cache.close().await;
}

#[tokio::test]
async fn cached_probes_before_region_load_are_misses() {
    let directory = tempfile::tempdir().expect("create isolated cache directory");
    let path = directory.path().join("objects.sqlite");
    let mut writer = open_cache(&path);
    let full = fixture_message(vectors::OBJECT_UPDATE);
    writer.record_update(SIMULATOR, &full);
    flush(&writer).await;
    writer.close().await;

    let mut cache = open_cache(&path);
    let (region_data, references) = cached_references(&full);
    let update = cached_message(region_data, references);
    let before_load = resolve(&mut cache, &update, 55);
    assert!(before_load.blocks.iter().all(|operation| {
        matches!(operation, Some(super::ObjectOperation::Cached { cache_miss_type: protocol::messages::CacheMissType::Full, .. }))
    }));

    assert_eq!(load_region(&mut cache).await, 5);
    let after_load = resolve(&mut cache, &update, 56);
    assert!(
        after_load
            .blocks
            .iter()
            .all(|operation| { matches!(operation, Some(super::ObjectOperation::Observe(_) | super::ObjectOperation::Patch(_))) })
    );
    cache.close().await;
}

#[tokio::test]
async fn corrupt_entries_are_misses_and_removed() {
    let directory = tempfile::tempdir().expect("create isolated cache directory");
    let path = directory.path().join("objects.sqlite");
    let mut cache = open_cache(&path);
    let full = fixture_message(vectors::OBJECT_UPDATE);
    cache.record_update(SIMULATOR, &full);
    flush(&cache).await;
    cache.close().await;

    let mut cache = open_cache(&path);
    let (region_data, references) = cached_references(&full);
    let scope = cache.identities.get(&SIMULATOR).expect("simulator has a cache scope").clone();
    let corrupt_local_id = references[0].local_id;
    let corrupt = SqliteBatch::new(1, 64, move |transaction| {
        transaction.execute(
            "UPDATE object_entries SET payload = zeroblob(length(payload))
             WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3 AND local_id = ?4",
            params![
                scope.grid_id,
                scope.region_id.as_bytes().as_slice(),
                scope.cache_id.as_bytes().as_slice(),
                i64::from(corrupt_local_id)
            ],
        )?;
        Ok(())
    })
    .expect("corruption batch fits SQLite limits");
    cache.db.as_ref().expect("cache database is open").submit_batches(vec![corrupt]).expect("queue corrupt fixture");
    flush(&cache).await;
    assert_eq!(load_region(&mut cache).await, 5);

    let first = cached_message(region_data, references[..1].to_vec());
    let resolved = resolve(&mut cache, &first, 60);
    assert!(matches!(
        resolved.blocks[0],
        Some(super::ObjectOperation::Cached { cache_miss_type: protocol::messages::CacheMissType::Full, .. })
    ));
    flush(&cache).await;
    assert_eq!(cache_count(cache.db.as_ref().expect("cache database is open")).await, references.len() - 1);
    cache.close().await;
}

#[tokio::test]
async fn cache_id_change_invalidates_the_old_region_scope() {
    let directory = tempfile::tempdir().expect("create isolated cache directory");
    let mut cache = open_cache(directory.path().join("objects.sqlite"));
    assert_eq!(load_region(&mut cache).await, 7);
    let full = fixture_message(vectors::OBJECT_UPDATE);
    cache.record_update(SIMULATOR, &full);
    flush(&cache).await;
    assert_eq!(cache.region_handshake_flags(SIMULATOR), 5);

    cache.set_region_identity(SIMULATOR, &region_handshake(NEXT_CACHE_ID));
    assert_eq!(cache.region_handshake_flags(SIMULATOR), 7);
    flush(&cache).await;
    cache.set_region_identity(SIMULATOR, &region_handshake(CACHE_ID));
    assert_eq!(load_region(&mut cache).await, 7);
    let (region_data, references) = cached_references(&full);
    let result = resolve(&mut cache, &cached_message(region_data, references[..1].to_vec()), 71);
    assert!(matches!(
        result.blocks[0],
        Some(super::ObjectOperation::Cached { cache_miss_type: protocol::messages::CacheMissType::Full, .. })
    ));
    cache.close().await;
}

#[tokio::test]
async fn unavailable_cache_still_advertises_support_and_empty_state() {
    let mut cache = ObjectCacheSession::new(Grid::Agni);
    assert!(cache.handle_event(ObjectCacheEvent::OpenFailed("test failure".into())).is_none());
    cache.set_region_identity(SIMULATOR, &region_handshake(CACHE_ID));
    cache.record_update(SIMULATOR, &fixture_message(vectors::OBJECT_UPDATE));
    assert_eq!(cache.region_handshake_flags(SIMULATOR), 7);
    assert_eq!(cache.begin_region_load(SIMULATOR, &Handle::current()), Some(7));
    cache.close().await;
}

#[tokio::test]
async fn timed_out_region_load_returns_empty_handshake_flags() {
    let directory = tempfile::tempdir().expect("create isolated cache directory");
    let mut cache = open_cache(directory.path().join("objects.sqlite"));
    let scope = cache.identities.get(&SIMULATOR).expect("simulator has a cache scope").clone();
    let permit = cache.load_slots.clone().try_acquire_owned().expect("region load has an admission permit");
    let reply = cache.handle_event(ObjectCacheEvent::RegionLoaded {
        simulator_id: SIMULATOR,
        scope,
        result: RegionLoadResult::TimedOut,
        _permit: permit,
    });
    assert_eq!(reply, Some((SIMULATOR, 7)));
    cache.close().await;
}

#[test]
#[ignore = "manual synthetic region arrival measurement"]
fn measure_warm_and_cold_five_thousand_object_cache_arrival() {
    let runtime = Builder::new_current_thread().enable_all().build().expect("create benchmark runtime");
    runtime.block_on(async {
        let directory = tempfile::tempdir().expect("create isolated cache directory");
        let mut warm_cache = open_cache(directory.path().join("warm.sqlite"));
        let mut cold_cache = open_cache(directory.path().join("cold.sqlite"));
        let Message::ObjectUpdate(template_update) = fixture_message(vectors::OBJECT_UPDATE) else {
            panic!("full update fixture should be an object update");
        };
        let region_data = template_update.region_data;
        let template = template_update.objects[0].clone();
        let mut objects = Vec::with_capacity(255);
        let mut references = Vec::with_capacity(5_000);
        for local_id in 1..=5_000_u32 {
            let mut object = template.clone();
            object.local_id = local_id;
            object.id = Uuid::from_u128(u128::from(local_id));
            object.crc = local_id.wrapping_mul(31);
            objects.push(object.clone());
            references.push(CachedObjectReference { local_id, crc: object.crc, update_flags: object.update_flags });
            if objects.len() == 255 {
                warm_cache.record_update(
                    SIMULATOR,
                    &Message::ObjectUpdate(FullUpdate { region_data, objects: std::mem::take(&mut objects) }),
                );
                flush(&warm_cache).await;
            }
        }
        if !objects.is_empty() {
            warm_cache.record_update(SIMULATOR, &Message::ObjectUpdate(FullUpdate { region_data, objects }));
            flush(&warm_cache).await;
        }
        let cold_started = Instant::now();
        let cold_handshake_flags = load_region(&mut cold_cache).await;
        let cold_handshake_us = cold_started.elapsed().as_micros();
        let warm_started = Instant::now();
        let warm_handshake_flags = load_region(&mut warm_cache).await;
        let warm_handshake_us = warm_started.elapsed().as_micros();
        let mut cold_us = 0_u128;
        let mut warm_us = 0_u128;
        let mut cold_misses = 0;
        let mut warm_misses = 0;
        let packet_count = references.chunks(255).count();
        for chunk in references.chunks(255) {
            let cold = cached_message(region_data, chunk.to_vec());
            let started = Instant::now();
            let resolved = resolve(&mut cold_cache, &cold, 80);
            cold_us += started.elapsed().as_micros();
            cold_misses += resolved.blocks.iter().filter(|operation| matches!(operation, Some(super::ObjectOperation::Cached { .. }))).count();
        }
        for chunk in references.chunks(255) {
            let warm = cached_message(region_data, chunk.to_vec());
            let started = Instant::now();
            let resolved = resolve(&mut warm_cache, &warm, 81);
            warm_us += started.elapsed().as_micros();
            warm_misses += resolved.blocks.iter().filter(|operation| matches!(operation, Some(super::ObjectOperation::Cached { .. }))).count();
        }
        println!(
            "5000 objects / {packet_count} cached packets: cold handshake load={cold_handshake_us}us flags={cold_handshake_flags}; cold resolve={cold_us}us total, {cold_misses} misses / {} request packets; warm handshake load={warm_handshake_us}us flags={warm_handshake_flags}; warm resolve={warm_us}us total, {warm_misses} misses / {} request packets",
            cold_misses.div_ceil(255),
            warm_misses.div_ceil(255),
        );
        println!(
            "per-packet averages: cold={cold_per_packet}us, warm={warm_per_packet}us",
            cold_per_packet = cold_us / u128::try_from(packet_count).unwrap_or(1),
            warm_per_packet = warm_us / u128::try_from(packet_count).unwrap_or(1)
        );
        cold_cache.close().await;
        warm_cache.close().await;
    });
}
