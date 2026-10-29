use std::{path::Path, time::Instant};

use network::{Grid, SimulatorId};
use protocol::{
    lludp::{Message, Packet},
    messages::{
        CachedObjectReference, ObjectUpdate as FullUpdate, ObjectUpdateCached, RegionHandshake, region::region_handshake::RegionInfo2,
    },
};
use rusqlite::Transaction;
use storage::sqlite::{SqliteBatch, SqliteDb};
pub(super) use test_support::lludp::vectors;
use test_support::parse_hex_fixture;
use tokio::runtime::Handle;
use uuid::Uuid;
use world::{OrderEvidence, Region, RegionHandle};

use crate::session::inbound::{ObjectBatch, decode_object_message};
use crate::session::object_cache::storage::open_at;
use crate::session::object_cache::{CacheScope, CacheWriteWorker, ObjectCacheEvent, ObjectCacheSession};
use crate::session::world::{SessionWorld, viewer_world_limits};
pub(super) const SIMULATOR: SimulatorId = SimulatorId::new(77);
pub(super) const REGION: Region = Region::new(77);
pub(super) const REGION_HANDLE: u64 = 0x2222_0000_0000_0002;
pub(super) const REGION_ID: Uuid = Uuid::from_u128(0x1234);
pub(super) const CACHE_ID: Uuid = Uuid::from_u128(0x5678);
pub(super) const NEXT_CACHE_ID: Uuid = Uuid::from_u128(0x9abc);

pub(super) fn fixture_message(fixture: &str) -> Message {
    let bytes = parse_hex_fixture(fixture).expect("fixture is valid hex");
    Packet::decode(&bytes).expect("fixture is a valid LLUDP packet").into_message()
}

pub(super) fn region_handshake(cache_id: Uuid) -> RegionHandshake {
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

pub(super) fn open_cache(path: impl AsRef<Path>) -> ObjectCacheSession {
    let (db, status) = open_at(path).expect("open test database");
    let mut cache = ObjectCacheSession::new(Grid::Agni);
    let db = std::sync::Arc::new(db);
    let writer = CacheWriteWorker::start(std::sync::Arc::clone(&db), Handle::current()).expect("start cache writer");
    assert!(cache.handle_event(ObjectCacheEvent::Ready { db, status, writer }).is_none());
    cache.set_region_identity(SIMULATOR, &region_handshake(CACHE_ID));
    cache
}

pub(super) async fn load_region(cache: &mut ObjectCacheSession) -> u32 {
    if let Some(flags) = cache.begin_region_load(SIMULATOR, &Handle::current()) {
        return flags;
    }
    loop {
        let event = cache.next_event().await.expect("region cache load completes");
        if let Some((_, flags)) = cache.handle_event(event) {
            return flags;
        }
    }
}

pub(super) fn cached_references(message: &Message) -> (protocol::messages::object::RegionData, Vec<CachedObjectReference>) {
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

pub(super) fn cached_message(
    region_data: protocol::messages::object::RegionData,
    objects: Vec<CachedObjectReference>,
) -> ObjectUpdateCached {
    ObjectUpdateCached { region_data, objects }
}

pub(super) fn decoded_cached_batch(update: &ObjectUpdateCached, sequence: u32) -> ObjectBatch {
    let message = Message::ObjectUpdateCached(update.clone());
    decode_object_message(REGION, sequence, OrderEvidence::Ordered(u64::from(sequence)), &message)
        .expect("cached update decodes into an object batch")
}

pub(super) async fn resolve(cache: &mut ObjectCacheSession, update: &ObjectUpdateCached, sequence: u32) -> ObjectBatch {
    let batch = decoded_cached_batch(update, sequence);
    cache.resolve_cached(SIMULATOR, REGION, sequence, OrderEvidence::Ordered(u64::from(sequence)), |_| false, update, batch).await.0
}

pub(super) async fn flush(cache: &mut ObjectCacheSession) {
    while cache.outstanding_writes > 0 {
        let event = cache.next_event().await.expect("cache writer returns its result");
        let _ = cache.handle_event(event);
    }
    let barrier = SqliteBatch::new(0, 0, |_: &Transaction<'_>| Ok(())).expect("barrier is within SQLite limits");
    let receipt = cache.db.as_ref().expect("cache database is open").submit_batches(vec![barrier]).expect("queue database barrier");
    receipt.wait().await.expect("database barrier completes");
    while let Ok(event) = cache.event_receiver.try_recv() {
        let _ = cache.handle_event(event);
    }
}

pub(super) async fn cache_count(db: &SqliteDb) -> usize {
    let receipt = db
        .read(32, |transaction| transaction.query_row("SELECT COUNT(*) FROM object_entries", [], |row| row.get::<_, i64>(0)))
        .expect("queue cache count");
    receipt.wait().await.expect("read cache count") as usize
}

pub(super) async fn cache_scope_entry_count(db: &SqliteDb, scope: &CacheScope) -> usize {
    let grid_id = scope.grid_id.clone();
    let region_id = scope.region_id.as_bytes().to_vec();
    let cache_id = scope.cache_id.as_bytes().to_vec();
    let receipt = db
        .read(64, move |transaction| {
            transaction.query_row(
                "SELECT COUNT(*) FROM object_entries WHERE grid_id = ?1 AND region_id = ?2 AND cache_id = ?3",
                rusqlite::params![grid_id, region_id, cache_id],
                |row| row.get::<_, i64>(0),
            )
        })
        .expect("queue cache scope count");
    usize::try_from(receipt.wait().await.expect("read cache scope count")).expect("cache row count fits usize")
}

pub(super) async fn cache_totals(db: &SqliteDb) -> (i64, i64, i64, i64) {
    let receipt = db
        .read(64, |transaction| {
            transaction.query_row(
                "SELECT (SELECT COUNT(*) FROM object_entries),
                        (SELECT COALESCE(SUM(length(payload)), 0) FROM object_entries),
                        entry_count,
                        payload_bytes
                 FROM object_cache_totals WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
        })
        .expect("queue cache totals");
    receipt.wait().await.expect("read cache totals")
}

pub(super) fn synthetic_region_updates() -> (protocol::messages::object::RegionData, Vec<Message>, Vec<CachedObjectReference>) {
    let Message::ObjectUpdate(template_update) = fixture_message(vectors::OBJECT_UPDATE) else {
        panic!("full update fixture should be an object update");
    };
    let mut region_data = template_update.region_data;
    region_data.region_handle = REGION_HANDLE;
    let template = template_update.objects.first().expect("full update fixture has an object").clone();
    let mut messages = Vec::with_capacity(20);
    let mut references = Vec::with_capacity(5_000);
    let mut objects = Vec::with_capacity(255);
    for local_id in 1..=5_000_u32 {
        let mut object = template.clone();
        object.local_id = local_id;
        object.id = Uuid::from_u128(u128::from(local_id));
        object.crc = local_id.wrapping_mul(31);
        references.push(CachedObjectReference { local_id, crc: object.crc, update_flags: object.update_flags });
        objects.push(object);
        if objects.len() == 255 {
            messages
                .push(Message::ObjectUpdate(FullUpdate { region_data, objects: std::mem::replace(&mut objects, Vec::with_capacity(255)) }));
        }
    }
    if !objects.is_empty() {
        messages.push(Message::ObjectUpdate(FullUpdate { region_data, objects }));
    }
    (region_data, messages, references)
}

pub(super) fn region_world() -> SessionWorld {
    let mut world = SessionWorld::new(viewer_world_limits());
    world.register_simulator(SIMULATOR, Some(RegionHandle::new(REGION_HANDLE))).expect("register synthetic region");
    world
}

pub(super) async fn measure_full_update_intake(cache: &mut ObjectCacheSession, messages: Vec<Message>) -> (std::time::Duration, usize) {
    let mut world = region_world();
    let started = Instant::now();
    for (index, message) in messages.into_iter().enumerate() {
        let sequence = u32::try_from(index + 1).expect("synthetic packet sequence fits u32");
        let order = OrderEvidence::Ordered(u64::from(sequence));
        let batch = decode_object_message(REGION, sequence, order, &message).expect("full update decodes into World operations");
        let summary = world.apply_object_batch_cooperatively(SIMULATOR, batch, |_, _, _| {}).await;
        assert!(summary.is_some(), "synthetic update applies to the registered region");
        cache.record_update(SIMULATOR, message);
    }
    let elapsed = started.elapsed();
    assert_eq!(world.world().object_count(), 5_000);
    (elapsed, world.world().object_count())
}
