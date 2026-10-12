use std::{collections::HashSet, num::NonZeroU32};

use base::UUID;
use math::{Quat, Vec3};
use world::ApplyError::{InvalidValue, KindMismatch};
use world::ApplyOutcome::{
    AgentHandedOff, Deferred, Introduced, RegionAdopted, RegionPatched, Removed, TombstoneRecorded, Unchanged, Updated,
};
use world::DeferredReason::{AmbiguousOrder, UnknownObject};
use world::FieldGroup;
use world::FieldGroup::{AngularVelocity, Scale};
use world::FieldRejectionReason::{InvalidRotation, ParentCycle, ParentDependsOnRejectedParent, UntrustedSource};
use world::Kind::{Avatar, Object, Unknown};
use world::OrderEvidence::{Ambiguous, Degraded, Ordered};
use world::Parent;
use world::Parent::{Pending, Resolved};
use world::ParentUpdate::{Local, Root};
use world::{
    AgentHandoff, FieldRejection, FieldRejectionReason, Kind, LinearMotion, LocalAddress, MotionPatch, ObjectObservation, ObjectPatch,
    ParentUpdate, Pose, PropertiesPatch, Region, RegionInfo, SitTarget, UpdateAuthority, UpdateSource, World, WorldLimits,
};

fn limits() -> WorldLimits {
    WorldLimits {
        max_region_registrations: 4,
        max_entities: 8,
        max_entities_per_region: 8,
        max_tombstones_per_region: 4,
        max_tombstones: 8,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(4),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(8),
        max_journal_entries: 16,
        max_journal_changes: 64,
        max_region_name_bytes: 64,
        max_recovery_targets: 4,
    }
}

fn uuid(value: u128) -> UUID {
    UUID::from_u128(value)
}

fn local_id(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("local ID must be nonzero")
}

fn address(region: Region, local_id_value: u32) -> LocalAddress {
    LocalAddress::new(region, local_id(local_id_value))
}

fn source(region: Region, order: u64) -> UpdateSource {
    ordered(region, order, 0)
}

fn ordered(region: Region, order: u64, block_ordinal: u32) -> UpdateSource {
    UpdateSource::new(region, order as u32, Ordered(order), block_ordinal)
}

fn untracked(region: Region, order: u64) -> UpdateSource {
    source(region, order).with_authority(UpdateAuthority::Untracked)
}

fn object(source: UpdateSource, id: u128, local_id_value: u32, kind: Kind, parent: ParentUpdate) -> ObjectObservation {
    ObjectObservation {
        source,
        id: uuid(id),
        local_id: local_id(local_id_value),
        kind,
        parent,
        pose: None,
        linear_motion: None,
        angular_velocity: None,
        scale: None,
        attachment_point: None,
    }
}

fn with_region(region: Region) -> World {
    let mut world = World::new(limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    world
}

#[test]
fn object_count_tracks_live_objects() {
    let region = Region::new(1);
    let mut world = with_region(region);
    assert_eq!(world.object_count(), 0);

    assert!(matches!(world.observe_object(object(source(region, 1), 1, 1, Object, Root)).unwrap().outcome, Introduced(_)));
    assert!(matches!(world.observe_object(object(source(region, 2), 2, 2, Object, Root)).unwrap().outcome, Introduced(_)));
    assert_eq!(world.object_count(), 2);

    assert!(matches!(world.observe_object(object(source(region, 3), 1, 3, Object, Root)).unwrap().outcome, Updated(_)));
    assert_eq!(world.object_count(), 2);

    assert_eq!(world.remove_object(source(region, 4), local_id(3)).unwrap().outcome, Removed(uuid(1)));
    assert_eq!(world.object_count(), 1);
}

#[test]
fn self_parented_introduction_is_atomic() {
    let region = Region::new(42);
    let mut world = with_region(region);
    let revision = world.revision();
    let id = uuid(420);
    let addr = address(region, 1);

    let result = world.observe_object(object(source(region, 1), 420, 1, Object, Local(local_id(1))));

    assert_eq!(result, Err(InvalidValue { field: "parent_cycle" }));
    assert_eq!(world.revision(), revision);
    assert_eq!(world.entity(id), None);
    assert_eq!(world.entity_at(addr), None);
    assert_eq!(world.objects_in_region(region).count(), 0);
}

#[test]
fn live_binding_precedes_region_floor() {
    let region = Region::new(43);
    let mut world_limits = limits();
    world_limits.max_tombstones_per_region = 1;
    world_limits.max_tombstones = 1;
    world_limits.max_tombstone_bytes_per_region = WorldLimits::tombstone_bytes_for_count(1);
    world_limits.max_tombstone_bytes = WorldLimits::tombstone_bytes_for_count(1);
    let mut world = World::new(world_limits);
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let addr = address(region, 1);
    let created = world.observe_object(object(source(region, 10), 430, 1, Object, Root)).unwrap();
    let Introduced(entity) = created.outcome else { panic!("expected object introduction") };

    assert_eq!(world.remove_object(source(region, 100), local_id(2)).unwrap().outcome, TombstoneRecorded(address(region, 2)));
    assert_eq!(world.remove_object(source(region, 200), local_id(3)).unwrap().outcome, TombstoneRecorded(address(region, 3)));

    let mut update = object(source(region, 50), 430, 1, Object, Root);
    update.scale = Some(Vec3::splat(2.0));
    let applied = world.observe_object(update).unwrap();

    assert_eq!(applied.outcome, Updated(entity));
    assert_eq!(world.entity_at(addr), Some(entity));
    assert_eq!(world.object(entity).unwrap().scale(), Some(Vec3::splat(2.0)));
}

#[test]
fn all_invalid_patch_is_a_noop() {
    let region = Region::new(44);
    let mut world = with_region(region);
    let id = uuid(440);
    let mut initial = object(source(region, 1), 440, 1, Object, Root);
    initial.pose = Some(Pose { position: Some(Vec3::X), rotation: Some(Quat::IDENTITY) });
    initial.linear_motion = Some(LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::Y) });
    initial.angular_velocity = Some(Vec3::Z);
    initial.scale = Some(Vec3::ONE);
    let created = world.observe_object(initial).unwrap();
    let Introduced(entity) = created.outcome else { panic!("expected object introduction") };
    let revision = world.revision();

    let patch = ObjectPatch {
        pose: Some(Pose { position: Some(Vec3::splat(f32::NAN)), rotation: None }),
        linear_motion: MotionPatch { velocity: Some(Vec3::splat(f32::INFINITY)), acceleration: None },
        angular_velocity: Some(Vec3::splat(f32::NAN)),
        scale: Some(Vec3::splat(f32::NAN)),
        ..ObjectPatch::new(source(region, 2), local_id(1))
    };
    let applied = world.patch_object(patch).unwrap();

    assert_eq!(applied.outcome, Unchanged);
    assert_eq!(applied.revision, None);
    assert_eq!(world.revision(), revision);
    assert_eq!(
        applied.rejections.iter().copied().collect::<Vec<_>>(),
        [
            FieldRejection { group: FieldGroup::Pose, reason: FieldRejectionReason::InvalidValue { field: "position" } },
            FieldRejection { group: FieldGroup::LinearMotion, reason: FieldRejectionReason::InvalidValue { field: "velocity" } },
            FieldRejection { group: AngularVelocity, reason: FieldRejectionReason::InvalidValue { field: "angular_velocity" } },
            FieldRejection { group: Scale, reason: FieldRejectionReason::InvalidValue { field: "scale" } },
        ]
    );
    assert_eq!(applied.rejections.len(), 4);
    assert!(!applied.rejections.is_empty());
    let current = world.object(entity).unwrap();
    assert_eq!(current.identity().id(), id);
    assert_eq!(current.position(), Some(Vec3::X));
    assert_eq!(current.rotation(), Some(Quat::IDENTITY));
    assert_eq!(current.linear_motion(), LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::Y) });
    assert_eq!(current.angular_velocity(), Some(Vec3::Z));
    assert_eq!(current.scale(), Some(Vec3::ONE));
}

#[test]
fn sit_target_clear_respects_freshness() {
    let region = Region::new(47);
    let mut world = with_region(region);
    let entity = match world.observe_object(object(source(region, 1), 470, 1, Object, Root)).unwrap().outcome {
        Introduced(entity) => entity,
        other => panic!("expected object introduction, got {other:?}"),
    };
    let initial = SitTarget { position: Vec3::X, rotation: Quat::IDENTITY };
    assert_eq!(world.patch_sit_target(source(region, 10), uuid(470), Some(initial)).unwrap().outcome, Updated(entity));

    let stale = SitTarget { position: Vec3::Y, rotation: Quat::IDENTITY };
    assert_eq!(world.patch_sit_target(source(region, 9), uuid(470), Some(stale)).unwrap().outcome, Unchanged);
    assert_eq!(world.object(entity).unwrap().sit_target(), Some(initial));

    assert_eq!(world.patch_sit_target(source(region, 11), uuid(470), None).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().sit_target(), None);
    let revision = world.revision();
    assert_eq!(world.patch_sit_target(source(region, 12), uuid(470), None).unwrap().outcome, Unchanged);
    assert_eq!(world.revision(), revision);
}

#[test]
fn sit_target_respects_tracked_authority() {
    let region = Region::new(48);
    let mut world = with_region(region);
    let entity = match world.observe_object(object(source(region, 1), 480, 1, Object, Root)).unwrap().outcome {
        Introduced(entity) => entity,
        other => panic!("expected object introduction, got {other:?}"),
    };
    let tracked = SitTarget { position: Vec3::X, rotation: Quat::IDENTITY };
    assert_eq!(world.patch_sit_target(source(region, 2), uuid(480), Some(tracked)).unwrap().outcome, Updated(entity));

    let untrusted_target = SitTarget { position: Vec3::Y, rotation: Quat::IDENTITY };
    let applied = world.patch_sit_target(untracked(region, 1_000), uuid(480), Some(untrusted_target)).unwrap();

    assert_eq!(applied.outcome, Unchanged);
    assert_eq!(
        applied.rejections.iter().copied().collect::<Vec<_>>(),
        [FieldRejection { group: FieldGroup::SitTarget, reason: UntrustedSource }]
    );
    assert_eq!(world.object(entity).unwrap().sit_target(), Some(tracked));
}

#[test]
fn sit_target_defers_unknown_and_unordered() {
    let region = Region::new(49);
    let mut world = with_region(region);
    let target = SitTarget { position: Vec3::X, rotation: Quat::IDENTITY };

    let unknown = world.patch_sit_target(source(region, 1), uuid(490), Some(target)).unwrap();
    assert_eq!(unknown.outcome, Deferred(UnknownObject));
    assert_eq!(unknown.recovery, None);

    let entity = match world.observe_object(object(source(region, 2), 490, 1, Object, Root)).unwrap().outcome {
        Introduced(entity) => entity,
        other => panic!("expected object introduction, got {other:?}"),
    };
    let uncertain = UpdateSource::new(region, 3, Degraded, 0);
    let applied = world.patch_sit_target(uncertain, uuid(490), Some(target)).unwrap();

    assert_eq!(applied.outcome, Deferred(AmbiguousOrder));
    assert_eq!(applied.recovery.unwrap().targets, [address(region, 1)]);
    assert_eq!(world.object(entity).unwrap().sit_target(), None);
}

#[test]
fn local_ids_are_region_scoped() {
    let first_region = Region::new(22);
    let second_region = Region::new(23);
    let mut world = World::new(limits());
    assert_eq!(world.adopt_region(first_region).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(second_region).unwrap().outcome, RegionAdopted);

    let first = world.observe_object(object(source(first_region, 1), 204, 1, Object, Root)).unwrap();
    let Introduced(first_entity) = first.outcome else { panic!("expected first object introduction") };
    let second = world.observe_object(object(source(second_region, 1), 205, 1, Object, Root)).unwrap();
    let Introduced(second_entity) = second.outcome else { panic!("expected second object introduction") };

    assert_ne!(first_entity, second_entity);
    assert_eq!(world.entity_at(address(first_region, 1)), Some(first_entity));
    assert_eq!(world.entity_at(address(second_region, 1)), Some(second_entity));
    assert_eq!(world.objects_in_region(first_region).count(), 1);
    assert_eq!(world.objects_in_region(second_region).count(), 1);
}

#[test]
fn unknown_pcode_is_preserved() {
    let region = Region::new(24);
    let mut world = with_region(region);
    let result = world.observe_object(object(source(region, 1), 206, 1, Unknown(231), Root)).unwrap();
    let Introduced(entity) = result.outcome else { panic!("expected object introduction") };

    assert_eq!(world.object(entity).unwrap().kind(), Unknown(231));
}

#[test]
fn stale_kind_mismatch_is_rejected() {
    let region = Region::new(25);
    let mut world = with_region(region);
    let added = world.observe_object(object(source(region, 10), 250, 1, Object, Root)).unwrap();
    let Introduced(entity) = added.outcome else { panic!("expected object introduction") };
    let revision = world.revision();

    assert_eq!(
        world.observe_object(object(source(region, 9), 250, 1, Avatar, Root)),
        Err(KindMismatch { current: Object, observed: Avatar })
    );
    assert_eq!(world.revision(), revision);
    assert_eq!(world.entity(uuid(250)), Some(entity));
    assert_eq!(world.object(entity).unwrap().kind(), Object);
}

#[test]
fn shape_and_appearance_are_independent() {
    let region = Region::new(25);
    let mut world = with_region(region);
    let result = world.observe_object(object(source(region, 1), 207, 1, Unknown(231), Root)).unwrap();
    let Introduced(entity) = result.outcome else { panic!("expected object introduction") };
    let object = world.object(entity).unwrap();

    assert_eq!(object.kind(), Unknown(231));
    assert_eq!(object.shape(), None);
    assert_eq!(object.appearance(), None);
}

#[test]
fn world_queries_follow_lifecycle() {
    let first_region = Region::new(20);
    let second_region = Region::new(21);
    let mut world = World::new(limits());
    assert_eq!(world.adopt_region(first_region).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(second_region).unwrap().outcome, RegionAdopted);
    assert_eq!(
        world.patch_region(first_region, RegionInfo { name: Some("First".to_owned()), ..RegionInfo::default() }).unwrap().outcome,
        RegionPatched
    );
    assert_eq!(
        world.patch_region(second_region, RegionInfo { name: Some("Second".to_owned()), ..RegionInfo::default() }).unwrap().outcome,
        RegionPatched
    );

    let parent = world.observe_object(object(source(first_region, 1), 200, 1, Object, Root)).unwrap();
    let Introduced(parent_entity) = parent.outcome else { panic!("expected parent introduction") };
    let child = world.observe_object(object(source(first_region, 2), 201, 2, Object, Local(local_id(1)))).unwrap();
    let Introduced(child_entity) = child.outcome else { panic!("expected child introduction") };
    let avatar = world.observe_object(object(source(second_region, 1), 202, 3, Avatar, Root)).unwrap();
    let Introduced(avatar_entity) = avatar.outcome else { panic!("expected avatar introduction") };
    let other_object = world.observe_object(object(source(second_region, 2), 203, 4, Object, Root)).unwrap();
    let Introduced(other_entity) = other_object.outcome else { panic!("expected object introduction") };

    assert_eq!(world.object(child_entity).unwrap().parent(), Resolved(parent_entity));
    assert_eq!(world.object_count(), 4);
    assert_eq!(world.objects().count(), 4);
    assert_eq!(
        world.objects_in_region(first_region).map(|object| object.identity().id()).collect::<HashSet<_>>(),
        [uuid(200), uuid(201)].into()
    );
    assert_eq!(
        world.objects_in_region(second_region).map(|object| object.identity().id()).collect::<HashSet<_>>(),
        [uuid(202), uuid(203)].into()
    );
    assert_eq!(world.objects_in_region(Region::new(99)).count(), 0);
    assert_eq!(world.children(parent_entity).map(|object| object.identity().id()).collect::<HashSet<_>>(), [uuid(201)].into());
    assert_eq!(world.children(child_entity).count(), 0);
    assert_eq!(world.avatars().map(|object| object.identity().id()).collect::<HashSet<_>>(), [uuid(202)].into());
    assert_eq!(world.regions().count(), 2);
    assert!(world.regions().any(|(region, info)| region == first_region && info.name.as_deref() == Some("First")));
    assert!(world.regions().any(|(region, info)| region == second_region && info.name.as_deref() == Some("Second")));

    let detach = ObjectPatch { parent: Some(Root), ..ObjectPatch::new(source(first_region, 3), local_id(2)) };
    assert_eq!(world.patch_object(detach).unwrap().outcome, Updated(child_entity));
    assert_eq!(world.object(child_entity).unwrap().parent(), Parent::Root);
    assert_eq!(world.children(parent_entity).count(), 0);

    assert_eq!(world.remove_object(source(second_region, 3), local_id(4)).unwrap().outcome, Removed(uuid(203)));
    assert!(world.object(other_entity).is_none());
    assert_eq!(world.entity(uuid(203)), None);
    assert_eq!(world.objects_in_region(second_region).map(|object| object.identity().id()).collect::<HashSet<_>>(), [uuid(202)].into());
    assert_eq!(world.avatars().map(|object| object.identity().id()).collect::<HashSet<_>>(), [uuid(202)].into());

    let replacement = world.observe_object(object(source(second_region, 4), 203, 4, Object, Root)).unwrap();
    let Introduced(replacement_entity) = replacement.outcome else { panic!("expected replacement introduction") };
    assert_ne!(other_entity, replacement_entity);
    assert!(world.object(other_entity).is_none());
    assert_eq!(world.entity(uuid(203)), Some(replacement_entity));
    assert_eq!(world.object_count(), 4);
    assert_eq!(
        world.objects_in_region(second_region).map(|object| object.identity().id()).collect::<HashSet<_>>(),
        [uuid(202), uuid(203)].into()
    );
    assert_eq!(world.object(avatar_entity).unwrap().identity().id(), uuid(202));
}

#[test]
fn replacement_pends_cascaded_parent() {
    let region = Region::new(1);
    let mut world = with_region(region);
    world.set_self_agent(uuid(1)).unwrap();
    let avatar = world.observe_object(object(source(region, 0), 1, 1, Avatar, Root)).unwrap();
    let Introduced(avatar) = avatar.outcome else { panic!("expected avatar introduction") };

    let child = world.observe_object(object(UpdateSource::new(region, 1, Ordered(0), 0), 2, 272, Object, Local(local_id(1)))).unwrap();
    let Introduced(child) = child.outcome else { panic!("expected child introduction") };
    let applied = world.observe_object(object(UpdateSource::new(region, 2, Ordered(0), 32), 3, 1, Object, Local(local_id(272)))).unwrap();
    assert_eq!(applied.recovery.unwrap().targets, vec![address(region, 272)]);
    let Introduced(replacement) = applied.outcome else { panic!("expected replacement introduction") };

    assert!(world.object(avatar).is_none());
    assert!(world.object(child).is_none());
    assert_eq!(world.object(replacement).unwrap().parent(), Pending { address: address(region, 272), expected_uuid: Some(uuid(2)) });
    assert!(world.children(child).next().is_none());
}

#[test]
fn replacement_keeps_newer_target() {
    let region = Region::new(1);
    let mut world = with_region(region);
    let root = world.observe_object(object(source(region, 0), 1, 1, Object, Root)).unwrap();
    let Introduced(root) = root.outcome else { panic!("expected root introduction") };
    let child = world.observe_object(object(source(region, 10), 2, 272, Object, Local(local_id(1)))).unwrap();
    let Introduced(child) = child.outcome else { panic!("expected child introduction") };

    let replacement =
        world.observe_object(object(UpdateSource::new(region, 11, Ordered(5), 32), 3, 1, Object, Local(local_id(272)))).unwrap();
    let Introduced(replacement) = replacement.outcome else { panic!("expected replacement introduction") };

    assert!(world.object(root).is_none());
    assert_eq!(world.object(child).unwrap().parent(), Pending { address: address(region, 1), expected_uuid: Some(uuid(1)) });
    assert_eq!(world.object(replacement).unwrap().parent(), Resolved(child));
    assert_eq!(world.children(child).map(|object| object.entity()).collect::<Vec<_>>(), vec![replacement]);
}

#[test]
fn replacement_stops_at_avatar() {
    let region = Region::new(1);
    let mut world = with_region(region);
    let root = world.observe_object(object(source(region, 0), 1, 1, Object, Root)).unwrap();
    let Introduced(root) = root.outcome else { panic!("expected root introduction") };
    let avatar = world.observe_object(object(source(region, 0), 2, 272, Avatar, Local(local_id(1)))).unwrap();
    let Introduced(avatar) = avatar.outcome else { panic!("expected avatar introduction") };
    let target = world.observe_object(object(source(region, 0), 3, 273, Object, Local(local_id(272)))).unwrap();
    let Introduced(target) = target.outcome else { panic!("expected target introduction") };

    let replacement =
        world.observe_object(object(UpdateSource::new(region, 1, Ordered(1), 0), 4, 1, Object, Local(local_id(273)))).unwrap();
    let Introduced(replacement) = replacement.outcome else { panic!("expected replacement introduction") };

    assert!(world.object(root).is_none());
    assert_eq!(world.object(avatar).unwrap().parent(), Parent::Root);
    assert_eq!(world.object(target).unwrap().parent(), Resolved(avatar));
    assert_eq!(world.object(replacement).unwrap().parent(), Resolved(target));
}

#[test]
fn partial_patch_merge_and_ordering() {
    let region = Region::new(1);
    let mut world = with_region(region);
    let mut initial = object(source(region, 10), 1, 10, Object, Root);
    initial.pose = Some(Pose { position: Some(Vec3::new(1.0, 2.0, 3.0)), rotation: None });
    initial.scale = Some(Vec3::splat(2.0));
    initial.linear_motion = Some(LinearMotion { velocity: Some(Vec3::X), acceleration: None });
    let created = world.observe_object(initial).unwrap();
    let Introduced(entity) = created.outcome else { panic!("expected introduction") };
    assert!(matches!(
        world
            .patch_properties(source(region, 11), uuid(1), PropertiesPatch { owner_id: Some(UUID::nil()), creator_id: None })
            .unwrap()
            .outcome,
        Updated(_)
    ));
    {
        let current = world.object(entity).unwrap();
        assert_eq!(current.position(), Some(Vec3::new(1.0, 2.0, 3.0)));
        assert_eq!(current.rotation(), None);
        assert_eq!(current.scale(), Some(Vec3::splat(2.0)));
        assert_eq!(current.local_transform(), None);
        assert_eq!(current.linear_motion().velocity, Some(Vec3::X));
        assert_eq!(current.properties().owner_id, Some(UUID::nil()));
    }

    let rotation = Quat::from_rotation_y(0.5);
    let patch = ObjectPatch {
        pose: Some(Pose { position: None, rotation: Some(rotation) }),
        linear_motion: MotionPatch { velocity: Some(Vec3::new(4.0, 0.0, 0.0)), acceleration: None },
        scale: Some(Vec3::splat(3.0)),
        ..ObjectPatch::new(source(region, 12), local_id(10))
    };
    assert_eq!(world.patch_object(patch).unwrap().outcome, Updated(entity));
    {
        let current = world.object(entity).unwrap();
        assert_eq!(current.transform().position(), Some(Vec3::new(1.0, 2.0, 3.0)));
        assert_eq!(current.transform().rotation(), Some(rotation));
        assert_eq!(current.transform().scale(), Some(Vec3::splat(3.0)));
        assert_eq!(current.linear_motion().velocity, Some(Vec3::new(4.0, 0.0, 0.0)));
    }

    let revision = world.revision();
    let stale = ObjectPatch {
        pose: Some(Pose { position: Some(Vec3::splat(9.0)), rotation: None }),
        linear_motion: MotionPatch { velocity: Some(Vec3::splat(9.0)), acceleration: None },
        ..ObjectPatch::new(source(region, 11), local_id(10))
    };
    assert_eq!(world.patch_object(stale).unwrap().outcome, Unchanged);
    assert_eq!(world.revision(), revision);

    let next_block = ObjectPatch { angular_velocity: Some(Vec3::Y), ..ObjectPatch::new(ordered(region, 12, 1), local_id(10)) };
    assert_eq!(world.patch_object(next_block).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().angular_velocity(), Some(Vec3::Y));

    let removed = world.remove_object(source(region, 13), local_id(10)).unwrap();
    assert_eq!(removed.outcome, Removed(uuid(1)));
    assert_eq!(world.entity(uuid(1)), None);
    assert_eq!(world.remove_object(source(region, 12), local_id(10)).unwrap().outcome, Unchanged);
}

#[test]
fn invalid_rotation_does_not_advance_pose() {
    let region = Region::new(27);
    let mut world = with_region(region);
    let mut initial = object(source(region, 100), 270, 1, Object, Root);
    initial.pose = Some(Pose { position: Some(Vec3::X), rotation: Some(Quat::IDENTITY) });
    initial.scale = Some(Vec3::ONE);
    let introduced = world.observe_object(initial).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected introduction") };

    let mut invalid = object(source(region, 200), 270, 1, Object, Root);
    invalid.pose = Some(Pose { position: Some(Vec3::Y), rotation: Some(Quat::from_xyzw(0.0, 0.0, 0.0, 1.2)) });
    invalid.scale = Some(Vec3::splat(2.0));
    let applied = world.observe_object(invalid).unwrap();
    assert_eq!(applied.outcome, Updated(entity));
    assert_eq!(
        applied.rejections.iter().copied().collect::<Vec<_>>(),
        [FieldRejection { group: FieldGroup::Pose, reason: InvalidRotation }]
    );
    assert_eq!(world.object(entity).unwrap().position(), Some(Vec3::X));
    assert_eq!(world.object(entity).unwrap().rotation(), Some(Quat::IDENTITY));
    assert_eq!(world.object(entity).unwrap().scale(), Some(Vec3::splat(2.0)));

    let mut older_pose = object(source(region, 150), 270, 1, Object, Root);
    older_pose.pose = Some(Pose { position: Some(Vec3::Y), rotation: None });
    assert_eq!(world.observe_object(older_pose).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().position(), Some(Vec3::Y));
}

#[test]
fn quaternion_sign_and_freshness() {
    let region = Region::new(28);
    let mut world = with_region(region);
    let rotation = Quat::from_rotation_y(0.25);
    let negative = Quat::from_xyzw(-rotation.x, -rotation.y, -rotation.z, -rotation.w);
    let mut initial = object(source(region, 1), 280, 1, Object, Root);
    initial.pose = Some(Pose { position: Some(Vec3::X), rotation: Some(rotation) });
    let introduced = world.observe_object(initial).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected introduction") };

    let revision = world.revision();
    let sign_only =
        ObjectPatch { pose: Some(Pose { position: None, rotation: Some(negative) }), ..ObjectPatch::new(source(region, 2), local_id(1)) };
    let applied = world.patch_object(sign_only).unwrap();
    assert_eq!(applied.outcome, Unchanged);
    assert_eq!(applied.revision, None);
    assert_eq!(world.revision(), revision);
    assert_eq!(world.object(entity).unwrap().rotation(), Some(rotation));

    let between_updates = ObjectPatch {
        pose: Some(Pose { position: None, rotation: Some(Quat::IDENTITY) }),
        ..ObjectPatch::new(ordered(region, 1, 1), local_id(1))
    };
    assert_eq!(world.patch_object(between_updates).unwrap().outcome, Unchanged);
    assert_eq!(world.object(entity).unwrap().rotation(), Some(rotation));

    let with_position = ObjectPatch {
        pose: Some(Pose { position: Some(Vec3::Y), rotation: Some(negative) }),
        ..ObjectPatch::new(source(region, 3), local_id(1))
    };
    let applied = world.patch_object(with_position).unwrap();
    assert_eq!(applied.outcome, Updated(entity));
    assert_eq!(applied.revision.unwrap().get(), revision.get() + 1);
    assert_eq!(world.object(entity).unwrap().rotation(), Some(rotation));

    let near_rotation = Quat::from_rotation_y(0.25001);
    let stale_rotation = ObjectPatch {
        pose: Some(Pose { position: None, rotation: Some(near_rotation) }),
        ..ObjectPatch::new(source(region, 2), local_id(1))
    };
    assert_eq!(world.patch_object(stale_rotation).unwrap().outcome, Unchanged);
    let fresh_rotation = ObjectPatch {
        pose: Some(Pose { position: None, rotation: Some(near_rotation) }),
        ..ObjectPatch::new(source(region, 4), local_id(1))
    };
    assert_eq!(world.patch_object(fresh_rotation).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().rotation(), Some(near_rotation));
}

#[test]
fn partial_sign_flip_changes_rotation() {
    let region = Region::new(31);
    let mut world = with_region(region);
    let rotation = Quat::from_rotation_y(0.25);
    let mut initial = object(source(region, 1), 310, 1, Object, Root);
    initial.pose = Some(Pose { position: Some(Vec3::X), rotation: Some(rotation) });
    let introduced = world.observe_object(initial).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected introduction") };

    let partial_flip = Quat::from_xyzw(rotation.x, -rotation.y, rotation.z, rotation.w);
    let revision = world.revision();
    let patch = ObjectPatch {
        pose: Some(Pose { position: None, rotation: Some(partial_flip) }),
        ..ObjectPatch::new(source(region, 2), local_id(1))
    };

    assert_eq!(world.patch_object(patch).unwrap().outcome, Updated(entity));
    assert_eq!(world.revision().get(), revision.get() + 1);
    assert_ne!(world.object(entity).unwrap().rotation(), Some(rotation));
}

#[test]
fn rounded_quaternion_sign_preserves_pose() {
    let region = Region::new(29);
    let mut world = with_region(region);
    let mut initial = object(source(region, 1), 290, 1, Object, Root);
    initial.pose = Some(Pose { position: Some(Vec3::X), rotation: Some(Quat::IDENTITY) });
    let introduced = world.observe_object(initial).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected introduction") };

    let axis = Vec3::new(1.0, 2.0, 3.0).normalize();
    let rotation = (1..=2048)
        .find_map(|step| {
            let next = Quat::from_axis_angle(axis, step as f32 * 0.00137);
            let _applied = world
                .patch_object(ObjectPatch {
                    pose: Some(Pose { position: None, rotation: Some(next) }),
                    ..ObjectPatch::new(source(region, step + 1), local_id(1))
                })
                .expect("apply valid rotation");
            let stored = world.object(entity).unwrap().rotation().unwrap();
            (stored.length_squared() != 1.0 && stored.normalize() != stored).then_some(stored)
        })
        .expect("find a normalized f32 quaternion with rounding residue");
    let revision = world.revision();
    let negative = Quat::from_xyzw(-rotation.x, -rotation.y, -rotation.z, -rotation.w);

    let sign_only = ObjectPatch {
        pose: Some(Pose { position: None, rotation: Some(negative) }),
        ..ObjectPatch::new(source(region, 3000), local_id(1))
    };
    let applied = world.patch_object(sign_only).unwrap();

    assert_eq!(applied.outcome, Unchanged);
    assert_eq!(applied.revision, None);
    assert_eq!(world.revision(), revision);
    assert_eq!(world.object(entity).unwrap().rotation(), Some(rotation));

    let older = ObjectPatch {
        pose: Some(Pose { position: None, rotation: Some(Quat::IDENTITY) }),
        ..ObjectPatch::new(source(region, 2999), local_id(1))
    };
    assert_eq!(world.patch_object(older).unwrap().outcome, Unchanged);
    assert_eq!(world.object(entity).unwrap().rotation(), Some(rotation));
}

#[test]
fn equal_packet_blocks_keep_order() {
    let region = Region::new(35);
    let mut world = with_region(region);
    let mut initial = object(source(region, 1), 350, 1, Object, Root);
    initial.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
    let introduced = world.observe_object(initial).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected introduction") };

    let later_block = ObjectPatch {
        pose: Some(Pose { position: Some(Vec3::Z), rotation: None }),
        ..ObjectPatch::new(ordered(region, 2, 1), local_id(1))
    };
    let earlier_block = ObjectPatch {
        pose: Some(Pose { position: Some(Vec3::Y), rotation: None }),
        ..ObjectPatch::new(ordered(region, 2, 0), local_id(1))
    };
    assert!(matches!(world.patch_object(later_block).unwrap().outcome, Updated(_)));
    assert_eq!(world.patch_object(earlier_block).unwrap().outcome, Unchanged);
    assert_eq!(world.object(entity).unwrap().position(), Some(Vec3::Z));
}

#[test]
fn cycle_barrier_blocks_older_parent() {
    let region = Region::new(29);
    let mut world = with_region(region);
    let mut first = object(source(region, 1), 290, 1, Object, Root);
    first.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
    let introduced = world.observe_object(first).unwrap();
    let Introduced(first_entity) = introduced.outcome else { panic!("expected first object") };
    let second = world.observe_object(object(source(region, 2), 291, 2, Object, Local(local_id(1)))).unwrap();
    let Introduced(second_entity) = second.outcome else { panic!("expected second object") };
    let third = world.observe_object(object(source(region, 3), 292, 3, Object, Root)).unwrap();
    let Introduced(third_entity) = third.outcome else { panic!("expected third object") };

    let revision = world.revision();
    let cyclic = ObjectPatch {
        parent: Some(Local(local_id(2))),
        pose: Some(Pose { position: Some(Vec3::Y), rotation: None }),
        scale: Some(Vec3::splat(2.0)),
        ..ObjectPatch::new(source(region, 50), local_id(1))
    };
    let applied = world.patch_object(cyclic).unwrap();
    assert_eq!(applied.outcome, Updated(first_entity));
    assert_eq!(applied.revision.unwrap().get(), revision.get() + 1);
    assert_eq!(
        applied.rejections.iter().copied().collect::<Vec<_>>(),
        [
            FieldRejection { group: FieldGroup::Pose, reason: ParentDependsOnRejectedParent },
            FieldRejection { group: FieldGroup::Parent, reason: ParentCycle },
        ]
    );
    assert_eq!(world.object(first_entity).unwrap().parent(), Parent::Root);
    assert_eq!(world.object(first_entity).unwrap().position(), Some(Vec3::X));
    assert_eq!(world.object(first_entity).unwrap().scale(), Some(Vec3::splat(2.0)));
    assert_eq!(world.object(second_entity).unwrap().parent(), Resolved(first_entity));

    let delayed = ObjectPatch { parent: Some(Local(local_id(3))), ..ObjectPatch::new(source(region, 40), local_id(1)) };
    let applied = world.patch_object(delayed).unwrap();
    assert_eq!(applied.outcome, Unchanged);
    assert_eq!(world.object(first_entity).unwrap().parent(), Parent::Root);

    let fresh = ObjectPatch { parent: Some(Local(local_id(3))), ..ObjectPatch::new(source(region, 60), local_id(1)) };
    assert_eq!(world.patch_object(fresh).unwrap().outcome, Updated(first_entity));
    assert_eq!(world.object(first_entity).unwrap().parent(), Resolved(third_entity));
}

#[test]
fn transfer_ignores_previous_region_cycle_barrier() {
    let first_region = Region::new(70);
    let second_region = Region::new(71);
    let mut world = with_region(first_region);
    assert_eq!(world.adopt_region(second_region).unwrap().outcome, RegionAdopted);

    let first_id = 700;
    let parent_id = 701;
    let first = world.observe_object(object(source(first_region, 1), first_id, 1, Object, Root)).unwrap();
    let Introduced(first_entity) = first.outcome else { panic!("expected first object") };
    let parent = world.observe_object(object(source(first_region, 2), parent_id, 2, Object, Local(local_id(1)))).unwrap();
    let Introduced(parent_entity) = parent.outcome else { panic!("expected parent object") };

    let cycle = ObjectPatch { parent: Some(Local(local_id(2))), ..ObjectPatch::new(source(first_region, 200), local_id(1)) };
    let rejected = world.patch_object(cycle).unwrap();
    assert_eq!(
        rejected.rejections.iter().copied().collect::<Vec<_>>(),
        [FieldRejection { group: FieldGroup::Parent, reason: ParentCycle }]
    );

    let transfer = object(source(second_region, 1), first_id, 1, Object, Root);
    assert_eq!(world.observe_object(transfer).unwrap().outcome, Updated(first_entity));

    assert_eq!(world.object(first_entity).unwrap().address(), address(second_region, 1));
    assert_eq!(world.object(first_entity).unwrap().parent(), Parent::Root);
    assert_eq!(
        world.object(parent_entity).unwrap().parent(),
        Pending { address: address(first_region, 1), expected_uuid: Some(uuid(first_id)) }
    );
}

#[test]
fn observed_parent_cycle_is_rejected() {
    let region = Region::new(30);
    let mut world = with_region(region);
    let first = world.observe_object(object(source(region, 1), 300, 1, Object, Root)).unwrap();
    let Introduced(first_entity) = first.outcome else { panic!("expected first object") };
    let second = world.observe_object(object(source(region, 2), 301, 2, Object, Local(local_id(1)))).unwrap();
    let Introduced(second_entity) = second.outcome else { panic!("expected second object") };
    let revision = world.revision();

    let cyclic = world.observe_object(object(source(region, 3), 300, 1, Object, Local(local_id(2)))).unwrap();

    assert_eq!(cyclic.outcome, Unchanged);
    assert_eq!(cyclic.revision, None);
    assert_eq!(cyclic.rejections.iter().copied().collect::<Vec<_>>(), [FieldRejection { group: FieldGroup::Parent, reason: ParentCycle }]);
    assert_eq!(world.revision(), revision);
    assert_eq!(world.object(first_entity).unwrap().parent(), Parent::Root);
    assert_eq!(world.object(second_entity).unwrap().parent(), Resolved(first_entity));
}

#[test]
fn untracked_updates_respect_field_authority() {
    let region = Region::new(32);
    let mut world = with_region(region);
    let mut initial = object(source(region, 1), 320, 1, Object, Root);
    initial.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
    initial.scale = Some(Vec3::ONE);
    let introduced = world.observe_object(initial).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected introduction") };
    let properties =
        world.patch_properties(source(region, 2), uuid(320), PropertiesPatch { owner_id: Some(uuid(321)), creator_id: None }).unwrap();
    assert_eq!(properties.outcome, Updated(entity));

    let patch = ObjectPatch {
        pose: Some(Pose { position: Some(Vec3::Y), rotation: None }),
        linear_motion: MotionPatch { velocity: Some(Vec3::Z), acceleration: None },
        scale: Some(Vec3::splat(2.0)),
        ..ObjectPatch::new(untracked(region, 100), local_id(1))
    };
    let applied = world.patch_object(patch).unwrap();
    assert_eq!(applied.outcome, Updated(entity));
    assert_eq!(
        applied.rejections.iter().copied().collect::<Vec<_>>(),
        [
            FieldRejection { group: FieldGroup::Pose, reason: UntrustedSource },
            FieldRejection { group: FieldGroup::Scale, reason: UntrustedSource },
        ]
    );
    let current = world.object(entity).unwrap();
    assert_eq!(current.position(), Some(Vec3::X));
    assert_eq!(current.scale(), Some(Vec3::ONE));
    assert_eq!(current.linear_motion().velocity, Some(Vec3::Z));
    drop(current);

    let mut observation = object(untracked(region, 200), 320, 1, Object, Root);
    observation.pose = Some(Pose { position: Some(Vec3::Y), rotation: None });
    observation.scale = Some(Vec3::splat(3.0));
    let rejected = world.observe_object(observation).unwrap();
    assert_eq!(rejected.outcome, Unchanged);
    assert_eq!(
        rejected.rejections.iter().copied().collect::<Vec<_>>(),
        [
            FieldRejection { group: FieldGroup::Pose, reason: UntrustedSource },
            FieldRejection { group: FieldGroup::Scale, reason: UntrustedSource },
            FieldRejection { group: FieldGroup::Parent, reason: UntrustedSource },
        ]
    );

    let stale_alias = world.observe_object(object(untracked(region, 1_000), 320, 2, Object, Root)).unwrap();
    assert_eq!(stale_alias.outcome, Deferred(UnknownObject));
    assert_eq!(world.entity_at(address(region, 1)), Some(entity));
    assert_eq!(world.entity_at(address(region, 2)), None);

    let older_tracked = ObjectPatch {
        linear_motion: MotionPatch { velocity: Some(Vec3::X), acceleration: None },
        ..ObjectPatch::new(source(region, 3), local_id(1))
    };
    assert_eq!(world.patch_object(older_tracked).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().linear_motion().velocity, Some(Vec3::X));

    let untrusted_properties = world
        .patch_properties(untracked(region, 1_000), uuid(320), PropertiesPatch { owner_id: Some(uuid(322)), creator_id: Some(uuid(323)) })
        .unwrap();
    assert_eq!(
        untrusted_properties.rejections.iter().copied().collect::<Vec<_>>(),
        [FieldRejection { group: FieldGroup::Properties, reason: UntrustedSource }]
    );
    assert_eq!(world.object(entity).unwrap().properties().owner_id, Some(uuid(321)));
    assert_eq!(world.object(entity).unwrap().properties().creator_id, None);
    assert_eq!(world.remove_object(untracked(region, 1_001), local_id(1)).unwrap().outcome, Unchanged);
    assert_eq!(world.entity(uuid(320)), Some(entity));
}

#[test]
fn tracked_property_overrides_untracked() {
    let region = Region::new(45);
    let mut world = with_region(region);
    let object_id = uuid(450);
    let introduced = world.observe_object(object(source(region, 1), 450, 1, Object, Root)).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };

    let untracked_owner = uuid(451);
    let untracked = world
        .patch_properties(untracked(region, 1_000), object_id, PropertiesPatch { owner_id: Some(untracked_owner), creator_id: None })
        .unwrap();
    assert_eq!(untracked.outcome, Updated(entity));
    assert!(untracked.rejections.is_empty());
    assert_eq!(world.object(entity).unwrap().properties().owner_id, Some(untracked_owner));
    let revision = world.revision();

    let tracked_owner = uuid(452);
    let tracked =
        world.patch_properties(source(region, 2), object_id, PropertiesPatch { owner_id: Some(tracked_owner), creator_id: None }).unwrap();

    assert_eq!(tracked.outcome, Updated(entity));
    assert!(tracked.rejections.is_empty());
    assert_eq!(tracked.revision.unwrap().get(), revision.get() + 1);
    assert_eq!(world.object(entity).unwrap().properties().owner_id, Some(tracked_owner));
}

#[test]
fn untracked_motion_cannot_replace_tracked_motion() {
    let region = Region::new(31);
    let mut world = with_region(region);
    let mut initial = object(source(region, 1), 310, 1, Object, Root);
    initial.linear_motion = Some(LinearMotion { velocity: Some(Vec3::X), acceleration: None });
    let introduced = world.observe_object(initial).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };

    let patch = ObjectPatch {
        linear_motion: MotionPatch { velocity: Some(Vec3::Y), acceleration: None },
        ..ObjectPatch::new(untracked(region, 100), local_id(1))
    };
    let patched = world.patch_object(patch).unwrap();
    assert_eq!(patched.outcome, Unchanged);
    assert_eq!(
        patched.rejections.iter().copied().collect::<Vec<_>>(),
        [FieldRejection { group: FieldGroup::LinearMotion, reason: UntrustedSource }]
    );
    assert_eq!(world.object(entity).unwrap().linear_motion().velocity, Some(Vec3::X));

    let mut observation = object(untracked(region, 101), 310, 1, Object, Root);
    observation.linear_motion = Some(LinearMotion { velocity: Some(Vec3::Z), acceleration: None });
    let observed = world.observe_object(observation).unwrap();
    assert_eq!(observed.outcome, Unchanged);
    assert_eq!(
        observed.rejections.iter().copied().collect::<Vec<_>>(),
        [
            FieldRejection { group: FieldGroup::LinearMotion, reason: UntrustedSource },
            FieldRejection { group: FieldGroup::Parent, reason: UntrustedSource }
        ]
    );
    assert_eq!(world.object(entity).unwrap().linear_motion().velocity, Some(Vec3::X));
}

#[test]
fn untracked_updates_fill_unfenced_groups() {
    let region = Region::new(36);
    let mut world = with_region(region);
    let introduced = world.observe_object(object(source(region, 1), 360, 1, Object, Root)).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };

    let mut observation = object(untracked(region, 100), 360, 1, Object, Root);
    observation.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
    observation.linear_motion = Some(LinearMotion { velocity: Some(Vec3::Y), acceleration: None });
    observation.angular_velocity = Some(Vec3::Z);
    observation.scale = Some(Vec3::splat(2.0));
    let observed = world.observe_object(observation).unwrap();
    assert_eq!(observed.outcome, Updated(entity));
    assert_eq!(
        observed.rejections.iter().copied().collect::<Vec<_>>(),
        [FieldRejection { group: FieldGroup::Parent, reason: UntrustedSource }]
    );
    let current = world.object(entity).unwrap();
    assert_eq!(current.position(), Some(Vec3::X));
    assert_eq!(current.linear_motion().velocity, Some(Vec3::Y));
    assert_eq!(current.angular_velocity(), Some(Vec3::Z));
    assert_eq!(current.scale(), Some(Vec3::splat(2.0)));
    drop(current);

    let patch = ObjectPatch {
        pose: Some(Pose { position: Some(Vec3::Y), rotation: None }),
        linear_motion: MotionPatch { velocity: Some(Vec3::Z), acceleration: None },
        angular_velocity: Some(Vec3::X),
        scale: Some(Vec3::splat(3.0)),
        ..ObjectPatch::new(untracked(region, 200), local_id(1))
    };
    let patched = world.patch_object(patch).unwrap();
    assert_eq!(patched.outcome, Updated(entity));
    assert!(patched.rejections.is_empty());
    let current = world.object(entity).unwrap();
    assert_eq!(current.position(), Some(Vec3::Y));
    assert_eq!(current.linear_motion().velocity, Some(Vec3::Z));
    assert_eq!(current.angular_velocity(), Some(Vec3::X));
    assert_eq!(current.scale(), Some(Vec3::splat(3.0)));
}

#[test]
fn transfer_pends_old_region_children() {
    let source_region = Region::new(33);
    let destination = Region::new(34);
    let mut world = World::new(limits());
    assert_eq!(world.adopt_region(source_region).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(destination).unwrap().outcome, RegionAdopted);
    let parent_address = address(source_region, 10);
    let child_address = address(source_region, 11);
    let parent = world.observe_object(object(source(source_region, 1), 330, 10, Object, Root)).unwrap();
    let Introduced(parent_entity) = parent.outcome else { panic!("expected parent introduction") };
    let child = world.observe_object(object(source(source_region, 2), 331, 11, Object, Local(local_id(10)))).unwrap();
    let Introduced(child_entity) = child.outcome else { panic!("expected child introduction") };

    let moved_parent = world.observe_object(object(source(destination, 1), 330, 20, Object, Root)).unwrap();
    assert_eq!(moved_parent.outcome, Updated(parent_entity));
    assert_eq!(moved_parent.recovery.unwrap().targets, [child_address]);
    assert_eq!(world.object(parent_entity).unwrap().address(), address(destination, 20));
    assert_eq!(world.object(child_entity).unwrap().parent(), Pending { address: parent_address, expected_uuid: Some(uuid(330)) });

    let replacement = world.observe_object(object(source(source_region, 10), 332, 10, Object, Root)).unwrap();
    assert!(matches!(replacement.outcome, Introduced(_)));
    assert_eq!(world.object(child_entity).unwrap().parent(), Pending { address: parent_address, expected_uuid: Some(uuid(330)) });
}

#[test]
fn stale_observation_fills_only_unset_groups() {
    let region = Region::new(7);
    let mut world = with_region(region);

    let mut initial = object(source(region, 100), 70, 10, Object, Root);
    initial.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
    let created = world.observe_object(initial).unwrap();
    let Introduced(entity) = created.outcome else { panic!("expected introduction") };

    let mut newer_pose = object(source(region, 120), 70, 10, Object, Root);
    newer_pose.pose = Some(Pose { position: Some(Vec3::Y), rotation: None });
    assert_eq!(world.observe_object(newer_pose).unwrap().outcome, Updated(entity));

    let mut older_scale = object(source(region, 110), 70, 10, Object, Root);
    older_scale.pose = Some(Pose { position: Some(Vec3::splat(9.0)), rotation: None });
    older_scale.scale = Some(Vec3::splat(2.0));
    assert_eq!(world.observe_object(older_scale).unwrap().outcome, Updated(entity));

    assert_eq!(
        world
            .patch_properties(source(region, 115), uuid(70), PropertiesPatch { owner_id: Some(uuid(72)), creator_id: None })
            .unwrap()
            .outcome,
        Updated(entity)
    );
    let current = world.object(entity).unwrap();
    assert_eq!(current.transform().position(), Some(Vec3::Y));
    assert_eq!(current.transform().scale(), Some(Vec3::splat(2.0)));
    assert_eq!(current.properties().owner_id, Some(uuid(72)));
    drop(current);

    let mut stale_alias = object(source(region, 115), 70, 11, Object, Root);
    stale_alias.pose = Some(Pose { position: Some(Vec3::splat(8.0)), rotation: None });
    stale_alias.scale = Some(Vec3::splat(8.0));
    assert_eq!(world.observe_object(stale_alias).unwrap().outcome, Unchanged);
    assert_eq!(world.entity_at(address(region, 11)), None);
    assert_eq!(world.object(entity).unwrap().transform().scale(), Some(Vec3::splat(2.0)));

    let mut same_values_observation = object(source(region, 130), 70, 10, Object, Root);
    same_values_observation.pose = Some(Pose { position: Some(Vec3::Y), rotation: None });
    same_values_observation.scale = Some(Vec3::splat(2.0));
    let revision_before_same_values = world.revision();
    let same_values = world.observe_object(same_values_observation).unwrap();
    assert_eq!(same_values.outcome, Unchanged);
    assert_eq!(same_values.revision, None);
    assert_eq!(world.revision(), revision_before_same_values);
    assert_eq!(world.remove_object(source(region, 125), local_id(10)).unwrap().outcome, Unchanged);
    assert_eq!(world.entity(uuid(70)), Some(entity));
}

#[test]
fn property_patch_independent_of_spatial() {
    let region = Region::new(16);
    let mut world = with_region(region);
    let created = world.observe_object(object(source(region, 1), 160, 1, Object, Root)).unwrap();
    let Introduced(entity) = created.outcome else { panic!("expected object introduction") };
    let revision = world.revision().get();

    let mut update = object(source(region, 2), 160, 1, Object, Root);
    update.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
    let applied = world.observe_object(update).unwrap();

    assert_eq!(applied.outcome, Updated(entity));
    assert_eq!(applied.revision.unwrap().get(), revision + 1);
    let applied = world
        .patch_properties(source(region, 3), uuid(160), PropertiesPatch { owner_id: Some(uuid(161)), creator_id: Some(uuid(162)) })
        .unwrap();
    assert_eq!(applied.outcome, Updated(entity));
    assert_eq!(applied.revision.unwrap().get(), revision + 2);
    assert_eq!(world.revision().get(), revision + 2);
    let object = world.object(entity).unwrap();
    assert_eq!(object.position(), Some(Vec3::X));
    assert_eq!(object.properties().owner_id, Some(uuid(161)));
    assert_eq!(object.properties().creator_id, Some(uuid(162)));
}

#[test]
fn property_patch_defers_unknown_and_unordered() {
    let region = Region::new(50);
    let mut world = with_region(region);
    let patch = PropertiesPatch { owner_id: Some(uuid(501)), creator_id: None };
    let unknown = world.patch_properties(source(region, 1), uuid(500), patch).unwrap();
    assert_eq!(unknown.outcome, Deferred(UnknownObject));
    assert_eq!(unknown.recovery, None);

    let entity = match world.observe_object(object(source(region, 2), 500, 1, Object, Root)).unwrap().outcome {
        Introduced(entity) => entity,
        other => panic!("expected object introduction, got {other:?}"),
    };
    let ambiguous = UpdateSource::new(region, 3, Ambiguous, 0);
    let applied = world.patch_properties(ambiguous, uuid(500), patch).unwrap();

    assert_eq!(applied.outcome, Deferred(AmbiguousOrder));
    assert_eq!(applied.recovery.unwrap().targets, [address(region, 1)]);
    assert_eq!(world.object(entity).unwrap().properties().owner_id, None);
}

#[test]
fn patch_respects_field_fences_not_binding() {
    let region = Region::new(12);
    let mut world = with_region(region);
    let mut initial = object(source(region, 100), 120, 10, Object, Root);
    initial.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
    initial.scale = Some(Vec3::ONE);
    let introduced = world.observe_object(initial).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected introduction") };

    let properties =
        world.patch_properties(source(region, 200), uuid(120), PropertiesPatch { owner_id: Some(uuid(121)), creator_id: None }).unwrap();
    assert_eq!(properties.outcome, Updated(entity));
    let patch = ObjectPatch { scale: Some(Vec3::splat(2.0)), ..ObjectPatch::new(source(region, 150), local_id(10)) };
    assert_eq!(world.patch_object(patch).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().scale(), Some(Vec3::splat(2.0)));

    assert_eq!(world.remove_object(source(region, 190), local_id(10)).unwrap().outcome, Unchanged);
    assert_eq!(world.entity(uuid(120)), Some(entity));
}

#[test]
fn rebind_fences_old_local_id_at_rebind_stamp() {
    let region = Region::new(13);
    let mut world = with_region(region);
    let created = world.observe_object(object(source(region, 100), 130, 1, Object, Root)).unwrap();
    let Introduced(original) = created.outcome else { panic!("expected introduction") };

    assert_eq!(world.observe_object(object(source(region, 150), 130, 2, Object, Root)).unwrap().outcome, Updated(original));
    assert_eq!(world.observe_object(object(source(region, 120), 131, 1, Object, Root)).unwrap().outcome, Unchanged);
    assert_eq!(world.entity(uuid(131)), None);

    let replacement = world.observe_object(object(source(region, 160), 131, 1, Object, Root)).unwrap();
    let Introduced(replacement) = replacement.outcome else { panic!("expected a new binding after the rebind tombstone") };
    assert_ne!(original, replacement);
    assert_eq!(world.entity(uuid(130)), Some(original));
}

#[test]
fn rebind_survives_replacing_its_parent() {
    let region = Region::new(36);
    let mut world = with_region(region);
    let parent = world.observe_object(object(source(region, 1), 360, 1, Object, Root)).unwrap();
    let Introduced(parent_entity) = parent.outcome else { panic!("expected parent introduction") };
    let child = world.observe_object(object(source(region, 2), 361, 2, Object, Local(local_id(1)))).unwrap();
    let Introduced(child_entity) = child.outcome else { panic!("expected child introduction") };
    assert_eq!(world.object(child_entity).unwrap().parent(), Resolved(parent_entity));

    let rebound = world.observe_object(object(source(region, 3), 361, 1, Object, Root)).unwrap();
    assert_eq!(rebound.outcome, Updated(child_entity));
    assert_eq!(world.entity(uuid(360)), None);
    assert_eq!(world.entity(uuid(361)), Some(child_entity));
    assert_eq!(world.entity_at(address(region, 1)), Some(child_entity));
    assert_eq!(world.object(child_entity).unwrap().parent(), Parent::Root);
    assert_eq!(world.object_count(), 1);
}

#[test]
fn ambiguous_updates_defer_with_recovery() {
    let region = Region::new(2);
    let mut world = with_region(region);
    let expected_address = address(region, 7);
    let observation = object(source(region, 1), 2, 7, Object, Root);

    let ambiguous = UpdateSource::new(region, 1, Ambiguous, 0);
    let result = world.observe_object(ObjectObservation { source: ambiguous, ..observation }).unwrap();
    assert_eq!(result.outcome, Deferred(AmbiguousOrder));
    assert_eq!(result.recovery.unwrap().targets, [expected_address]);

    let degraded = UpdateSource::new(region, 2, Degraded, 0);
    let result = world.observe_object(ObjectObservation { source: degraded, ..observation }).unwrap();
    assert_eq!(result.outcome, Deferred(AmbiguousOrder));
    assert_eq!(result.recovery.unwrap().targets, [expected_address]);

    let result = world.patch_object(ObjectPatch::new(ambiguous, local_id(7))).unwrap();
    assert_eq!(result.outcome, Deferred(AmbiguousOrder));
    assert_eq!(result.recovery.unwrap().targets, [expected_address]);
}

#[test]
fn unordered_removal_requests_recovery() {
    let region = Region::new(51);
    let mut world = with_region(region);
    let entity = match world.observe_object(object(source(region, 1), 510, 7, Object, Root)).unwrap().outcome {
        Introduced(entity) => entity,
        other => panic!("expected object introduction, got {other:?}"),
    };
    let revision = world.revision();
    let unordered = UpdateSource::new(region, 2, Degraded, 0);

    let applied = world.remove_object(unordered, local_id(7)).unwrap();

    assert_eq!(applied.outcome, Deferred(AmbiguousOrder));
    assert_eq!(applied.recovery.unwrap().targets, [address(region, 7)]);
    assert_eq!(world.revision(), revision);
    assert_eq!(world.entity(uuid(510)), Some(entity));
}

#[test]
fn parent_cascade_respects_newer_child_evidence() {
    let region = Region::new(3);
    let mut world = with_region(region);
    let parent_address = address(region, 1);
    let parent_id = uuid(30);
    let child_id = uuid(31);

    let child = world.observe_object(object(source(region, 10), 31, 2, Object, Local(local_id(1)))).unwrap();
    let Introduced(child_entity) = child.outcome else { panic!("expected child introduction") };
    assert_eq!(world.object(child_entity).unwrap().parent(), Pending { address: parent_address, expected_uuid: None });
    assert_eq!(child.recovery.unwrap().targets, [parent_address]);

    let parent = world.observe_object(object(source(region, 11), 30, 1, Object, Root)).unwrap();
    let Introduced(parent_entity) = parent.outcome else { panic!("expected parent introduction") };
    assert_eq!(world.object(child_entity).unwrap().parent(), Resolved(parent_entity));

    assert_eq!(
        world
            .patch_object(ObjectPatch { scale: Some(Vec3::splat(2.0)), ..ObjectPatch::new(source(region, 13), local_id(2)) })
            .unwrap()
            .outcome,
        Updated(child_entity)
    );
    let revision = world.revision();
    let removed = world.remove_object(source(region, 12), local_id(1)).unwrap();
    assert_eq!(removed.outcome, Removed(parent_id));
    assert_eq!(world.revision().get(), revision.get() + 1);
    assert_eq!(removed.recovery.unwrap().targets, [address(region, 2)]);
    assert!(world.entity(child_id).is_some());
    assert_eq!(world.object(child_entity).unwrap().parent(), Pending { address: parent_address, expected_uuid: Some(parent_id) });

    let replacement = world.observe_object(object(source(region, 14), 30, 1, Object, Root)).unwrap();
    let Introduced(replacement_entity) = replacement.outcome else { panic!("expected parent reintroduction") };
    assert_eq!(world.object(child_entity).unwrap().parent(), Resolved(replacement_entity));

    let revision = world.revision();
    assert_eq!(world.remove_object(source(region, 15), local_id(1)).unwrap().outcome, Removed(parent_id));
    assert_eq!(world.revision().get(), revision.get() + 1);
    assert_eq!(world.entity(child_id), None);
    assert!(world.object(replacement_entity).is_none());

    assert_eq!(world.observe_object(object(source(region, 14), 32, 2, Object, Root)).unwrap().outcome, Unchanged);
    let reintroduced = world.observe_object(object(source(region, 16), 32, 2, Object, Root)).unwrap();
    let Introduced(reintroduced) = reintroduced.outcome else { panic!("expected a new child after the cascade tombstone") };
    assert_ne!(child_entity, reintroduced);
}

#[test]
fn pending_cycle_pins_parent_identity() {
    let region = Region::new(37);
    let mut world = with_region(region);
    let first = world.observe_object(object(source(region, 1), 370, 1, Object, Local(local_id(2)))).unwrap();
    let Introduced(first) = first.outcome else { panic!("expected first object introduction") };
    let waiting = world.observe_object(object(source(region, 2), 371, 3, Object, Local(local_id(2)))).unwrap();
    let Introduced(waiting) = waiting.outcome else { panic!("expected waiting object introduction") };

    let parent_applied = world.observe_object(object(source(region, 3), 372, 2, Object, Local(local_id(1)))).unwrap();
    let Introduced(parent) = parent_applied.outcome else { panic!("expected parent introduction") };

    assert_eq!(world.object(first).unwrap().parent(), Pending { address: address(region, 2), expected_uuid: Some(uuid(372)) });
    assert_eq!(world.object(parent).unwrap().parent(), Resolved(first));
    assert_eq!(world.object(waiting).unwrap().parent(), Resolved(parent));
    assert_eq!(world.children(parent).map(|object| object.entity()).collect::<HashSet<_>>(), HashSet::from([waiting]));
    assert_eq!(parent_applied.recovery.unwrap().targets, [address(region, 1)]);
    assert_eq!(parent_applied.recovery_cancellations, [address(region, 3)]);

    let unrelated = world.observe_object(object(source(region, 4), 373, 2, Object, Root)).unwrap();
    let Introduced(unrelated) = unrelated.outcome else { panic!("expected local-ID replacement") };
    assert_eq!(world.entity(uuid(372)), None);
    assert_eq!(world.entity(uuid(373)), Some(unrelated));
    assert!(world.object(waiting).is_none());
    assert_eq!(world.object(first).unwrap().parent(), Pending { address: address(region, 2), expected_uuid: Some(uuid(372)) });
}

#[test]
fn transfer_scopes_regional_fields() {
    let first = Region::new(38);
    let second = Region::new(39);
    let mut world = with_region(first);
    assert_eq!(world.adopt_region(second).unwrap().outcome, RegionAdopted);
    let object_id = uuid(380);
    let owner = uuid(381);
    let creator = uuid(382);
    let first_object = world.observe_object(object(source(first, 1), 380, 1, Object, Root)).unwrap();
    let Introduced(entity) = first_object.outcome else { panic!("expected object introduction") };

    assert_eq!(
        world.patch_properties(source(first, 200), object_id, PropertiesPatch { owner_id: Some(owner), creator_id: None }).unwrap().outcome,
        Updated(entity)
    );
    let stale_creator =
        world.patch_properties(source(first, 199), object_id, PropertiesPatch { owner_id: None, creator_id: Some(creator) }).unwrap();
    assert_eq!(stale_creator.outcome, Unchanged);
    assert_eq!(world.object(entity).unwrap().properties().creator_id, None);

    let target = SitTarget { position: Vec3::X, rotation: Quat::IDENTITY };
    assert_eq!(world.patch_sit_target(source(first, 201), object_id, Some(target)).unwrap().outcome, Updated(entity));
    let transfer = world.observe_object(object(source(second, 1), 380, 1, Object, Root)).unwrap();
    assert_eq!(transfer.outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().sit_target(), None);
    assert_eq!(world.object(entity).unwrap().properties().owner_id, Some(owner));

    assert_eq!(
        world
            .patch_properties(source(second, 2), object_id, PropertiesPatch { owner_id: None, creator_id: Some(creator) })
            .unwrap()
            .outcome,
        Updated(entity)
    );
    let destination_target = SitTarget { position: Vec3::Y, rotation: Quat::IDENTITY };
    assert_eq!(world.patch_sit_target(source(second, 2), object_id, Some(destination_target)).unwrap().outcome, Updated(entity));

    assert_eq!(
        world
            .patch_properties(source(first, 300), object_id, PropertiesPatch { owner_id: None, creator_id: Some(uuid(383)) })
            .unwrap()
            .outcome,
        Unchanged
    );
    assert_eq!(world.patch_sit_target(source(first, 301), object_id, Some(target)).unwrap().outcome, Unchanged);
    let view = world.object(entity).unwrap();
    assert_eq!(view.properties().owner_id, Some(owner));
    assert_eq!(view.properties().creator_id, Some(creator));
    assert_eq!(view.sit_target(), Some(destination_target));
}

#[test]
fn invalid_sit_target_keeps_freshness() {
    let region = Region::new(40);
    let mut world = with_region(region);
    let object_id = uuid(400);
    let added = world.observe_object(object(source(region, 1), 400, 1, Object, Root)).unwrap();
    let Introduced(entity) = added.outcome else { panic!("expected object introduction") };
    let accepted = SitTarget { position: Vec3::X, rotation: Quat::IDENTITY };
    assert_eq!(world.patch_sit_target(source(region, 10), object_id, Some(accepted)).unwrap().outcome, Updated(entity));
    let revision = world.revision();

    let invalid = SitTarget { position: Vec3::new(f32::NAN, 0.0, 0.0), rotation: Quat::IDENTITY };
    let rejected = world.patch_sit_target(source(region, 100), object_id, Some(invalid)).unwrap();
    assert_eq!(rejected.outcome, Unchanged);
    assert_eq!(rejected.revision, None);
    assert_eq!(world.revision(), revision);
    assert_eq!(
        rejected.rejections.iter().copied().collect::<Vec<_>>(),
        [FieldRejection { group: FieldGroup::SitTarget, reason: FieldRejectionReason::InvalidValue { field: "sit_position" } }]
    );
    assert_eq!(world.object(entity).unwrap().sit_target(), Some(accepted));

    let newer_than_valid = SitTarget { position: Vec3::Y, rotation: Quat::IDENTITY };
    assert_eq!(world.patch_sit_target(source(region, 90), object_id, Some(newer_than_valid)).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().sit_target(), Some(newer_than_valid));
}

#[test]
fn recovery_targets_respect_world_limit() {
    let region = Region::new(41);
    let mut world_limits = limits();
    world_limits.max_recovery_targets = 2;
    let mut world = World::new(world_limits);
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);

    let root = world.observe_object(object(source(region, 100), 410, 1, Object, Root)).unwrap();
    let Introduced(_) = root.outcome else { panic!("expected root introduction") };
    let mut child_entities = Vec::new();
    for (order, id, local) in [(110, 411, 2), (111, 412, 3), (112, 413, 4)] {
        let child = world.observe_object(object(source(region, order), id, local, Object, Local(local_id(1)))).unwrap();
        let Introduced(child) = child.outcome else { panic!("expected child introduction") };
        child_entities.push(child);
    }

    let removed = world.remove_object(source(region, 105), local_id(1)).unwrap();
    assert_eq!(removed.outcome, Removed(uuid(410)));
    let targets = removed.recovery.unwrap().targets;
    assert_eq!(targets.len(), 2);
    assert!(targets.iter().all(|target| [2, 3, 4].contains(&target.local_id().get())));
    assert!(
        child_entities
            .iter()
            .all(|child| world.object(*child).unwrap().parent() == Pending { address: address(region, 1), expected_uuid: Some(uuid(410)) })
    );
}

#[test]
fn fresh_declaration_rebinds_pending_parent() {
    let region = Region::new(14);
    let mut world = with_region(region);
    let parent_address = address(region, 1);
    let parent_id = uuid(140);
    let child_id = uuid(141);
    let parent = world.observe_object(object(source(region, 10), 140, 1, Object, Root)).unwrap();
    let Introduced(parent_entity) = parent.outcome else { panic!("expected parent introduction") };
    let child = world.observe_object(object(source(region, 11), 141, 2, Object, Local(local_id(1)))).unwrap();
    let Introduced(child_entity) = child.outcome else { panic!("expected child introduction") };

    let properties =
        world.patch_properties(source(region, 200), child_id, PropertiesPatch { owner_id: Some(uuid(142)), creator_id: None }).unwrap();
    assert_eq!(properties.outcome, Updated(child_entity));
    assert_eq!(world.remove_object(source(region, 12), local_id(1)).unwrap().outcome, Removed(parent_id));
    assert_eq!(world.object(child_entity).unwrap().parent(), Pending { address: parent_address, expected_uuid: Some(parent_id) });

    assert_eq!(world.observe_object(object(source(region, 11), 141, 2, Object, Local(local_id(1)))).unwrap().outcome, Unchanged);
    let unrelated = world.observe_object(object(source(region, 20), 143, 1, Object, Root)).unwrap();
    let Introduced(unrelated_entity) = unrelated.outcome else { panic!("expected local ID reuse") };
    assert_ne!(parent_entity, unrelated_entity);
    assert_eq!(world.object(child_entity).unwrap().parent(), Pending { address: parent_address, expected_uuid: Some(parent_id) });

    let declaration = world.observe_object(object(source(region, 15), 141, 2, Object, Local(local_id(1)))).unwrap();
    assert_eq!(declaration.outcome, Unchanged);
    assert_eq!(world.object(child_entity).unwrap().parent(), Pending { address: parent_address, expected_uuid: Some(parent_id) });

    let declaration = world.observe_object(object(source(region, 25), 141, 2, Object, Local(local_id(1)))).unwrap();
    assert_eq!(declaration.outcome, Updated(child_entity));
    assert_eq!(world.object(child_entity).unwrap().parent(), Resolved(unrelated_entity));

    let replacement = world.observe_object(object(source(region, 30), 140, 1, Object, Root)).unwrap();
    assert!(matches!(replacement.outcome, Introduced(_)));
    assert_eq!(world.entity(uuid(143)), None);
    assert_eq!(world.object(child_entity).unwrap().parent(), Pending { address: parent_address, expected_uuid: Some(uuid(143)) });
}

#[test]
fn delayed_declaration_keeps_pending_parent() {
    let region = Region::new(17);
    let mut world = with_region(region);
    let parent_address = address(region, 1);
    let parent_id = uuid(170);
    let child_id = uuid(171);

    let parent = world.observe_object(object(source(region, 10), 170, 1, Object, Root)).unwrap();
    assert!(matches!(parent.outcome, Introduced(_)));
    let child = world.observe_object(object(source(region, 11), 171, 2, Object, Local(local_id(1)))).unwrap();
    let Introduced(child_entity) = child.outcome else { panic!("expected child introduction") };
    assert_eq!(
        world
            .patch_properties(source(region, 200), child_id, PropertiesPatch { owner_id: Some(uuid(172)), creator_id: None })
            .unwrap()
            .outcome,
        Updated(child_entity)
    );
    assert_eq!(world.remove_object(source(region, 12), local_id(1)).unwrap().outcome, Removed(parent_id));

    let delayed = world.observe_object(object(source(region, 13), 171, 2, Object, Local(local_id(1)))).unwrap();
    assert_eq!(delayed.outcome, Unchanged);
    assert_eq!(world.object(child_entity).unwrap().parent(), Pending { address: parent_address, expected_uuid: Some(parent_id) });

    let replacement = world.observe_object(object(source(region, 14), 170, 1, Object, Root)).unwrap();
    let Introduced(parent_entity) = replacement.outcome else { panic!("expected parent reintroduction") };
    assert_eq!(world.object(child_entity).unwrap().parent(), Resolved(parent_entity));
}

#[test]
fn reused_parent_address_requires_fresh_declaration() {
    let region = Region::new(19);
    let mut world = with_region(region);
    let parent_address = address(region, 1);

    let parent = world.observe_object(object(source(region, 10), 190, 1, Object, Root)).unwrap();
    assert!(matches!(parent.outcome, Introduced(_)));
    assert_eq!(world.remove_object(source(region, 12), local_id(1)).unwrap().outcome, Removed(uuid(190)));

    let reused_parent = world.observe_object(object(source(region, 20), 191, 1, Object, Root)).unwrap();
    let Introduced(reused_parent) = reused_parent.outcome else { panic!("expected local ID reuse") };
    let child = world.observe_object(object(source(region, 15), 192, 2, Object, Local(local_id(1)))).unwrap();
    let Introduced(child) = child.outcome else { panic!("expected child introduction") };
    assert_eq!(world.object(child).unwrap().parent(), Pending { address: parent_address, expected_uuid: None });

    let refreshed_parent = world.observe_object(object(source(region, 21), 191, 1, Object, Root)).unwrap();
    assert_eq!(refreshed_parent.outcome, Unchanged);
    assert_eq!(refreshed_parent.recovery.unwrap().targets, [address(region, 2)]);
    assert_eq!(world.object(child).unwrap().parent(), Pending { address: parent_address, expected_uuid: None });

    assert_eq!(world.observe_object(object(source(region, 22), 192, 2, Object, Local(local_id(1)))).unwrap().outcome, Updated(child));
    assert_eq!(world.object(child).unwrap().parent(), Resolved(reused_parent));
}

#[test]
fn empty_pose_patch_does_not_advance_pose_fence() {
    let region = Region::new(18);
    let mut world = with_region(region);
    let created = world.observe_object(object(source(region, 1), 180, 1, Object, Root)).unwrap();
    let Introduced(entity) = created.outcome else { panic!("expected object introduction") };
    let revision = world.revision();

    let empty = ObjectPatch { pose: Some(Pose::default()), ..ObjectPatch::new(source(region, 100), local_id(1)) };
    let ignored = world.patch_object(empty).unwrap();
    assert_eq!(ignored.outcome, Unchanged);
    assert_eq!(ignored.revision, None);
    assert_eq!(world.revision(), revision);

    let pose =
        ObjectPatch { pose: Some(Pose { position: Some(Vec3::X), rotation: None }), ..ObjectPatch::new(source(region, 90), local_id(1)) };
    assert_eq!(world.patch_object(pose).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().position(), Some(Vec3::X));
}

#[test]
fn parent_resolution_keeps_parent_fence() {
    let region = Region::new(16);
    let mut world = with_region(region);
    let child = world.observe_object(object(source(region, 10), 160, 2, Object, Local(local_id(1)))).unwrap();
    let Introduced(child_entity) = child.outcome else { panic!("expected child introduction") };
    let other_parent = world.observe_object(object(source(region, 14), 163, 3, Object, Root)).unwrap();
    let Introduced(other_parent_entity) = other_parent.outcome else { panic!("expected other parent introduction") };

    let parent = world.observe_object(object(source(region, 20), 161, 1, Object, Root)).unwrap();
    let Introduced(parent_entity) = parent.outcome else { panic!("expected parent introduction") };
    assert_eq!(world.object(child_entity).unwrap().parent(), Resolved(parent_entity));

    let delayed_declaration = ObjectPatch { parent: Some(Local(local_id(3))), ..ObjectPatch::new(source(region, 15), local_id(2)) };
    let updated = world.patch_object(delayed_declaration).unwrap();
    assert_eq!(updated.outcome, Updated(child_entity));
    assert_eq!(world.object(child_entity).unwrap().parent(), Resolved(other_parent_entity));
}

#[test]
fn parent_removal_detaches_seated_avatar() {
    let region = Region::new(15);
    let mut world = with_region(region);
    let parent = world.observe_object(object(source(region, 10), 150, 1, Object, Root)).unwrap();
    let Introduced(parent_entity) = parent.outcome else { panic!("expected parent introduction") };
    let avatar = world.observe_object(object(source(region, 11), 151, 2, Avatar, Local(local_id(1)))).unwrap();
    let Introduced(avatar_entity) = avatar.outcome else { panic!("expected avatar introduction") };

    assert_eq!(world.remove_object(source(region, 12), local_id(1)).unwrap().outcome, Removed(uuid(150)));
    assert!(world.object(parent_entity).is_none());
    assert_eq!(world.entity(uuid(151)), Some(avatar_entity));
    assert_eq!(world.object(avatar_entity).unwrap().parent(), Parent::Root);
}

#[test]
fn agent_region_change_requires_handoff() {
    let source_region = Region::new(4);
    let destination_region = Region::new(5);
    let mut world = World::new(limits());
    assert_eq!(world.adopt_region(source_region).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(destination_region).unwrap().outcome, RegionAdopted);
    let agent_id = uuid(40);
    world.set_self_agent(agent_id).unwrap();

    let mut initial = object(source(source_region, 100), 40, 8, Avatar, Root);
    initial.pose = Some(Pose { position: Some(Vec3::new(4.0, 5.0, 6.0)), rotation: Some(Quat::IDENTITY) });
    initial.scale = Some(Vec3::splat(1.5));
    initial.linear_motion = Some(LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::Y) });
    initial.angular_velocity = Some(Vec3::Z);
    let created = world.observe_object(initial).unwrap();
    let Introduced(agent) = created.outcome else { panic!("expected agent introduction") };
    assert!(matches!(
        world
            .patch_properties(source(source_region, 101), agent_id, PropertiesPatch { owner_id: Some(uuid(41)), creator_id: None })
            .unwrap()
            .outcome,
        Updated(_)
    ));
    assert_eq!(world.remove_object(source(source_region, 100), local_id(8)).unwrap().outcome, Unchanged);
    assert_eq!(world.entity(agent_id), Some(agent));

    let direct_move = world.observe_object(object(source(destination_region, 1), 40, 8, Avatar, Root));
    assert_eq!(direct_move, Err(InvalidValue { field: "agent_handoff_required" }));
    assert_eq!(world.object(agent).unwrap().address(), address(source_region, 8));

    let handoff = AgentHandoff {
        source: source(destination_region, 1),
        local_id: local_id(9),
        parent: Root,
        pose: None,
        linear_motion: None,
        angular_velocity: None,
        scale: None,
    };
    let applied = world.handoff_agent(source_region, handoff).unwrap();
    assert_eq!(applied.outcome, AgentHandedOff(agent));
    assert_eq!(world.entity(agent_id), Some(agent));
    assert_eq!(world.entity_at(address(source_region, 8)), None);
    let current = world.object(agent).unwrap();
    assert_eq!(current.address(), address(destination_region, 9));
    assert_eq!(current.transform().position(), None);
    assert_eq!(current.transform().rotation(), None);
    assert_eq!(current.transform().scale(), Some(Vec3::splat(1.5)));
    assert_eq!(current.linear_motion(), LinearMotion::default());
    assert_eq!(current.angular_velocity(), None);
    assert_eq!(current.properties().owner_id, Some(uuid(41)));
    drop(current);

    let return_handoff = AgentHandoff {
        source: source(source_region, 102),
        local_id: local_id(10),
        parent: Root,
        pose: None,
        linear_motion: None,
        angular_velocity: None,
        scale: None,
    };
    assert_eq!(world.handoff_agent(destination_region, return_handoff).unwrap().outcome, AgentHandedOff(agent));
    let stale_property = PropertiesPatch { owner_id: Some(uuid(42)), creator_id: None };
    assert_eq!(world.patch_properties(source(source_region, 101), agent_id, stale_property).unwrap().outcome, Unchanged);
    assert_eq!(world.object(agent).unwrap().properties().owner_id, Some(uuid(41)));
}

#[test]
fn cycles_are_rejected_without_graph_changes() {
    let region = Region::new(6);
    let mut world = with_region(region);
    let first = world.observe_object(object(source(region, 1), 60, 1, Object, Root)).unwrap();
    let Introduced(first_entity) = first.outcome else { panic!("expected introduction") };
    let second = world.observe_object(object(source(region, 2), 61, 2, Object, Local(local_id(1)))).unwrap();
    let Introduced(second_entity) = second.outcome else { panic!("expected introduction") };

    let revision = world.revision();
    let cycle = ObjectPatch { parent: Some(Local(local_id(2))), ..ObjectPatch::new(source(region, 4), local_id(1)) };
    let applied = world.patch_object(cycle).unwrap();
    assert_eq!(applied.outcome, Unchanged);
    assert_eq!(applied.rejections.iter().copied().collect::<Vec<_>>(), [FieldRejection { group: FieldGroup::Parent, reason: ParentCycle }]);
    assert_eq!(world.revision(), revision);
    assert_eq!(world.object(first_entity).unwrap().parent(), Parent::Root);
    assert_eq!(world.object(second_entity).unwrap().parent(), Resolved(first_entity));
}
