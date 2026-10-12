use std::{collections::HashSet, num::NonZeroU32};

use OrderEvidence::{Degraded, Ordered};
use ParentUpdate::{Local, Root};
use base::UUID;
use math::{DVec3, Quat, Vec3};
use world::ApplyError::{EntityBudgetExhausted, InvalidValue, RegionRetired, RegistrationBudgetExhausted, StaleRegion};
use world::ApplyOutcome::{Deferred, Introduced, RegionAdopted, RegionPatched, TombstoneRecorded, Unchanged, Updated};
use world::DeferredReason::AmbiguousOrder;
use world::Kind::Object;
use world::{
    ApplyOutcome, FieldGroup, FieldRejection, FieldRejectionReason, LocalAddress, LocalTransform, ObjectObservation, ObjectPatch,
    OrderEvidence, ParentUpdate, Pose, Region, RegionHandle, RegionInfo, UpdateSource, World, WorldLimits,
};

fn limits() -> WorldLimits {
    WorldLimits {
        max_region_registrations: 2,
        max_entities: 2,
        max_entities_per_region: 2,
        max_tombstones_per_region: 4,
        max_tombstones: 8,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(4),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(8),
        max_journal_entries: 3,
        max_journal_changes: 32,
        max_region_name_bytes: 8,
        max_recovery_targets: 4,
    }
}

fn uuid(value: u128) -> UUID {
    UUID::from_u128(value)
}

fn address(region: Region, local_id: u32) -> LocalAddress {
    LocalAddress::new(region, NonZeroU32::new(local_id).expect("local ID must be nonzero"))
}

fn source(region: Region, order: u64) -> UpdateSource {
    UpdateSource::new(region, order as u32, Ordered(order), 0)
}

fn observation(source: UpdateSource, id: u128, local_id: u32) -> ObjectObservation {
    ObjectObservation {
        source,
        id: uuid(id),
        local_id: NonZeroU32::new(local_id).expect("local ID must be nonzero"),
        kind: Object,
        parent: Root,
        pose: None,
        linear_motion: None,
        angular_velocity: None,
        scale: None,
        attachment_point: None,
    }
}

#[test]
fn entity_handles_are_world_scoped() {
    let region = Region::new(6);
    let mut first_world = World::new(limits());
    let mut second_world = World::new(limits());
    assert_eq!(first_world.adopt_region(region).unwrap().outcome, RegionAdopted);
    assert_eq!(second_world.adopt_region(region).unwrap().outcome, RegionAdopted);

    let first = first_world.observe_object(observation(source(region, 1), 60, 1)).unwrap();
    let Introduced(first_entity) = first.outcome else { panic!("expected first world entity") };
    let second = second_world.observe_object(observation(source(region, 1), 61, 1)).unwrap();
    let Introduced(second_entity) = second.outcome else { panic!("expected second world entity") };

    assert_ne!(first_entity, second_entity);
    assert_eq!(first_world.object(first_entity).unwrap().identity().id(), uuid(60));
    assert_eq!(second_world.object(second_entity).unwrap().identity().id(), uuid(61));
    assert!(first_world.object(second_entity).is_none());
    assert!(second_world.object(first_entity).is_none());
}

#[test]
fn old_region_events_are_rejected_by_new_world() {
    let old_region = Region::new(30);
    let new_region = Region::new(31);
    let metadata = RegionInfo {
        region_id: Some(uuid(301)),
        handle: Some(RegionHandle::new((256_u64 << 32) | 512)),
        name: Some("Mauve".to_owned()),
        origin: Some(DVec3::new(256.0, 512.0, 0.0)),
    };

    let mut old_world = World::new(limits());
    assert_eq!(old_world.adopt_region(old_region).unwrap().outcome, RegionAdopted);
    assert_eq!(old_world.patch_region(old_region, metadata.clone()).unwrap().outcome, RegionPatched);
    let old_entity = old_world.observe_object(observation(source(old_region, 1), 302, 1)).unwrap();
    let Introduced(old_entity) = old_entity.outcome else { panic!("expected old-session object") };

    let mut new_world = World::new(limits());
    assert_eq!(new_world.adopt_region(new_region).unwrap().outcome, RegionAdopted);
    assert_eq!(new_world.patch_region(new_region, metadata.clone()).unwrap().outcome, RegionPatched);
    let new_entity = new_world.observe_object(observation(source(new_region, 1), 303, 1)).unwrap();
    let Introduced(new_entity) = new_entity.outcome else { panic!("expected new-session object") };
    let revision = new_world.revision();

    assert_eq!(new_world.observe_object(observation(source(old_region, 2), 304, 2)), Err(StaleRegion));
    assert_eq!(new_world.revision(), revision);
    assert_eq!(new_world.object_count(), 1);
    assert_eq!(new_world.entity(uuid(304)), None);
    assert_eq!(new_world.entity_at(address(old_region, 2)), None);
    assert_eq!(new_world.entity(uuid(303)), Some(new_entity));
    assert!(new_world.object(old_entity).is_none());
    assert_eq!(new_world.region(new_region), Some(&metadata));
}

#[test]
fn region_metadata_merges_monotonically() {
    let region = Region::new(7);
    let mut world = World::new(limits());

    let adopted = world.adopt_region(region).unwrap();
    assert_eq!(adopted.outcome, RegionAdopted);
    assert_eq!(adopted.revision, Some(world.revision()));
    assert_eq!(world.region(region), Some(&RegionInfo::default()));

    let info = RegionInfo {
        region_id: Some(uuid(1)),
        handle: Some(RegionHandle::new((512_u64 << 32) | 768)),
        name: Some("Test".to_owned()),
        origin: Some(DVec3::new(512.0, 768.0, 0.0)),
    };
    let patched = world.patch_region(region, info.clone()).unwrap();
    assert_eq!(patched.outcome, RegionPatched);
    assert_eq!(world.region(region), Some(&info));

    let revision = world.revision();
    let repeated = world.patch_region(region, info.clone()).unwrap();
    assert_eq!(repeated.outcome, Unchanged);
    assert_eq!(repeated.revision, None);
    assert_eq!(world.revision(), revision);

    let unchanged = world.adopt_region(region).unwrap();
    assert_eq!(unchanged.outcome, Unchanged);
    assert_eq!(unchanged.revision, None);
    assert_eq!(world.revision(), revision);
    assert_eq!(world.region(region), Some(&info));

    assert_eq!(world.patch_region(Region::new(99), RegionInfo::default()), Err(StaleRegion));
    assert_eq!(
        world.patch_region(Region::new(99), RegionInfo { name: Some("too long for this region".to_owned()), ..RegionInfo::default() }),
        Err(StaleRegion)
    );
}

#[test]
fn region_metadata_conflicts_are_atomic() {
    let region = Region::new(70);
    let mut world = World::new(limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let original = RegionInfo {
        region_id: Some(uuid(1)),
        handle: Some(RegionHandle::new((512_u64 << 32) | 768)),
        name: Some("Mauve".to_owned()),
        origin: Some(DVec3::new(512.0, 768.0, 0.0)),
    };
    assert_eq!(world.patch_region(region, original.clone()).unwrap().outcome, RegionPatched);
    let revision = world.revision();

    for (incoming, error) in [
        (RegionInfo { region_id: Some(uuid(2)), ..RegionInfo::default() }, InvalidValue { field: "region_id" }),
        (RegionInfo { handle: Some(RegionHandle::new(1)), ..RegionInfo::default() }, InvalidValue { field: "region_handle" }),
        (RegionInfo { origin: Some(DVec3::new(1.0, 2.0, 0.0)), ..RegionInfo::default() }, InvalidValue { field: "region_origin" }),
        (RegionInfo { name: Some("Other".to_owned()), ..RegionInfo::default() }, InvalidValue { field: "region_name" }),
    ] {
        assert_eq!(world.patch_region(region, incoming), Err(error));
        assert_eq!(world.region(region), Some(&original));
        assert_eq!(world.revision(), revision);
    }
}

#[test]
fn region_origin_matches_handle() {
    let region = Region::new(74);
    let handle = RegionHandle::new((512_u64 << 32) | 768);
    let wrong_origin = DVec3::new(512.0, 769.0, 0.0);
    let cases = [
        (None, RegionInfo { handle: Some(handle), origin: Some(wrong_origin), ..RegionInfo::default() }),
        (
            Some(RegionInfo { handle: Some(handle), ..RegionInfo::default() }),
            RegionInfo { origin: Some(wrong_origin), name: Some("Mauve".to_owned()), ..RegionInfo::default() },
        ),
        (
            Some(RegionInfo { origin: Some(wrong_origin), ..RegionInfo::default() }),
            RegionInfo { handle: Some(handle), name: Some("Mauve".to_owned()), ..RegionInfo::default() },
        ),
    ];

    for (initial, conflicting) in cases {
        let mut world = World::new(limits());
        assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
        if let Some(initial) = initial {
            assert_eq!(world.patch_region(region, initial).unwrap().outcome, RegionPatched);
        }
        let metadata = world.region(region).unwrap().clone();
        let revision = world.revision();

        assert_eq!(world.patch_region(region, conflicting), Err(InvalidValue { field: "region_origin" }));
        assert_eq!(world.region(region), Some(&metadata));
        assert_eq!(world.revision(), revision);
    }
}

#[test]
fn invalid_region_metadata_is_atomic() {
    let region = Region::new(71);
    let mut world = World::new(limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let revision = world.revision();

    for (info, error) in [
        (RegionInfo { region_id: Some(uuid(0)), ..RegionInfo::default() }, InvalidValue { field: "region_id" }),
        (RegionInfo { name: Some("Too long name".to_owned()), ..RegionInfo::default() }, InvalidValue { field: "region_name" }),
        (RegionInfo { origin: Some(DVec3::new(f64::NAN, 0.0, 0.0)), ..RegionInfo::default() }, InvalidValue { field: "region_origin" }),
    ] {
        assert_eq!(world.patch_region(region, info), Err(error));
        assert_eq!(world.region(region), Some(&RegionInfo::default()));
        assert_eq!(world.revision(), revision);
    }
}

#[test]
fn self_agent_cannot_be_reassigned() {
    let mut world = World::new(limits());
    let agent = uuid(80);

    assert_eq!(world.set_self_agent(uuid(0)), Err(InvalidValue { field: "agent_id" }));
    world.set_self_agent(agent).unwrap();
    assert_eq!(world.set_self_agent(uuid(81)), Err(InvalidValue { field: "agent_id" }));
    assert_eq!(world.set_self_agent(agent), Ok(()));
}

#[test]
fn region_coordinates_decode_correctly() {
    let region = Region::new(19);
    let address = address(region, 42);
    let packed = (512_u64 << 32) | 768;
    let handle = RegionHandle::new(packed);

    assert_eq!(region.get(), 19);
    assert_eq!(address.region(), region);
    assert_eq!(address.local_id(), NonZeroU32::new(42).unwrap());
    assert_eq!(handle.get(), packed);
    assert_eq!(handle.origin(), DVec3::new(512.0, 768.0, 0.0));
}

#[test]
fn local_transform_needs_all_components() {
    let region = Region::new(72);
    let mut world = World::new(limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let position = Vec3::new(1.0, 2.0, 3.0);
    let rotation = Quat::IDENTITY;
    let scale = Vec3::new(4.0, 5.0, 6.0);
    let mut initial = observation(source(region, 1), 72, 1);
    initial.pose = Some(Pose { position: Some(position), rotation: Some(rotation) });
    let introduced = world.observe_object(initial).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };
    assert_eq!(world.object(entity).unwrap().local_transform(), None);

    let mut patch = ObjectPatch::new(source(region, 2), NonZeroU32::new(1).unwrap());
    patch.scale = Some(scale);
    assert_eq!(world.patch_object(patch).unwrap().outcome, Updated(entity));

    assert_eq!(world.object(entity).unwrap().local_transform(), Some(LocalTransform { position, rotation, scale }));
}

#[test]
fn region_index_tracks_transfer() {
    let source_region = Region::new(75);
    let destination_region = Region::new(76);
    let mut world = World::new(limits());
    assert_eq!(world.adopt_region(source_region).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(destination_region).unwrap().outcome, RegionAdopted);

    let introduced = world.observe_object(observation(source(source_region, 1), 75, 1)).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };
    assert_eq!(world.objects_in_region(source_region).map(|object| object.entity()).collect::<HashSet<_>>(), [entity].into());

    let transferred = world.observe_object(observation(source(destination_region, 1), 75, 1)).unwrap();
    assert_eq!(transferred.outcome, Updated(entity));
    assert_eq!(world.entity_at(address(source_region, 1)), None);
    assert_eq!(world.entity_at(address(destination_region, 1)), Some(entity));
    assert_eq!(world.objects_in_region(source_region).count(), 0);
    assert_eq!(world.objects_in_region(destination_region).map(|object| object.entity()).collect::<HashSet<_>>(), [entity].into());
    assert_eq!(world.objects().map(|object| object.entity()).collect::<HashSet<_>>(), [entity].into());
}

#[test]
fn retirement_clears_objects_and_blocks_reuse() {
    let region = Region::new(8);
    let other_region = Region::new(9);
    let mut world_limits = limits();
    world_limits.max_region_registrations = 2;
    world_limits.max_entities = 3;
    world_limits.max_entities_per_region = 2;
    let mut world = World::new(world_limits);
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(other_region).unwrap().outcome, RegionAdopted);
    let shared_metadata = RegionInfo {
        region_id: Some(uuid(81)),
        handle: Some(RegionHandle::new((256_u64 << 32) | 512)),
        name: Some("Mauve".to_owned()),
        origin: Some(DVec3::new(256.0, 512.0, 0.0)),
    };
    assert_eq!(world.patch_region(region, shared_metadata.clone()).unwrap().outcome, RegionPatched);
    assert_eq!(world.patch_region(other_region, shared_metadata.clone()).unwrap().outcome, RegionPatched);
    let root = world.observe_object(observation(source(region, 1), 10, 3)).unwrap();
    let Introduced(root) = root.outcome else { panic!("expected root introduction") };
    let mut child_observation = observation(source(region, 2), 11, 4);
    child_observation.parent = Local(NonZeroU32::new(3).expect("local ID is nonzero"));
    let child = world.observe_object(child_observation).unwrap();
    let Introduced(child) = child.outcome else { panic!("expected child introduction") };
    let remaining = world.observe_object(observation(source(other_region, 1), 12, 3)).unwrap();
    let Introduced(remaining) = remaining.outcome else { panic!("expected other-region introduction") };
    assert_eq!(world.children(root).map(|object| object.entity()).collect::<Vec<_>>(), [child]);

    let retired = world.retire_region(region).unwrap();
    assert_eq!(retired.outcome, ApplyOutcome::RegionRetired { entities_removed: 2 });
    assert_eq!(retired.recovery_cancellations.into_iter().collect::<HashSet<_>>(), HashSet::from([address(region, 3), address(region, 4)]));
    assert_eq!(world.entity(uuid(10)), None);
    assert_eq!(world.entity(uuid(11)), None);
    assert_eq!(world.entity_at(address(region, 3)), None);
    assert_eq!(world.entity_at(address(region, 4)), None);
    assert!(world.object(root).is_none());
    assert!(world.object(child).is_none());
    assert_eq!(world.children(root).count(), 0);
    assert_eq!(world.children(child).count(), 0);
    assert_eq!(world.objects_in_region(region).count(), 0);
    assert_eq!(world.objects_in_region(other_region).count(), 1);
    assert_eq!(world.object_count(), 1);
    assert_eq!(world.entity(uuid(12)), Some(remaining));
    assert_eq!(world.entity_at(address(other_region, 3)), Some(remaining));
    assert_eq!(world.region(region), None);
    assert_eq!(world.region(other_region), Some(&shared_metadata));
    assert_eq!(world.retire_region(region).unwrap().outcome, Unchanged);
    assert_eq!(world.adopt_region(region), Err(RegionRetired));
    assert_eq!(world.observe_object(observation(source(region, 2), 11, 4)), Err(StaleRegion));
}

#[test]
fn world_registration_limits_are_enforced() {
    let region_a = Region::new(1);
    let region_b = Region::new(2);
    let mut world = World::new(WorldLimits {
        max_region_registrations: 1,
        max_entities: 1,
        max_entities_per_region: 1,
        max_tombstones_per_region: 1,
        max_tombstones: 1,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
        max_journal_entries: 1,
        max_journal_changes: 32,
        max_region_name_bytes: 1,
        max_recovery_targets: 4,
    });

    assert_eq!(world.adopt_region(region_a).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(region_b), Err(RegistrationBudgetExhausted));
    assert_eq!(
        world.patch_region(region_a, RegionInfo { name: Some("longer".to_owned()), ..RegionInfo::default() }),
        Err(InvalidValue { field: "region_name" })
    );

    assert!(matches!(world.observe_object(observation(source(region_a, 1), 20, 1)).unwrap().outcome, Introduced(_)));
    assert_eq!(world.observe_object(observation(source(region_a, 2), 21, 2)), Err(EntityBudgetExhausted));
}

#[test]
fn entity_budgets_are_independent() {
    let first_region = Region::new(40);
    let second_region = Region::new(41);
    let third_region = Region::new(42);
    let mut world_limits = limits();
    world_limits.max_region_registrations = 3;
    world_limits.max_entities = 2;
    world_limits.max_entities_per_region = 1;
    let mut world = World::new(world_limits);
    for region in [first_region, second_region, third_region] {
        assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    }

    let first = world.observe_object(observation(source(first_region, 1), 401, 1)).unwrap();
    let Introduced(first) = first.outcome else { panic!("expected first object") };
    let revision = world.revision();
    assert_eq!(world.observe_object(observation(source(first_region, 2), 402, 2)), Err(EntityBudgetExhausted));
    assert_eq!(world.revision(), revision);
    assert_eq!(world.object_count(), 1);
    assert_eq!(world.entity(uuid(402)), None);
    assert_eq!(world.entity_at(address(first_region, 2)), None);

    let second = world.observe_object(observation(source(second_region, 1), 403, 1)).unwrap();
    let Introduced(second) = second.outcome else { panic!("expected second-region object") };
    let revision = world.revision();
    assert_eq!(world.observe_object(observation(source(third_region, 1), 404, 1)), Err(EntityBudgetExhausted));
    assert_eq!(world.revision(), revision);
    assert_eq!(world.object_count(), 2);
    assert_eq!(world.entity(uuid(404)), None);
    assert_eq!(world.entity_at(address(third_region, 1)), None);
    assert_eq!(world.entity(uuid(401)), Some(first));
    assert_eq!(world.entity_at(address(first_region, 1)), Some(first));
    assert_eq!(world.entity(uuid(403)), Some(second));
    assert_eq!(world.entity_at(address(second_region, 1)), Some(second));
}

#[test]
fn invalid_observation_rejects_pose() {
    let region = Region::new(3);
    let mut world = World::new(limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let revision = world.revision();

    assert_eq!(world.observe_object(observation(source(region, 1), 0, 1)), Err(InvalidValue { field: "object_id" }));

    let mut invalid = observation(source(region, 2), 30, 1);
    invalid.pose = Some(Pose { position: Some(Vec3::new(f32::NAN, 0.0, 0.0)), rotation: None });
    let applied = world.observe_object(invalid).unwrap();
    let Introduced(entity) = applied.outcome else { panic!("expected identity introduction") };
    assert_eq!(
        applied.rejections.iter().copied().collect::<Vec<_>>(),
        [FieldRejection { group: FieldGroup::Pose, reason: FieldRejectionReason::InvalidValue { field: "position" } }]
    );
    assert_eq!(world.entity(uuid(30)), Some(entity));
    assert_eq!(world.object(entity).unwrap().position(), None);
    assert_eq!(world.revision().get(), revision.get() + 1);
}

#[test]
fn degraded_order_requests_recovery() {
    let region = Region::new(73);
    let mut world = World::new(limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let revision = world.revision();
    let source = UpdateSource::new(region, 12, Degraded, 3);
    assert_eq!(source.region(), region);
    assert_eq!(source.sequence(), 12);
    assert_eq!(source.order(), Degraded);
    assert_eq!(source.block_ordinal(), 3);

    let result = world.observe_object(observation(source, 73, 4)).unwrap();
    assert_eq!(result.outcome, Deferred(AmbiguousOrder));
    assert_eq!(result.recovery.unwrap().targets, [address(region, 4)]);
    assert_eq!(world.object_count(), 0);
    assert_eq!(world.revision(), revision);
}

#[test]
fn tombstone_byte_charge_saturates() {
    assert_eq!(WorldLimits::tombstone_bytes_for_count(usize::MAX), usize::MAX);
}

#[test]
fn zero_tombstone_budget_uses_region_floor() {
    for (region_id, max_tombstones, max_per_region) in [(90, 0, 1), (91, 1, 0)] {
        let region = Region::new(region_id);
        let mut world_limits = limits();
        world_limits.max_tombstones = max_tombstones;
        world_limits.max_tombstones_per_region = max_per_region;
        let mut world = World::new(world_limits);
        assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
        let killed_address = address(region, 1);

        assert_eq!(
            world.remove_object(source(region, 10), NonZeroU32::new(1).expect("nonzero local ID")).unwrap().outcome,
            TombstoneRecorded(killed_address)
        );

        let stale = world.observe_object(observation(source(region, 9), u128::from(region_id), 1)).unwrap();
        assert_eq!(stale.outcome, Deferred(AmbiguousOrder));
        assert_eq!(stale.recovery.unwrap().targets, [killed_address]);

        assert!(matches!(world.observe_object(observation(source(region, 11), u128::from(region_id), 1)).unwrap().outcome, Introduced(_)));
        assert_eq!(world.object_count(), 1);
    }
}

#[test]
fn tombstone_eviction_advances_region_floor() {
    let region = Region::new(9);
    let mut world = World::new(WorldLimits {
        max_region_registrations: 1,
        max_entities: 4,
        max_entities_per_region: 4,
        max_tombstones_per_region: 8,
        max_tombstones: 8,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
        max_journal_entries: 8,
        max_journal_changes: 32,
        max_region_name_bytes: 8,
        max_recovery_targets: 2,
    });
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let adopted_revision = world.revision();

    assert_eq!(world.remove_object(source(region, 5), NonZeroU32::new(1).unwrap()).unwrap().outcome, TombstoneRecorded(address(region, 1)));
    assert_eq!(
        world.remove_object(source(region, 10), NonZeroU32::new(2).unwrap()).unwrap().outcome,
        TombstoneRecorded(address(region, 2))
    );
    assert_eq!(world.revision(), adopted_revision);

    assert_eq!(world.observe_object(observation(source(region, 9), 90, 2)).unwrap().outcome, Unchanged);
    let stale_evicted = world.observe_object(observation(source(region, 5), 91, 1)).unwrap();
    assert_eq!(stale_evicted.outcome, Deferred(AmbiguousOrder));
    assert_eq!(stale_evicted.recovery.unwrap().targets, [address(region, 1)]);

    let stale_unbound = world.observe_object(observation(source(region, 5), 92, 3)).unwrap();
    assert_eq!(stale_unbound.outcome, Deferred(AmbiguousOrder));
    assert_eq!(stale_unbound.recovery.unwrap().targets, [address(region, 3)]);
    assert!(matches!(world.observe_object(observation(source(region, 11), 93, 3)).unwrap().outcome, Introduced(_)));
}

#[test]
fn tombstone_limits_are_per_region_and_evict_oldest() {
    let region_a = Region::new(10);
    let region_b = Region::new(11);
    let mut world = World::new(WorldLimits {
        max_region_registrations: 2,
        max_entities: 4,
        max_entities_per_region: 4,
        max_tombstones_per_region: 8,
        max_tombstones: 8,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(2),
        max_journal_entries: 8,
        max_journal_changes: 32,
        max_region_name_bytes: 8,
        max_recovery_targets: 2,
    });
    assert_eq!(world.adopt_region(region_a).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(region_b).unwrap().outcome, RegionAdopted);

    assert!(matches!(world.remove_object(source(region_a, 5), NonZeroU32::new(1).unwrap()).unwrap().outcome, TombstoneRecorded(_)));
    assert!(matches!(world.remove_object(source(region_b, 5), NonZeroU32::new(1).unwrap()).unwrap().outcome, TombstoneRecorded(_)));
    assert!(matches!(world.remove_object(source(region_a, 10), NonZeroU32::new(2).unwrap()).unwrap().outcome, TombstoneRecorded(_)));

    let evicted = world.observe_object(observation(source(region_a, 5), 100, 1)).unwrap();
    assert_eq!(evicted.outcome, Deferred(AmbiguousOrder));
    assert_eq!(evicted.recovery.unwrap().targets, [address(region_a, 1)]);
    assert_eq!(world.observe_object(observation(source(region_b, 5), 101, 1)).unwrap().outcome, Unchanged);
}

#[test]
fn tombstone_pressure_only_degrades_affected_region() {
    let region_a = Region::new(14);
    let region_b = Region::new(15);
    let mut world = World::new(WorldLimits {
        max_region_registrations: 2,
        max_entities: 4,
        max_entities_per_region: 4,
        max_tombstones_per_region: 4,
        max_tombstones: 1,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(4),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
        max_journal_entries: 8,
        max_journal_changes: 32,
        max_region_name_bytes: 8,
        max_recovery_targets: 2,
    });
    assert_eq!(world.adopt_region(region_a).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(region_b).unwrap().outcome, RegionAdopted);

    assert!(matches!(world.remove_object(source(region_a, 5), NonZeroU32::new(1).unwrap()).unwrap().outcome, TombstoneRecorded(_)));
    assert!(matches!(world.remove_object(source(region_b, 5), NonZeroU32::new(1).unwrap()).unwrap().outcome, TombstoneRecorded(_)));
    assert!(matches!(world.remove_object(source(region_b, 10), NonZeroU32::new(2).unwrap()).unwrap().outcome, TombstoneRecorded(_)));

    assert_eq!(world.observe_object(observation(source(region_a, 5), 140, 1)).unwrap().outcome, Unchanged);
    let degraded = world.observe_object(observation(source(region_b, 5), 141, 2)).unwrap();
    assert_eq!(degraded.outcome, Deferred(AmbiguousOrder));
    assert_eq!(degraded.recovery.unwrap().targets, [address(region_b, 2)]);
}

#[test]
fn region_retirement_keeps_registration_budget() {
    let first = Region::new(12);
    let second = Region::new(13);
    let mut world = World::new(WorldLimits {
        max_region_registrations: 1,
        max_entities: 1,
        max_entities_per_region: 1,
        max_tombstones_per_region: 1,
        max_tombstones: 1,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
        max_journal_entries: 4,
        max_journal_changes: 32,
        max_region_name_bytes: 8,
        max_recovery_targets: 1,
    });

    assert_eq!(world.adopt_region(first).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(first).unwrap().outcome, Unchanged);
    assert!(matches!(world.remove_object(source(first, 1), NonZeroU32::new(1).unwrap()).unwrap().outcome, TombstoneRecorded(_)));
    let retired = world.retire_region(first).unwrap();
    assert_eq!(retired.recovery, None);
    assert_eq!(world.adopt_region(second), Err(RegistrationBudgetExhausted));
}
