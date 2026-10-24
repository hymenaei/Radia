use std::sync::Arc;

use utils::AssetId;
use uuid::Uuid;
use world::{
    ApplyError, EnvironmentAssignment, EnvironmentScope, EnvironmentSource, MAX_ENVIRONMENT_BYTES, MAX_ENVIRONMENT_SOURCE_BYTES,
    OrderEvidence, ParcelId, Region, RegionOutcome, UpdateSource, World, WorldChange, WorldLimits,
};

fn new_world() -> World {
    let mut world = World::new(WorldLimits {
        max_region_registrations: 16,
        max_entities: 8,
        max_entities_per_region: 8,
        max_tombstones_per_region: 8,
        max_tombstones: 8,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(8),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(8),
        max_journal_entries: 64,
        max_journal_changes: 128,
        max_region_name_bytes: 32,
        max_recovery_targets: 4,
    });
    let _ = world.adopt_region(Region::new(1)).unwrap();
    world
}

fn source(region: u64, order: u32) -> UpdateSource {
    UpdateSource::new(Region::new(region), order, OrderEvidence::Ordered(u64::from(order)), 0)
}

fn assignment(source: EnvironmentSource, version: u32) -> EnvironmentAssignment {
    EnvironmentAssignment {
        source,
        version: Some(version),
        day_length_seconds: Some(14_400),
        day_offset_seconds: Some(-3600),
        altitudes: Some([1000.0, 2000.0, 3000.0]),
    }
}

#[test]
fn assignments_share_source_and_journal() {
    let mut world = new_world();
    let region = Region::new(1);
    assert_eq!(world.environment(region).unwrap().region_assignment, None);
    let target = world.environment_target(region, EnvironmentScope::Region).unwrap();
    let barrier = world.read_barrier();
    let bytes: Arc<[u8]> = Arc::from(&b"{'type':'daycycle'}"[..]);
    let expected = assignment(EnvironmentSource::Inline(bytes.clone()), 4);
    let _ = world.observe_environment(target, source(1, 1), expected.clone()).unwrap();
    let read = world.environment(region).unwrap();
    assert_eq!(read.hydration_revision, world.revision());
    assert_eq!(read.region_assignment, Some(expected));
    let EnvironmentSource::Inline(shared) = read.region_assignment.unwrap().source else { panic!("inline source missing") };
    assert!(Arc::ptr_eq(&bytes, &shared), "World reads share encoded sources without copying");
    let changes = world.change_page(barrier.changes, 8).unwrap();
    assert_eq!(changes.records.len(), 1);
    assert_eq!(changes.records[0].change, WorldChange::EnvironmentUpdated(region));
    let _ = world.observe_environment(target, source(1, 2), assignment(EnvironmentSource::Inherit, 5)).unwrap();
    assert_eq!(world.environment(region).unwrap().region_assignment.unwrap().source, EnvironmentSource::Inherit);
    assert_eq!(&*shared, &*bytes, "owned reads survive source replacement");
}

#[test]
fn parcel_changes_reject_stale_results() {
    let mut world = new_world();
    let region = Region::new(1);
    let a = ParcelId::new(10).unwrap();
    let b = ParcelId::new(20).unwrap();
    let scope = EnvironmentScope::Parcel(a);
    assert_eq!(ParcelId::new(-1), None);
    assert_eq!(world.environment_target(region, scope), None);
    let _ = world.observe_agent_parcel(source(1, 1), Some(a)).unwrap();
    let old = world.environment_target(region, scope).unwrap();
    let _ = world.observe_environment(old, source(1, 2), assignment(EnvironmentSource::Inherit, 1)).unwrap();
    let _ = world.observe_agent_parcel(source(1, 3), Some(b)).unwrap();
    let _ = world.observe_agent_parcel(source(1, 4), Some(a)).unwrap();
    let current = world.environment_target(region, scope).unwrap();
    assert_ne!(current, old, "returning to a parcel creates a new observation generation");
    assert_eq!(
        world.observe_environment(old, source(1, 5), assignment(EnvironmentSource::Inherit, 2)).unwrap().outcome,
        RegionOutcome::Unchanged
    );
    assert_eq!(world.environment(region).unwrap().parcel.unwrap().assignment, None);
    let revision = world.revision();
    let _ = world.observe_agent_parcel(source(1, 6), Some(a)).unwrap();
    let _ = world.observe_agent_parcel(source(1, 5), Some(b)).unwrap();
    assert_eq!(world.revision(), revision, "same-parcel observations advance ordering without publishing changes");
    assert_eq!(world.environment_target(region, scope), Some(current));
    let mut other = new_world();
    assert_eq!(
        other.observe_environment(current, source(1, 7), assignment(EnvironmentSource::Inherit, 3)).unwrap().outcome,
        RegionOutcome::Unchanged
    );
    let _ = world.retire_region(region).unwrap();
    assert_eq!(world.environment(region), None);
    assert_eq!(
        world.observe_environment(current, source(1, 8), assignment(EnvironmentSource::Inherit, 3)).unwrap().outcome,
        RegionOutcome::Unchanged
    );
}

#[test]
fn versions_and_failures_are_atomic() {
    let mut world = new_world();
    let region = Region::new(1);
    let target = world.environment_target(region, EnvironmentScope::Region).unwrap();
    let original = assignment(EnvironmentSource::Inherit, 10);
    let _ = world.observe_environment(target, source(1, 10), original.clone()).unwrap();
    let revision = world.revision();
    for (order, version) in [(9, 11), (11, 9)] {
        assert_eq!(
            world.observe_environment(target, source(1, order), assignment(EnvironmentSource::Inherit, version)).unwrap().outcome,
            RegionOutcome::Unchanged
        );
    }
    let mut conflict = original.clone();
    conflict.day_offset_seconds = Some(0);
    assert_eq!(world.observe_environment(target, source(1, 12), conflict).unwrap().outcome, RegionOutcome::Unchanged);
    let _ = world.observe_environment(target, source(1, 20), original.clone()).unwrap();
    assert_eq!(
        world.observe_environment(target, source(1, 19), assignment(EnvironmentSource::Inherit, 11)).unwrap().outcome,
        RegionOutcome::Unchanged
    );
    for invalid in [
        EnvironmentSource::Inline(Arc::from([])),
        EnvironmentSource::Inline(vec![0; MAX_ENVIRONMENT_SOURCE_BYTES + 1].into()),
        EnvironmentSource::Asset(AssetId::new(Uuid::nil())),
    ] {
        assert!(matches!(world.observe_environment(target, source(1, 30), assignment(invalid, 11)), Err(ApplyError::InvalidValue { .. })));
        assert_eq!(world.environment(region).unwrap().region_assignment, Some(original.clone()));
        assert_eq!(world.revision(), revision);
    }
    assert!(matches!(
        world.observe_environment(target, source(2, 30), original.clone()),
        Err(ApplyError::InvalidValue { field: "environment_region" })
    ));
    let mut invalid = original;
    invalid.altitudes = Some([f32::NAN, 0.0, 0.0]);
    assert!(matches!(
        world.observe_environment(target, source(1, 30), invalid),
        Err(ApplyError::InvalidValue { field: "environment_altitudes" })
    ));
    let _ = world.observe_environment(target, source(1, 21), assignment(EnvironmentSource::Inherit, 11)).unwrap();
    assert!(world.revision() > revision, "rejected input cannot advance ordering fences");
}

#[test]
fn retirement_releases_source_capacity() {
    let mut world = new_world();
    let bytes: Arc<[u8]> = vec![0; MAX_ENVIRONMENT_SOURCE_BYTES].into();
    let count = MAX_ENVIRONMENT_BYTES / MAX_ENVIRONMENT_SOURCE_BYTES;
    for id in 1..=count as u64 + 1 {
        let region = Region::new(id);
        let _ = world.adopt_region(region).unwrap();
        let target = world.environment_target(region, EnvironmentScope::Region).unwrap();
        let result = world.observe_environment(target, source(id, 1), assignment(EnvironmentSource::Inline(bytes.clone()), 1));
        if id <= count as u64 {
            let _ = result.unwrap();
        } else {
            assert_eq!(result.unwrap_err(), ApplyError::EnvironmentBudgetExhausted);
            assert_eq!(world.environment(region).unwrap().region_assignment, None);
        }
    }
    let _ = world.retire_region(Region::new(1)).unwrap();
    let id = count as u64 + 1;
    let target = world.environment_target(Region::new(id), EnvironmentScope::Region).unwrap();
    let _ = world.observe_environment(target, source(id, 1), assignment(EnvironmentSource::Inline(bytes), 1)).unwrap();
    assert!(world.environment(Region::new(id)).unwrap().region_assignment.is_some());
}
