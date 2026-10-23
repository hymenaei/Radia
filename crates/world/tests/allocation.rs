use std::num::NonZeroU32;

use math::{Quat, Vec3};
use testing::allocation::{CountingAllocator, live_bytes, measure};
use uuid::Uuid;
use world::{
    Appearance,
    ApplyOutcome::Introduced,
    FieldUpdate,
    Kind::Object,
    MAX_MEDIA_URL_BYTES, MAX_MEDIA_WHITELIST_BYTES, MAX_MEDIA_WHITELIST_ENTRIES, MAX_OBJECT_DETAIL_BYTES, MAX_OBJECT_MEDIA_READ_BYTES,
    MAX_QUERY_STATE_MAINTENANCE_WORK, MAX_SURFACE_FACES, MediaEntry, ObjectMediaUpdate, ObjectObservation, ObjectPatch,
    OrderEvidence::Ordered,
    ParentUpdate::{Local, Root},
    Pose, Region, UpdateSource, World, WorldLimits,
};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn world_limits() -> WorldLimits {
    WorldLimits {
        max_region_registrations: 8,
        max_entities: 65_536,
        max_entities_per_region: 10_000,
        max_tombstones_per_region: 4_096,
        max_tombstones: 16_384,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(4_096),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(16_384),
        max_journal_entries: 4_096,
        max_journal_changes: 65_536,
        max_region_name_bytes: 256,
        max_recovery_targets: 256,
    }
}

#[test]
fn world_memory_baseline() {
    for count in [40_000_usize, 65_536] {
        let regions = (0..count.div_ceil(10_000)).map(|index| Region::new(index as u64 + 1)).collect::<Vec<_>>();
        let baseline = live_bytes();
        let (world, allocations) = measure(|| {
            let limits = world_limits();
            let mut world = World::new(limits);
            for region in &regions {
                let _ = world.adopt_region(*region).expect("adopt benchmark region");
            }
            for index in 0..count {
                let region_index = index / 10_000;
                let local_id = NonZeroU32::new((index % 10_000 + 1) as u32).expect("nonzero local ID");
                let parent = if local_id.get() == 1 { Root } else { Local(NonZeroU32::new(1).expect("root local ID")) };
                let mut observation = ObjectObservation::new(
                    UpdateSource::new(regions[region_index], 100_001 + index as u32, Ordered(100_001 + index as u64), 0),
                    Uuid::from_u128(index as u128 + 1),
                    local_id,
                    Object,
                    parent,
                );
                observation.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
                observation.scale = Some(Vec3::ONE);
                assert!(matches!(world.observe_object(observation).expect("insert benchmark object").outcome, Introduced(_)));
            }
            loop {
                let maintenance = world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).expect("maintain query state");
                if maintenance.dirty_scopes == 0 {
                    break;
                }
            }
            world
        });

        assert_eq!(world.object_count(), count);
        assert!(allocations.live_bytes > 0);
        assert!(allocations.peak_bytes >= allocations.live_bytes);
        assert!(allocations.allocation_calls > 0);
        let mut world = world;
        let (removed, retirement) = measure(|| {
            let mut removed = 0;
            for region in &regions {
                let applied = world.retire_region(*region).expect("retire benchmark region");
                let world::RegionOutcome::Retired { entities_removed, .. } = applied.outcome else { panic!("expected region retirement") };
                removed += entities_removed;
            }
            removed
        });
        assert_eq!(removed, count);
        assert_eq!(world.object_count(), 0);
        drop(world);
        let remaining_bytes = live_bytes();
        assert_eq!(
            remaining_bytes,
            baseline,
            "{count} objects left {} B allocated after World drop; retained={} B, peak={} B, allocations={}, reallocations={}, retirement allocations={}, retirement reallocations={}, retirement peak additional={} B",
            remaining_bytes - baseline,
            allocations.live_bytes,
            allocations.peak_bytes,
            allocations.allocation_calls,
            allocations.reallocation_calls,
            retirement.allocation_calls,
            retirement.reallocation_calls,
            retirement.peak_bytes,
        );
        eprintln!(
            "{count} linked objects: retained={} B, peak={} B, allocations={}, reallocations={}, retained/object={:.1} B",
            allocations.live_bytes,
            allocations.peak_bytes,
            allocations.allocation_calls,
            allocations.reallocation_calls,
            allocations.live_bytes as f64 / count as f64,
        );
        eprintln!(
            "{count} linked object retirements: allocations={}, reallocations={}, peak additional={} B",
            retirement.allocation_calls, retirement.reallocation_calls, retirement.peak_bytes,
        );
    }
}

#[test]
fn media_snapshot_copy_stays_within_its_budget() {
    let mut world = World::new(world_limits());
    let region = Region::new(1);
    let _ = world.adopt_region(region).unwrap();
    let local_id = NonZeroU32::new(1).unwrap();
    let source = |order| UpdateSource::new(region, order as u32, Ordered(order), 0);
    let observation = ObjectObservation {
        appearance: FieldUpdate::Set(Appearance::Surface),
        ..ObjectObservation::new(source(1), Uuid::from_u128(1), local_id, Object, Root)
    };
    let Introduced(entity) = world.observe_object(observation).unwrap().outcome else { panic!("object introduced") };
    let (_, before) = measure(|| world.object_detail(entity).unwrap());
    let entry = MediaEntry {
        current_url: "x".repeat(MAX_MEDIA_URL_BYTES),
        home_url: "y".repeat(MAX_MEDIA_URL_BYTES),
        whitelist: vec!["z".repeat(MAX_MEDIA_WHITELIST_BYTES / MAX_MEDIA_WHITELIST_ENTRIES); MAX_MEDIA_WHITELIST_ENTRIES],
        ..MediaEntry::default()
    };
    let media = ObjectMediaUpdate::new(1, vec![Some(entry); MAX_SURFACE_FACES]).unwrap();
    let _ = world.patch_object(ObjectPatch { object_media: Some(media), ..ObjectPatch::new(source(2), local_id) }).unwrap();

    let (snapshot, media) = measure(|| world.object_media(entity).unwrap());
    let (_, after) = measure(|| world.object_detail(entity).unwrap());
    assert_eq!(snapshot.media.as_ref().unwrap().faces().count(), MAX_SURFACE_FACES);
    assert!(media.live_bytes > MAX_OBJECT_DETAIL_BYTES as i128, "fixture exceeds the core detail budget");
    assert!(media.peak_bytes + size_of_val(&snapshot) as i128 <= MAX_OBJECT_MEDIA_READ_BYTES as i128, "{media:?}");
    assert_eq!(after.peak_bytes, before.peak_bytes, "core detail copies do not grow with media size");
}
