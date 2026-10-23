use std::{future::pending, num::NonZeroU32};

use futures::{join, poll};
use uuid::Uuid;
use world::{Kind, ObjectDetailResult, ObjectObservation, OrderEvidence, ParentUpdate, Region, UpdateSource};

use super::*;
use world::{EnvironmentAssignment, EnvironmentScope, EnvironmentSource, MAX_ENVIRONMENT_SOURCE_BYTES, ParcelId, WorldLimits};

fn world() -> World {
    World::new(WorldLimits {
        max_region_registrations: 8,
        max_entities: 16,
        max_entities_per_region: 16,
        max_tombstones_per_region: 16,
        max_tombstones: 16,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(16),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(16),
        max_journal_entries: 32,
        max_journal_changes: 128,
        max_region_name_bytes: 256,
        max_recovery_targets: 8,
    })
}

async fn deliver<T>(read: impl Future<Output = Result<T, ReadError>>, service: &mut WorldReadHost, world: &World) -> Result<T, ReadError> {
    join!(read, async {
        let Err(request) = service.select(pending::<()>()).await else { unreachable!() };
        request.serve(world);
    })
    .0
}

fn insert(world: &mut World, region: Region, id: u128, local: u32, order: u64) -> Entity {
    let observation = ObjectObservation::new(
        UpdateSource::new(region, order as u32, OrderEvidence::Ordered(order), 0),
        Uuid::from_u128(id),
        NonZeroU32::new(local).unwrap(),
        Kind::Object,
        ParentUpdate::Root,
    );
    let _ = world.observe_object(observation).unwrap();
    world.entity(Uuid::from_u128(id)).unwrap()
}

#[tokio::test]
async fn environment_reads_fit_admission() {
    let mut world = world();
    let region = Region::new(1);
    let _ = world.adopt_region(region).unwrap();
    let source = UpdateSource::new(region, 1, OrderEvidence::Ordered(1), 0);
    let parcel = ParcelId::new(42).unwrap();
    let _ = world.observe_agent_parcel(source, Some(parcel)).unwrap();
    for scope in [EnvironmentScope::Region, EnvironmentScope::Parcel(parcel)] {
        let target = world.environment_target(region, scope).unwrap();
        let assignment = EnvironmentAssignment {
            source: EnvironmentSource::Inline(vec![0; MAX_ENVIRONMENT_SOURCE_BYTES].into()),
            version: Some(1),
            day_length_seconds: None,
            day_offset_seconds: None,
            altitudes: None,
        };
        let _ = world.observe_environment(target, source, assignment).unwrap();
    }
    let mut service = WorldReadHost::new(&world, Handle::current());
    let reader = service.reader();
    let read = deliver(reader.environment(region), &mut service, &world).await.unwrap().unwrap();
    assert_eq!(read.region, region);
    assert_eq!(read.hydration_revision, world.revision());
    assert_eq!(read, world.environment(region).unwrap());
    assert!(read.retained_bytes() <= MAX_ENVIRONMENT_READ_BYTES);
    let _ = world.retire_region(region).unwrap();
    assert_eq!(deliver(reader.environment(region), &mut service, &world).await.unwrap(), None);
    assert!(
        matches!(read.region_assignment.unwrap().source, EnvironmentSource::Inline(source) if source.len() == MAX_ENVIRONMENT_SOURCE_BYTES)
    );
}

#[tokio::test]
async fn pages_preserve_identity_and_cursors() {
    let mut world = world();
    let region = Region::new(1);
    let _ = world.adopt_region(region).unwrap();
    let old = insert(&mut world, region, 20, 1, 1);
    insert(&mut world, region, 30, 2, 2);
    let mut service = WorldReadHost::new(&world, Handle::current());
    let reader = service.reader();
    let barrier = deliver(reader.barrier(), &mut service, &world).await.unwrap();
    assert_eq!(barrier.object_count, 2);
    let first = deliver(reader.objects(None, 1), &mut service, &world).await.unwrap();
    assert_eq!(first.objects[0].id, Uuid::from_u128(20));
    insert(&mut world, region, 10, 3, 3);
    let next = deliver(reader.objects(first.next, 1), &mut service, &world).await.unwrap();
    assert_eq!(next.objects[0].id, Uuid::from_u128(30));
    assert_eq!(next.hydration_revision, world.revision());
    assert!(next.next.is_none());
    let source = UpdateSource::new(region, 4, OrderEvidence::Ordered(4), 0);
    let _ = world.remove_object(source, NonZeroU32::new(1).unwrap()).unwrap();
    let replacement = insert(&mut world, region, 20, 1, 5);
    assert_ne!(old, replacement);
    let details = deliver(reader.details(&[old, replacement]), &mut service, &world).await.unwrap();
    assert_eq!(details.hydration_revision, world.revision());
    assert!(matches!(&details.details[..], [ObjectDetailResult::Gone(entity), ObjectDetailResult::Found(detail)]
            if *entity == old && detail.summary.entity == replacement));
    let regions = deliver(reader.regions(None, 64), &mut service, &world).await.unwrap();
    assert_eq!(regions.regions.len(), 1);
    assert_eq!(regions.regions[0].region, region);
    assert_eq!(regions.hydration_revision, world.revision());
    let source = UpdateSource::new(region, 10, OrderEvidence::Ordered(10), 0);
    let patch = world::TerrainPatch::new(0, 0, Box::new([0.0; TERRAIN_PATCH_SAMPLES])).unwrap();
    let _ = world.observe_terrain_patch(source, patch).unwrap();
    let _ = world.observe_water_height(source, 0.0).unwrap();
    let terrain = deliver(reader.terrain(region, None, 8), &mut service, &world).await.unwrap().unwrap();
    assert_eq!(terrain, world.terrain_page(region, None, 8).unwrap().unwrap());
    assert_eq!(terrain.patches[0].heights(), &[0.0; TERRAIN_PATCH_SAMPLES]);
    assert_eq!(terrain.water_height, Some(0.0));
    let barrier = deliver(reader.barrier(), &mut service, &world).await.unwrap();
    let _ = world.retire_region(region).unwrap();
    assert_eq!(deliver(reader.terrain(region, None, 8), &mut service, &world).await, Ok(None));
    assert_eq!(terrain.patches[0].heights()[0], 0.0, "delivered terrain remains owned after retirement");
    let mut cursor = barrier.changes;
    let mut partial = false;
    loop {
        let page = deliver(reader.changes(cursor, 1), &mut service, &world).await.unwrap();
        assert_eq!(page, world.change_page(cursor, 1).unwrap(), "bridge must preserve the complete journal contract");
        partial |= page.has_more && page.completed_revision < page.observed_revision;
        cursor = page.cursor;
        if !page.has_more {
            assert_eq!(page.completed_revision, world.revision());
            break;
        }
    }
    assert!(partial, "region retirement must span a logical revision across pages");
    let other = self::world();
    assert_eq!(
        deliver(reader.changes(other.read_barrier().changes, 1), &mut service, &world).await,
        Err(ReadError::World(WorldReadError::StaleSession))
    );
    assert_eq!(reader.objects(None, 257).await, Err(ReadError::World(WorldReadError::InvalidLimit)));
}

#[tokio::test]
async fn delivery_releases_admission() {
    let world = world();
    let mut service = WorldReadHost::new(&world, Handle::current());
    let reader = service.reader();
    let mut pending = (0..MAX_OUTSTANDING_READS).map(|_| Box::pin(reader.barrier())).collect::<Vec<_>>();
    for read in &mut pending {
        assert!(poll!(read.as_mut()).is_pending());
    }
    assert_eq!(reader.barrier().await, Err(ReadError::Busy));
    let request = service.next().await;
    request.serve(&world);
    assert_eq!(reader.barrier().await, Err(ReadError::Busy), "a queued reply still owns admission");
    let delivered = pending.remove(0).await.unwrap();
    let mut admitted = Box::pin(reader.barrier());
    assert!(poll!(admitted.as_mut()).is_pending(), "delivery releases admission even while the value is retained");
    assert_eq!(delivered.object_count, 0);
    drop(pending);
    drop(admitted);
    while let Ok(request) = service.receiver.try_recv() {
        request.serve(&world);
    }
    assert!(deliver(reader.barrier(), &mut service, &world).await.is_ok());
}

#[tokio::test]
async fn reply_bytes_limit_admission() {
    let world = world();
    let mut service = WorldReadHost::new(&world, Handle::current());
    let reader = service.reader();
    let mut pending = Vec::new();
    for _ in 0..3 {
        let mut page = Box::pin(reader.regions(None, MAX_REGION_PAGE_ITEMS));
        assert!(poll!(page.as_mut()).is_pending());
        service.next().await.serve(&world);
        pending.push(page);
    }
    assert_eq!(reader.regions(None, MAX_REGION_PAGE_ITEMS).await, Err(ReadError::Busy));
    let _ = pending.remove(0).await.unwrap();
    assert!(deliver(reader.regions(None, MAX_REGION_PAGE_ITEMS), &mut service, &world).await.is_ok());
}

#[tokio::test(start_paused = true)]
async fn deadlines_and_retirement_reject_stale() {
    let world = world();
    let mut service = WorldReadHost::new(&world, Handle::current());
    let reader = service.reader();
    let mut timed_out = Box::pin(reader.barrier());
    assert!(poll!(timed_out.as_mut()).is_pending());
    tokio::time::advance(READ_TIMEOUT).await;
    assert_eq!(timed_out.await, Err(ReadError::Busy));
    service.next().await.serve(&world);
    let mut late = Box::pin(reader.barrier());
    assert!(poll!(late.as_mut()).is_pending());
    service.next().await.serve(&world);
    let mut queued = Box::pin(reader.barrier());
    assert!(poll!(queued.as_mut()).is_pending());
    let mut observed = reader.clone();
    let mut changed = Box::pin(observed.changed());
    assert!(poll!(changed.as_mut()).is_pending());
    service.finish();
    assert_eq!(late.await, Err(ReadError::StaleSession), "a completed reply cannot escape after session retirement");
    assert_eq!(changed.await, Err(ReadError::StaleSession));
    assert_eq!(queued.await, Err(ReadError::StaleSession));
    assert_eq!(reader.barrier().await, Err(ReadError::StaleSession));
    let service = WorldReadHost::new(&world, Handle::current());
    let reader = service.reader();
    let mut interrupted = Box::pin(reader.barrier());
    assert!(poll!(interrupted.as_mut()).is_pending());
    drop(service);
    assert_eq!(interrupted.await, Err(ReadError::Shutdown));
}

#[tokio::test]
async fn revisions_coalesce_updates() {
    let mut world = world();
    let service = WorldReadHost::new(&world, Handle::current());
    let mut reader = service.reader();
    for i in 1..=3 {
        let _ = world.adopt_region(Region::new(i)).unwrap();
        service.publish(&world);
    }
    assert_eq!(reader.changed().await.unwrap(), world.revision());
    assert!(poll!(Box::pin(reader.changed()).as_mut()).is_pending());
}
